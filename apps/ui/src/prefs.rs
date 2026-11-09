//! This window's own choices, in `rune-ui.json` under the Forge home. Read and
//! written by the client worker, never the UI thread.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "rune-ui.json";
/// A preferences file this large was not written by this app.
const MAX_BYTES: u64 = 64 * 1024;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// A `tokens.ts` base; absent follows the system.
    #[serde(default)]
    pub theme_base: Option<String>,
}

#[must_use]
pub fn path() -> Option<PathBuf> {
    let inputs = client::paths::PathInputs::from_env();
    client::paths::resolve(&inputs)
        .ok()
        .map(|paths| paths.home.join(FILE))
}

/// The saved choices, or the defaults when there are none or they do not read:
/// a damaged file costs a theme, never a launch.
#[must_use]
pub fn load_from(path: &Path) -> Prefs {
    let Ok(meta) = std::fs::metadata(path) else {
        return Prefs::default();
    };
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Prefs::default();
    }
    let mut text = String::new();
    let read =
        std::fs::File::open(path).and_then(|file| file.take(MAX_BYTES).read_to_string(&mut text));
    if read.is_err() {
        return Prefs::default();
    }
    serde_json::from_str(&text).unwrap_or_default()
}

/// Writes beside the target and renames over it, so a crash mid-write leaves
/// the previous choices rather than half a file.
pub fn save_to(path: &Path, prefs: &Prefs) -> std::io::Result<()> {
    let text = serde_json::to_string_pretty(prefs).map_err(std::io::Error::other)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let staging = path.with_extension("json.tmp");
    std::fs::write(&staging, text)?;
    std::fs::rename(&staging, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("forge-ui-prefs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join(FILE)
    }

    #[test]
    fn a_saved_choice_reads_back() {
        let path = scratch("round-trip");
        let prefs = Prefs {
            theme_base: Some("gruvbox".into()),
        };
        save_to(&path, &prefs).expect("saves");
        assert_eq!(load_from(&path), prefs);
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn a_missing_damaged_or_oversized_file_is_the_default() {
        let path = scratch("damaged");
        assert_eq!(load_from(&path), Prefs::default());
        std::fs::create_dir_all(path.parent().expect("dir")).expect("dir");
        std::fs::write(&path, "{ not json").expect("write");
        assert_eq!(load_from(&path), Prefs::default());
        std::fs::write(&path, vec![b' '; (MAX_BYTES + 1) as usize]).expect("write");
        assert_eq!(load_from(&path), Prefs::default());
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }
}
