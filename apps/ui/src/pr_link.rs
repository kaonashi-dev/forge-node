//! Review Pull Request…: a pasted link opens the same detail a sidebar row
//! opens. One the daemon has not listed is read first, off this thread.

use rune::{El, TextEdit, col, text};
use rune_kit::{Button, ButtonVariant, Kit, TextInput};

use crate::app::AppState;
use crate::pr_url::PullRequestLink;
use crate::workbench::Panel;

pub struct LinkDialog {
    pub url: TextEdit,
    /// The pull request the daemon is reading; the dialog waits on it.
    pub pending: Option<PullRequestLink>,
    pub error: Option<String>,
}

impl LinkDialog {
    fn new() -> Self {
        let mut url = TextEdit::new("");
        url.focused = true;
        Self {
            url,
            pending: None,
            error: None,
        }
    }
}

impl AppState {
    pub fn open_pr_link(&mut self) {
        self.pr_link = Some(LinkDialog::new());
    }

    pub fn submit_pr_link(&mut self) {
        let Some(dialog) = self.pr_link.as_mut() else {
            return;
        };
        if dialog.pending.is_some() {
            return;
        }
        let link = match crate::pr_url::parse(&dialog.url.text) {
            Ok(link) => link,
            Err(error) => {
                dialog.error = Some(error);
                return;
            }
        };
        if self.open_listed(&link) {
            return;
        }
        let Some(dialog) = self.pr_link.as_mut() else {
            return;
        };
        let Some(connection) = &self.connection else {
            dialog.error = Some("Not connected to the daemon.".into());
            return;
        };
        match connection.fetch_pull_request(link.clone()) {
            Ok(()) => {
                dialog.pending = Some(link);
                dialog.error = None;
            }
            Err(error) => dialog.error = Some(error),
        }
    }

    /// The press on the sidebar row, with the Pull Requests view brought forward.
    fn open_listed(&mut self, link: &PullRequestLink) -> bool {
        let Some(key) = self.store.as_ref().and_then(|store| {
            store
                .pull_requests
                .pull_requests
                .iter()
                .find(|pr| link.names(pr))
                .map(crate::pr_filters::key)
        }) else {
            return false;
        };
        self.pr_link = None;
        self.show_panel(Panel::PullRequests);
        self.open_pr_detail(key);
        true
    }

    /// `PullRequestFetched`. The state carrying the pull request arrived just before it.
    pub fn pr_link_fetched(
        &mut self,
        host: &str,
        repository: &str,
        number: u32,
        error: Option<String>,
    ) {
        let Some(link) = self
            .pr_link
            .as_ref()
            .and_then(|dialog| dialog.pending.clone())
        else {
            return;
        };
        if !link.is(host, repository, number) {
            return;
        }
        let error = match error {
            Some(error) => Some(error),
            None if self.open_listed(&link) => None,
            None => Some("GitHub answered, but the pull request did not arrive; try again.".into()),
        };
        if let (Some(error), Some(dialog)) = (error, self.pr_link.as_mut()) {
            dialog.pending = None;
            dialog.error = Some(error);
        }
    }

    /// The daemon refused the read before it started, e.g. a host not in `[github]`.
    pub fn pr_link_refused(&mut self, message: &str) {
        if let Some(dialog) = self.pr_link.as_mut() {
            dialog.pending = None;
            dialog.error = Some(message.to_owned());
        }
    }
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(dialog) = &s.pr_link else {
        return;
    };
    let waiting = dialog.pending.is_some();
    let note = match (&dialog.error, &dialog.pending) {
        (Some(error), _) => error.clone(),
        (None, Some(link)) => format!(
            "Reading {}#{} from {}…",
            link.repository, link.number, link.host
        ),
        (None, None) => "Opens the pull request's details, where Review starts a read-only \
                         agent review."
            .into(),
    };
    let body = col()
        .w_full()
        .gap(crate::ds::SPACE_6)
        .child(
            TextInput::new(&dialog.url)
                .label("Pull request link")
                .placeholder("https://github.com/owner/repo/pull/123")
                .width(None)
                .on_edit(|s: &mut AppState| match s.pr_link.as_mut() {
                    Some(dialog) => {
                        dialog.error = None;
                        &mut dialog.url
                    }
                    None => &mut s.scratch_edit,
                })
                .on_submit(|s: &mut AppState, _| s.submit_pr_link())
                .build(k)
                .key("pr-link-url"),
        )
        .child(
            text(note)
                .key("pr-link-note")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(if dialog.error.is_some() {
                    k.c.warning.fg
                } else {
                    k.c.text_faint
                })
                .wrap()
                .pb(crate::ds::SPACE_4),
        );
    crate::modal::Modal::new("pr-link", "Review a pull request")
        .width(crate::modal::WIDTH_STANDARD)
        .on_close(|s: &mut AppState| s.pr_link = None)
        .child(body)
        .footer([
            Button::new("Cancel")
                .variant(ButtonVariant::GhostMuted)
                .on_press(|s: &mut AppState| s.pr_link = None)
                .build(k)
                .key("pr-link-cancel"),
            Button::new(if waiting { "Opening…" } else { "Open" })
                .variant(ButtonVariant::Contrast)
                .disabled(waiting || dialog.url.text.trim().is_empty())
                .on_press(|s: &mut AppState| s.submit_pr_link())
                .build(k)
                .key("pr-link-open"),
        ])
        .build(s, k, layer);
}
