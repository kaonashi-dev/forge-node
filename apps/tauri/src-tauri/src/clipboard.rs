//! The system clipboard, read and written by the host.
//!
//! The WebView cannot do either reliably: WKWebView refuses
//! `navigator.clipboard.readText()` even inside a gesture, and a write
//! delivered through `evaluateJavaScript` depends on WebKit treating that as a
//! user gesture — a detail of the implementation, not a contract. macOS reads
//! and writes `NSPasteboard` directly here; on a platform with no reader the
//! caller falls back to the WebView.

use std::sync::{Arc, Mutex};
use std::thread;

/// Text read from the pasteboard, already bounded.
pub struct ClipboardText {
    pub text: String,
    /// Bytes the pasteboard held when `text` was cut to the requested limit.
    pub total: Option<usize>,
}

/// Whether this build can read the system clipboard itself.
///
/// `false` sends a caller to its own fallback. Linux would need the WebView's
/// clipboard access enabled as a window attribute, which this build does not
/// set.
#[must_use]
pub fn can_read() -> bool {
    cfg!(target_os = "macos")
}

/// Read the clipboard's text, cut to `limit` bytes.
///
/// The length is checked against the limit *before* the bytes are converted,
/// so a pasteboard nobody meant to paste spends no memory on it. `Some(total)`
/// reports that text was dropped, which is what lets the caller say the paste
/// was partial instead of looking complete.
#[must_use]
pub fn read_text(limit: usize) -> Option<ClipboardText> {
    platform::read_text(limit)
}

/// Write `text` to the system clipboard.
pub fn write_text(text: &str) {
    platform::write_text(text);
}

/// Where a copy goes.
///
/// A seam so the bridge's rule — only a terminal this window shows may set the
/// clipboard — is testable without AppKit or a desktop session.
pub trait ClipboardSink: Send + Sync {
    fn store(&self, text: &str);
}

/// The clipboard writer thread, with latest-wins delivery.
///
/// A write can wait on the application that owns the pasteboard (a promised
/// type is produced on request), so it never runs on the bridge thread, which
/// also carries `RuntimeCommand::Input`. The pending slot holds one value and
/// a newer copy replaces it: the clipboard only ever holds the newest, and a
/// writer that fell behind must not replay copies nobody can see.
pub struct ClipboardWriter {
    pending: Arc<Mutex<Option<String>>>,
    /// One wake per write. The writer always drains it before blocking again,
    /// so a failed send only means a wake is already queued behind the value
    /// in `pending`.
    wake: flume::Sender<()>,
}

impl ClipboardWriter {
    #[must_use]
    pub fn spawn() -> Self {
        let pending = Arc::new(Mutex::new(None::<String>));
        let (wake, woke) = flume::bounded(1);
        let slot = Arc::clone(&pending);
        thread::Builder::new()
            .name("forge-clipboard".to_string())
            .spawn(move || {
                while woke.recv().is_ok() {
                    let Some(text) = lock(&slot).take() else {
                        continue;
                    };
                    write_text(&text);
                }
            })
            .expect("failed to start the clipboard writer");
        Self { pending, wake }
    }
}

impl ClipboardSink for ClipboardWriter {
    fn store(&self, text: &str) {
        if text.is_empty() {
            return;
        }
        *lock(&self.pending) = Some(text.to_string());
        let _ = self.wake.try_send(());
    }
}

/// Recovers from poisoning: a panicked writer must not lose the next copy.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(target_os = "macos")]
mod platform {
    use super::ClipboardText;
    use objc2::rc::autoreleasepool;
    use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
    use objc2_foundation::NSString;

    pub fn read_text(limit: usize) -> Option<ClipboardText> {
        // A background thread has no autorelease pool of its own, and the
        // pasteboard hands back autoreleased objects.
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let data = pasteboard.dataForType(unsafe { NSPasteboardTypeString })?;
            let length = data.len();
            if length == 0 {
                return None;
            }
            let bytes = unsafe { data.as_bytes_unchecked() };
            if length <= limit {
                return Some(ClipboardText {
                    text: String::from_utf8_lossy(bytes).into_owned(),
                    total: None,
                });
            }
            let mut cut = limit;
            while cut > 0 && bytes[cut] & 0xc0 == 0x80 {
                cut -= 1;
            }
            let text = String::from_utf8_lossy(&bytes[..cut]).into_owned();
            (!text.is_empty()).then_some(ClipboardText {
                text,
                total: Some(length),
            })
        })
    }

    pub fn write_text(text: &str) {
        autoreleasepool(|_| {
            let pasteboard = NSPasteboard::generalPasteboard();
            let string = NSString::from_str(text);
            pasteboard.clearContents();
            let _ = pasteboard.setString_forType(&string, unsafe { NSPasteboardTypeString });
        });
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::ClipboardText;

    /// No host-side reader on this platform yet: the caller's WebView fallback
    /// is what a paste goes through.
    pub fn read_text(_limit: usize) -> Option<ClipboardText> {
        None
    }

    /// No host-side writer yet, so a copy is a no-op. Linux needs either the
    /// WebView's clipboard access enabled as a window attribute or a native
    /// writer here.
    pub fn write_text(_text: &str) {}
}
