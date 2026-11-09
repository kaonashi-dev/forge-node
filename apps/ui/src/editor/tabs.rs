//! The Code strip's bookkeeping: the files closed in each checkout, for ⇧⌘T,
//! and the step between editors for ⌥⌘←/→, as Tauri's `viewsStore`.

use std::collections::HashMap;

use domain::{SessionId, WorkspaceId};

/// "Reopen the last thing I closed" is a few steps of undo, not a history.
const CLOSED_DEPTH: usize = 16;

#[derive(Default)]
pub struct Closed {
    by_workspace: HashMap<WorkspaceId, Vec<String>>,
}

impl Closed {
    pub fn remember(&mut self, workspace: WorkspaceId, path: &str) {
        let stack = self.by_workspace.entry(workspace).or_default();
        stack.retain(|held| held != path);
        stack.push(path.to_string());
        if stack.len() > CLOSED_DEPTH {
            stack.remove(0);
        }
    }

    /// The last file closed in `workspace`, taken off its stack.
    pub fn take(&mut self, workspace: WorkspaceId) -> Option<String> {
        self.by_workspace.get_mut(&workspace)?.pop()
    }
}

/// The editor `delta` steps from `active` along `editors`, wrapping; from
/// outside the strip a step forward lands on the first and back on the last.
#[must_use]
pub fn step(editors: &[SessionId], active: Option<SessionId>, delta: isize) -> Option<SessionId> {
    if editors.is_empty() {
        return None;
    }
    let count = editors.len() as isize;
    let next = match active.and_then(|id| editors.iter().position(|editor| *editor == id)) {
        Some(at) => (at as isize + delta).rem_euclid(count),
        None if delta < 0 => count - 1,
        None => 0,
    };
    Some(editors[next as usize])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_closed_file_comes_back_last_first_and_only_once() {
        let (here, there) = (WorkspaceId::new(), WorkspaceId::new());
        let mut closed = Closed::default();
        closed.remember(here, "a.rs");
        closed.remember(here, "b.rs");
        closed.remember(here, "a.rs");
        closed.remember(there, "c.rs");
        assert_eq!(closed.take(here).as_deref(), Some("a.rs"));
        assert_eq!(closed.take(here).as_deref(), Some("b.rs"));
        assert_eq!(closed.take(here), None, "a.rs was remembered once");
        assert_eq!(closed.take(there).as_deref(), Some("c.rs"));
        for n in 0..40 {
            closed.remember(here, &format!("{n}.rs"));
        }
        let mut count = 0;
        while closed.take(here).is_some() {
            count += 1;
        }
        assert_eq!(count, CLOSED_DEPTH);
    }

    #[test]
    fn steps_wrap_and_enter_the_strip_at_its_ends() {
        let ids: Vec<SessionId> = (0..3).map(|_| SessionId::new()).collect();
        assert_eq!(step(&ids, Some(ids[2]), 1), Some(ids[0]));
        assert_eq!(step(&ids, Some(ids[0]), -1), Some(ids[2]));
        assert_eq!(step(&ids, None, 1), Some(ids[0]));
        assert_eq!(step(&ids, Some(SessionId::new()), -1), Some(ids[2]));
        assert_eq!(step(&[], None, 1), None);
    }
}
