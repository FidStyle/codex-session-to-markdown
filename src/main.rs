use codex_session_to_markdown::{
    expand_home, extract_prompts_with_mode, warn_malformed_lines, write_records, DedupeMode,
    OutputMode,
};
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "\
Usage:
  codex-session-to-markdown [--with-context] [--no-dedupe] <session.jsonl|session-id>
  codex-session-to-markdown -h|--help

Convert a Codex session JSONL file or session UUID to a prompt transcript.

Options:
  --with-context  Include the nearest previous assistant message for each prompt.
  --no-dedupe     Keep duplicate or superseded prompts.
  -h, --help      Show this help text.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Extract {
        path: PathBuf,
        output_mode: OutputMode,
        dedupe_mode: DedupeMode,
    },
}

fn main() -> ExitCode {
    match run(std::env::args().skip(1)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            eprintln!();
            eprint!("{USAGE}");
            ExitCode::from(1)
        }
    }
}

fn run<I>(args: I) -> Result<(), String>
where
    I: IntoIterator<Item = String>,
{
    match parse_args(args)? {
        Command::Help => {
            print!("{USAGE}");
            Ok(())
        }
        Command::Extract {
            path,
            output_mode,
            dedupe_mode,
        } => {
            let path = resolve_input_path(&path)?;
            let file = File::open(&path)
                .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
            let reader = BufReader::new(file);
            let (records, warnings) = extract_prompts_with_mode(reader, dedupe_mode);

            warn_malformed_lines(&mut io::stderr().lock(), &path, &warnings)
                .map_err(|err| format!("failed to write warnings: {err}"))?;
            write_records(&mut io::stdout().lock(), &records, output_mode)
                .map_err(|err| format!("failed to write output: {err}"))?;
            Ok(())
        }
    }
}

fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut output_mode = OutputMode::PromptsOnly;
    let mut dedupe_mode = DedupeMode::Enabled;
    let mut path: Option<PathBuf> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--with-context" => output_mode = OutputMode::WithContext,
            "--no-dedupe" => dedupe_mode = DedupeMode::Disabled,
            _ if arg.starts_with('-') => return Err(format!("unknown option: {arg}")),
            _ => {
                if path.is_some() {
                    return Err(format!("unexpected extra argument: {arg}"));
                }
                path = Some(expand_home(&arg));
            }
        }
    }

    match path {
        Some(path) => Ok(Command::Extract {
            path,
            output_mode,
            dedupe_mode,
        }),
        None => Err("missing session JSONL path".to_owned()),
    }
}

fn resolve_input_path(input: &Path) -> Result<PathBuf, String> {
    if input.is_file() {
        return Ok(input.to_path_buf());
    }

    if let Some(session_id) = bare_session_id(input) {
        return find_session_file(&codex_sessions_dir(), session_id);
    }

    Ok(input.to_path_buf())
}

fn codex_sessions_dir() -> PathBuf {
    expand_home("~/.codex/sessions")
}

fn bare_session_id(input: &Path) -> Option<&str> {
    let mut components = input.components();
    let component = match components.next()? {
        Component::Normal(component) => component,
        _ => return None,
    };

    if components.next().is_some() {
        return None;
    }

    let raw = component.to_str()?;
    let raw = raw.strip_prefix("codex_").unwrap_or(raw);
    let raw = raw.strip_suffix(".jsonl").unwrap_or(raw);

    if is_session_uuid(raw) {
        Some(raw)
    } else {
        None
    }
}

fn is_session_uuid(value: &str) -> bool {
    if value.len() != 36 {
        return false;
    }

    value.char_indices().all(|(index, ch)| match index {
        8 | 13 | 18 | 23 => ch == '-',
        _ => ch.is_ascii_hexdigit(),
    })
}

fn find_session_file(root: &Path, session_id: &str) -> Result<PathBuf, String> {
    if !root.is_dir() {
        return Err(format!(
            "Codex sessions directory not found: {}",
            root.display()
        ));
    }

    let needle = session_id.to_ascii_lowercase();
    let mut matches = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries =
            fs::read_dir(&dir).map_err(|err| format!("failed to read {}: {err}", dir.display()))?;

        for entry in entries {
            let entry =
                entry.map_err(|err| format!("failed to read entry in {}: {err}", dir.display()))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|err| format!("failed to inspect {}: {err}", path.display()))?;

            if file_type.is_dir() {
                stack.push(path);
                continue;
            }

            if !file_type.is_file() {
                continue;
            }

            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }

            let Some(file_stem) = path.file_stem().and_then(|name| name.to_str()) else {
                continue;
            };

            if file_stem.to_ascii_lowercase().ends_with(&needle) {
                matches.push(path);
            }
        }
    }

    matches.sort();

    match matches.len() {
        0 => Err(format!(
            "no Codex session JSONL found for {session_id} under {}",
            root.display()
        )),
        1 => Ok(matches.remove(0)),
        _ => {
            let paths = matches
                .iter()
                .map(|path| format!("  {}", path.display()))
                .collect::<Vec<_>>()
                .join("\n");
            Err(format!(
                "multiple Codex session JSONL files found for {session_id} under {}:\n{paths}",
                root.display()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_help_short_and_long() {
        assert_eq!(parse_args(["-h".to_owned()]).unwrap(), Command::Help);
        assert_eq!(parse_args(["--help".to_owned()]).unwrap(), Command::Help);
    }

    #[test]
    fn parses_default_extract_command() {
        assert_eq!(
            parse_args(["session.jsonl".to_owned()]).unwrap(),
            Command::Extract {
                path: PathBuf::from("session.jsonl"),
                output_mode: OutputMode::PromptsOnly,
                dedupe_mode: DedupeMode::Enabled,
            }
        );
    }

    #[test]
    fn parses_context_flag() {
        assert_eq!(
            parse_args(["--with-context".to_owned(), "session.jsonl".to_owned()]).unwrap(),
            Command::Extract {
                path: PathBuf::from("session.jsonl"),
                output_mode: OutputMode::WithContext,
                dedupe_mode: DedupeMode::Enabled,
            }
        );
    }

    #[test]
    fn parses_no_dedupe_flag() {
        assert_eq!(
            parse_args(["--no-dedupe".to_owned(), "session.jsonl".to_owned()]).unwrap(),
            Command::Extract {
                path: PathBuf::from("session.jsonl"),
                output_mode: OutputMode::PromptsOnly,
                dedupe_mode: DedupeMode::Disabled,
            }
        );
    }

    #[test]
    fn rejects_missing_path() {
        assert!(parse_args(std::iter::empty::<String>()).is_err());
    }

    #[test]
    fn detects_bare_session_uuid_inputs() {
        let session_id = "019dfa57-dbd2-7f61-84c4-0c468f27dd1a";

        assert_eq!(bare_session_id(Path::new(session_id)), Some(session_id));
        assert_eq!(
            bare_session_id(Path::new("codex_019dfa57-dbd2-7f61-84c4-0c468f27dd1a")),
            Some(session_id)
        );
        assert_eq!(
            bare_session_id(Path::new(
                "codex_019dfa57-dbd2-7f61-84c4-0c468f27dd1a.jsonl"
            )),
            Some(session_id)
        );
        assert_eq!(bare_session_id(Path::new("not-a-uuid")), None);
        assert_eq!(bare_session_id(Path::new("2026/05/19/session.jsonl")), None);
    }

    #[test]
    fn finds_session_file_by_full_uuid() {
        let session_id = "019dfa57-dbd2-7f61-84c4-0c468f27dd1a";
        let root = temp_dir("finds_session_file_by_full_uuid");
        let day_dir = root.join("2026").join("05").join("19");
        fs::create_dir_all(&day_dir).unwrap();
        let expected = day_dir.join(format!("rollout-2026-05-19T12-00-00-{session_id}.jsonl"));
        fs::write(&expected, "").unwrap();
        fs::write(
            day_dir.join(format!("rollout-2026-05-19T12-00-00-{session_id}.txt")),
            "",
        )
        .unwrap();
        fs::write(
            day_dir.join(format!(
                "rollout-2026-05-19T12-00-01-{session_id}-copy.jsonl"
            )),
            "",
        )
        .unwrap();

        let resolved = find_session_file(&root, session_id).unwrap();

        assert_eq!(resolved, expected);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_ambiguous_session_uuid_matches() {
        let session_id = "019dfa57-dbd2-7f61-84c4-0c468f27dd1a";
        let root = temp_dir("rejects_ambiguous_session_uuid_matches");
        let day_dir = root.join("2026").join("05").join("19");
        fs::create_dir_all(&day_dir).unwrap();
        fs::write(
            day_dir.join(format!("rollout-2026-05-19T12-00-00-{session_id}.jsonl")),
            "",
        )
        .unwrap();
        fs::write(
            day_dir.join(format!("rollout-2026-05-19T12-00-01-{session_id}.jsonl")),
            "",
        )
        .unwrap();

        let err = find_session_file(&root, session_id).unwrap_err();

        assert!(err.contains("multiple Codex session JSONL files found"));
        fs::remove_dir_all(root).unwrap();
    }

    fn temp_dir(test_name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "codex_session_to_markdown_{test_name}_{}_{}",
            std::process::id(),
            unique
        ))
    }
}
