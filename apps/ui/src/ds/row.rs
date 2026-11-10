use rune::{El, row, text};
use rune_kit::Kit;

use super::metrics::{CONTROL, INDENT, SPACE_2, SPACE_4};
use super::tone::Tone;
use crate::theme::roles::Roles;

/// What a crowded row keeps of its title before its meta gives way.
const TITLE_MIN_W: f32 = 56.0;

/// How loudly a row's title reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Emphasis {
    /// A parent in a tree: label weight.
    Strong,
    #[default]
    Plain,
    /// Muted until chosen: a leaf among many.
    Quiet,
}

/// One line of a list or tree with fixed slots: leading marks, a title that
/// truncates, a meta caption, trailing status, and actions shown only while
/// the row is hovered, focused or chosen. Handlers go on the built element.
pub struct Row {
    key: String,
    role: &'static str,
    label: Option<String>,
    indent: u8,
    height: f32,
    leading: Vec<El>,
    title: String,
    emphasis: Emphasis,
    meta: Option<(String, Tone)>,
    trailing: Vec<El>,
    actions: Vec<El>,
    selected: bool,
}

impl Row {
    pub fn new(key: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            role: "button",
            label: None,
            indent: 0,
            height: CONTROL,
            leading: Vec::new(),
            title: title.into(),
            emphasis: Emphasis::Plain,
            meta: None,
            trailing: Vec::new(),
            actions: Vec::new(),
            selected: false,
        }
    }

    #[must_use]
    pub fn role(mut self, role: &'static str) -> Self {
        self.role = role;
        self
    }

    /// The accessible name, when the title alone does not say enough.
    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = Some(label.into());
        self
    }

    #[must_use]
    pub fn indent(mut self, levels: u8) -> Self {
        self.indent = levels;
        self
    }

    /// The density's row height; `CONTROL` until set.
    #[must_use]
    pub fn height(mut self, height: f32) -> Self {
        self.height = height;
        self
    }

    #[must_use]
    pub fn leading(mut self, mark: El) -> Self {
        self.leading.push(mark);
        self
    }

    #[must_use]
    pub fn emphasis(mut self, emphasis: Emphasis) -> Self {
        self.emphasis = emphasis;
        self
    }

    /// A caption after the title; a tone other than `Neutral` colours it.
    #[must_use]
    pub fn meta(mut self, meta: impl Into<String>, tone: Tone) -> Self {
        self.meta = Some((meta.into(), tone));
        self
    }

    #[must_use]
    pub fn trailing(mut self, mark: El) -> Self {
        self.trailing.push(mark);
        self
    }

    /// A control revealed while the row is hovered, focused or chosen, so a
    /// long list does not repeat it on every line.
    #[must_use]
    pub fn action(mut self, control: El) -> Self {
        self.actions.push(control);
        self
    }

    #[must_use]
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    #[must_use]
    pub fn build(self, k: &Kit, roles: &Roles) -> El {
        let (font, ink) = match self.emphasis {
            Emphasis::Strong => (k.ty.ui_medium, k.c.text_base),
            Emphasis::Plain if self.selected => (k.ty.ui_medium, k.c.text_strong),
            Emphasis::Plain => (k.ty.ui, k.c.text_base),
            Emphasis::Quiet if self.selected => (k.ty.ui_medium, k.c.text_strong),
            Emphasis::Quiet => (k.ty.ui, k.c.text_muted),
        };
        let reveal = !self.actions.is_empty();
        let mut line = row()
            .key(self.key)
            .role(self.role)
            .label(self.label.unwrap_or_else(|| self.title.clone()))
            .checked(self.selected)
            .w_full()
            .h(self.height)
            .pl(SPACE_2 + f32::from(self.indent) * INDENT)
            .pr(SPACE_2)
            .gap(SPACE_4)
            .radius(k.radius.md)
            .center_y()
            .children(self.leading)
            .child(
                text(self.title)
                    .font(font)
                    .color(ink)
                    .truncate()
                    .min_w(TITLE_MIN_W)
                    .grow(1.0),
            );
        if self.selected {
            line = line.fill(roles.surface);
        }
        if let Some((meta, tone)) = self.meta {
            let ink = match tone {
                Tone::Neutral => k.c.text_faint,
                tone => tone.ink(k, roles).fg,
            };
            line = line.child(
                text(meta)
                    .font(k.ty.small_medium)
                    .color(ink)
                    .truncate()
                    .shrink(),
            );
        }
        line = line.children(self.trailing);
        if reveal {
            let shown = if self.selected { 1.0 } else { 0.0 };
            line = line.hover_group_focus().children(
                self.actions
                    .into_iter()
                    .map(|action| action.group_opacity(shown, 1.0)),
            );
        }
        k.interactive(line)
    }
}
