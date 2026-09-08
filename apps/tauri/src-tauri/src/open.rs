//! Desktop integrations: open a directory in an editor or file manager.
//!
//! Daemon-free side effects.rs` and `file_manager.rs`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Editor {
    Zed,
    Cursor,
}

impl Editor {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Zed => "Zed",
            Self::Cursor => "Cursor",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "zed" => Some(Self::Zed),
            "cursor" => Some(Self::Cursor),
            _ => None,
        }
    }

    const fn clis(self) -> &'static [&'static str] {
        match self {
            Self::Zed => &["zed"],
            Self::Cursor => &["cursor"],
        }
    }

    const fn bundles(self) -> &'static [&'static str] {
        match self {
            Self::Zed => &["Zed.app"],
            Self::Cursor => &["Cursor.app"],
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    Cli(PathBuf),
    Bundle(PathBuf),
}

pub fn locate(editor: Editor) -> Option<Launch> {
    if let Some(path) = editor.clis().iter().find_map(|cli| on_path(cli)) {
        return Some(Launch::Cli(path));
    }
    application_dirs()
        .iter()
        .flat_map(|dir| editor.bundles().iter().map(move |bundle| dir.join(bundle)))
        .find(|bundle| bundle.is_dir())
        .map(Launch::Bundle)
}

pub fn open_editor(editor: Editor, path: &Path) -> Result<(), String> {
    let launch = locate(editor).ok_or_else(|| format!("{} is not installed", editor.label()))?;
    let (program, args) = editor_invocation(&launch, path);
    Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("{}: {error}", program.display()))
}

pub fn open_file_manager(path: &Path) -> Result<(), String> {
    let (program, args) = file_manager_invocation(path);
    Command::new(&program)
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("{}: {error}", program.display()))
}

pub fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with("http://") && !url.starts_with("https://") {
        return Err("only http and https URLs are allowed".to_string());
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("/usr/bin/open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("open: {error}"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        Command::new("xdg-open")
            .arg(url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(|error| format!("xdg-open: {error}"))
    }
}

fn editor_invocation(launch: &Launch, path: &Path) -> (PathBuf, Vec<OsString>) {
    match launch {
        Launch::Cli(program) => (program.clone(), vec![path.into()]),
        Launch::Bundle(bundle) => (
            PathBuf::from("/usr/bin/open"),
            vec!["-a".into(), bundle.into(), path.into()],
        ),
    }
}

fn file_manager_invocation(path: &Path) -> (PathBuf, Vec<OsString>) {
    #[cfg(target_os = "macos")]
    {
        (PathBuf::from("/usr/bin/open"), vec![path.into()])
    }
    #[cfg(not(target_os = "macos"))]
    {
        (PathBuf::from("xdg-open"), vec![path.into()])
    }
}

fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable(candidate))
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![PathBuf::from("/Applications")];
    if let Some(home) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join("Applications"));
    }
    dirs
}
