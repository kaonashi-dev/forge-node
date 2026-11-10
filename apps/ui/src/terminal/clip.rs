//! What ⌘V hands a terminal when the clipboard holds more than text: a
//! terminal only takes text, so files and images travel as paths, which is
//! how an agent CLI attaches them.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use rune::{ClipboardContent, ClipboardImage};

use crate::terminal::links;

#[derive(Debug, PartialEq, Eq)]
pub enum Pasted<'a> {
    Text(String),
    Image(&'a ClipboardImage),
    Nothing,
}

/// Files win over text because Finder also offers each file's bare name as
/// text; an image is pasted only when nothing readable came with it.
#[must_use]
pub fn choose(content: &ClipboardContent) -> Pasted<'_> {
    if !content.files().is_empty() {
        return quoted_paths(content.files()).map_or(Pasted::Nothing, Pasted::Text);
    }
    if !content.text().is_empty() {
        return Pasted::Text(content.text().to_string());
    }
    content.image().map_or(Pasted::Nothing, Pasted::Image)
}

/// Each path shell-quoted, space-separated, with a trailing space so the
/// next word does not run into the last path; `None` when any path holds a
/// control character, which a bracketed paste could not carry safely.
#[must_use]
pub fn quoted_paths(files: &[PathBuf]) -> Option<String> {
    let words = files
        .iter()
        .map(|file| links::shell_quote(&file.display().to_string()))
        .collect::<Option<Vec<_>>>()
        .filter(|words| !words.is_empty())?;
    let mut text = words.join(" ");
    text.push(' ');
    Some(text)
}

#[must_use]
pub fn paste_dir() -> PathBuf {
    std::env::temp_dir().join("forge-paste")
}

/// `create_new`, so a name planted in the shared temp directory is never
/// written through.
pub fn save_image(image: &ClipboardImage, dir: &Path) -> std::io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    std::fs::create_dir_all(dir)?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_millis());
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("paste-{}-{stamp}-{n}.png", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    file.write_all(image.png())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png() -> ClipboardImage {
        let bytes = [
            &b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR"[..],
            &[0, 0, 0, 2, 0, 0, 0, 3],
        ]
        .concat();
        ClipboardImage::from_png(bytes).expect("a PNG header")
    }

    #[test]
    fn files_beat_their_names_and_text_beats_an_image() {
        let finder = ClipboardContent::new("shot.png").with_files([PathBuf::from("/a b/shot.png")]);
        assert_eq!(choose(&finder), Pasted::Text("'/a b/shot.png' ".into()));
        let both = ClipboardContent::new("alt").with_image(png());
        assert_eq!(choose(&both), Pasted::Text("alt".into()));
        let image = png();
        let alone = ClipboardContent::new("").with_image(image.clone());
        assert_eq!(choose(&alone), Pasted::Image(&image));
        assert_eq!(choose(&ClipboardContent::new("")), Pasted::Nothing);
    }

    #[test]
    fn a_path_with_a_control_character_pastes_nothing() {
        let files = [PathBuf::from("/ok"), PathBuf::from("/a\nb")];
        assert_eq!(quoted_paths(&files), None);
        assert_eq!(quoted_paths(&[]), None);
    }

    #[test]
    fn a_saved_image_is_its_png_under_a_fresh_name() {
        let dir = std::env::temp_dir().join(format!("forge-ui-clip-{}", std::process::id()));
        let image = png();
        let first = save_image(&image, &dir).expect("written");
        let second = save_image(&image, &dir).expect("written");
        assert_ne!(first, second);
        assert_eq!(std::fs::read(&first).expect("read back"), image.png());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
