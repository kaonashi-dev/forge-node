//! `ui.editor.autosave` is a preference of this window, and the editor process
//! is what acts on it: each live editor session is told once per change, as
//! the original client's `createEditorAutosaveSync`.

use std::collections::HashMap;

use domain::SessionId;

#[derive(Default)]
pub struct AutosaveSync {
    applied: HashMap<SessionId, bool>,
}

impl AutosaveSync {
    /// Forgets what was told, after a reconnect or a refused request.
    pub fn reset(&mut self) {
        self.applied.clear();
    }

    /// The sessions among `live` that have not been told `enabled` yet; they
    /// count as told from here on, and sessions gone from `live` are forgotten.
    pub fn due(&mut self, live: &[SessionId], enabled: bool) -> Vec<SessionId> {
        self.applied.retain(|session, _| live.contains(session));
        live.iter()
            .copied()
            .filter(|session| self.applied.insert(*session, enabled) != Some(enabled))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_live_editor_is_told_once_per_change() {
        let (a, b) = (SessionId::new(), SessionId::new());
        let mut sync = AutosaveSync::default();
        assert_eq!(sync.due(&[a], false), vec![a]);
        assert_eq!(sync.due(&[a], false), Vec::<SessionId>::new());
        assert_eq!(sync.due(&[a, b], false), vec![b]);
        assert_eq!(sync.due(&[a, b], true), vec![a, b]);
        assert_eq!(sync.due(&[b], true), Vec::<SessionId>::new());
        assert_eq!(sync.due(&[a, b], true), vec![a], "a left and came back");
        sync.reset();
        assert_eq!(sync.due(&[a, b], true), vec![a, b]);
    }
}
