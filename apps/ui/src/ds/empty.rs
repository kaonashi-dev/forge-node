use rune::{El, col, text};
use rune_kit::Kit;

use super::metrics::{SPACE_2, SPACE_3, SPACE_5};

/// An empty place: what is missing, why, and what fills it.
pub struct Empty {
    key: String,
    title: String,
    why: Option<String>,
    actions: Vec<El>,
}

impl Empty {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            title: title.into(),
            why: None,
            actions: Vec::new(),
        }
    }

    #[must_use]
    pub fn why(mut self, why: impl Into<String>) -> Self {
        self.why = Some(why.into());
        self
    }

    #[must_use]
    pub fn action(mut self, action: El) -> Self {
        self.actions.push(action);
        self
    }

    #[must_use]
    pub fn build(self, k: &Kit) -> El {
        let mut view = col()
            .key(self.key)
            .role("status")
            .w_full()
            .gap(SPACE_3)
            .padding(SPACE_5)
            .center()
            .child(
                text(self.title)
                    .font(k.ty.ui_medium)
                    .color(k.c.text_base)
                    .align(rune::Align::Center),
            );
        if let Some(why) = self.why {
            view = view.child(
                text(why)
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint)
                    .wrap()
                    .align(rune::Align::Center),
            );
        }
        if !self.actions.is_empty() {
            view = view.child(
                col()
                    .pt(SPACE_3)
                    .gap(SPACE_2)
                    .items_center()
                    .children(self.actions),
            );
        }
        view
    }
}
