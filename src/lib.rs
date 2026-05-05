use serde_json::Value;
use std::fmt;
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptRecord {
    pub prompt: String,
    pub previous_assistant: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseWarning {
    pub line: usize,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputMode {
    PromptsOnly,
    WithContext,
}

#[derive(Debug)]
pub enum CliError {
    Io(std::io::Error),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for CliError {}

impl From<std::io::Error> for CliError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = home_dir() {
            return home;
        }
    }

    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }

    PathBuf::from(path)
}

pub fn extract_prompts<R: BufRead>(reader: R) -> (Vec<PromptRecord>, Vec<ParseWarning>) {
    let mut records = Vec::new();
    let mut warnings = Vec::new();
    let mut last_assistant: Option<String> = None;

    for (index, line_result) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = match line_result {
            Ok(line) => line,
            Err(err) => {
                warnings.push(ParseWarning {
                    line: line_number,
                    message: err.to_string(),
                });
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        let value: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                warnings.push(ParseWarning {
                    line: line_number,
                    message: err.to_string(),
                });
                continue;
            }
        };

        if value.get("type").and_then(Value::as_str) != Some("response_item") {
            continue;
        }

        let Some(payload) = value.get("payload") else {
            continue;
        };

        if payload.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }

        let role = payload.get("role").and_then(Value::as_str);
        let text = extract_message_text(payload);

        match role {
            Some("assistant") => {
                if let Some(text) = text {
                    last_assistant = Some(text);
                }
            }
            Some("user") => {
                if let Some(prompt) = text {
                    records.push(PromptRecord {
                        prompt,
                        previous_assistant: last_assistant.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    (records, warnings)
}

pub fn write_records<W: Write>(
    writer: &mut W,
    records: &[PromptRecord],
    mode: OutputMode,
) -> std::io::Result<()> {
    for (index, record) in records.iter().enumerate() {
        if index > 0 {
            writeln!(writer)?;
            writeln!(writer)?;
        }

        match mode {
            OutputMode::PromptsOnly => write!(writer, "{}", record.prompt)?,
            OutputMode::WithContext => {
                writeln!(writer, "## Prompt {}", index + 1)?;
                writeln!(writer)?;
                if let Some(previous) = &record.previous_assistant {
                    writeln!(writer, "### Previous Assistant")?;
                    writeln!(writer)?;
                    writeln!(writer, "{previous}")?;
                    writeln!(writer)?;
                }
                writeln!(writer, "### User Prompt")?;
                writeln!(writer)?;
                write!(writer, "{}", record.prompt)?;
            }
        }
    }

    Ok(())
}

pub fn warn_malformed_lines<W: Write>(
    writer: &mut W,
    path: &Path,
    warnings: &[ParseWarning],
) -> std::io::Result<()> {
    for warning in warnings {
        writeln!(
            writer,
            "warning: {}:{}: skipped malformed JSONL line: {}",
            path.display(),
            warning.line,
            warning.message
        )?;
    }
    Ok(())
}

fn extract_message_text(payload: &Value) -> Option<String> {
    let content = payload.get("content")?;

    if let Some(text) = content.as_str() {
        return clean_text(text);
    }

    let parts = content.as_array()?;
    let mut texts = Vec::new();

    for part in parts {
        let part_type = part.get("type").and_then(Value::as_str);
        if !matches!(
            part_type,
            Some("input_text") | Some("output_text") | Some("text")
        ) {
            continue;
        }

        if let Some(text) = part
            .get("text")
            .and_then(Value::as_str)
            .and_then(clean_text)
        {
            texts.push(text);
        }
    }

    if texts.is_empty() {
        None
    } else {
        Some(texts.join("\n"))
    }
}

fn clean_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn extracts_user_prompts_in_order() {
        let input = r#"
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first\nprompt"}]}}
{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"ignore me"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second prompt"}]}}
"#;

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(
            records,
            vec![
                PromptRecord {
                    prompt: "first\nprompt".to_owned(),
                    previous_assistant: None,
                },
                PromptRecord {
                    prompt: "second prompt".to_owned(),
                    previous_assistant: None,
                },
            ]
        );
    }

    #[test]
    fn pairs_prompt_with_nearest_previous_assistant_response_item() {
        let input = r#"
{"type":"event_msg","payload":{"last_agent_message":"do not use this"}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"first assistant"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first user"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"second assistant"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"second user"}]}}
"#;

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 2);
        assert_eq!(
            records[0].previous_assistant.as_deref(),
            Some("first assistant")
        );
        assert_eq!(
            records[1].previous_assistant.as_deref(),
            Some("second assistant")
        );
    }

    #[test]
    fn skips_malformed_lines_with_warning() {
        let input = r#"
not json
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"valid"}]}}
"#;

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, "valid");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].line, 2);
    }

    #[test]
    fn writes_prompts_only_without_raw_json() {
        let records = vec![
            PromptRecord {
                prompt: "first".to_owned(),
                previous_assistant: Some("assistant".to_owned()),
            },
            PromptRecord {
                prompt: "second".to_owned(),
                previous_assistant: None,
            },
        ];
        let mut output = Vec::new();

        write_records(&mut output, &records, OutputMode::PromptsOnly).unwrap();

        assert_eq!(String::from_utf8(output).unwrap(), "first\n\nsecond");
    }

    #[test]
    fn writes_context_mode_as_markdown() {
        let records = vec![PromptRecord {
            prompt: "user prompt".to_owned(),
            previous_assistant: Some("agent reply".to_owned()),
        }];
        let mut output = Vec::new();

        write_records(&mut output, &records, OutputMode::WithContext).unwrap();

        let output = String::from_utf8(output).unwrap();
        assert!(output.contains("## Prompt 1"));
        assert!(output.contains("### Previous Assistant"));
        assert!(output.contains("agent reply"));
        assert!(output.contains("### User Prompt"));
        assert!(output.contains("user prompt"));
    }

    #[test]
    fn expands_home_path_prefix() {
        let home = std::env::var("HOME").expect("HOME should be set in test environment");

        assert_eq!(expand_home("~"), PathBuf::from(&home));
        assert_eq!(
            expand_home("~/session.jsonl"),
            PathBuf::from(home).join("session.jsonl")
        );
        assert_eq!(
            expand_home("/tmp/session.jsonl"),
            PathBuf::from("/tmp/session.jsonl")
        );
    }
}
