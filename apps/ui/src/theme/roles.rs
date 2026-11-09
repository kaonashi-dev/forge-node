//! The chrome's colour roles, as `toCssVariables` derives them in the original client: read
//! from `themes.json` for a base, derived by `chrome_colors` for a custom
//! theme, and laid over rune-kit's tokens by `Roles::kit`.

use rune::Color;
use rune_kit::{Kit, PaletteSeed, StateColors};
use serde::Deserialize;

use super::Theme;
use super::custom::{Palette, contrast, hex, mix, parse_hex, pct};

pub const HUE_COUNT: usize = 9;
/// The slot a project wears until someone picks another: Violet.
pub const DEFAULT_HUE: usize = 8;

const CALM_HUES_DARK: [u32; HUE_COUNT] = [
    0x5cc8c0, 0x7aa2f7, 0x7ed69a, 0xd98ae0, 0xf28fad, 0xb5d86a, 0x6cc4f0, 0xe0c36a, 0xa78bfa,
];
const CALM_HUES_LIGHT: [u32; HUE_COUNT] = [
    0x0c6e68, 0x2f5fc9, 0x1f7343, 0x94379f, 0xb0355f, 0x536d0f, 0x146892, 0x86640b, 0x6d28d9,
];
const TEXT_FLOOR: f64 = 4.5;
const FOCUS_FLOOR: f64 = 3.0;

/// `toCssVariables`' derived chrome as hex, named as `themes.json` carries it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeColors {
    pub border: String,
    pub border_hi: String,
    pub border_strong: String,
    pub hover: String,
    pub selected: String,
    pub pressed: String,
    pub sidebar_hover: String,
    pub text_strong: String,
    pub fg_default: String,
    pub fg_muted: String,
    pub fg_subtle: String,
    pub focus_ring: String,
    pub accent_text: String,
    pub accent_fg: String,
    pub accent_soft: String,
    pub accent_hover: String,
    pub danger_text: String,
    pub danger_fg: String,
    pub danger_soft: String,
    pub success_text: String,
    pub success_fg: String,
    pub warning_text: String,
    pub warning_fg: String,
    pub info_text: String,
    pub info_fg: String,
    pub needs_you_text: String,
    pub needs_you_fg: String,
    pub needs_you_tint: String,
    pub diff_added_bg: String,
    pub diff_removed_bg: String,
    pub git_added: String,
    pub git_modified: String,
    pub git_deleted: String,
    pub git_untracked: String,
    pub git_conflict: String,
    pub git_ignored: String,
    pub tab: String,
    pub strip_rule: String,
    /// Percent of the accent a selected row's ground takes.
    pub select_tint: u8,
    /// Percent of a project hue behind its avatar.
    pub hue_tint: u8,
    pub hues: Vec<String>,
}

fn c(value: &str) -> u32 {
    parse_hex(value).unwrap_or(0)
}

/// `readableColor`: keep the hue while it clears `floor` on every ground,
/// else step toward `toward` (or whichever of black and white reads better).
fn readable(color: u32, grounds: &[u32], toward: u32, floor: f64) -> u32 {
    let score = |candidate: u32| {
        grounds
            .iter()
            .map(|&ground| contrast(candidate, ground))
            .fold(f64::INFINITY, f64::min)
    };
    if score(color) >= floor {
        return color;
    }
    let mut target = toward;
    if score(target) < floor {
        for candidate in [0, 0x00ff_ffff] {
            if score(candidate) > score(target) {
                target = candidate;
            }
        }
    }
    for step in 1..=25 {
        let candidate = mix(color, target, pct(f64::from(step) * 4.0));
        if score(candidate) >= floor {
            return candidate;
        }
    }
    if score(target) > score(color) {
        target
    } else {
        color
    }
}

fn on(fill: u32, bg: u32, text: u32) -> u32 {
    if contrast(text, fill) >= contrast(bg, fill) {
        text
    } else {
        bg
    }
}

fn light_ground(bg: u32) -> bool {
    contrast(bg, 0) > contrast(bg, 0x00ff_ffff)
}

/// The derivation `toCssVariables`, `semanticColors` and `projectHues` run,
/// for a palette `themes.json` does not carry.
#[must_use]
pub fn chrome_colors(p: &Palette) -> ChromeColors {
    let [bg, text, rail, sidebar, strip, surface, surface_hi, editor] = [
        &p.bg,
        &p.text,
        &p.rail,
        &p.sidebar,
        &p.strip,
        &p.surface,
        &p.surface_hi,
        &p.editor,
    ]
    .map(|value| c(value));
    let [accent, red, green, amber, needs_you, blue] =
        [&p.accent, &p.red, &p.green, &p.amber, &p.needs_you, &p.blue].map(|value| c(value));
    let toward_text = |amount: f64| mix(bg, text, pct(amount));
    let border = p.border.as_deref().map_or(toward_text(7.0), c);
    let hover = p
        .finish
        .as_ref()
        .and_then(|finish| finish.hover.as_deref())
        .map_or(toward_text(6.0), c);
    let selected = toward_text(14.0);
    let pressed = toward_text(20.0);
    let sidebar_hover = mix(sidebar, text, pct(6.0));
    let diff_added_bg = mix(editor, c(&p.git_added), pct(14.0));
    let diff_removed_bg = mix(editor, c(&p.git_deleted), pct(14.0));
    let needs_you_tint = mix(bg, needs_you, pct(14.0));
    let soft = |fill: u32| mix(bg, fill, pct(14.0));
    let mut grounds = vec![
        bg,
        rail,
        sidebar,
        surface,
        surface_hi,
        editor,
        hover,
        selected,
        pressed,
        sidebar_hover,
        mix(sidebar, text, pct(14.0)),
        diff_added_bg,
        diff_removed_bg,
        needs_you_tint,
        mix(bg, amber, pct(12.0)),
    ];
    grounds.extend([accent, red, green, amber, needs_you, blue].map(soft));
    for ground in [bg, sidebar, surface, surface_hi] {
        grounds.extend([accent, needs_you].map(|tint| mix(ground, tint, pct(13.0))));
    }
    let text_grounds = [rail, sidebar, bg, strip, surface, editor];
    let read = |color: u32| hex(readable(color, &grounds, text, TEXT_FLOOR));
    let foreground = |fill: u32| {
        let candidate = on(fill, bg, text);
        hex(if contrast(candidate, fill) >= TEXT_FLOOR {
            candidate
        } else {
            on(fill, 0, 0x00ff_ffff)
        })
    };
    ChromeColors {
        border: hex(border),
        border_hi: hex(toward_text(15.0)),
        border_strong: hex(toward_text(25.0)),
        hover: hex(hover),
        selected: hex(selected),
        pressed: hex(pressed),
        sidebar_hover: hex(sidebar_hover),
        text_strong: read(text),
        fg_default: read(c(p.fg_default.as_deref().unwrap_or(&p.text))),
        fg_muted: hex(readable(c(&p.muted), &text_grounds, text, TEXT_FLOOR)),
        fg_subtle: hex(readable(
            c(p.fg_subtle.as_deref().unwrap_or(&p.faint)),
            &text_grounds,
            text,
            TEXT_FLOOR,
        )),
        focus_ring: hex(readable(accent, &grounds, text, FOCUS_FLOOR)),
        accent_text: read(accent),
        accent_fg: foreground(accent),
        accent_soft: hex(soft(accent)),
        accent_hover: hex(mix(accent, text, pct(12.0))),
        danger_text: read(red),
        danger_fg: foreground(red),
        danger_soft: hex(soft(red)),
        success_text: read(green),
        success_fg: foreground(green),
        warning_text: read(amber),
        warning_fg: foreground(amber),
        info_text: read(blue),
        info_fg: foreground(blue),
        needs_you_text: read(p.needs_you_text.as_deref().map_or(needs_you, c)),
        needs_you_fg: foreground(needs_you),
        needs_you_tint: hex(needs_you_tint),
        diff_added_bg: hex(diff_added_bg),
        diff_removed_bg: hex(diff_removed_bg),
        git_added: read(c(&p.git_added)),
        git_modified: read(c(&p.git_modified)),
        git_deleted: read(c(&p.git_deleted)),
        git_untracked: read(c(&p.git_untracked)),
        git_conflict: read(c(&p.git_conflict)),
        git_ignored: read(c(&p.git_ignored)),
        tab: p
            .finish
            .as_ref()
            .map_or_else(|| hex(border), |finish| finish.tab.clone()),
        strip_rule: p
            .finish
            .as_ref()
            .map_or_else(|| p.surface.clone(), |finish| finish.rule.clone()),
        select_tint: if p.finish.is_some() { 8 } else { 14 },
        hue_tint: hue_tint(bg),
        hues: project_hues(p).into_iter().map(hex).collect(),
    }
}

fn hue_tint(bg: u32) -> u8 {
    if light_ground(bg) { 12 } else { 15 }
}

/// Every hue clears 4.5:1 as an avatar's initial on its own tint over the
/// sidebar, the card and the strip; the tint is made of the hue, so the
/// search repeats until the hue stops moving.
fn project_hues(p: &Palette) -> Vec<u32> {
    let bg = c(&p.bg);
    let source = if light_ground(bg) {
        CALM_HUES_LIGHT
    } else {
        CALM_HUES_DARK
    };
    let amount = pct(f64::from(hue_tint(bg)));
    let plain = [&p.sidebar, &p.bg, &p.strip, &p.surface].map(|value| c(value));
    let text = c(&p.text);
    source
        .iter()
        .map(|&color| {
            let mut hue = color;
            for _ in 0..4 {
                let grounds: Vec<u32> = plain
                    .iter()
                    .copied()
                    .chain(plain.iter().map(|&ground| mix(ground, hue, amount)))
                    .collect();
                let next = readable(hue, &grounds, text, TEXT_FLOOR);
                if next == hue {
                    break;
                }
                hue = next;
            }
            hue
        })
        .collect()
}

/// A status hue in its four uses: the fill, type set in it, type set on its
/// fill, and the quiet wash behind a callout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub solid: Color,
    pub text: Color,
    pub fg: Color,
    pub soft: Color,
}

/// Git status ink, already lifted to read on every chrome ground.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GitColors {
    pub added: Color,
    pub modified: Color,
    pub deleted: Color,
    pub untracked: Color,
    pub conflict: Color,
    pub ignored: Color,
}

/// The resolved theme as Rune colours: every role the original client's chrome paints with.
#[derive(Clone, Debug, PartialEq)]
pub struct Roles {
    pub light: bool,
    /// The window behind the sidebar and the card (`--bg-window`).
    pub rail: Color,
    pub sidebar: Color,
    /// The card's tab strip, halfway between rail and card.
    pub strip: Color,
    /// The card (`--bg-panel`).
    pub bg: Color,
    pub surface: Color,
    pub surface_hi: Color,
    /// Overlays and raised controls, a step above the card (`--bg-float`).
    pub float: Color,
    pub editor: Color,
    pub step: Color,
    pub border: Color,
    pub border_hi: Color,
    pub border_strong: Color,
    pub hover: Color,
    pub selected: Color,
    pub pressed: Color,
    pub sidebar_hover: Color,
    pub text_strong: Color,
    pub text: Color,
    pub muted: Color,
    pub subtle: Color,
    pub focus_ring: Color,
    pub accent: Tone,
    pub accent_hover: Color,
    pub danger: Tone,
    pub success: Tone,
    pub warning: Tone,
    pub info: Tone,
    /// Ember: a session waiting on a person.
    pub needs_you: Tone,
    /// A chosen row's ground: the accent at `select_tint`, over whatever holds the row.
    pub select: Color,
    /// The active tab's raised chip.
    pub tab: Color,
    /// The hairline under the tab strip.
    pub strip_rule: Color,
    pub diff_added_bg: Color,
    pub diff_removed_bg: Color,
    pub git: GitColors,
    pub hues: [Color; HUE_COUNT],
    /// Opacity of a hue's wash behind its avatar, 0–1.
    pub hue_tint: f32,
    pub term_bg: Color,
    pub term_fg: Color,
    pub ansi: [Color; 16],
}

fn color(value: &str) -> Color {
    rune::color::hex(value).unwrap_or(Color::BLACK)
}

impl Roles {
    #[must_use]
    pub fn new(theme: &Theme) -> Roles {
        let p = &theme.palette;
        let k = &theme.chrome;
        let bg = c(&p.bg);
        let soft = |fill: &str| color(&hex(mix(bg, c(fill), pct(14.0))));
        let tone = |solid: &str, text: &str, fg: &str| Tone {
            solid: color(solid),
            text: color(text),
            fg: color(fg),
            soft: soft(solid),
        };
        let accent = color(&p.accent);
        let hue = |slot: usize| k.hues.get(slot).map_or(accent, |value| color(value));
        let ansi = |slot: usize| p.ansi.get(slot).map_or(Color::BLACK, |value| color(value));
        Roles {
            light: theme.light,
            rail: color(&p.rail),
            sidebar: color(&p.sidebar),
            strip: color(&p.strip),
            bg: color(&p.bg),
            surface: color(&p.surface),
            surface_hi: color(&p.surface_hi),
            float: color(&hex(mix(bg, c(&p.surface), pct(40.0)))),
            editor: color(&p.editor),
            step: color(&p.step),
            border: color(&k.border),
            border_hi: color(&k.border_hi),
            border_strong: color(&k.border_strong),
            hover: color(&k.hover),
            selected: color(&k.selected),
            pressed: color(&k.pressed),
            sidebar_hover: color(&k.sidebar_hover),
            text_strong: color(&k.text_strong),
            text: color(&k.fg_default),
            muted: color(&k.fg_muted),
            subtle: color(&k.fg_subtle),
            focus_ring: color(&k.focus_ring),
            accent: Tone {
                soft: color(&k.accent_soft),
                ..tone(&p.accent, &k.accent_text, &k.accent_fg)
            },
            accent_hover: color(&k.accent_hover),
            danger: Tone {
                soft: color(&k.danger_soft),
                ..tone(&p.red, &k.danger_text, &k.danger_fg)
            },
            success: tone(&p.green, &k.success_text, &k.success_fg),
            warning: tone(&p.amber, &k.warning_text, &k.warning_fg),
            info: tone(&p.blue, &k.info_text, &k.info_fg),
            needs_you: Tone {
                soft: color(&k.needs_you_tint),
                ..tone(&p.needs_you, &k.needs_you_text, &k.needs_you_fg)
            },
            select: accent.alpha(f32::from(k.select_tint) / 100.0),
            tab: color(&k.tab),
            strip_rule: color(&k.strip_rule),
            diff_added_bg: color(&k.diff_added_bg),
            diff_removed_bg: color(&k.diff_removed_bg),
            git: GitColors {
                added: color(&k.git_added),
                modified: color(&k.git_modified),
                deleted: color(&k.git_deleted),
                untracked: color(&k.git_untracked),
                conflict: color(&k.git_conflict),
                ignored: color(&k.git_ignored),
            },
            hues: std::array::from_fn(hue),
            hue_tint: f32::from(k.hue_tint) / 100.0,
            term_bg: color(&p.term_bg),
            term_fg: color(&p.term_fg),
            ansi: std::array::from_fn(ansi),
        }
    }

    /// A project's hue; an unknown slot wears the default, as `--hue-default`.
    #[must_use]
    pub fn hue(&self, slot: Option<usize>) -> Color {
        self.hues[slot.filter(|&slot| slot < HUE_COUNT).unwrap_or(DEFAULT_HUE)]
    }

    /// rune-kit's tokens with the original client's roles laid over them: what rune-kit
    /// derives from bg/fg/accent alone is a different theme in every base.
    #[must_use]
    pub fn kit(&self) -> Kit {
        let mut kit = Kit::from_palette(PaletteSeed {
            bg: self.bg,
            fg: self.text_strong,
            accent: self.accent.solid,
        });
        let float = self.float;
        let select = kit.alpha(self.accent.solid, self.select.a);
        let state = |tone: Tone| StateColors {
            bg: tone.soft,
            fg: tone.text,
            border: kit.alpha(tone.solid, 0.4),
            icon: tone.solid,
        };
        let [success, warning, danger, info] =
            [self.success, self.warning, self.danger, self.info].map(state);
        let p = &mut kit.c;
        p.page = self.rail;
        p.bg_deep = self.rail;
        p.bg_base = self.bg;
        p.layer_01 = float;
        p.layer_02 = self.surface;
        p.layer_03 = self.surface_hi;
        p.layer_04 = self.step;
        p.bg_button_neutral = self.surface;
        p.bg_accent = self.accent.solid;
        p.text_strong = self.text_strong;
        p.text_base = self.text;
        p.std_text = self.text;
        p.text_muted = self.muted;
        p.text_weak = self.muted;
        p.text_faint = self.subtle;
        p.text_accent = self.accent.text;
        p.icon_base = self.text;
        p.std_icon_strong = self.text_strong;
        p.icon_muted = self.muted;
        p.std_icon = self.muted;
        p.icon_faint = self.subtle;
        p.icon_weaker = self.subtle;
        p.icon_accent = self.accent.text;
        p.border_muted = self.border;
        p.border_weak = self.border;
        p.border_base = self.border_hi;
        p.std_border = self.border_hi;
        p.border_strong = self.border_strong;
        p.border_hover = self.border_strong;
        p.border_focus = self.focus_ring;
        p.hover = self.hover;
        p.pressed = self.pressed;
        p.surface_hover = select;
        p.surface_base = self.surface;
        p.surface_weak = float;
        p.surface_raised = self.surface;
        p.surface_raised_base = self.surface;
        p.surface_raised_hover = self.surface_hi;
        p.success = success;
        p.warning = warning;
        p.danger = danger;
        p.info = info;
        kit
    }
}

#[cfg(test)]
mod tests {
    use super::super::custom::{self, ThemeColor, slider};
    use super::super::{bases, kit, roles, roles_for, theme};
    use super::*;

    fn hex_of(color: Color) -> String {
        color.hex()[..7].to_string()
    }

    #[test]
    fn the_port_derives_what_tokens_ts_emits_for_every_base() {
        for id in bases() {
            let found = theme(id).expect("listed");
            assert_eq!(
                chrome_colors(&found.palette),
                found.chrome,
                "{id}: toCssVariables drifted from this port"
            );
        }
    }

    #[test]
    fn every_base_paints_each_role_in_its_tokens_ts_colour() {
        for id in bases() {
            let found = theme(id).expect("listed");
            let (p, k) = (&found.palette, &found.chrome);
            let r = roles_for(id, true);
            let pairs: Vec<(&str, Color, &str)> = vec![
                ("rail", r.rail, &p.rail),
                ("sidebar", r.sidebar, &p.sidebar),
                ("strip", r.strip, &p.strip),
                ("bg", r.bg, &p.bg),
                ("surface", r.surface, &p.surface),
                ("surfaceHi", r.surface_hi, &p.surface_hi),
                ("editor", r.editor, &p.editor),
                ("accent", r.accent.solid, &p.accent),
                ("needsYou", r.needs_you.solid, &p.needs_you),
                ("red", r.danger.solid, &p.red),
                ("green", r.success.solid, &p.green),
                ("amber", r.warning.solid, &p.amber),
                ("blue", r.info.solid, &p.blue),
                ("termBg", r.term_bg, &p.term_bg),
                ("termFg", r.term_fg, &p.term_fg),
                ("border", r.border, &k.border),
                ("borderHi", r.border_hi, &k.border_hi),
                ("borderStrong", r.border_strong, &k.border_strong),
                ("hover", r.hover, &k.hover),
                ("selected", r.selected, &k.selected),
                ("pressed", r.pressed, &k.pressed),
                ("textStrong", r.text_strong, &k.text_strong),
                ("fgDefault", r.text, &k.fg_default),
                ("fgMuted", r.muted, &k.fg_muted),
                ("fgSubtle", r.subtle, &k.fg_subtle),
                ("focusRing", r.focus_ring, &k.focus_ring),
                ("accentText", r.accent.text, &k.accent_text),
                ("accentSoft", r.accent.soft, &k.accent_soft),
                ("dangerText", r.danger.text, &k.danger_text),
                ("dangerSoft", r.danger.soft, &k.danger_soft),
                ("needsYouText", r.needs_you.text, &k.needs_you_text),
                ("needsYouTint", r.needs_you.soft, &k.needs_you_tint),
                ("gitAdded", r.git.added, &k.git_added),
                ("gitModified", r.git.modified, &k.git_modified),
                ("gitDeleted", r.git.deleted, &k.git_deleted),
                ("gitUntracked", r.git.untracked, &k.git_untracked),
                ("gitConflict", r.git.conflict, &k.git_conflict),
                ("gitIgnored", r.git.ignored, &k.git_ignored),
                ("tab", r.tab, &k.tab),
                ("stripRule", r.strip_rule, &k.strip_rule),
            ];
            for (role, got, want) in pairs {
                assert_eq!(hex_of(got), want.to_ascii_lowercase(), "{id} {role}");
            }
            for (slot, want) in k.hues.iter().enumerate() {
                assert_eq!(hex_of(r.hues[slot]), *want, "{id} hue {slot}");
            }
            assert_eq!(r.hue(None), r.hues[DEFAULT_HUE]);
            for (slot, want) in p.ansi.iter().enumerate() {
                assert_eq!(
                    hex_of(r.ansi[slot]),
                    want.to_ascii_lowercase(),
                    "{id} ansi {slot}"
                );
            }

            let painted = kit(id, true);
            for (token, got, want) in [
                ("bg_deep", painted.c.bg_deep, r.rail),
                ("bg_base", painted.c.bg_base, r.bg),
                ("bg_accent", painted.c.bg_accent, r.accent.solid),
                ("border_muted", painted.c.border_muted, r.border),
                ("hover", painted.c.hover, r.hover),
                ("text_strong", painted.c.text_strong, r.text_strong),
                ("text_base", painted.c.text_base, r.text),
                ("text_muted", painted.c.text_muted, r.muted),
                ("text_faint", painted.c.text_faint, r.subtle),
                ("danger.fg", painted.c.danger.fg, r.danger.text),
                ("warning.icon", painted.c.warning.icon, r.warning.solid),
            ] {
                assert_eq!(got, want, "{id} kit {token}");
            }
            assert_eq!(
                *roles(&painted),
                *r,
                "{id}: the kit leads back to its roles"
            );
        }
    }

    #[test]
    fn a_custom_theme_overrides_the_base_it_came_from() {
        let source = theme("calm-dark").expect("calm-dark").palette.clone();
        let edited = custom::with_color(&source, ThemeColor::Accent, "#7aa2f7").expect("valid");
        let edited = custom::with_color(&edited, ThemeColor::Bg, "#202830").expect("valid");
        let json = custom::variant("Mine", edited, 80).to_json();
        let base = roles_for("calm-dark", true);
        let mine = roles_for(&json, true);
        let found = theme(&json).expect("custom resolves");
        assert_eq!(found.chrome, chrome_colors(&found.palette));
        assert_eq!(hex_of(mine.accent.solid), "#7aa2f7");
        assert_eq!(hex_of(mine.bg), "#202830");
        assert_eq!(hex_of(mine.rail), found.palette.rail);
        assert_ne!(mine.rail, base.rail);
        assert_ne!(mine.muted, base.muted, "contrast 80 lifts secondary text");
        assert_eq!(kit(&json, true).c.bg_accent, mine.accent.solid);
        assert_eq!(*roles(&kit(&json, true)), *mine);
    }

    #[test]
    fn an_unknown_base_paints_as_the_default() {
        assert_eq!(
            *roles_for("no-such-base", false),
            *roles_for("calm-light", false)
        );
        assert_eq!(
            *roles_for("no-such-base", true),
            *roles_for("calm-dark", true)
        );
    }

    #[test]
    fn the_contrast_slider_is_continuous_and_never_compounds() {
        assert_eq!(slider::level(0.0), 0);
        assert_eq!(slider::level(0.555), 56);
        assert_eq!(slider::level(2.0), 100);
        assert_eq!(slider::level(f32::NAN), custom::DEFAULT_CONTRAST);
        assert_eq!(slider::level(slider::position(73)), 73);
        let start = custom::variant("Mine", theme("ocean").expect("ocean").palette.clone(), 60);
        let dragged = (0..=100).fold(start.clone(), |at, level| slider::at(&at, level));
        assert_eq!(dragged, slider::at(&start, 100));
        assert_eq!(slider::at(&dragged, 60).palette, start.palette);
    }
}
