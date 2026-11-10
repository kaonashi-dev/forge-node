//! Background outcomes. A toast is a receipt for something that already
//! happened, never a question: failures wait longer than successes, and every
//! one can be dismissed without moving focus into the region.

use std::time::{Duration, Instant};

use rune::El;
use rune_kit::Kit;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Success,
    Danger,
    Neutral,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub id: u64,
    pub tone: Tone,
    pub title: String,
    pub detail: Option<String>,
    shown: Instant,
    life: Duration,
}

const SUCCESS_FOR: Duration = Duration::from_secs(4);
const NEUTRAL_FOR: Duration = Duration::from_secs(4);
const DANGER_FOR: Duration = Duration::from_secs(8);
const VISIBLE: usize = 4;

#[derive(Default)]
pub struct Toasts {
    items: Vec<Toast>,
    next: u64,
}

impl Toasts {
    fn push(&mut self, tone: Tone, title: impl Into<String>, detail: Option<String>) {
        let id = self.next;
        self.next = self.next.wrapping_add(1);
        let life = match tone {
            Tone::Success => SUCCESS_FOR,
            Tone::Danger => DANGER_FOR,
            Tone::Neutral => NEUTRAL_FOR,
        };
        self.items.push(Toast {
            id,
            tone,
            title: title.into(),
            detail,
            shown: Instant::now(),
            life,
        });
        if self.items.len() > VISIBLE {
            self.items.remove(0);
        }
    }

    pub fn success(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(Tone::Success, title, detail);
    }

    pub fn danger(&mut self, title: impl Into<String>, detail: impl Into<String>) {
        let detail = detail.into();
        self.push(Tone::Danger, title, (!detail.is_empty()).then_some(detail));
    }

    pub fn neutral(&mut self, title: impl Into<String>, detail: Option<String>) {
        self.push(Tone::Neutral, title, detail);
    }

    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|toast| toast.id != id);
        before != self.items.len()
    }

    /// Drops what has been on screen long enough; true when the list changed.
    pub fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items
            .retain(|toast| now.saturating_duration_since(toast.shown) < toast.life);
        before != self.items.len()
    }

    #[must_use]
    pub fn visible(&self) -> &[Toast] {
        &self.items
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

/// The bottom-right region, mounted after every other overlay so a toast is
/// never hidden by a dialog.
pub fn layer(s: &crate::app::AppState, k: &Kit) -> Option<El> {
    if s.toasts.is_empty() {
        return None;
    }
    let mut region = rune_kit::ToastRegion::new();
    for toast in s.toasts.visible() {
        let id = toast.id;
        let icon = match toast.tone {
            Tone::Success => &crate::glyphs::DONE,
            Tone::Danger => &crate::glyphs::FAILED,
            Tone::Neutral => &crate::glyphs::PALETTE,
        };
        let mut built = rune_kit::Toast::new(format!("toast-{id}"), toast.title.clone())
            .icon(k, icon)
            .on_close(move |s: &mut crate::app::AppState| {
                s.toasts.dismiss(id);
            });
        if let Some(detail) = &toast.detail {
            built = built.description(detail.clone());
        }
        region = region.toast(built.build(k));
    }
    Some(region.build(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_toast_can_be_dismissed_and_expires_on_its_own() {
        let mut toasts = Toasts::default();
        toasts.success("Saved", None);
        toasts.danger("Commit failed", "not a git repository");
        assert_eq!(toasts.visible().len(), 2);
        let id = toasts.visible()[0].id;
        assert!(toasts.dismiss(id));
        assert!(!toasts.dismiss(id));
        assert!(!toasts.expire(Instant::now()));
        assert!(toasts.expire(Instant::now() + DANGER_FOR));
        assert!(toasts.is_empty());
    }

    #[test]
    fn the_oldest_toast_leaves_when_the_region_is_full() {
        let mut toasts = Toasts::default();
        for n in 0..6 {
            toasts.neutral(format!("note {n}"), None);
        }
        assert_eq!(toasts.visible().len(), VISIBLE);
        assert_eq!(toasts.visible()[0].title, "note 2");
    }
}
