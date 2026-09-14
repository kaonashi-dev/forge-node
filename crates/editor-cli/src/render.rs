//! Paints the viewport with absolute cursor moves.
//!
//! A frame rewrites every row; there is no delta yet, and F1 measures whether
//! that is enough before anything is built on top of it.

use std::io::{self, Write};

use crossterm::{cursor, queue, style, terminal};

use crate::app::App;
use crate::view;

pub fn draw(app: &App, out: &mut impl Write) -> io::Result<()> {
    let height = app.content_height();
    for row in 0..height {
        queue!(
            out,
            cursor::MoveTo(0, row as u16),
            terminal::Clear(terminal::ClearType::CurrentLine)
        )?;
        let index = app.top() + row;
        if index < app.document().line_count() {
            let number = index + 1;
            queue!(
                out,
                style::Print(format!("{number:>width$} ", width = app.number_width()))
            )?;
            let window =
                view::cell_window(app.document().line(index), app.left(), app.content_width());
            if !window.is_empty() {
                queue!(out, style::Print(window))?;
            }
        }
    }

    queue!(
        out,
        cursor::MoveTo(0, height as u16),
        terminal::Clear(terminal::ClearType::CurrentLine)
    )?;
    queue!(out, style::Print(status_line(app)))?;

    let cursor_row = app.cursor().line.checked_sub(app.top());
    let column = view::display_col(app.document().line(app.cursor().line), app.cursor().col);
    let cursor_column = app.gutter_width() + column.saturating_sub(app.left());
    match cursor_row {
        Some(row) if row < height && cursor_column < app.width() as usize => {
            queue!(
                out,
                cursor::MoveTo(cursor_column as u16, row as u16),
                cursor::Show
            )?;
        }
        _ => {
            queue!(out, cursor::Hide)?;
        }
    }
    out.flush()
}

fn status_line(app: &App) -> String {
    let width = app.width() as usize;
    if let Some(message) = app.status() {
        return view::cell_window(message, 0, width);
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
    let position = format!(
        "{}:{}/{}",
        app.cursor().line + 1,
        app.cursor().col + 1,
        document.line_count()
    );
    let text = format!("{}{dirty}{read_only} — {position}", app.path().display());
    view::cell_window(&text, 0, width)
}
