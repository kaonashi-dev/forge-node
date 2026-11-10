//! Space, control heights and icon sizes rune-kit does not name. Radii come
//! from `Kit::radius`, and density-driven heights from `ChromeMetrics`.

pub const SPACE_1: f32 = 2.0;
pub const SPACE_2: f32 = 4.0;
pub const SPACE_3: f32 = 6.0;
pub const SPACE_4: f32 = 8.0;
pub const SPACE_5: f32 = 12.0;
pub const SPACE_6: f32 = 16.0;
pub const SPACE_7: f32 = 24.0;

// Anything shorter is rune-kit's `ControlSize` (`Regular` is 24, `Small` 20).

/// A row, a tab or a button, before density moves it.
pub const CONTROL: f32 = 28.0;
pub const CONTROL_FIELD: f32 = 32.0;

/// Inside a chip, and a chevron.
pub const ICON_SMALL: f32 = 12.0;
pub const ICON_ROW: f32 = 14.0;
pub const ICON_BUTTON: f32 = 16.0;

pub const CHIP_H: f32 = 18.0;
pub const DOT: f32 = 6.0;
pub const AVATAR: f32 = 18.0;
/// One level of a tree.
pub const INDENT: f32 = 20.0;
