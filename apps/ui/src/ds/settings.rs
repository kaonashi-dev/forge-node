//! The anatomy every settings page shares: a sticky header over sections,
//! each section a titled card whose rows are split by hairlines.

use rune::{El, ScrollSpec, Shadow, col, el, row, rule, text};
use rune_kit::Kit;

use super::metrics::{SPACE_2, SPACE_5, SPACE_6, SPACE_7, SPACE_8};

/// Past this a label and its control drift apart across a wide window.
const SETTINGS_COLUMN: f32 = 720.0;
/// A card's rows start this far in from its edge.
const SETTINGS_INSET: f32 = 20.0;
const ROW_PAD: f32 = SPACE_6;
const HEADER_FADE: f32 = SPACE_7;

/// A titled section; its children become the rows of one card.
pub struct Group {
    key: String,
    title: String,
    description: Option<String>,
    action: Option<El>,
    rows: Vec<El>,
}

#[must_use]
pub fn group(key: &str, title: &str, description: Option<&str>) -> Group {
    Group {
        key: key.to_string(),
        title: title.to_string(),
        description: description.map(str::to_string),
        action: None,
        rows: Vec::new(),
    }
}

impl Group {
    #[must_use]
    pub fn child(mut self, row: El) -> Self {
        self.rows.push(row);
        self
    }

    #[must_use]
    pub fn children(mut self, rows: impl IntoIterator<Item = El>) -> Self {
        self.rows.extend(rows);
        self
    }

    /// A control at the end of the section's title line, such as "Reset all".
    #[must_use]
    pub fn action(mut self, action: El) -> Self {
        self.action = Some(action);
        self
    }

    pub fn build(self, k: &Kit) -> El {
        let mut words = col().grow(1.0).min_w(0.0).gap(SPACE_2).child(
            text(self.title.clone())
                .role("heading")
                .font(k.ty.ui_medium)
                .color(k.c.text_strong),
        );
        if let Some(description) = self.description {
            words = words.child(text(description).font(k.ty.ui).color(k.c.text_muted).wrap());
        }
        let head = row()
            .w_full()
            .gap(SPACE_6)
            .items_end()
            .child(words)
            .children(self.action);
        let outer = col()
            .key(self.key.clone())
            .role("group")
            .label(self.title)
            .w_full()
            .gap(SPACE_5)
            .child(head);
        if self.rows.is_empty() {
            return outer;
        }
        outer.child(card(k, &format!("{}-card", self.key), self.rows))
    }
}

/// Rows in a raised card, a hairline between each pair.
pub fn card(k: &Kit, key: &str, rows: Vec<El>) -> El {
    let count = rows.len();
    let mut list = col()
        .key(key.to_string())
        .w_full()
        .px(SETTINGS_INSET)
        .radius(k.radius.lg)
        .fill(k.c.layer_01)
        .box_shadow(Shadow {
            x: 0.0,
            y: 0.0,
            blur: 0.0,
            spread: 0.5,
            color: k.c.border_muted,
            inset: true,
        });
    for (at, item) in rows.into_iter().enumerate() {
        list = list.child(col().w_full().py(ROW_PAD).child(item));
        if at + 1 < count {
            list = list.child(rule(0.5, k.c.border_base));
        }
    }
    list
}

/// A title and its explanation on the left, the control on the right,
/// centred on the words.
pub fn setting_row(k: &Kit, title: &str, description: Option<&str>, control: El) -> El {
    let mut words = col().gap(SPACE_2).grow(1.0).min_w(0.0).child(
        text(title.to_string())
            .font(k.ty.ui_medium)
            .color(k.c.text_base)
            .wrap(),
    );
    if let Some(description) = description {
        words = words.child(
            text(description.to_string())
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap(),
        );
    }
    row()
        .w_full()
        .gap(SPACE_6)
        .items_center()
        .child(words)
        .child(row().justify_end().items_center().child(control))
}

/// A read-only fact as a row's control.
pub fn setting_value(k: &Kit, value: impl Into<String>) -> El {
    text(value.into()).font(k.ty.ui).color(k.c.text_muted)
}

/// A page: its title and summary in a band that stays at the top while the
/// sections scroll under it.
pub fn settings_page(k: &Kit, key: &str, title: &str, summary: &str, body: El) -> El {
    let fade = el()
        .key("settings-page-fade")
        .pin(Some(0.0), None, Some(0.0), Some(-HEADER_FADE))
        .h(HEADER_FADE)
        .gradient(k.c.bg_base, k.c.bg_base.alpha(0.0));
    let head = col()
        .key("settings-page-head")
        .w_full()
        .pt(SPACE_8)
        .pb(SPACE_2)
        .fill(k.c.bg_base)
        .sticky_top(0.0)
        .child(
            col()
                .gap(SPACE_2)
                .child(
                    text(title.to_string())
                        .role("heading")
                        .font(rune::tok::TextStyle {
                            size: k.ty.body_medium.size + 1.0,
                            weight: 600.0,
                            ..k.ty.body_medium
                        })
                        .color(k.c.text_strong),
                )
                .child(
                    text(summary.to_string())
                        .font(k.ty.ui)
                        .color(k.c.text_muted)
                        .wrap(),
                ),
        )
        .child(fade);
    col().key("settings").w_full().h_full().min_h(0.0).child(
        col()
            .key(key.to_string())
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .px(SPACE_7)
            .items_center()
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .key("settings-column")
                    .w_full()
                    .max_w(SETTINGS_COLUMN)
                    .child(head)
                    .child(
                        col()
                            .key("settings-body")
                            .w_full()
                            .gap(SPACE_8)
                            .pt(SPACE_7)
                            .pb(SPACE_8 * 2.5)
                            .child(body),
                    ),
            ),
    )
}
