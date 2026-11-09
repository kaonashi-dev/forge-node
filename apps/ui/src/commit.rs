//! Committing a workspace: Juva drafts a message, a person edits it, and only
//! then is it committed. Nothing here pushes.

use domain::{JuvaDraft, WorkspaceId};
use rune::{El, TextEdit, row, text};
use rune_kit::{Button, ButtonVariant, Kit};

use crate::app::AppState;

#[derive(Default)]
pub struct CommitDialog {
    pub workspace: Option<WorkspaceId>,
    pub message: TextEdit,
    pub drafting: bool,
    /// Juva's endpoint was configured but did not answer; the draft is local.
    pub fell_back: bool,
    pub committing: bool,
    pub error: Option<String>,
}

impl CommitDialog {
    pub fn open(&mut self, workspace: WorkspaceId) {
        *self = Self {
            workspace: Some(workspace),
            message: TextEdit::new(""),
            drafting: true,
            ..Self::default()
        };
        self.message.focused = true;
    }

    pub fn close(&mut self) {
        *self = Self::default();
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

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let commit = &s.commit;
    if commit.workspace.is_none() {
        return;
    }
    let note = if commit.committing {
        "Committing…".to_string()
    } else if let Some(error) = &commit.error {
        format!("The daemon refused: {error}")
    } else if commit.drafting {
        "Juva is drafting a message…".to_string()
    } else if commit.fell_back {
        "Drafted locally: the Juva endpoint did not answer.".to_string()
    } else {
        "Edit the message, then commit. Nothing is pushed.".to_string()
    };
    let area = rune_kit::TextArea::new(&commit.message)
        .label("Commit message")
        .on_edit(|s: &mut AppState| &mut s.commit.message)
        .build(k)
        .key("commit-message")
        .h(180.0);
    let go = Button::new("Commit")
        .variant(ButtonVariant::Contrast)
        .on_press(|s: &mut AppState| s.create_commit())
        .build(k)
        .key("commit-go");
    rune_kit::Dialog::new("commit", "Commit changes")
        .open(true)
        .width(560.0)
        .fit()
        .on_close(|s: &mut AppState| s.commit.close())
        .child(area)
        .child(
            row()
                .pt(10.0)
                .gap(12.0)
                .items_center()
                .child(
                    text(note)
                        .key("commit-note")
                        .role("status")
                        .font(k.ty.text_xsmall)
                        .color(if commit.error.is_some() {
                            k.c.warning.fg
                        } else {
                            k.c.text_faint
                        })
                        .grow(1.0),
                )
                .child(go),
        )
        .build(k, layer);
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

    #[test]
    fn a_draft_fills_an_empty_message_once() {
        let workspace = WorkspaceId::new();
        let mut dialog = CommitDialog::default();
        dialog.open(workspace);
        dialog.apply_draft(workspace, &draft("Fix the thing", "Because."), false);
        assert_eq!(dialog.message.text, "Fix the thing\n\nBecause.");
        dialog.apply_draft(workspace, &draft("Other", ""), false);
        assert_eq!(
            dialog.message.text, "Fix the thing\n\nBecause.",
            "drafted once"
        );
    }

    #[test]
    fn a_late_draft_never_overwrites_what_was_typed() {
        let workspace = WorkspaceId::new();
        let mut dialog = CommitDialog::default();
        dialog.open(workspace);
        dialog.message = TextEdit::new("my words");
        dialog.apply_draft(workspace, &draft("Juva's words", ""), true);
        assert_eq!(dialog.message.text, "my words");
        assert!(dialog.fell_back);
        assert_eq!(dialog.ready_message(), Some("my words".into()));
    }

    #[test]
    fn a_draft_for_another_workspace_is_ignored() {
        let mut dialog = CommitDialog::default();
        dialog.open(WorkspaceId::new());
        dialog.apply_draft(WorkspaceId::new(), &draft("x", ""), false);
        assert!(dialog.drafting);
        assert_eq!(dialog.ready_message(), None);
    }
}
