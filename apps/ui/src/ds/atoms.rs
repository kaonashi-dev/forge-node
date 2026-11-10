use rune::{El, Icon, TextStyle, el, row, text};
use rune_kit::{ControlSize, IconButton, IconButtonVariant, Kit};

use super::metrics::{AVATAR, CHIP_H, CONTROL, DOT, SPACE_2, SPACE_3};
use super::tone::Tone;
use crate::theme::roles::Roles;

/// rune-kit's `IconButton` at Forge's two sizes; `label` is what a screen
/// reader says, so every one needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IconButtonSize {
    /// 28 with a 16 glyph: a bar's action.
    Regular,
    /// 20 with a 16 glyph: an action inside a row.
    Mini,
}

#[must_use]
pub fn icon_button(
    glyph: &'static Icon,
    label: impl Into<String>,
    size: IconButtonSize,
) -> IconButton {
    IconButton::new(glyph)
        .variant(IconButtonVariant::GhostMuted)
        .control_size(match size {
            IconButtonSize::Regular => ControlSize::Large,
            IconButtonSize::Mini => ControlSize::Small,
        })
        .label(label)
}

/// A short status in a tone: caption type on the tone's wash.
#[must_use]
pub fn chip(k: &Kit, roles: &Roles, label: impl Into<String>, tone: Tone) -> El {
    let label = label.into();
    let ink = tone.ink(k, roles);
    row()
        .role("status")
        .label(label.clone())
        .h(CHIP_H)
        .px(SPACE_3)
        .radius(k.radius.sm)
        .center_y()
        .fill(ink.soft)
        .child(
            text(label)
                .font(k.ty.small_medium)
                .color(ink.fg)
                .a11y_hidden(),
        )
}

/// A status point; the row it sits in says the status in words.
#[must_use]
pub fn dot(k: &Kit, roles: &Roles, tone: Tone) -> El {
    el().size(DOT, DOT)
        .radius(DOT / 2.0)
        .fill(tone.ink(k, roles).solid)
        .a11y_hidden()
}

/// The only place a project's hue is drawn: its mark on a wash of that hue.
#[must_use]
pub fn avatar(k: &Kit, roles: &Roles, hue: Option<usize>, mark: &str) -> El {
    const EDGE: f32 = 0.25;
    let ink = roles.hue(hue);
    row()
        .size(AVATAR, AVATAR)
        .radius(k.radius.sm)
        .center()
        .fill(ink.alpha(roles.hue_tint))
        .border(1.0, ink.alpha(EDGE))
        .child(
            text(mark.to_string())
                .font(TextStyle {
                    weight: 600.0,
                    ..k.ty.small
                })
                .color(ink),
        )
        .a11y_hidden()
}

/// A section's title in caption type, its count, and one action at the end.
#[must_use]
pub fn section_header(k: &Kit, title: &str, count: Option<usize>, action: Option<El>) -> El {
    row()
        .w_full()
        .h(CONTROL)
        .pl(SPACE_2)
        .gap(SPACE_3)
        .center_y()
        .child(
            text(title.to_string())
                .role("heading")
                .font(k.ty.small_medium)
                .color(k.c.text_faint),
        )
        .children(count.map(|count| {
            text(count.to_string())
                .font(k.ty.small)
                .color(k.c.text_faint)
                .a11y_hidden()
        }))
        .child(el().grow(1.0))
        .children(action)
}
