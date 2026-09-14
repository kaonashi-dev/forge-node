//! Argument parsing for the standalone binary.
//!
//! Hand-rolled for the spike: two flags do not justify a parser dependency in
//! the one artifact the plan wants independently distributable. The contract
//! is the plan's §5.3 — path, `--read-only`, `--` for a path that starts with
//! a dash, help and version — and richer flags arrive with F3.

use std::path::PathBuf;

pub const USAGE: &str = "\
forge-editor — terminal editor (standalone spike)

Usage:
  forge-editor [--read-only] [--] <file>

Options:
  --read-only   Open the file without allowing edits.
  -h, --help    Print this help and exit.
  -V, --version Print the version and exit.
";

#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Edit(Options),
    Help,
    Version,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Options {
    pub path: PathBuf,
    pub read_only: bool,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut read_only = false;
    let mut positional_only = false;
    let mut path: Option<PathBuf> = None;

    for arg in args {
        if !positional_only {
            match arg.as_str() {
                "--help" | "-h" => return Ok(Command::Help),
                "--version" | "-V" => return Ok(Command::Version),
                "--read-only" => {
                    read_only = true;
                    continue;
                }
                "--" => {
                    positional_only = true;
                    continue;
                }
                // `-` is a path (it will just not exist), not an option.
                _ if arg.starts_with('-') && arg != "-" => {
                    return Err(format!("unknown option: {arg}"));
                }
                _ => {}
            }
        }
        if path.is_some() {
            return Err(format!("expected exactly one file, got an extra: {arg}"));
        }
        path = Some(PathBuf::from(arg));
    }

    match path {
        Some(path) => Ok(Command::Edit(Options { path, read_only })),
        None => Err("missing file".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|item| (*item).to_string()).collect()
    }

    #[test]
    fn parses_a_path_and_the_read_only_flag() {
        assert_eq!(
            parse(&args(&["--read-only", "src/main.rs"])),
            Ok(Command::Edit(Options {
                path: PathBuf::from("src/main.rs"),
                read_only: true,
            }))
        );
    }

    #[test]
    fn a_double_dash_ends_options() {
        assert_eq!(
            parse(&args(&["--", "--read-only"])),
            Ok(Command::Edit(Options {
                path: PathBuf::from("--read-only"),
                read_only: false,
            }))
        );
    }

    #[test]
    fn rejects_unknown_options_and_extra_paths() {
        assert!(parse(&args(&["--wat", "a.rs"])).is_err());
        assert!(parse(&args(&["a.rs", "b.rs"])).is_err());
        assert!(parse(&[]).is_err());
    }

    #[test]
    fn help_and_version_do_not_need_a_path() {
        assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(&args(&["-V"])), Ok(Command::Version));
    }
}
