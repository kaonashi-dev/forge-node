//! Settings › Theme: a base, the system's pair, or a custom theme stored as
//! JSON in the same `ui.theme_base` key. Each edit saves at once; choosing
//! the original base is the reset.

use rune::{El, FileDialogOptions, TextEdit, col, el, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit};

use crate::app::AppState;
use crate::theme::custom::{self, CustomTheme, MAX_THEME_BYTES, ThemeColor};
use crate::theme::{self, CUSTOM_KEY, SYSTEM, THEME_KEY};

const COLORS: [(ThemeColor, &str, &str); 3] = [
    (ThemeColor::Accent, "Accent", "accent"),
    (ThemeColor::Bg, "Background", "bg"),
    (ThemeColor::Text, "Foreground", "text"),
];
const CONTRAST_STEP: u8 = 5;

#[derive(Default)]
pub struct ThemeEditor {
    pub customizing: bool,
    /// Accent, background and foreground, in `COLORS` order.
    pub hex: [TextEdit; 3],
    pub source: TextEdit,
    pub error: Option<String>,
    pub status: Option<String>,
    /// The preference the hex fields were filled from.
    pub filled_for: Option<String>,
}

fn color_of(palette: &custom::Palette, key: ThemeColor) -> &str {
    match key {
        ThemeColor::Accent => &palette.accent,
        ThemeColor::Bg => &palette.bg,
        ThemeColor::Text => &palette.text,
    }
}

impl AppState {
    /// The theme being painted, as a custom theme so it can be edited: a base
    /// becomes one named after it.
    #[must_use]
    pub fn active_theme(&self, dark: bool) -> Option<CustomTheme> {
        let preference = self.theme_base(dark);
        if theme::is_custom(preference) {
            return custom::parse(preference).ok();
        }
        let found = theme::theme(preference)?;
        Some(CustomTheme {
            name: found.label.clone(),
            mode: if found.light { "light" } else { "dark" }.into(),
            palette: found.palette.clone(),
            adjustments: None,
        })
    }

    /// The custom theme on offer: the chosen one, else the last one saved.
    #[must_use]
    pub fn saved_custom(&self) -> Option<(String, CustomTheme)> {
        let chosen = self
            .theme_pref
            .as_deref()
            .filter(|pref| theme::is_custom(pref));
        let source = chosen.or_else(|| {
            self.store
                .as_ref()
                .and_then(|store| store.app_state_value(CUSTOM_KEY))
        })?;
        custom::parse(source)
            .ok()
            .map(|parsed| (source.to_string(), parsed))
    }

    /// Picks a base, `"system"` or a custom theme's JSON, and saves it where
    /// the daemon keeps it; a custom one is also kept as the last custom theme.
    pub fn choose_theme(&mut self, value: String) {
        let mut saved = self.write_pref_checked(THEME_KEY, &value);
        if saved.is_ok() && theme::is_custom(&value) {
            saved = self.write_pref_checked(CUSTOM_KEY, &value);
        }
        self.theme_pref = Some(value);
        self.theme_editor.error = saved.err();
        self.theme_editor.status = None;
    }

    /// The contrast as a drag moves, painted without a daemon write; releasing
    /// commits it, so a gesture is one `app_state` row rather than one per pixel.
    pub fn preview_contrast(&mut self, contrast: u8, dark: bool) {
        let Some(active) = self.active_theme(dark) else {
            return;
        };
        let edited = custom::variant(&active.name, active.origin().clone(), contrast);
        self.theme_pref = Some(edited.to_json());
        self.theme_editor.filled_for = None;
    }

    /// One edit of the active theme: a colour, or the contrast, or both.
    pub fn edit_theme(&mut self, color: Option<(ThemeColor, &str)>, contrast: u8, dark: bool) {
        let Some(active) = self.active_theme(dark) else {
            return;
        };
        let origin = active.origin().clone();
        let palette = match color {
            Some((key, value)) => match custom::with_color(&origin, key, value) {
                Ok(palette) => palette,
                Err(error) => {
                    self.theme_editor.error = Some(error);
                    self.theme_editor.filled_for = None;
                    return;
                }
            },
            None => origin,
        };
        let edited = custom::variant(&active.name, palette, contrast);
        self.choose_theme(edited.to_json());
        self.sync_theme_editor(dark, true);
    }

    pub fn import_theme(&mut self, source: &str) {
        match custom::parse(source) {
            Ok(parsed) => {
                self.choose_theme(parsed.to_json());
                self.theme_editor.source = TextEdit::new("");
                self.theme_editor.filled_for = None;
            }
            Err(error) => self.theme_editor.error = Some(error),
        }
    }

    /// Refills the hex fields when the theme changed under them, unless one
    /// is being typed in and the change did not come from it (`force`).
    pub fn sync_theme_editor(&mut self, dark: bool, force: bool) {
        let editor = &self.theme_editor;
        if !editor.customizing || (!force && editor.hex.iter().any(|edit| edit.focused)) {
            return;
        }
        let preference = self.theme_base(dark).to_string();
        if editor.filled_for.as_deref() == Some(preference.as_str()) {
            return;
        }
        let Some(active) = self.active_theme(dark) else {
            return;
        };
        for (index, (key, _, _)) in COLORS.iter().enumerate() {
            self.theme_editor.hex[index] =
                TextEdit::new(color_of(active.origin(), *key).to_ascii_uppercase());
        }
        self.theme_editor.filled_for = Some(preference);
    }
}

fn read_theme_file(path: &std::path::Path) -> Result<String, String> {
    use std::io::Read;
    let too_big = || "Theme JSON must be 16 KB or smaller.".to_string();
    let size = std::fs::metadata(path)
        .map_err(|error| error.to_string())?
        .len();
    if size > MAX_THEME_BYTES as u64 {
        return Err(too_big());
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .map_err(|error| error.to_string())?
        .take(MAX_THEME_BYTES as u64 + 1)
        .read_to_string(&mut text)
        .map_err(|_| "The file is not UTF-8 text.".to_string())?;
    if text.len() > MAX_THEME_BYTES {
        return Err(too_big());
    }
    Ok(text)
}

/// Ringed, so a swatch the colour of the ground it sits on still shows.
fn swatch(k: &Kit, color: &str, size: f32) -> El {
    el().w(size)
        .h(size)
        .radius(size / 2.0)
        .fill(rune::color::hex(color).unwrap_or(rune::Color::TRANSPARENT))
        .border(1.0, k.c.border_weak)
}

/// A miniature window in the palette's own grounds: rail, sidebar bars, the card.
fn card(k: &Kit, palette: &custom::Palette, label: &str, note: Option<&str>, chosen: bool) -> El {
    let c = |value: &str| rune::color::hex(value).unwrap_or(rune::Color::TRANSPARENT);
    let bar = |color: &str| el().h(4.0).w_full().radius(2.0).fill(c(color));
    let window = row()
        .h(52.0)
        .w_full()
        .gap(crate::ds::SPACE_3)
        .padding(6.0)
        .radius(k.radius.md)
        .fill(c(&palette.rail))
        .child(
            col()
                .w(28.0)
                .gap(crate::ds::SPACE_2)
                .child(bar(&palette.surface_hi))
                .child(bar(&palette.accent))
                .child(bar(&palette.surface_hi)),
        )
        .child(
            el().grow(1.0)
                .h_full()
                .radius(k.radius.sm)
                .fill(c(&palette.bg))
                .border(
                    1.0,
                    c(palette.border.as_deref().unwrap_or(&palette.surface)),
                ),
        );
    let mut name = row().gap(crate::ds::SPACE_3).items_center().child(
        text(label.to_string())
            .font(k.ty.small)
            .color(k.c.text_base)
            .truncate()
            .grow(1.0)
            .min_w(0.0),
    );
    if let Some(note) = note {
        name = name.child(
            text(note.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    col()
        .w(132.0)
        .gap(crate::ds::SPACE_3)
        .padding(6.0)
        .radius(k.radius.lg)
        .border(
            if chosen { 2.0 } else { 1.0 },
            if chosen {
                k.c.text_base
            } else {
                k.c.border_weak
            },
        )
        .child(window)
        .child(name)
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let dark = k.dark;
    let editor = &s.theme_editor;
    let preference = s.theme_pref.as_deref();
    let following = preference == Some(SYSTEM);
    let painted = s.theme_base(dark).to_string();
    let active = s.active_theme(dark);
    let match_system = rune_kit::Switch::new(following)
        .label("Match system")
        .on_toggle(move |s: &mut AppState| {
            if s.theme_pref.as_deref() == Some(SYSTEM) {
                let base = s.theme_base(dark).to_string();
                s.choose_theme(base);
            } else {
                s.choose_theme(SYSTEM.to_string());
            }
        })
        .build(k)
        .key("theme-system");
    let mut cards = row()
        .key("theme-options")
        .w_full()
        .gap(crate::ds::SPACE_4)
        .flex_wrap()
        .role("radiogroup")
        .label("Theme");
    for base in theme::bases() {
        let Some(found) = theme::theme(base) else {
            continue;
        };
        let chosen = !theme::is_custom(&painted) && painted == *base;
        let note = base
            .starts_with("calm-")
            .then_some(if found.light { "light" } else { "dark" });
        let id = base.clone();
        cards = cards.child(
            card(k, &found.palette, &found.label, note, chosen)
                .key(format!("theme-{base}"))
                .role("radio")
                .label(found.label.clone())
                .checked(chosen)
                .focusable(true)
                .on_press(move |s: &mut AppState| s.choose_theme(id.clone())),
        );
    }
    if let Some((source, saved)) = s.saved_custom() {
        let chosen = theme::is_custom(&painted);
        cards = cards.child(
            card(k, &saved.palette, &saved.name, Some("custom"), chosen)
                .key("theme-custom")
                .role("radio")
                .label(saved.name.clone())
                .checked(chosen)
                .focusable(true)
                .on_press(move |s: &mut AppState| s.choose_theme(source.clone())),
        );
    }
    let action = |label: &str| {
        Button::new(label.to_string())
            .variant(ButtonVariant::Ghost)
            .control_size(ControlSize::Small)
    };
    let customize = Button::new("Customize colors…")
        .variant(if editor.customizing {
            ButtonVariant::Outline
        } else {
            ButtonVariant::Ghost
        })
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| {
            s.theme_editor.customizing = !s.theme_editor.customizing;
            s.theme_editor.filled_for = None;
            s.sync_theme_editor(dark, true);
        })
        .build(k)
        .key("theme-customize");
    let import = action("Import JSON")
        .on_press_tx(|_s: &mut AppState, tx| {
            tx.open_file(
                FileDialogOptions {
                    title: "Import a theme".into(),
                    ..FileDialogOptions::default()
                }
                .filter("Theme JSON", &["json"]),
                |s: &mut AppState, answer, _tx| {
                    if let Ok(Some(path)) = answer {
                        match read_theme_file(&path) {
                            Ok(source) => s.import_theme(&source),
                            Err(error) => s.theme_editor.error = Some(error),
                        }
                    }
                },
            );
        })
        .build(k)
        .key("theme-import-file");
    let download = action("Download JSON")
        .on_press_tx(move |s: &mut AppState, tx| {
            let Some(json) = s.active_theme(dark).map(|theme| theme.to_pretty_json()) else {
                return;
            };
            tx.save_file(
                FileDialogOptions {
                    title: "Save the theme".into(),
                    file_name: Some("forge-theme.json".into()),
                    ..FileDialogOptions::default()
                }
                .filter("Theme JSON", &["json"]),
                move |s: &mut AppState, answer, _tx| {
                    if let Ok(Some(path)) = answer {
                        match std::fs::write(&path, format!("{json}\n")) {
                            Ok(()) => s.theme_editor.status = Some("Theme saved.".into()),
                            Err(error) => {
                                s.theme_editor.error =
                                    Some(format!("Could not save theme: {error}"));
                            }
                        }
                    }
                },
            );
        })
        .build(k)
        .key("theme-download");
    let mut view = col()
        .key("appearance")
        .gap(crate::ds::SPACE_5)
        .child(
            row()
                .items_center()
                .child(
                    text("Theme")
                        .font(k.ty.body_medium)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(match_system),
        )
        .child(cards)
        .child(
            row()
                .gap(crate::ds::SPACE_2)
                .child(customize)
                .child(import)
                .child(download),
        );

    let light = active.as_ref().is_some_and(|theme| theme.mode == "light");
    let current_accent = active
        .as_ref()
        .map(|theme| theme.origin().accent.to_ascii_lowercase());
    let contrast = active
        .as_ref()
        .map_or(custom::DEFAULT_CONTRAST, CustomTheme::contrast);
    let mut swatches = row()
        .key("accent-swatches")
        .gap(crate::ds::SPACE_4)
        .items_center()
        .role("radiogroup")
        .label("Accent");
    for accent in theme::accents(light) {
        let chosen = current_accent.as_deref() == Some(accent.color.as_str());
        let color = accent.color.clone();
        swatches = swatches.child(
            el().padding(2.0)
                .radius(12.0)
                .border(
                    2.0,
                    if chosen {
                        k.c.text_base
                    } else {
                        rune::Color::TRANSPARENT
                    },
                )
                .child(swatch(k, &accent.color, 18.0))
                .key(format!("accent-{}", accent.name.to_ascii_lowercase()))
                .role("radio")
                .label(accent.name.clone())
                .checked(chosen)
                .focusable(true)
                .on_press(move |s: &mut AppState| {
                    s.edit_theme(Some((ThemeColor::Accent, &color)), contrast, dark);
                }),
        );
    }
    view = view.child(
        row()
            .gap(crate::ds::SPACE_5)
            .items_center()
            .child(text("Accent").font(k.ty.small_medium).color(k.c.text_base))
            .child(swatches)
            .child(
                text("Ember stays reserved for “needs you”.")
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            ),
    );

    if editor.customizing {
        view = view.child(controls(s, k, active.as_ref(), contrast));
    }
    if let Some(error) = &editor.error {
        view = view.child(
            text(error.clone())
                .key("theme-error")
                .role("alert")
                .font(k.ty.small)
                .color(k.c.warning.fg),
        );
    }
    if let Some(status) = &editor.status {
        view = view.child(
            text(status.clone())
                .key("theme-status")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    view
}

fn controls(s: &AppState, k: &Kit, active: Option<&CustomTheme>, contrast: u8) -> El {
    let dark = k.dark;
    let mut rows = col().key("theme-controls").gap(crate::ds::SPACE_4);
    for (index, (key, label, id)) in COLORS.iter().copied().enumerate() {
        let shown = active.map_or("#000000", |theme| color_of(theme.origin(), key));
        let commit = move |s: &mut AppState| {
            let typed = s.theme_editor.hex[index].text.clone();
            if custom::normalize_color(&typed).is_none() {
                s.theme_editor.error = Some("Use a six-digit hex color, such as #458588.".into());
                s.theme_editor.filled_for = None;
                s.theme_editor.hex[index].focused = false;
                s.sync_theme_editor(dark, true);
                return;
            }
            let contrast = s
                .active_theme(dark)
                .map_or(custom::DEFAULT_CONTRAST, |theme| theme.contrast());
            s.edit_theme(Some((key, &typed)), contrast, dark);
        };
        let field = rune_kit::TextInput::new(&s.theme_editor.hex[index])
            .label(format!("{label} hex"))
            .width(Some(110.0))
            .spellcheck(false)
            .on_edit(move |s: &mut AppState| &mut s.theme_editor.hex[index])
            .on_submit(move |s: &mut AppState, _tx| commit(s))
            .on_key(move |s: &mut AppState, ev, _| {
                if ev.key == rune::Key::Escape {
                    s.theme_editor.hex[index].focused = false;
                    s.theme_editor.filled_for = None;
                    s.sync_theme_editor(dark, true);
                    return true;
                }
                false
            })
            .build(k)
            .key(format!("theme-{id}-hex"));
        rows = rows.child(
            row()
                .gap(10.0)
                .items_center()
                .child(
                    text(label.to_string())
                        .font(k.ty.small)
                        .color(k.c.text_muted)
                        .w(110.0),
                )
                .child(swatch(k, shown, 22.0))
                .child(field),
        );
    }
    if let Some(theme) = active {
        let mut ansi = row()
            .key("theme-ansi")
            .gap(crate::ds::SPACE_2)
            .items_center();
        for color in &theme.origin().ansi {
            ansi = ansi.child(swatch(k, color, 12.0));
        }
        rows = rows.child(
            row()
                .gap(10.0)
                .items_center()
                .child(
                    text("Terminal")
                        .font(k.ty.small)
                        .color(k.c.text_muted)
                        .w(110.0),
                )
                .child(ansi),
        );
    }
    let step = |label: &str, key: &str, next: u8| {
        Button::new(label.to_string())
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| s.edit_theme(None, next, dark))
            .build(k)
            .key(key.to_string())
    };
    let level = f32::from(contrast) / 100.0;
    let slider = el()
        .key("theme-contrast-slider")
        .role("slider")
        .label(format!("Contrast {contrast}"))
        .range_value(f64::from(contrast), 0.0, 100.0)
        .w(180.0)
        .h(20.0)
        .focusable(true)
        .on_drag(move |s: &mut AppState, drag: &mut rune::Drag, tx| {
            let width = drag.w.max(1.0);
            let next = ((drag.x / width).clamp(0.0, 1.0) * 100.0).round() as u8;
            if drag.phase == rune::Phase::Up {
                s.edit_theme(None, next, dark);
                return;
            }
            s.preview_contrast(next, dark);
            tx.instant();
        })
        .on_key(move |s: &mut AppState, ev, _| match ev.key {
            rune::Key::Left | rune::Key::Down => {
                s.edit_theme(None, contrast.saturating_sub(1), dark);
                true
            }
            rune::Key::Right | rune::Key::Up => {
                s.edit_theme(None, contrast.saturating_add(1).min(100), dark);
                true
            }
            _ => false,
        })
        .child(
            el().absolute(0.0, 7.0)
                .w_full()
                .h(6.0)
                .radius(3.0)
                .fill(k.c.layer_03),
        )
        .child(
            el().absolute(0.0, 7.0)
                .w(180.0 * level)
                .h(6.0)
                .radius(3.0)
                .fill(k.c.bg_accent),
        )
        .child(
            el().absolute(180.0 * level - 6.0, 4.0)
                .size(12.0, 12.0)
                .radius(k.radius.md)
                .fill(k.c.text_strong)
                .border(1.0, k.c.border_strong),
        );
    rows = rows.child(
        row()
            .key("theme-contrast")
            .gap(10.0)
            .items_center()
            .role("group")
            .label(format!("Contrast {contrast}"))
            .child(
                text("Contrast")
                    .font(k.ty.small)
                    .color(k.c.text_muted)
                    .w(110.0),
            )
            .child(slider)
            .child(
                text(contrast.to_string())
                    .font(k.ty.small_medium)
                    .color(k.c.text_base)
                    .w(32.0),
            )
            .child(step(
                "−",
                "theme-contrast-down",
                contrast.saturating_sub(CONTRAST_STEP),
            ))
            .child(step(
                "+",
                "theme-contrast-up",
                contrast.saturating_add(CONTRAST_STEP).min(100),
            ))
            .child(
                text(
                    "Drag for fine contrast. Arrow keys move one step; the buttons move five. \
                     Choose the original theme to reset your changes.",
                )
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .grow(1.0)
                .min_w(0.0)
                .wrap(),
            ),
    );
    let paste = rune_kit::TextArea::new(&s.theme_editor.source)
        .label("Paste theme JSON")
        .placeholder("Paste theme JSON")
        .spellcheck(false)
        .on_edit(|s: &mut AppState| &mut s.theme_editor.source)
        .build(k)
        .key("theme-source")
        .h(96.0);
    let apply = Button::new("Import and apply")
        .variant(ButtonVariant::Outline)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| {
            let source = s.theme_editor.source.text.clone();
            if !source.trim().is_empty() {
                s.import_theme(&source);
            }
        })
        .build(k)
        .key("theme-import");
    let copy = Button::new("Copy theme")
        .variant(ButtonVariant::Ghost)
        .control_size(ControlSize::Small)
        .on_press_tx(move |s: &mut AppState, tx| {
            if let Some(theme) = s.active_theme(dark) {
                tx.copy(theme.to_pretty_json());
                s.theme_editor.error = None;
                s.theme_editor.status = Some("Theme copied.".into());
            }
        })
        .build(k)
        .key("theme-copy");
    rows.child(paste)
        .child(row().gap(crate::ds::SPACE_3).child(apply).child(copy))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connection::fixture_store;
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn a_chosen_theme_survives_the_next_replica_rebuild() {
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let mut state = AppState::for_bench(
            store,
            Arc::new(crate::connection::Metrics::default()),
            crate::terminal::SharedTerminal::new(),
        );
        let base = state.active_theme(true).expect("a painted theme");
        let custom = custom::variant("Mine", base.palette, 0).to_json();
        state.choose_theme(custom.clone());
        let session = state.store.as_ref().expect("store").sessions[0].clone();
        state.apply_events(
            vec![crate::connection::UiEvent::Daemon {
                generation: state.generation,
                event: Box::new(client::DaemonEvent::SessionUpdated(session)),
            }],
            &mut rune::Tx::default(),
        );
        assert_eq!(state.theme_pref.as_deref(), Some(custom.as_str()));
        assert_eq!(state.pref(CUSTOM_KEY), Some(custom.as_str()));
    }

    #[test]
    fn a_theme_file_over_the_budget_is_refused_before_it_is_read() {
        let dir = std::env::temp_dir().join(format!("forge-ui-theme-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let big = dir.join("big.json");
        std::fs::write(&big, vec![b' '; MAX_THEME_BYTES + 1]).expect("write");
        assert_eq!(
            read_theme_file(&big).unwrap_err(),
            "Theme JSON must be 16 KB or smaller."
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
