//! Argument parsing for the standalone binary.
//!
//! Hand-rolled: a handful of flags do not justify a parser dependency in the
//! one artifact that has to stay independently distributable. `+N` is the
//! convention every terminal editor honours, so a compiler error pasted into a
//! shell opens at the right line.

use std::path::PathBuf;

pub const USAGE: &str = "\
forge-editor — terminal editor (standalone spike)

Usage:
  forge-editor [--read-only] [+LINE | --line LINE] [--] <file>

Options:
  --read-only   Open the file without allowing edits.
  --line LINE   Put the caret on LINE (1-based); +LINE does the same.
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
    /// 1-based line to open on, the way a compiler error counts.
    pub line: Option<usize>,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    let mut read_only = false;
    let mut positional_only = false;
    let mut path: Option<PathBuf> = None;
    let mut line: Option<usize> = None;
    let mut expecting_line = false;

    for arg in args {
        if expecting_line {
            line = Some(parse_line(arg)?);
            expecting_line = false;
            continue;
        }
        if !positional_only {
            match arg.as_str() {
                "--help" | "-h" => return Ok(Command::Help),
                "--version" | "-V" => return Ok(Command::Version),
                "--read-only" => {
                    read_only = true;
                    continue;
                }
                "--line" => {
                    expecting_line = true;
                    continue;
                }
                "--" => {
                    positional_only = true;
                    continue;
                }
                _ if arg.starts_with('+') => {
                    line = Some(parse_line(&arg[1..])?);
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

    if expecting_line {
        return Err("--line needs a number".to_string());
    }
    match path {
        Some(path) => Ok(Command::Edit(Options {
            path,
            read_only,
            line,
        })),
        None => Err("missing file".to_string()),
    }
}

fn parse_line(raw: &str) -> Result<usize, String> {
    match raw.parse::<usize>() {
        Ok(line) if line > 0 => Ok(line),
        _ => Err(format!("not a line number: {raw}")),
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
                line: None,
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
                line: None,
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
    fn a_line_arrives_as_a_plus_number_or_a_flag() {
        for form in [&["+12", "a.rs"][..], &["--line", "12", "a.rs"][..]] {
            let parsed = parse(&args(form)).expect("valid");
            assert_eq!(
                parsed,
                Command::Edit(Options {
                    path: PathBuf::from("a.rs"),
                    read_only: false,
                    line: Some(12),
                })
            );
        }
        assert!(parse(&args(&["+0", "a.rs"])).is_err());
        assert!(parse(&args(&["--line", "a.rs"])).is_err());
    }

    #[test]
    fn help_and_version_do_not_need_a_path() {
        assert_eq!(parse(&args(&["--help"])), Ok(Command::Help));
        assert_eq!(parse(&args(&["-V"])), Ok(Command::Version));
    }
}
