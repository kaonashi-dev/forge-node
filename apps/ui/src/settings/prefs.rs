//! The `ui.*` preferences Settings › Appearance writes, read and written
//! exactly as this client does, so the two clients share
//! one `app_state` row per key.

pub const DENSITY_KEY: &str = "ui.density";
pub const REDUCE_MOTION_KEY: &str = "ui.reduce_motion";
pub const GLASS_KEY: &str = "ui.glass";
pub const TRIM_KEY: &str = "ui.trim";
pub const UI_FONT_SIZE_KEY: &str = "ui.font_size";
pub const EDITOR_FONT_SIZE_KEY: &str = "ui.editor.font_size";
pub const TERMINAL_ZOOM_KEY: &str = "ui.terminal.zoom";
pub const AUTOSAVE_KEY: &str = "ui.editor.autosave";
pub const EDITOR_LINE_HEIGHT_KEY: &str = "ui.editor.line_height";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Range {
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub fallback: f64,
}

/// The chrome's `sm` rung in points; 13 is the original client's `metrics.textSM`.
pub const UI_FONT_SIZE: Range = Range {
    min: 12.0,
    max: 16.0,
    step: 0.5,
    fallback: 13.0,
};
pub const EDITOR_FONT_SIZE: Range = Range {
    min: 9.0,
    max: 24.0,
    step: 1.0,
    fallback: 13.0,
};
/// A multiplier, shown as a percentage.
pub const TERMINAL_ZOOM: Range = Range {
    min: 0.5,
    max: 2.0,
    step: 0.1,
    fallback: 1.0,
};
/// Wider than the four named spacings, because the original client clamps a stored value
/// to this range rather than to the choices.
pub const EDITOR_LINE_HEIGHT: Range = Range {
    min: 1.1,
    max: 2.2,
    step: 0.05,
    fallback: 1.35,
};

pub const LINE_HEIGHTS: [(f64, &str); 4] = [
    (1.2, "Tight"),
    (1.35, "Default"),
    (1.5, "Relaxed"),
    (1.8, "Loose"),
];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ChromeMetrics {
    pub row: f32,
    pub tree_row: f32,
    pub tab: f32,
    pub frame: f32,
    pub pad: f32,
    pub sidebar_pad: [f32; 2],
}

impl ChromeMetrics {
    /// the original client's compact ladder, or the cozy one, with Trim pulling the frame
    /// and paddings in rather than changing a row's height.
    #[must_use]
    pub fn of(density: Density, trim: bool) -> Self {
        match density {
            Density::Compact => Self {
                row: 26.0,
                tree_row: 28.0,
                tab: 30.0,
                frame: 8.0,
                pad: 10.0,
                sidebar_pad: [18.0, 10.0],
            },
            Density::Cozy => Self {
                row: 30.0,
                tree_row: 34.0,
                tab: 32.0,
                frame: 8.0,
                pad: 12.0,
                sidebar_pad: [18.0, 10.0],
            },
        }
        .trimmed(trim)
    }

    fn trimmed(mut self, trim: bool) -> Self {
        if trim {
            self.row -= 2.0;
            self.tab -= 2.0;
            self.frame = 4.0;
            self.pad -= 2.0;
            self.sidebar_pad = [12.0, 6.0];
        }
        self
    }
}

/// Lay the four visual preferences over the kit: density changes the radii,
/// Trim flattens them, Reduce Motion lands every spring at once, and Glass
/// makes the chrome wash translucent in a dark theme (light themes ignore it).
pub fn apply_kit(
    kit: &mut rune_kit::Kit,
    density: Density,
    reduce_motion: bool,
    glass: bool,
    trim: bool,
) {
    let density_radius = if density == Density::Cozy { 1.0 } else { 0.0 };
    let trim_scale = if trim { 0.75 } else { 1.0 };
    let radius = kit.radius;
    let scale = |value: f32| ((value + density_radius) * trim_scale).max(0.0);
    kit.radius = rune_kit::Radius {
        xs: scale(radius.xs),
        sm: scale(radius.sm),
        md: scale(radius.md),
        lg: scale(radius.lg),
        xl: scale(radius.xl),
    };
    if reduce_motion {
        kit.motion = rune_kit::Motion {
            control: rune::Spring::NONE,
            overlay: rune::Spring::NONE,
            disclosure: rune::Spring::NONE,
        };
    }
    if glass && kit.dark {
        kit.c.bg_deep = kit.c.bg_deep.alpha(0.92);
        kit.c.layer_01 = kit.c.layer_01.alpha(0.86);
        kit.c.layer_02 = kit.c.layer_02.alpha(0.90);
        kit.c.surface_base = kit.c.surface_base.alpha(0.88);
        kit.c.surface_raised = kit.c.surface_raised.alpha(0.90);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    #[default]
    Compact,
    Cozy,
}

impl Density {
    pub const ALL: [Density; 2] = [Density::Compact, Density::Cozy];

    /// A stored value, mapping the ids of the retired three-step scale.
    #[must_use]
    pub fn read(value: Option<&str>) -> Self {
        match value {
            Some("cozy" | "comfortable") => Density::Cozy,
            _ => Density::Compact,
        }
    }

    #[must_use]
    pub fn value(self) -> &'static str {
        match self {
            Density::Compact => "compact",
            Density::Cozy => "cozy",
        }
    }

    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Density::Compact => "Compact",
            Density::Cozy => "Cozy",
        }
    }

    /// the original client's `DENSITY_NOTES`: the tree row is 28 pt compact, six more cozy.
    #[must_use]
    pub fn note(self) -> &'static str {
        match self {
            Density::Compact => "Rows are 28px — the most sessions visible at once.",
            Density::Cozy => "Rows are 34px — easier to hit, fewer on screen.",
        }
    }
}

/// Only `"true"` is on; anything else stored is off, and nothing is `fallback`.
#[must_use]
pub fn read_flag(value: Option<&str>, fallback: bool) -> bool {
    value.map_or(fallback, |value| value == "true")
}

#[must_use]
pub fn flag_value(on: bool) -> &'static str {
    if on { "true" } else { "false" }
}

/// A stored number, clamped: a value an older build or the other client
/// wrote must not be able to break the grid.
#[must_use]
pub fn read_scale(value: Option<&str>, range: Range) -> f64 {
    match value.and_then(|value| value.trim().parse::<f64>().ok()) {
        Some(parsed) if parsed.is_finite() => parsed.clamp(range.min, range.max),
        _ => range.fallback,
    }
}

/// Two decimals, so `0.1` steps do not drift into `1.2000000000000002`,
/// which reads back as a different zoom.
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// One stepper press: snapped to the step, moved, clamped.
#[must_use]
pub fn step_value(current: f64, up: bool, range: Range) -> f64 {
    let snapped = (current / range.step).round() * range.step;
    let delta = if up { range.step } else { -range.step };
    round2((snapped + delta).clamp(range.min, range.max))
}

#[must_use]
pub fn can_step(current: f64, up: bool, range: Range) -> bool {
    step_value(current, up, range) != round2(current)
}

/// The number as JavaScript's `String(number)` writes it: `13`, `12.5`, `1.2`.
#[must_use]
pub fn number_value(value: f64) -> String {
    format!("{}", round2(value))
}

/// The line spacing a stored value names, or the stored one when it is none.
#[must_use]
pub fn line_height_choice(value: f64) -> Option<usize> {
    LINE_HEIGHTS
        .iter()
        .position(|(choice, _)| (choice - value).abs() < 1e-6)
}

/// The kit's type scale grown or shrunk so its 13 pt rung reads `sm`.
#[must_use]
pub fn scale_type(ty: rune_kit::Type, sm: f64) -> rune_kit::Type {
    let ratio = (sm / UI_FONT_SIZE.fallback) as f32;
    if (ratio - 1.0).abs() < f32::EPSILON {
        return ty;
    }
    let scale = |mut style: rune::tok::TextStyle| {
        style.size *= ratio;
        style.line_height *= ratio;
        style
    };
    rune_kit::Type {
        ui: scale(ty.ui),
        ui_medium: scale(ty.ui_medium),
        ui_compact: scale(ty.ui_compact),
        small: scale(ty.small),
        small_medium: scale(ty.small_medium),
        body: scale(ty.body),
        body_medium: scale(ty.body_medium),
        text_small: scale(ty.text_small),
        text_small_medium: scale(ty.text_small_medium),
        text_xsmall: scale(ty.text_xsmall),
        text_xsmall_medium: scale(ty.text_xsmall_medium),
        mono: ty.mono,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stored_scale_is_clamped_and_garbage_falls_back() {
        assert_eq!(read_scale(Some("14.5"), UI_FONT_SIZE), 14.5);
        assert_eq!(read_scale(Some("40"), UI_FONT_SIZE), 16.0);
        assert_eq!(read_scale(Some("3"), EDITOR_FONT_SIZE), 9.0);
        assert_eq!(read_scale(Some("NaN"), TERMINAL_ZOOM), 1.0);
        assert_eq!(read_scale(Some("big"), TERMINAL_ZOOM), 1.0);
        assert_eq!(read_scale(None, EDITOR_LINE_HEIGHT), 1.35);
    }

    #[test]
    fn steps_snap_clamp_and_do_not_drift() {
        assert_eq!(step_value(13.0, true, UI_FONT_SIZE), 13.5);
        assert_eq!(step_value(12.0, false, UI_FONT_SIZE), 12.0);
        assert!(!can_step(16.0, true, UI_FONT_SIZE));
        assert!(can_step(16.0, false, UI_FONT_SIZE));
        let mut zoom = 1.0;
        for _ in 0..2 {
            zoom = step_value(zoom, true, TERMINAL_ZOOM);
        }
        assert_eq!(number_value(zoom), "1.2", "no float drift is stored");
        assert_eq!(step_value(1.23, true, TERMINAL_ZOOM), 1.3, "snapped first");
        assert_eq!(step_value(0.5, false, TERMINAL_ZOOM), 0.5);
        assert_eq!(number_value(13.0), "13");
        assert_eq!(number_value(12.5), "12.5");
    }

    #[test]
    fn flags_and_densities_read_as_the_original_client_writes_them() {
        assert!(read_flag(Some("true"), false));
        assert!(!read_flag(Some("yes"), true));
        assert!(read_flag(None, true));
        assert_eq!(Density::read(Some("comfortable")), Density::Cozy);
        assert_eq!(Density::read(Some("default")), Density::Compact);
        assert_eq!(Density::read(None), Density::Compact);
        assert_eq!(Density::Cozy.value(), "cozy");
        assert_eq!(line_height_choice(1.5), Some(2));
        assert_eq!(line_height_choice(1.4), None);
    }

    #[test]
    fn the_type_scale_follows_the_chrome_size() {
        let ty = rune_kit::Kit::neutral(true).ty;
        let same = scale_type(ty, 13.0);
        assert_eq!(same.small.size, ty.small.size);
        let large = scale_type(ty, 15.6);
        assert!((large.ui.size - ty.ui.size * 1.2).abs() < 0.01);
        assert_eq!(large.mono.size, ty.mono.size, "the mono face is content");
    }
}
