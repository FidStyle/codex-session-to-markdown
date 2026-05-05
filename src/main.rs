use codex_prompts::{
    expand_home, extract_prompts, warn_malformed_lines, write_records, OutputMode,
};
use std::fs::File;
use std::io::{self, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
Usage:
  codex-prompts [--with-context] <session.jsonl>
  codex-prompts -h|--help

Extract user prompts from a Codex session JSONL file.

Options:
  --with-context  Include the nearest previous assistant message for each prompt.
  -h, --help      Show this help text.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    Extract { path: PathBuf, mode: OutputMode },
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
        Command::Extract { path, mode } => {
            let file = File::open(&path)
                .map_err(|err| format!("failed to open {}: {err}", path.display()))?;
            let reader = BufReader::new(file);
            let (records, warnings) = extract_prompts(reader);

            warn_malformed_lines(&mut io::stderr().lock(), &path, &warnings)
                .map_err(|err| format!("failed to write warnings: {err}"))?;
            write_records(&mut io::stdout().lock(), &records, mode)
                .map_err(|err| format!("failed to write output: {err}"))?;
            Ok(())
        }
    }
}

fn parse_args<I>(args: I) -> Result<Command, String>
where
    I: IntoIterator<Item = String>,
{
    let mut mode = OutputMode::PromptsOnly;
    let mut path: Option<PathBuf> = None;

    for arg in args {
        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--with-context" => mode = OutputMode::WithContext,
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
        Some(path) => Ok(Command::Extract { path, mode }),
        None => Err("missing session JSONL path".to_owned()),
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
                mode: OutputMode::PromptsOnly,
            }
        );
    }

    #[test]
    fn parses_context_flag() {
        assert_eq!(
            parse_args(["--with-context".to_owned(), "session.jsonl".to_owned()]).unwrap(),
            Command::Extract {
                path: PathBuf::from("session.jsonl"),
                mode: OutputMode::WithContext,
            }
        );
    }

    #[test]
    fn rejects_missing_path() {
        assert!(parse_args(std::iter::empty::<String>()).is_err());
    }
}
