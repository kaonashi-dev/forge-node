//! Settings › Appearance: the theme, then Tauri's density, motion, finish,
//! type size and editor rows (`SettingsRoute.tsx` `Appearance`).

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Switch};

use super::prefs::{self, Density, Range};
use super::{group, setting_row};
use crate::app::AppState;

fn switch(
    k: &Kit,
    key: &'static str,
    label: &str,
    on: bool,
    disabled: bool,
    pref: &'static str,
) -> El {
    Switch::new(on)
        .label(label.to_string())
        .hide_label()
        .disabled(disabled)
        .on_toggle(move |s: &mut AppState| {
            let next = !prefs::read_flag(s.pref(pref), false);
            s.write_pref(pref, prefs::flag_value(next));
        })
        .build(k)
        .key(key)
}

struct Stepper {
    key: &'static str,
    label: &'static str,
    /// Completes "Smaller …" and "Larger …" for the buttons' help.
    noun: &'static str,
    pref: &'static str,
    range: Range,
    format: fn(f64) -> String,
}

const STEPPERS: [Stepper; 3] = [
    Stepper {
        key: "type-chrome",
        label: "Chrome",
        noun: "chrome text",
        pref: prefs::UI_FONT_SIZE_KEY,
        range: prefs::UI_FONT_SIZE,
        format: prefs::number_value,
    },
    Stepper {
        key: "type-editor",
        label: "Editor",
        noun: "editor text",
        pref: prefs::EDITOR_FONT_SIZE_KEY,
        range: prefs::EDITOR_FONT_SIZE,
        format: prefs::number_value,
    },
    Stepper {
        key: "type-terminal",
        label: "Terminal",
        noun: "terminal text",
        pref: prefs::TERMINAL_ZOOM_KEY,
        range: prefs::TERMINAL_ZOOM,
        format: percent,
    },
];

fn percent(value: f64) -> String {
    format!("{}%", (value * 100.0).round())
}

fn stepper(s: &AppState, k: &Kit, spec: &Stepper) -> El {
    let Stepper {
        key,
        label,
        noun,
        pref,
        range,
        format,
    } = *spec;
    let value = prefs::read_scale(s.pref(pref), range);
    let step = move |up: bool| {
        move |s: &mut AppState| {
            let current = prefs::read_scale(s.pref(pref), range);
            if prefs::can_step(current, up, range) {
                let next = prefs::step_value(current, up, range);
                s.write_pref(pref, &prefs::number_value(next));
            }
        }
    };
    let button = |glyph: &str, up: bool, suffix: &str| {
        Button::new(glyph.to_string())
            .variant(ButtonVariant::Outline)
            .control_size(ControlSize::Small)
            .help(format!("{} {noun}", if up { "Larger" } else { "Smaller" }))
            .disabled(!prefs::can_step(value, up, range))
            .on_press(step(up))
            .build(k)
            .key(format!("{key}-{suffix}"))
    };
    row()
        .key(key)
        .role("group")
        .label(format!("{label} {}", format(value)))
        .gap(8.0)
        .items_center()
        .child(text(label).font(k.ty.small).color(k.c.text_muted).w(90.0))
        .child(button("−", false, "down"))
        .child(
            text(format(value))
                .role("status")
                .font(k.ty.small_medium)
                .color(k.c.text_base)
                .w(48.0)
                .align(rune::Align::Center),
        )
        .child(button("+", true, "up"))
}

pub fn section(s: &AppState, k: &Kit) -> El {
    let density = Density::read(s.pref(prefs::DENSITY_KEY));
    let labels: Vec<&str> = Density::ALL.iter().map(|d| d.label()).collect();
    let densities = SegmentedControl::new(&labels, Density::ALL.iter().position(|d| *d == density))
        .label("Row density")
        .fit()
        .on_pick(|s: &mut AppState, index: usize, _tx| {
            if let Some(choice) = Density::ALL.get(index) {
                s.write_pref(prefs::DENSITY_KEY, choice.value());
            }
        })
        .build(k)
        .key("appearance-density");
    let light = s
        .active_theme(k.dark)
        .is_some_and(|theme| theme.mode == "light");
    let glass_on = prefs::read_flag(s.pref(prefs::GLASS_KEY), false);
    let layout = group(k, "appearance-layout", "Layout", None)
        .child(setting_row(k, "Density", Some(density.note()), densities))
        .child(setting_row(
            k,
            "Reduce motion",
            Some("On top of the system setting."),
            switch(
                k,
                "appearance-reduce-motion",
                "Reduce motion",
                s.reduce_motion(),
                false,
                prefs::REDUCE_MOTION_KEY,
            ),
        ))
        .child(setting_row(
            k,
            "Glass",
            Some(if light {
                "Glass is drawn for dark themes; pick one to use it."
            } else {
                "A lit backdrop, a translucent sidebar and a blue accent over the current theme."
            }),
            switch(
                k,
                "appearance-glass",
                "Frosted sidebar and pill tabs",
                glass_on,
                light,
                prefs::GLASS_KEY,
            ),
        ))
        .child(setting_row(
            k,
            "Trim",
            Some(
                "Pulls the window, rail and tabs in, and rounds every corner less. Row height \
                 stays with Density above.",
            ),
            switch(
                k,
                "appearance-trim",
                "Tighter chrome and flatter corners",
                prefs::read_flag(s.pref(prefs::TRIM_KEY), false),
                false,
                prefs::TRIM_KEY,
            ),
        ));

    let mut type_size = group(k, "appearance-type", "Type size", None);
    for spec in &STEPPERS {
        type_size = type_size.child(stepper(s, k, spec));
    }

    let line_height = prefs::read_scale(
        s.pref(prefs::EDITOR_LINE_HEIGHT_KEY),
        prefs::EDITOR_LINE_HEIGHT,
    );
    let spacing_labels: Vec<&str> = prefs::LINE_HEIGHTS.iter().map(|(_, l)| *l).collect();
    let spacing = SegmentedControl::new(&spacing_labels, prefs::line_height_choice(line_height))
        .label("Line spacing")
        .fit()
        .on_pick(|s: &mut AppState, index: usize, _tx| {
            if let Some((value, _)) = prefs::LINE_HEIGHTS.get(index) {
                s.write_pref(prefs::EDITOR_LINE_HEIGHT_KEY, &prefs::number_value(*value));
            }
        })
        .build(k)
        .key("appearance-line-spacing");
    let editor = group(k, "appearance-editor", "Editor", None)
        .child(setting_row(
            k,
            "Autosave",
            Some(
                "Off by default: agents write these same files, and a save on a pause races them.",
            ),
            switch(
                k,
                "appearance-autosave",
                "Autosave",
                prefs::read_flag(s.pref(prefs::AUTOSAVE_KEY), false),
                false,
                prefs::AUTOSAVE_KEY,
            ),
        ))
        .child(setting_row(k, "Line spacing", None, spacing));

    col()
        .key("appearance-page")
        .w_full()
        .gap(24.0)
        .child(crate::appearance::section(s, k))
        .child(layout)
        .child(type_size)
        .child(editor)
}
