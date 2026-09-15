//! Paints the rows a frame says changed, and nothing else.
//!
//! A keystroke damages one line, so a frame writes one row plus the status
//! line. Rewriting the viewport per event is what this replaced: it costs the
//! whole screen for an edit that moved one character.

use std::io::{self, Write};

use crossterm::{cursor, queue, style, terminal};

use crate::app::{App, Prompt, HELP};
use crate::view;

pub fn draw(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    if app.help_visible() {
        return draw_help(app, out);
    }
    let frame = app.take_frame();
    let height = app.content_height();
    if frame.clear {
        // A resize leaves half of a wide grapheme behind on rows the new
        // geometry no longer covers; clearing once is cheaper than tracking it.
        queue!(out, terminal::Clear(terminal::ClearType::All))?;
    }
    for row in frame.rows {
        draw_row(app, out, row)?;
        draw_ruler_cell(app, out, row)?;
    }
    // Integrated mode leaves the idle status bar to the GUI pane and only
    // claims the row for a prompt or a message; standalone always paints it.
    if app.needs_status_row() {
        draw_status(app, out, height)?;
    }
    draw_change_details(app, out, height)?;
    draw_completion(app, out, height)?;
    place_caret(app, out, height)?;
    // A copy leaves an OSC 52 for the terminal that owns the clipboard. Written
    // with the frame rather than at the keystroke, because this is the one
    // place that owns the output stream.
    if let Some(escape) = app.take_clipboard_escape() {
        out.write_all(escape.as_bytes())?;
    }
    out.flush()
}

fn draw_row(app: &App, out: &mut impl Write, row: usize) -> io::Result<()> {
    queue!(
        out,
        cursor::MoveTo(0, row as u16),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )?;
    let text = app.document().text();
    // A wrapped line spreads across rows; the row names which line and which of
    // its segments. Past the last line the row is blank.
    let Some((line, sub)) = app.row_line_sub(row) else {
        return Ok(());
    };
    if sub == 0 {
        // The caret's own line number reads in the default foreground rather
        // than the gutter grey — `highlightActiveLineGutter`, with no colour of
        // its own to fight the terminal's theme.
        let active = app.is_active_line(line);
        queue!(
            out,
            style::SetForegroundColor(if active {
                style::Color::Reset
            } else {
                style::Color::DarkGrey
            }),
            style::SetAttribute(if active {
                style::Attribute::Bold
            } else {
                style::Attribute::NormalIntensity
            }),
            style::Print(format!(
                "{number:>width$}",
                number = line + 1,
                width = app.number_width()
            )),
            style::SetAttribute(style::Attribute::NormalIntensity)
        )?;
        // The mark column, the fold column, then the space the text starts
        // after. All three are always printed: a gutter that widened the first
        // time git answered would shift every line of the file sideways.
        // A problem outranks a change on the one cell: a person who just ran a
        // checker is looking for what it found.
        match (app.diagnostic_at(line + 1), app.mark_at(line + 1)) {
            (Some(item), _) => queue!(
                out,
                style::SetForegroundColor(severity_colour(item.severity)),
                style::Print(severity_glyph(item.severity)),
            )?,
            (None, Some(kind)) => queue!(
                out,
                style::SetForegroundColor(mark_colour(kind)),
                style::Print(mark_glyph(kind)),
            )?,
            (None, None) => queue!(out, style::Print(" "))?,
        }
        // The checker's mark shares the fold column's neighbour rather than
        // taking one of its own: a gutter that grows the first time somebody
        // runs a linter would shift every line of the file sideways.
        match app.fold_state(line) {
            Some(folded) => queue!(
                out,
                style::SetForegroundColor(style::Color::DarkGrey),
                style::Print(if folded { '\u{25b8}' } else { '\u{25be}' }),
            )?,
            None => queue!(out, style::Print(" "))?,
        }
        queue!(
            out,
            style::SetForegroundColor(style::Color::Reset),
            style::Print(" ")
        )?;
        // A folded block says how much it swallowed, where the body would be.
        if app.fold_state(line) == Some(true) {
            let hidden = app.folded_line_count(line);
            queue!(
                out,
                style::SetForegroundColor(style::Color::DarkGrey),
                style::Print(format!("  \u{2026} {hidden} lines")),
                style::SetForegroundColor(style::Color::Reset)
            )?;
            return Ok(());
        }
    } else {
        // A continuation row has no number and no mark, but keeps the width so
        // the wrapped text stays under the line it belongs to.
        queue!(out, style::Print(" ".repeat(app.gutter_width())))?;
    }
    // The window starts at this segment's cell offset when wrapping, or at the
    // horizontal scroll when not.
    let base = if app.wrap() {
        sub * app.content_width()
    } else {
        app.left()
    };
    let marks = app.marks_in_line(line);
    let selections = app.selections_in_line(line);
    let carets = app.carets_in_line(line);
    let parts = view::window_parts(
        text.line(line),
        base,
        app.content_width(),
        view::RowDecor {
            selected: selections.first().copied(),
            also_selected: selections.get(1..).unwrap_or_default(),
            carets: &carets,
            scopes: app.syntax_line(line),
            marks: &marks,
            special_chars: app.special_chars(),
        },
    );
    for part in parts {
        // Colour first, then the attributes: a selected or marked run keeps its
        // scope colour as the foreground the terminal swaps, so selecting a
        // keyword does not flatten it to the default.
        match scope_colour(part.scope) {
            Some(colour) => queue!(out, style::SetForegroundColor(colour))?,
            None => queue!(out, style::SetForegroundColor(style::Color::Reset))?,
        }
        // Underline, not another reverse: the selection already owns reverse
        // video, and the current match *is* the selection, so a search hit has
        // to read as something else or the two stop being distinguishable.
        let underline = match part.decoration {
            view::Decoration::None | view::Decoration::Caret => false,
            view::Decoration::Match | view::Decoration::Bracket => true,
        };
        if part.decoration == view::Decoration::Bracket {
            queue!(out, style::SetAttribute(style::Attribute::Bold))?;
        }
        if underline {
            queue!(out, style::SetAttribute(style::Attribute::Underlined))?;
        }
        // An extra caret is reverse video on its one cell: the terminal has a
        // single hardware cursor and the primary owns it, so the others have to
        // be drawn as text.
        let reversed = part.selected || part.decoration == view::Decoration::Caret;
        if reversed {
            queue!(out, style::SetAttribute(style::Attribute::Reverse))?;
        }
        queue!(out, style::Print(part.text))?;
        if reversed {
            queue!(out, style::SetAttribute(style::Attribute::NoReverse))?;
        }
        if underline {
            queue!(out, style::SetAttribute(style::Attribute::NoUnderline))?;
        }
        if part.decoration == view::Decoration::Bracket {
            queue!(out, style::SetAttribute(style::Attribute::NormalIntensity))?;
        }
    }
    queue!(out, style::SetForegroundColor(style::Color::Reset))?;
    Ok(())
}

/// What the change at the caret replaced, over the rows it is about.
///
/// A panel and not a row: before and after are two lists, and a status line
/// that had to summarise them would be saying what the person just asked to
/// see. Painted over the text like the completion list, so it needs no layout
/// of its own and nothing under it moves.
fn draw_change_details(app: &App, out: &mut impl Write, height: usize) -> io::Result<()> {
    let Some(Prompt::ChangeDetails {
        before,
        after,
        offset,
        ..
    }) = app.prompt()
    else {
        return Ok(());
    };
    // Removed lines and then added ones, the way a hunk reads: a modification
    // is what it was before what it became, not two columns to compare.
    let lines: Vec<(char, style::Color, &str)> = before
        .iter()
        .map(|text| ('-', style::Color::Red, text.as_str()))
        .chain(
            after
                .iter()
                .map(|text| ('+', style::Color::Green, text.as_str())),
        )
        .skip(*offset)
        .take(height.saturating_sub(1))
        .collect();
    if lines.is_empty() {
        return Ok(());
    }
    let width = app.width() as usize;
    for (row, (sign, colour, text)) in lines.into_iter().enumerate() {
        queue!(
            out,
            cursor::MoveTo(0, row as u16),
            style::SetForegroundColor(colour),
            style::SetAttribute(style::Attribute::Reverse),
            style::Print(view::cell_window(&format!("{sign} {text}"), 0, width)),
            style::SetAttribute(style::Attribute::NoReverse),
            style::SetForegroundColor(style::Color::Reset)
        )?;
    }
    Ok(())
}

/// The completion list, under the caret when there is room and over it when
/// there is not.
///
/// Painted after the rows and before the caret, so it sits on top of the text
/// and the caret still shows where the word is being typed. The whole viewport
/// is damaged while a list is open, so it never leaves a strip behind.
fn draw_completion(app: &App, out: &mut impl Write, height: usize) -> io::Result<()> {
    let Some(open) = app.completion() else {
        return Ok(());
    };
    let Some((caret_row, caret_cell)) = app.caret_screen() else {
        return Ok(());
    };
    let words = &open.candidates.words;
    let rows = words.len().min(height.saturating_sub(1));
    if rows == 0 {
        return Ok(());
    }
    let width = words
        .iter()
        .map(|word| word.chars().count())
        .max()
        .unwrap_or(0)
        + 2;
    let left = caret_cell.min((app.width() as usize).saturating_sub(width));
    // Below the caret, unless the list would run off the bottom.
    let top = if caret_row + 1 + rows <= height {
        caret_row + 1
    } else {
        caret_row.saturating_sub(rows)
    };
    for (index, word) in words.iter().take(rows).enumerate() {
        queue!(
            out,
            cursor::MoveTo(left as u16, (top + index) as u16),
            style::SetAttribute(style::Attribute::Reverse)
        )?;
        if index == open.selected {
            queue!(out, style::SetAttribute(style::Attribute::Bold))?;
        }
        queue!(
            out,
            style::Print(view::cell_window(&format!(" {word} "), 0, width)),
            style::SetAttribute(style::Attribute::NormalIntensity),
            style::SetAttribute(style::Attribute::NoReverse)
        )?;
    }
    Ok(())
}

/// The overview ruler's one cell on this row.
///
/// Painted after the row so it survives the row's own `Clear`, and only where
/// there is something to say: an empty ruler is an empty column, not a rule.
fn draw_ruler_cell(app: &App, out: &mut impl Write, row: usize) -> io::Result<()> {
    if !app.ruler_visible() {
        return Ok(());
    }
    let Some(mark) = app.ruler_at(row) else {
        return Ok(());
    };
    use crate::app::RulerMark;
    let (glyph, colour) = match mark {
        RulerMark::Change => ('\u{2502}', style::Color::Yellow),
        RulerMark::Match => ('\u{2502}', style::Color::Blue),
        RulerMark::Caret => ('\u{25c0}', style::Color::Reset),
    };
    queue!(
        out,
        cursor::MoveTo(app.ruler_column() as u16, row as u16),
        style::SetForegroundColor(colour),
        style::Print(glyph),
        style::SetForegroundColor(style::Color::Reset)
    )
}

/// What a gutter mark looks like. One cell, and the same three shapes a diff
/// uses, so the column reads without a legend.
fn mark_glyph(kind: editor_control::WireMarkKind) -> char {
    use editor_control::WireMarkKind;
    match kind {
        WireMarkKind::Added => '+',
        WireMarkKind::Modified => '~',
        WireMarkKind::Deleted => '_',
    }
}

/// What a problem looks like in the gutter. One cell, and the three shapes a
/// compiler's own output uses, so the column reads without a legend.
fn severity_glyph(severity: editor_control::WireSeverity) -> char {
    use editor_control::WireSeverity;
    match severity {
        WireSeverity::Error => '\u{2717}',
        WireSeverity::Warning => '!',
        WireSeverity::Info => 'i',
    }
}

fn severity_colour(severity: editor_control::WireSeverity) -> style::Color {
    use editor_control::WireSeverity;
    match severity {
        WireSeverity::Error => style::Color::Red,
        WireSeverity::Warning => style::Color::Yellow,
        WireSeverity::Info => style::Color::Blue,
    }
}

fn mark_colour(kind: editor_control::WireMarkKind) -> style::Color {
    use editor_control::WireMarkKind;
    match kind {
        WireMarkKind::Added => style::Color::Green,
        WireMarkKind::Modified => style::Color::Yellow,
        WireMarkKind::Deleted => style::Color::Red,
    }
}

/// The terminal colour one scope paints in, or `None` for the default.
///
/// The ANSI 16 rather than a palette of our own, and that is not a limitation
/// in the pane: the canvas resolves those sixteen slots through
/// `--forge-ansi-*`, which is the active Forge theme, so a scope already lands
/// on the theme's own colour without a truecolour side-channel. Standalone it
/// lands on the person's terminal theme, which is the right answer there too —
/// a hard-coded RGB would fight every scheme but the one it was picked against.
/// The mapping is documented in `docs/editor.md`; keep the two in step.
fn scope_colour(scope: editor_core::Scope) -> Option<style::Color> {
    use editor_core::Scope;
    Some(match scope {
        Scope::Comment => style::Color::DarkGrey,
        Scope::Keyword => style::Color::Magenta,
        Scope::ControlKeyword => style::Color::Red,
        Scope::String => style::Color::Green,
        Scope::Number => style::Color::Yellow,
        Scope::Type => style::Color::Cyan,
        Scope::Function => style::Color::Blue,
        Scope::Property => style::Color::DarkCyan,
        Scope::Constant => style::Color::DarkYellow,
        Scope::Plain => return None,
    })
}

fn draw_status(app: &App, out: &mut impl Write, row: usize) -> io::Result<()> {
    queue!(
        out,
        cursor::MoveTo(0, row as u16),
        terminal::Clear(terminal::ClearType::CurrentLine),
        style::SetAttribute(style::Attribute::Reverse),
        style::Print(view::cell_window(
            &status_text(app),
            0,
            app.width() as usize
        )),
        style::SetAttribute(style::Attribute::NoReverse)
    )
}

fn status_text(app: &App) -> String {
    let width = app.width() as usize;
    if let Some(prompt) = app.prompt() {
        return match prompt {
            Prompt::Find { input } => format!("find: {input}_{}", app.query_flags()),
            Prompt::Replace {
                find,
                with,
                editing_replacement,
            } => {
                // Enter and Ctrl-R are different answers, so the row says which
                // is which: "Enter replaces all" was the copy that made a
                // one-match intention rewrite the file.
                let flags = app.query_flags();
                if *editing_replacement {
                    format!("replace: {find}  with: {with}_{flags}   (Tab switches, Enter: this one, Ctrl-R: all)")
                } else {
                    format!("replace: {find}_  with: {with}{flags}   (Tab switches, Enter: this one, Ctrl-R: all)")
                }
            }
            Prompt::GotoLine { input } => format!("go to line: {input}_"),
            Prompt::ConfirmClose => {
                "unsaved changes — (s)ave, (d)iscard, any other key cancels".to_string()
            }
            Prompt::ChangeDetails {
                line,
                before,
                after,
                truncated,
                ..
            } => {
                let cut = if *truncated { " (truncated)" } else { "" };
                format!(
                    "line {line}: {} removed, {} added{cut}   (Up/Down scroll, Esc closes)",
                    before.len(),
                    after.len()
                )
            }
            Prompt::Definitions {
                symbol,
                places,
                selected,
            } => {
                let at = selected + 1;
                let count = places.len();
                match places.get(*selected) {
                    Some(place) => format!(
                        "{symbol} {at}/{count}: {}:{}  {}   (Up/Down, Enter opens, Esc cancels)",
                        place.path,
                        place.line,
                        place.text.trim()
                    ),
                    None => format!("{symbol}: {count} places"),
                }
            }
        };
    }
    if let Some(message) = app.status() {
        return message.to_string();
    }
    // What the checker said about this line, when the row is otherwise idle:
    // a mark in the gutter that never says what it is about is a puzzle.
    if let Some(item) = app.caret_diagnostic() {
        return format!("{} {}", severity_glyph(item.severity), item.message);
    }
    let document = app.document();
    let dirty = if document.is_dirty() {
        " [modified]"
    } else {
        ""
    };
    let read_only = if document.is_read_only() {
        " [read-only]"
    } else {
        ""
    };
    let trimmed = if document.history_stats().dropped > 0 {
        " [history trimmed]"
    } else {
        ""
    };
    let (line, column) = app.caret_position();
    // The caret count only shows when there is more than one: it is the state a
    // person can forget they are in, and the one that makes the next keystroke
    // land in twenty places.
    let carets = match document.selection().count() {
        0 | 1 => String::new(),
        count => format!("  {count} carets"),
    };
    let right = format!(
        "{dirty}{read_only}{trimmed}{carets}  {line}:{}/{}  F1 help",
        column + 1,
        document.text().line_count()
    );
    // The path is the part that gives way. Truncating the row from the right
    // would drop the position and the modified flag, which are the two things
    // the status line exists to say.
    let room = width.saturating_sub(display_cells(&right));
    format!(
        "{}{right}",
        elide_front(&app.path().display().to_string(), room)
    )
}

fn display_cells(text: &str) -> usize {
    editor_core::metrics::display_column(text, text.len())
}

/// `text` padded to `room` cells, keeping its tail when it does not fit.
fn elide_front(text: &str, room: usize) -> String {
    if room == 0 {
        return String::new();
    }
    let cells = display_cells(text);
    if cells <= room {
        return format!("{text}{}", " ".repeat(room - cells));
    }
    let keep = room.saturating_sub(1);
    let start = editor_core::metrics::byte_column_for_display(text, cells - keep);
    format!("…{}", &text[start..])
}

fn place_caret(app: &App, out: &mut impl Write, height: usize) -> io::Result<()> {
    if app.prompt().is_some() {
        // The `_` in the prompt marks where typing lands; a hardware caret on
        // the status row would fight it in the Replace prompt's second field.
        return queue!(out, cursor::Hide);
    }
    // `caret_screen` already accounts for wrapping and the horizontal scroll,
    // and returns `None` when the caret is off screen.
    match app.caret_screen() {
        Some((row, cell)) if row < height && cell < app.width() as usize => {
            queue!(out, cursor::MoveTo(cell as u16, row as u16), cursor::Show)
        }
        _ => queue!(out, cursor::Hide),
    }
}

fn draw_help(app: &mut App, out: &mut impl Write) -> io::Result<()> {
    let _ = app.take_frame();
    queue!(
        out,
        terminal::Clear(terminal::ClearType::All),
        cursor::MoveTo(0, 0),
        cursor::Hide,
        style::Print("forge-editor — keys")
    )?;
    let width = app.width() as usize;
    for (index, (keys, what)) in HELP.iter().enumerate() {
        let row = index + 2;
        if row + 1 >= app.height_rows() {
            break;
        }
        queue!(
            out,
            cursor::MoveTo(0, row as u16),
            style::Print(view::cell_window(&format!("  {keys:<28} {what}"), 0, width))
        )?;
    }
    queue!(
        out,
        cursor::MoveTo(0, app.height_rows().saturating_sub(1) as u16),
        style::SetAttribute(style::Attribute::Reverse),
        style::Print(view::cell_window("press any key to return", 0, width)),
        style::SetAttribute(style::Attribute::NoReverse)
    )?;
    out.flush()
}
