//! Tauri's `ThemePreview`: a terminal under the theme, its sixteen ANSI
//! slots, and the nine project hues as the rail draws them.

use rune::{El, TextStyle, col, el, row, text};
use rune_kit::Kit;

use super::roles::{HUE_COUNT, Roles};

const SWATCH_H: f32 = 14.0;
const AVATAR: f32 = 18.0;
const HUE_EDGE: f32 = 0.25;

/// Every colour comes from `roles`, so a theme not yet applied (from
/// `theme::roles_for`) previews as it would paint; `k` only sets type.
pub fn preview(k: &Kit, roles: &Roles) -> El {
    let mono = TextStyle {
        size: k.ty.text_xsmall.size,
        ..k.ty.mono
    };
    let strong = TextStyle {
        weight: 600.0,
        ..mono
    };
    let span =
        |value: &str, style: TextStyle, color| text(value.to_string()).font(style).color(color);
    let line = |key: &'static str, parts: Vec<El>| row().key(key).children(parts);
    let toolbar = row()
        .key("theme-preview-toolbar")
        .w_full()
        .h(30.0)
        .px(12.0)
        .gap(6.0)
        .center_y()
        .border_sides([0.0, 0.0, 1.0, 0.0], roles.surface)
        .child(el().size(7.0, 7.0).radius(2.0).fill(roles.hue(None)))
        .child(el().size(6.0, 6.0).radius(3.0).fill(roles.success.solid))
        .child(
            text("Terminal 1")
                .font(k.ty.text_xsmall)
                .color(roles.subtle),
        );
    let terminal = col()
        .key("theme-preview-terminal")
        .w_full()
        .px(14.0)
        .py(12.0)
        .gap(2.0)
        .child(line(
            "prompt-context",
            vec![
                span("forge-node", strong, roles.ansi[4]),
                span(" on ", mono, roles.term_fg),
                span("main", mono, roles.ansi[13]),
                span(" via ", mono, roles.term_fg),
                span("v1.89.0", mono, roles.ansi[1]),
            ],
        ))
        .child(line(
            "prompt-command",
            vec![
                span("❯", mono, roles.ansi[2]),
                span(" scripts/dev check", mono, roles.term_fg),
            ],
        ))
        .child(line(
            "prompt-output",
            vec![span("   Finished in 41.2s", mono, roles.ansi[8])],
        ));
    let swatches = |from: usize| {
        row()
            .w_full()
            .gap(4.0)
            .children((from..from + 8).map(|slot| {
                el().key(format!("ansi-{slot}"))
                    .label(format!("ANSI {slot}"))
                    .h(SWATCH_H)
                    .grow(1.0)
                    .radius(3.0)
                    .fill(roles.ansi[slot])
            }))
    };
    let colors = col()
        .key("theme-preview-ansi")
        .label("Sixteen ANSI colors")
        .w_full()
        .px(14.0)
        .pb(14.0)
        .gap(4.0)
        .child(swatches(0))
        .child(swatches(8));
    let hues = row()
        .key("theme-preview-hues")
        .label("Project hues")
        .w_full()
        .px(14.0)
        .py(10.0)
        .gap(6.0)
        .center_y()
        .border_sides([1.0, 0.0, 0.0, 0.0], roles.surface)
        .child(
            text("Project hues")
                .font(k.ty.text_xsmall)
                .color(roles.subtle)
                .grow(1.0),
        )
        .children((0..HUE_COUNT).map(|slot| {
            avatar(k, roles, Some(slot), char::from(b'A' + slot as u8)).key(format!("hue-{slot}"))
        }));
    col()
        .key("theme-preview")
        .role("group")
        .label("Theme preview")
        .w_full()
        .fill(roles.strip)
        .border(1.0, roles.border)
        .radius(10.0)
        .clip_rounded(10.0)
        .child(toolbar)
        .child(terminal.fill(roles.term_bg))
        .child(colors.fill(roles.term_bg))
        .child(hues)
}

/// `.project-avatar.hued`: a letter in the project's hue on a wash of it.
pub fn avatar(k: &Kit, roles: &Roles, hue: Option<usize>, letter: char) -> El {
    let ink = roles.hue(hue);
    row()
        .size(AVATAR, AVATAR)
        .radius(5.0)
        .center()
        .fill(ink.alpha(roles.hue_tint))
        .border(1.0, ink.alpha(HUE_EDGE))
        .child(
            text(letter.to_string())
                .font(TextStyle {
                    size: 10.0,
                    weight: 600.0,
                    ..k.ty.text_xsmall
                })
                .color(ink),
        )
        .a11y_hidden()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_lays_out_sixteen_ansi_slots_and_nine_hues() {
        fn view(_: &(), _: &mut rune::Cx<()>) -> El {
            let k = super::super::kit("gruvbox", true);
            preview(&k, &super::super::roles(&k))
        }
        let env = rune::Env {
            system_dark: true,
            scale: 2.0,
        };
        let mut rt = rune::Runtime::new((), view, (520.0, 320.0), env);
        rt.frame(0.0);
        let keys: Vec<String> = rt
            .elements()
            .into_iter()
            .filter_map(|element| element.key)
            .collect();
        let laid_out = |key: String| assert!(keys.contains(&key), "{key} is missing");
        (0..16).for_each(|slot| laid_out(format!("ansi-{slot}")));
        (0..HUE_COUNT).for_each(|slot| laid_out(format!("hue-{slot}")));
    }
}
