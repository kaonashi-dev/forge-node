//! Shaped row geometry: caret, selection and pointer x positions.
//!
//! `EditorPlace.column` is UTF-16 while Rune's shaped runs index UTF-8 bytes,
//! so each row is measured once and reused until its fingerprint changes. The
//! text system here must use the window's families, or the caret drifts off
//! the glyphs it was measured against.

use std::collections::HashMap;
use std::rc::Rc;

use domain::EditorRow;
use rune::text::{Run, TextSystem};
use rune::tok::TextStyle;

pub const SANS: &str = "Inter";
pub const MONO_FAMILIES: &str = "ui-monospace, Inter";
pub const MONO_SIZE: f32 = 13.0;

#[must_use]
pub fn mono() -> TextStyle {
    TextStyle::new(MONO_SIZE, 400.0).mono()
}

#[must_use]
pub fn text_system() -> TextSystem {
    TextSystem::new(&[], (SANS, MONO_FAMILIES))
}

#[derive(Clone, Debug)]
pub struct RowMetrics {
    pub text: Rc<str>,
    pub utf16_len: u32,
    pub end_x: f32,
    /// One entry per grapheme start: (UTF-16 column, x).
    positions: Vec<(u32, f32)>,
}

impl RowMetrics {
    pub fn measure(text: &str, text_system: &mut TextSystem) -> Self {
        let run = Run::new(text.to_string());
        let layout = text_system.run_layout(&run, mono(), 1.0);
        let mut positions = Vec::with_capacity(text.chars().count() + 1);
        let mut utf16_len = 0u32;
        for (byte, ch) in text.char_indices() {
            let column = utf16_len;
            utf16_len += ch.len_utf16() as u32;
            if byte == 0 {
                positions.push((column, layout.x(byte).unwrap_or(0.0)));
            } else if let Some(x) = layout.x(byte) {
                positions.push((column, x));
            }
        }
        let end_x = if text.is_empty() {
            0.0
        } else {
            layout
                .layout
                .full_width()
                .max(positions.last().map_or(0.0, |(_, x)| *x))
        };
        RowMetrics {
            text: Rc::from(text),
            utf16_len,
            end_x,
            positions,
        }
    }

    /// The x of `column`, clamped to the row. A column inside a surrogate pair
    /// or a tab lands on the grapheme's start, which is where a caret can sit.
    #[must_use]
    pub fn column_to_x(&self, column: u32) -> f32 {
        if self.positions.is_empty() || column >= self.utf16_len {
            return self.end_x;
        }
        let index = self
            .positions
            .partition_point(|(col, _)| *col <= column)
            .saturating_sub(1);
        self.positions[index].1
    }

    /// The nearest caret column to `x`, snapping at each grapheme's midpoint.
    #[must_use]
    pub fn x_to_column(&self, x: f32) -> u32 {
        if x <= 0.0 || self.positions.is_empty() {
            return 0;
        }
        for (index, (column, start)) in self.positions.iter().enumerate() {
            let next_x = self
                .positions
                .get(index + 1)
                .map_or(self.end_x, |(_, x)| *x);
            if x < next_x {
                let midpoint = (*start + next_x) * 0.5;
                return if x >= midpoint {
                    self.positions
                        .get(index + 1)
                        .map_or(self.utf16_len, |(column, _)| *column)
                } else {
                    *column
                };
            }
        }
        self.utf16_len
    }
}

/// Measured rows keyed by `(line, fingerprint)`, so an edit re-measures only
/// the rows it changed; rows that left the window are dropped per frame.
pub struct MetricsCache {
    rows: HashMap<(u32, u64), Rc<RowMetrics>>,
    seen: Vec<(u32, u64)>,
    text_system: TextSystem,
    builds: u64,
}

impl MetricsCache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
            seen: Vec::new(),
            text_system: text_system(),
            builds: 0,
        }
    }

    /// Measures `row`, or returns the copy kept for its line and fingerprint.
    /// A fingerprint of `0` is unknown and never reused.
    pub fn row(&mut self, row: &EditorRow) -> Rc<RowMetrics> {
        let key = (row.line, row.fingerprint);
        if row.fingerprint != 0
            && let Some(metrics) = self.rows.get(&key)
        {
            self.seen.push(key);
            return Rc::clone(metrics);
        }
        let metrics = Rc::new(RowMetrics::measure(&row.text(), &mut self.text_system));
        self.builds += 1;
        if row.fingerprint != 0 {
            self.rows.insert(key, Rc::clone(&metrics));
            self.seen.push(key);
        }
        metrics
    }

    /// Drops every row not asked for since the previous call.
    pub fn retain_seen(&mut self) {
        let mut seen = std::mem::take(&mut self.seen);
        seen.sort_unstable();
        self.rows.retain(|key, _| seen.binary_search(key).is_ok());
        seen.clear();
        self.seen = seen;
    }

    #[cfg(test)]
    pub fn builds(&self) -> u64 {
        self.builds
    }
}

impl Default for MetricsCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn row(line: u32, text: &str, fingerprint: u64) -> EditorRow {
        EditorRow {
            line,
            truncated: false,
            spans: Arc::new(vec![domain::EditorSpan {
                text: text.into(),
                scope: domain::EditorScope::Plain,
            }]),
            mark: None,
            diagnostic: None,
            fold: None,
            fingerprint,
        }
    }

    #[test]
    fn wide_characters_take_more_than_one_cell() {
        let metrics = RowMetrics::measure("a界b", &mut text_system());
        let narrow = metrics.column_to_x(1) - metrics.column_to_x(0);
        let wide = metrics.column_to_x(2) - metrics.column_to_x(1);
        assert!(wide > narrow * 1.5, "wide {wide} vs narrow {narrow}");
        assert_eq!(metrics.x_to_column(metrics.column_to_x(2)), 2);
    }

    #[test]
    fn tabs_advance_to_the_next_four_column_stop() {
        let metrics = RowMetrics::measure("\tX", &mut text_system());
        let four_spaces = RowMetrics::measure("    X", &mut text_system());
        assert!((metrics.column_to_x(1) - four_spaces.column_to_x(4)).abs() < 1.0);
    }

    #[test]
    fn an_empty_row_has_a_zero_width_caret() {
        let metrics = RowMetrics::measure("", &mut text_system());
        assert_eq!(metrics.utf16_len, 0);
        assert_eq!(metrics.column_to_x(0), 0.0);
        assert_eq!(metrics.x_to_column(100.0), 0);
    }

    #[test]
    fn a_row_is_measured_again_only_when_its_fingerprint_changes() {
        let mut cache = MetricsCache::new();
        cache.row(&row(4, "fn a()", 11));
        cache.row(&row(4, "fn a()", 11));
        assert_eq!(cache.builds(), 1);
        cache.row(&row(4, "fn ab()", 12));
        assert_eq!(cache.builds(), 2);
        cache.row(&row(5, "x", 0));
        cache.row(&row(5, "x", 0));
        assert_eq!(cache.builds(), 4, "an unknown fingerprint is never reused");
    }

    #[test]
    fn rows_that_left_the_window_are_dropped() {
        let mut cache = MetricsCache::new();
        cache.row(&row(1, "a", 1));
        cache.row(&row(2, "b", 2));
        cache.retain_seen();
        cache.row(&row(2, "b", 2));
        cache.retain_seen();
        cache.row(&row(1, "a", 1));
        assert_eq!(cache.builds(), 3);
    }
}
