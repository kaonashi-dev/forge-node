//! The commit box under the changed files: a
//! message, a button that stages everything and commits, and Juva only when
//! asked. Nothing here pushes.

use domain::{JuvaDraft, WorkspaceId};
use rune::{El, TextEdit, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;

#[derive(Default)]
pub struct CommitBox {
    /// The workspace the message was typed for; another one starts empty.
    pub workspace: Option<WorkspaceId>,
    pub message: TextEdit,
    pub drafting: bool,
    /// Juva's endpoint was configured but did not answer; the draft is local.
    pub fell_back: bool,
    pub committing: bool,
    pub error: Option<String>,
    /// The last commit landed; said until the next edit.
    pub committed: bool,
}

/// `Commit 3 files`.
#[must_use]
pub fn commit_label(files: usize) -> String {
    match files {
        0 => "Nothing to commit".to_string(),
        1 => "Commit 1 file".to_string(),
        n => format!("Commit {n} files"),
    }
}

impl CommitBox {
    /// Keeps the box on `workspace`, dropping a message typed for another.
    pub fn follow(&mut self, workspace: Option<WorkspaceId>) {
        if self.workspace != workspace {
            *self = Self {
                workspace,
                ..Self::default()
            };
        }
    }

    /// Fills the message from a draft, unless the person already typed one:
    /// a late draft must not overwrite their words.
    pub fn apply_draft(&mut self, workspace: WorkspaceId, draft: &JuvaDraft, fell_back: bool) {
        if self.workspace != Some(workspace) || !self.drafting {
            return;
        }
        self.drafting = false;
        self.fell_back = fell_back;
        if !self.message.text.trim().is_empty() {
            return;
        }
        let body = draft.body.trim();
        let message = if body.is_empty() {
            draft.title.trim().to_string()
        } else {
            format!("{}\n\n{body}", draft.title.trim())
        };
        let focused = self.message.focused;
        self.message = TextEdit::new(message);
        self.message.focused = focused;
    }

    /// The message to commit, or `None` while there is nothing to say.
    #[must_use]
    pub fn ready_message(&self) -> Option<String> {
        let message = self.message.text.trim();
        (!message.is_empty() && !self.committing).then(|| message.to_string())
    }
}

pub fn commit_box(s: &AppState, k: &Kit, files: usize) -> El {
    let commit = &s.commit;
    let note = if commit.committing {
        "Committing…".to_string()
    } else if commit.error.is_some() {
        "The daemon refused the commit.".to_string()
    } else if commit.committed {
        "Committed".to_string()
    } else if commit.drafting {
        "Juva is drafting a message…".to_string()
    } else if commit.fell_back {
        "Drafted locally: the Juva endpoint did not answer.".to_string()
    } else {
        "Stages every change above, then commits. Nothing is pushed.".to_string()
    };
    let area = rune_kit::TextArea::new(&commit.message)
        .placeholder("What this change does")
        .label("Commit message")
        .on_edit(|s: &mut AppState| &mut s.commit.message)
        .on_change(|s: &mut AppState, _| s.commit.committed = false)
        .on_key(|s: &mut AppState, ev, _| {
            // ⌘↩ commits; a bare Return is a new line.
            if ev.key == rune::Key::Enter && ev.mods.cmd {
                s.create_commit();
                return true;
            }
            false
        })
        .build(k)
        .key("commit-message")
        .h(96.0);
    let draft = Button::new("Draft with Juva")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.draft_commit())
        .build(k)
        .key("commit-draft");
    let go = Button::new(format!("{} ⌘↩", commit_label(files)))
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.create_commit())
        .build(k)
        .key("commit-go");
    let mut view = col()
        .key("commit-box")
        .w_full()
        .min_w(0.0)
        .gap(crate::ds::SPACE_3)
        .pt(6.0)
        .child(area)
        .child(
            row()
                .gap(crate::ds::SPACE_3)
                .items_center()
                .w_full()
                .min_w(0.0)
                .child(draft)
                .child(rune::el().grow(1.0).min_w(0.0))
                .child(go),
        )
        .child(
            text(note)
                .key("commit-note")
                .w_full()
                .role("status")
                .font(k.ty.text_xsmall)
                .color(if commit.error.is_some() {
                    k.c.warning.fg
                } else {
                    k.c.text_faint
                }),
        );
    if let Some(error) = &commit.error {
        view = view.child(
            text(error.clone())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    view
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(title: &str, body: &str) -> JuvaDraft {
        JuvaDraft {
            kind: domain::JuvaKind::CommitMessage,
            title: title.into(),
            body: body.into(),
            context: domain::ChangeContext {
                branch: None,
                default_branch: None,
                dirty: true,
                ahead: None,
                behind: None,
                files: Vec::new(),
                patch: String::new(),
                truncated: false,
            },
        }
    }

    fn open(workspace: WorkspaceId) -> CommitBox {
        let mut commit = CommitBox::default();
        commit.follow(Some(workspace));
        commit.drafting = true;
        commit
    }

    #[test]
    fn the_button_counts_staged_and_unstaged_files() {
        assert_eq!(commit_label(0), "Nothing to commit");
        assert_eq!(commit_label(1), "Commit 1 file");
        assert_eq!(commit_label(3), "Commit 3 files");
    }

    #[test]
    fn a_draft_fills_an_empty_message_once() {
        let workspace = WorkspaceId::new();
        let mut commit = open(workspace);
        commit.apply_draft(workspace, &draft("Fix the thing", "Because."), false);
        assert_eq!(commit.message.text, "Fix the thing\n\nBecause.");
        commit.apply_draft(workspace, &draft("Other", ""), false);
        assert_eq!(
            commit.message.text, "Fix the thing\n\nBecause.",
            "drafted once"
        );
    }

    #[test]
    fn a_late_draft_never_overwrites_what_was_typed() {
        let workspace = WorkspaceId::new();
        let mut commit = open(workspace);
        commit.message = TextEdit::new("my words");
        commit.apply_draft(workspace, &draft("Juva's words", ""), true);
        assert_eq!(commit.message.text, "my words");
        assert!(commit.fell_back);
        assert_eq!(commit.ready_message(), Some("my words".into()));
    }

    #[test]
    fn another_workspace_starts_an_empty_box() {
        let mut commit = open(WorkspaceId::new());
        commit.message = TextEdit::new("for the first one");
        commit.follow(Some(WorkspaceId::new()));
        assert_eq!(commit.ready_message(), None);
    }
}
