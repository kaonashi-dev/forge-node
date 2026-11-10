//! The app's modal surface: a fitted `rune_kit::Dialog` whose body scrolls
//! when the window cannot hold it, and a virtualized list for the palettes.

use std::borrow::Cow;

use rune::{El, GrowthSpec, ScrollSpec, col, row};
use rune_kit::{Dialog, Kit};

use crate::app::AppState;

/// A question with a field or two.
pub const WIDTH_PROMPT: f32 = 440.0;
pub const WIDTH_STANDARD: f32 = 560.0;
/// A table or an editor of rules.
pub const WIDTH_WIDE: f32 = 720.0;
const PADDING: f32 = 16.0;
const FOOTER_GAP: f32 = 8.0;

/// A dialog that is always fitted to its content, with a bounded scrolling
/// body and an optional footer pinned below it.
pub struct Modal {
    dialog: Dialog,
    width: f32,
    body: Vec<El>,
    footer: Vec<El>,
}

impl Modal {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            dialog: Dialog::new(key, title),
            width: WIDTH_STANDARD,
            body: Vec::new(),
            footer: Vec::new(),
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.dialog = self.dialog.description(description);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }

    pub fn click_outside(mut self, close: bool) -> Self {
        self.dialog = self.dialog.click_outside(close);
        self
    }

    pub fn on_close<S: 'static>(mut self, f: impl Fn(&mut S) + Clone + 'static) -> Self {
        self.dialog = self.dialog.on_close(f);
        self
    }

    pub fn child(mut self, body: El) -> Self {
        self.body.push(body);
        self
    }

    pub fn footer(mut self, buttons: impl IntoIterator<Item = El>) -> Self {
        self.footer.extend(buttons);
        self
    }

    pub fn build(self, s: &AppState, k: &Kit, layer: &mut Vec<El>) {
        let mut body = col()
            .key("modal-body")
            .w_full()
            .padding(PADDING)
            .gap(10.0)
            .children(self.body);
        if !self.footer.is_empty() {
            body = body.pb(0.0);
        }
        let scroll = col()
            .key("modal-scroll")
            .w_full()
            .max_h(s.modal_body_max_h())
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_edge_fade(12.0)
            .scroll_margin(12.0)
            .child(body);
        let width = self
            .width
            .min((s.window.get()[0] - 2.0 * PADDING).max(240.0));
        let mut dialog = self.dialog.width(width).open(true).fit().child(scroll);
        if !self.footer.is_empty() {
            dialog = dialog.child(
                row()
                    .key("modal-footer")
                    .w_full()
                    .px(PADDING)
                    .pb(PADDING)
                    .gap(FOOTER_GAP)
                    .justify_end()
                    .children(self.footer),
            );
        }
        dialog.build(k, layer);
    }
}

type RowFn<S> = Box<dyn Fn(&S, usize) -> El>;

/// A bounded, virtualized list for a modal body. Rows outside the viewport
/// are not built, and `reveal_selected` keeps the selected one in view.
pub struct ModalList<S: 'static> {
    key: String,
    count: usize,
    row_h: f32,
    max_h: f32,
    selected: Option<usize>,
    role: Option<&'static str>,
    label: Option<String>,
    row: Option<RowFn<S>>,
}

impl<S: 'static> ModalList<S> {
    #[must_use]
    pub fn new(key: impl Into<String>, count: usize, row_h: f32, max_h: f32) -> Self {
        Self {
            key: key.into(),
            count,
            row_h: row_h.max(1.0),
            max_h: max_h.max(row_h),
            selected: None,
            role: None,
            label: None,
            row: None,
        }
    }

    #[must_use]
    pub fn selected(mut self, index: Option<usize>) -> Self {
        self.selected = index.filter(|index| *index < self.count);
        self
    }

    #[must_use]
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = Some(role);
        self
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn row(mut self, row: impl Fn(&S, usize) -> El + 'static) -> Self {
        self.row = Some(Box::new(row));
        self
    }

    #[must_use]
    pub fn build(self) -> El {
        let ModalList {
            key,
            count,
            row_h,
            max_h,
            selected,
            role,
            label,
            row,
        } = self;
        let height = (count as f32 * row_h).min(max_h);
        let row_key = key.clone();
        let row = row.expect("a ModalList needs a row builder before build");
        let mut rows = col()
            .key(format!("{key}-list"))
            .w_full()
            .growth(GrowthSpec::default());
        if let Some(role) = role {
            rows = rows.role(role);
        }
        if let Some(label) = label {
            rows = rows.label(label);
        }
        rows = rows
            .virtual_rows(
                count,
                row_h,
                move |_: &S, index| Cow::Owned(format!("{row_key}-{index}")),
                move |s: &S, index| row(s, index),
            )
            .reveal_selected(selected.is_some());
        // Keys named by position change only with the count, which Rune
        // notices itself, so layout need not ask for every row's key.
        rows = rows.virtual_rows_version(0u8);
        col()
            .key(format!("{key}-scroll"))
            .w_full()
            .h(height)
            .shrink()
            .min_h(if count == 0 { 0.0 } else { row_h })
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_edge_fade(12.0)
            .scroll_margin(12.0)
            .child(rows)
    }
}
