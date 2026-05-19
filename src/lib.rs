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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupeMode {
    Enabled,
    Disabled,
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
    extract_prompts_with_mode(reader, DedupeMode::Enabled)
}

pub fn extract_prompts_with_mode<R: BufRead>(
    reader: R,
    dedupe_mode: DedupeMode,
) -> (Vec<PromptRecord>, Vec<ParseWarning>) {
    let (records, warnings) = extract_prompts_raw(reader);
    let records = match dedupe_mode {
        DedupeMode::Enabled => dedupe_records(records),
        DedupeMode::Disabled => records,
    };

    (records, warnings)
}

fn extract_prompts_raw<R: BufRead>(reader: R) -> (Vec<PromptRecord>, Vec<ParseWarning>) {
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
                    if let Some(prompt) = clean_user_prompt(&prompt) {
                        records.push(PromptRecord {
                            prompt,
                            previous_assistant: last_assistant.clone(),
                        });
                    }
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

fn clean_user_prompt(text: &str) -> Option<String> {
    let trimmed = text.trim();

    if is_injected_prompt(trimmed) {
        return None;
    }

    let cleaned = strip_trellis_command_prefix(trimmed);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned.to_owned())
    }
}

fn is_injected_prompt(text: &str) -> bool {
    let starts_with_injected_block = [
        "# AGENTS.md instructions",
        "<skill>",
        "<workflow-state>",
        "<environment_context>",
        "<user_shell_command>",
        "<turn_aborted>",
        "<session-context>",
        "<current-state>",
        "<workflow>",
        "<guidelines>",
        "<task-status>",
        "<ready>",
        "<collaboration_mode>",
    ]
    .iter()
    .any(|prefix| text.starts_with(prefix));

    starts_with_injected_block
        || text.contains("<!-- TRELLIS:START -->")
        || text.contains("<name>trellis-")
        || text.contains("Trellis Instructions")
}

fn strip_trellis_command_prefix(text: &str) -> &str {
    let Some(rest) = text.strip_prefix("$trellis-") else {
        return text;
    };

    match rest.find(char::is_whitespace) {
        Some(index) => rest[index..].trim_start(),
        None => text,
    }
}

fn dedupe_records(records: Vec<PromptRecord>) -> Vec<PromptRecord> {
    let mut deduped: Vec<PromptRecord> = Vec::new();

    'records: for record in records {
        let key = dedupe_key(&record.prompt);
        if key.is_none() {
            deduped.push(record);
            continue;
        }
        let key = key.expect("checked above");

        let mut replace_index: Option<usize> = None;
        for (index, existing) in deduped.iter().enumerate() {
            let Some(existing_key) = dedupe_key(&existing.prompt) else {
                continue;
            };

            if key == existing_key {
                replace_index = Some(index);
                break;
            }

            if key.contains(&existing_key) || existing_key.contains(&key) {
                replace_index = Some(index);
                break;
            }
        }

        if let Some(index) = replace_index {
            if record.prompt.chars().count() >= deduped[index].prompt.chars().count() {
                deduped[index] = record;
            }
            continue 'records;
        }

        deduped.push(record);
    }

    deduped
}

fn dedupe_key(text: &str) -> Option<String> {
    const MIN_DEDUPE_CHARS: usize = 20;

    let key = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if key.chars().count() < MIN_DEDUPE_CHARS {
        None
    } else {
        Some(key)
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
    fn filters_trellis_injected_user_messages() {
        let input = r##"
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"# AGENTS.md instructions for /tmp/demo\n\n<!-- TRELLIS:START -->\nTrellis Instructions"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<skill>\n<name>trellis-brainstorm</name>\n</skill>"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<turn_aborted>\nThe user interrupted the previous turn on purpose.\n</turn_aborted>"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"$trellis-brainstorm 真正的用户需求"}]}}
"##;

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, "真正的用户需求");
    }

    #[test]
    fn dedupes_repeated_long_prompts_and_keeps_latest_superset() {
        let short = "请根据 target.md 生成新的实验室协议，包含合作基础、实验室定位、组织架构、研究方向、运营模式和保障条件。";
        let long = "请根据 target.md 生成新的实验室协议，包含合作基础、实验室定位、组织架构、研究方向、运营模式和保障条件。还需要补充网上官方资料，优先高校、研究所、央国企政企等体制内来源。";
        let input = format!(
            r#"
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{short}"}}]}}}}
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{long}"}}]}}}}
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{long}"}}]}}}}
"#
        );

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].prompt, long);
    }

    #[test]
    fn keeps_distinct_prompts_that_only_share_many_terms() {
        let first = "请根据 target.md 生成新的实验室协议，包含合作基础、实验室定位、组织架构、研究方向、运营模式和保障条件。";
        let second = "请根据 target.md 检查新的实验室协议，保留合作基础、实验室定位、组织架构、研究方向、运营模式和保障条件，但只输出问题清单。";
        let input = format!(
            r#"
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{first}"}}]}}}}
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{second}"}}]}}}}
"#
        );

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].prompt, first);
        assert_eq!(records[1].prompt, second);
    }

    #[test]
    fn no_dedupe_mode_keeps_repeated_prompts() {
        let prompt = "请根据 target.md 生成新的实验室协议，包含合作基础、实验室定位、组织架构、研究方向、运营模式和保障条件。";
        let input = format!(
            r#"
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{prompt}"}}]}}}}
{{"type":"response_item","payload":{{"type":"message","role":"user","content":[{{"type":"input_text","text":"{prompt}"}}]}}}}
"#
        );

        let (records, warnings) =
            extract_prompts_with_mode(Cursor::new(input), DedupeMode::Disabled);

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn does_not_dedupe_short_replies() {
        let input = r#"
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"可以"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"可以"}]}}
"#;

        let (records, warnings) = extract_prompts(Cursor::new(input));

        assert!(warnings.is_empty());
        assert_eq!(records.len(), 2);
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
