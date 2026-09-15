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
    if frame.full {
        // A resize leaves half of a wide grapheme behind on rows the new
        // geometry no longer covers; clearing once is cheaper than tracking it.
        queue!(out, terminal::Clear(terminal::ClearType::All))?;
    }
    for row in frame.rows {
        draw_row(app, out, row)?;
    }
    draw_status(app, out, height)?;
    place_caret(app, out, height)?;
    out.flush()
}

fn draw_row(app: &App, out: &mut impl Write, row: usize) -> io::Result<()> {
    queue!(
        out,
        cursor::MoveTo(0, row as u16),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )?;
    let line = app.top() + row;
    let text = app.document().text();
    if line >= text.line_count() {
        return Ok(());
    }
    queue!(
        out,
        style::Print(format!(
            "{number:>width$} ",
            number = line + 1,
            width = app.number_width()
        ))
    )?;
    let parts = view::window_parts(
        text.line(line),
        app.left(),
        app.content_width(),
        app.selection_in_line(line),
    );
    for part in parts {
        if part.selected {
            queue!(
                out,
                style::SetAttribute(style::Attribute::Reverse),
                style::Print(part.text),
                style::SetAttribute(style::Attribute::NoReverse)
            )?;
        } else {
            queue!(out, style::Print(part.text))?;
        }
    }
    Ok(())
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
            Prompt::Find { input } => format!("find: {input}_"),
            Prompt::Replace {
                find,
                with,
                editing_replacement,
            } => {
                if *editing_replacement {
                    format!("replace: {find}  with: {with}_   (Tab switches, Enter replaces all)")
                } else {
                    format!("replace: {find}_  with: {with}   (Tab switches, Enter replaces all)")
                }
            }
            Prompt::GotoLine { input } => format!("go to line: {input}_"),
            Prompt::ConfirmClose => {
                "unsaved changes — (s)ave, (d)iscard, any other key cancels".to_string()
            }
        };
    }
    if let Some(message) = app.status() {
        return message.to_string();
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
    let right = format!(
        "{dirty}{read_only}{trimmed}  {line}:{}/{}  F1 help",
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
    let (line, column) = app.caret_position();
    let row = (line - 1).checked_sub(app.top());
    let cell = app.gutter_width() + column.saturating_sub(app.left());
    match row {
        Some(row) if row < height && cell < app.width() as usize => {
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
