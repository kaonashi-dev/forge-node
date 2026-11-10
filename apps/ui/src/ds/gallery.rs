//! Every `ds` piece in the states a still frame can show, in a dark and a
//! light base side by side: `forge-ui --gallery`, or `--gallery --snap
//! out.png` for a review image.

use rune::{Cx, El, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Keybind, Kit};

use super::{
    Emphasis, Empty, IconButtonSize, Row, SPACE_4, SPACE_5, SPACE_6, SPACE_7, Tab, TabShape, Tone,
    avatar, chip, dot, icon_button, section_header, toolbar,
};
use crate::glyphs;
use crate::theme::roles::Roles;

pub const BASES: [&str; 2] = ["calm-dark", "calm-light"];
const TONES: [(Tone, &str); 7] = [
    (Tone::Neutral, "clean"),
    (Tone::Accent, "#12"),
    (Tone::Success, "approved"),
    (Tone::Warning, "changes ↑2"),
    (Tone::Danger, "conflict"),
    (Tone::Info, "↓3"),
    (Tone::NeedsYou, "Needs you"),
];
const PANEL_W: f32 = 340.0;

pub fn view(_: &(), _: &mut Cx<()>) -> El {
    row()
        .key("gallery")
        .w_full()
        .h_full()
        .children(BASES.map(|base| {
            let k = crate::theme::kit(base, base.ends_with("dark"));
            panel(&k, &crate::theme::roles(&k), base)
        }))
}

fn group(k: &Kit, title: &str, body: El) -> El {
    col()
        .w_full()
        .gap(SPACE_4)
        .child(section_header(k, title, None, None))
        .child(body)
}

fn panel(k: &Kit, roles: &Roles, base: &str) -> El {
    let nothing = |_: &mut ()| {};
    let rows = col()
        .w(PANEL_W)
        .gap(super::SPACE_1)
        .child(
            Row::new(format!("{base}-row-project"), "techne")
                .emphasis(Emphasis::Strong)
                .leading(rune::icon(&glyphs::CHEVRON_DOWN, super::ICON_SMALL).color(k.c.icon_faint))
                .leading(avatar(k, roles, Some(8), "T"))
                .action(
                    icon_button(&glyphs::PLUS, "New worktree", IconButtonSize::Mini)
                        .on_press(nothing)
                        .build(k),
                )
                .build(k, roles),
        )
        .child(
            Row::new(format!("{base}-row-checkout"), "main")
                .indent(1)
                .leading(rune::icon(&glyphs::GIT, super::ICON_ROW).color(k.c.icon_muted))
                .meta("main checkout", Tone::Neutral)
                .trailing(chip(k, roles, "changes ↑2", Tone::Warning))
                .build(k, roles),
        )
        .child(
            Row::new(format!("{base}-row-rest"), "zsh")
                .indent(2)
                .emphasis(Emphasis::Quiet)
                .leading(rune::icon(&glyphs::TERMINAL, super::ICON_ROW).color(k.c.icon_muted))
                .meta("finished", Tone::Neutral)
                .build(k, roles),
        )
        .child(
            Row::new(format!("{base}-row-selected"), "zsh · ll")
                .indent(2)
                .emphasis(Emphasis::Quiet)
                .selected(true)
                .leading(rune::icon(&glyphs::TERMINAL, super::ICON_ROW).color(k.c.icon_base))
                .trailing(dot(k, roles, Tone::Success))
                .action(
                    icon_button(&glyphs::CLOSE, "Close", IconButtonSize::Mini)
                        .on_press(nothing)
                        .build(k),
                )
                .build(k, roles),
        )
        .child(
            Row::new(format!("{base}-row-needs-you"), "Agent · sidebar modal")
                .indent(2)
                .emphasis(Emphasis::Quiet)
                .leading(
                    rune::icon(&glyphs::NEEDS_YOU, super::ICON_ROW).color(roles.needs_you.solid),
                )
                .meta("Needs you", Tone::NeedsYou)
                .build(k, roles),
        );
    let tabs = row()
        .gap(super::SPACE_1)
        .child(Tab::new(format!("{base}-tab"), "zsh", &glyphs::TERMINAL).build(k, roles))
        .child(
            Tab::new(format!("{base}-tab-chosen"), "zsh · ll", &glyphs::TERMINAL)
                .chosen(true)
                .close(k, format!("{base}-tab-close"), "Close".into(), nothing)
                .build(k, roles),
        )
        .child(
            Tab::new(format!("{base}-tab-waiting"), "agent", &glyphs::NEEDS_YOU)
                .tint(roles.needs_you.text)
                .build(k, roles),
        );
    let views = row()
        .w(PANEL_W)
        .gap(super::SPACE_1)
        .padding(super::SPACE_1)
        .radius(k.radius.lg)
        .fill(roles.surface)
        .children(
            [
                (&glyphs::PROJECTS, "Projects", true),
                (&glyphs::FILES, "Files", false),
                (&glyphs::HISTORY, "History", false),
                (&glyphs::GIT, "Git", false),
            ]
            .map(|(glyph, name, chosen)| {
                Tab::new(format!("{base}-view-{name}"), name, glyph)
                    .shape(TabShape::IconUntilChosen { width: 34.0 })
                    .chosen(chosen)
                    .build(k, roles)
            }),
        );
    let crumbs = row()
        .gap(SPACE_4)
        .center_y()
        .child(
            text("techne / main /")
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        )
        .child(text("~/d/techne").font(k.ty.mono).color(k.c.text_base));
    let buttons = row()
        .gap(SPACE_4)
        .center_y()
        .child(
            Button::new("Create")
                .variant(ButtonVariant::Contrast)
                .build(k),
        )
        .child(Button::new("Neutral").build(k))
        .child(
            Button::new("Outline")
                .variant(ButtonVariant::Outline)
                .build(k),
        )
        .child(Button::new("Ghost").variant(ButtonVariant::Ghost).build(k))
        .child(
            Button::new("Remove")
                .variant(ButtonVariant::Danger)
                .build(k),
        )
        .child(
            Button::new("Shell")
                .icon(&glyphs::TERMINAL)
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .build(k),
        );
    let marks = row()
        .gap(SPACE_4)
        .center_y()
        .children(TONES.map(|(tone, _)| dot(k, roles, tone)))
        .child(el().w(SPACE_4))
        .children((0..crate::theme::roles::HUE_COUNT).map(|slot| {
            avatar(
                k,
                roles,
                Some(slot),
                &char::from(b'A' + slot as u8).to_string(),
            )
        }))
        .child(el().w(SPACE_4))
        .child(Keybind::new(&["⌘K"]).build(k))
        .child(icon_button(&glyphs::SETTINGS, "Settings", IconButtonSize::Regular).build(k))
        .child(icon_button(&glyphs::MORE_HORIZONTAL, "More", IconButtonSize::Mini).build(k));
    col()
        .key(format!("gallery-{base}"))
        .w(0.0)
        .grow(1.0)
        .h_full()
        .padding(SPACE_7)
        .gap(SPACE_6)
        .fill(k.c.bg_base)
        .child(
            text(base.to_string())
                .role("heading")
                .font(k.ty.body_medium)
                .color(k.c.text_strong),
        )
        .child(group(
            k,
            "Chips",
            row()
                .gap(SPACE_4)
                .flex_wrap()
                .children(TONES.map(|(tone, label)| chip(k, roles, label, tone))),
        ))
        .child(group(k, "Dots · avatars · kbd · icon buttons", marks))
        .child(group(k, "Rows", rows))
        .child(group(
            k,
            "Tabs",
            col().gap(SPACE_5).child(views).child(tabs),
        ))
        .child(group(
            k,
            "Toolbar",
            toolbar(
                &format!("{base}-toolbar"),
                vec![crumbs],
                vec![chip(k, roles, "2 ahead", Tone::Info)],
            ),
        ))
        .child(group(k, "Buttons", buttons))
        .child(
            Empty::new(format!("{base}-empty"), "No sessions yet")
                .why("Open a shell or start an agent in this checkout.")
                .action(
                    Button::new("New shell")
                        .variant(ButtonVariant::Contrast)
                        .shortcut("⌘T")
                        .build(k),
                )
                .build(k),
        )
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    #[test]
    fn every_piece_draws_in_both_bases() {
        let mut rt = Runtime::new(
            (),
            super::view,
            (1180.0, 900.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.frame(0.0);
        for base in super::BASES {
            for key in [
                "row-selected",
                "row-needs-you",
                "tab-chosen",
                "toolbar",
                "empty",
            ] {
                assert!(
                    rt.find(&format!("{base}-{key}")).is_some(),
                    "{base}-{key} is laid out"
                );
            }
        }
    }
}
