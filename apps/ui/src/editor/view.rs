//! The editor pane: a scroll container as tall as the document's rows, the
//! mounted window inside it, and quads for caret, selection and decorations.
//!
//! Row text and gutters are memoised on what they paint (the row fingerprint,
//! the mark, the fold), so a caret move or a scroll rebuilds no text.

use domain::{
    EditorDecorationKind, EditorFrame, EditorInputEvent, EditorMark, EditorPlace, EditorPointer,
    EditorRow, EditorScope, EditorSeverity,
};
use rune::{
    Align, Color, Cursor, Drag, El, Phase, ScrollSpec, Span, Tx, col, el, memo, rich, row, text,
};
use rune_kit::Kit;

use super::metrics::{self, RowMetrics};
use super::theme::{self, EditorPalette};
use super::{
    EditorUi, FOLD_HIT_W, GUTTER_W, ROW_H, SCROLL_KEY, TEXT_PAD, content_rows, visual_base,
};
use crate::app::AppState;

const HEADER_H: f32 = 30.0;
const STATUS_H: f32 = 24.0;

pub fn pane(s: &AppState, k: &Kit) -> El {
    let palette = theme::calm_dark();
    let ui = &s.editor;
    let editor = s.active_editor_state();
    let path = editor.map_or("editor", |state| state.path.as_str());
    let header = row()
        .key("editor-header")
        .w_full()
        .h(HEADER_H)
        .items_center()
        .px(12.0)
        .gap(10.0)
        .child(
            text(path.to_string())
                .font(k.ty.small_medium)
                .color(palette.foreground),
        )
        .child(
            text(if editor.is_some_and(|state| state.dirty) {
                "modified"
            } else {
                ""
            })
            .font(k.ty.text_xsmall)
            .color(palette.git_modified),
        );
    let body = match &ui.frame {
        Some(frame) => scroller(ui, frame, palette),
        None => col()
            .key("editor-waiting")
            .w_full()
            .grow(1.0)
            .center_y()
            .padding(24.0)
            .child(
                text("Waiting for the editor's first window…")
                    .font(k.ty.small)
                    .color(palette.gutter_foreground),
            ),
    };
    let caret = ui.frame.as_ref().map(|frame| frame.caret);
    let status = row()
        .key("editor-status")
        .w_full()
        .h(STATUS_H)
        .items_center()
        .px(12.0)
        .fill(palette.active_line)
        .child(
            text(match caret {
                Some(caret) => format!("Ln {}, Col {}", caret.line + 1, caret.column + 1),
                None => String::new(),
            })
            .font(k.ty.text_xsmall)
            .color(palette.gutter_foreground),
        );
    col()
        .key("editor-pane")
        .role("textbox")
        .label(format!("Editor · {path}"))
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(palette.background)
        .focusable(true)
        .on_key(|s: &mut AppState, ev, tx| s.editor_key(ev, tx))
        .child(header)
        .child(body)
        .child(status)
}

fn scroller(ui: &EditorUi, frame: &EditorFrame, palette: EditorPalette) -> El {
    let mut metrics = ui.metrics.borrow_mut();
    let rows: Vec<El> = frame
        .rows
        .iter()
        .map(|row| {
            let measured = metrics.row(row);
            row_view(ui, frame, row, &measured, palette)
        })
        .collect();
    metrics.retain_seen();
    drop(metrics);

    let content_height = content_rows(frame) as f32 * ROW_H;
    let top = visual_base(frame) as f32 * ROW_H;
    let bottom = (content_height - top - frame.rows.len() as f32 * ROW_H).max(0.0);
    let mut content = col()
        .key("editor-content")
        .w_full()
        .h(content_height.max(top + frame.rows.len() as f32 * ROW_H));
    if top > 0.0 {
        content = content.child(el().key("editor-top").w_full().h(top));
    }
    content = content.children(rows);
    if bottom > 0.0 {
        content = content.child(el().key("editor-bottom").w_full().h(bottom));
    }

    let scroll = ui.scroll.clone();
    col()
        .key(SCROLL_KEY)
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .on_scroll_offset(move |at| scroll.set([at.offset[1], at.viewport[1]]))
        .child(content.cursor(Cursor::Text))
}

fn row_view(
    ui: &EditorUi,
    frame: &EditorFrame,
    line_row: &EditorRow,
    measured: &RowMetrics,
    palette: EditorPalette,
) -> El {
    let line = line_row.line;
    let foldable = line_row.fold.is_some();
    let mut view = el()
        .key(format!("row:{line}"))
        .w_full()
        .h(ROW_H)
        .items_center()
        .on_drag(move |s: &mut AppState, drag: &mut Drag, _tx: &mut Tx| {
            s.editor_drag(line, foldable, drag);
        });
    if frame.caret.line == line {
        view = view.child(
            el().key(format!("aline:{line}"))
                .absolute(0.0, 0.0)
                .w_full()
                .h(ROW_H)
                .instant_changes()
                .fill(palette.active_line),
        );
    }
    let text_x = GUTTER_W + TEXT_PAD;
    for (index, (start, end)) in range_spans(
        frame.selection.iter().map(|range| (range.from, range.to)),
        line,
        measured,
    )
    .into_iter()
    .enumerate()
    {
        view = view.child(
            el().key(format!("sel:{line}:{index}"))
                .absolute(text_x + start, 0.0)
                .w((end - start).max(1.0))
                .h(ROW_H)
                .instant_changes()
                .fill(palette.selection),
        );
    }
    for (index, decoration) in frame
        .decorations
        .iter()
        .filter(|decoration| decoration.range.from.line <= line && decoration.range.to.line >= line)
        .enumerate()
    {
        let spans = range_spans(
            std::iter::once((decoration.range.from, decoration.range.to)),
            line,
            measured,
        );
        if let Some((start, end)) = spans.first() {
            view = view.child(
                el().key(format!("deco:{line}:{index}"))
                    .absolute(text_x + start, 1.0)
                    .w((end - start).max(1.0))
                    .h(ROW_H - 2.0)
                    .radius(2.0)
                    .instant_changes()
                    .fill(decoration_color(decoration.kind, palette)),
            );
        }
    }
    view = view.child(gutter_view(line_row, frame.caret.line == line, palette));
    view = view.child(text_view(ui, frame, line_row, measured, palette));
    let carets = std::iter::once(frame.caret).chain(frame.extra_carets.iter().copied());
    for (index, place) in carets.enumerate().filter(|(_, place)| place.line == line) {
        let mut caret = el()
            .key(if index == 0 {
                "caret".to_string()
            } else {
                format!("caret:{index}")
            })
            .absolute(text_x + measured.column_to_x(place.column), 1.0)
            .size(1.5, ROW_H - 2.0)
            .instant_changes()
            .fill(palette.caret);
        if index == 0 {
            caret = caret.ime_cursor();
        } else {
            caret = caret.opacity(0.6);
        }
        view = view.child(caret);
        if index == 0 && !ui.preedit.is_empty() {
            view = view.child(preedit_view(
                &ui.preedit,
                text_x + measured.column_to_x(place.column),
                palette,
            ));
        }
    }
    view
}

/// The composition, over the text at the caret: marked text the editor has
/// not seen, underlined the way every input method shows it.
fn preedit_view(preedit: &str, x: f32, palette: EditorPalette) -> El {
    let underline = Span::new(0, preedit.len()).underline(palette.caret);
    el().key("editor-preedit")
        .absolute(x, 0.0)
        .h(ROW_H)
        .items_center()
        .fill(palette.background)
        .instant_changes()
        .child(
            rich(preedit.to_string(), vec![underline])
                .font(metrics::mono())
                .color(palette.foreground)
                .selectable(false)
                .inert(),
        )
}

/// `[x0, x1)` pieces of `ranges` on `line`, in row-local text coordinates.
fn range_spans(
    ranges: impl Iterator<Item = (EditorPlace, EditorPlace)>,
    line: u32,
    measured: &RowMetrics,
) -> Vec<(f32, f32)> {
    let mut spans = Vec::new();
    for (from, to) in ranges {
        if line < from.line || line > to.line {
            continue;
        }
        let start = if line == from.line { from.column } else { 0 };
        let end = if line == to.line {
            to.column
        } else {
            measured.utf16_len
        }
        .min(measured.utf16_len);
        let start = start.min(end);
        let (x0, x1) = (measured.column_to_x(start), measured.column_to_x(end));
        // A selection over a line break still marks an empty line, the way
        // every editor does, so the range reads as continuous.
        if x1 > x0 || (line != to.line && end == start) {
            spans.push((x0, x1.max(x0 + 6.0)));
        }
    }
    spans
}

fn gutter_view(line_row: &EditorRow, active: bool, palette: EditorPalette) -> El {
    let line = line_row.line;
    let (mark, diagnostic, fold) = (line_row.mark, line_row.diagnostic, line_row.fold);
    let version = (
        mark_code(mark),
        severity_code(diagnostic),
        fold,
        active,
        palette.rev,
    );
    memo(format!("gutter:{line}"), version, move || {
        let number = text(format!("{}", line + 1))
            .font(metrics::mono())
            .color(if active {
                palette.gutter_active_foreground
            } else {
                palette.gutter_foreground
            })
            .w(40.0)
            .align(Align::End)
            .truncate();
        let bar = el()
            .w(3.0)
            .h(14.0)
            .radius(1.5)
            .fill(mark_color(mark, palette));
        let dot = el()
            .size(6.0, 6.0)
            .radius(3.0)
            .fill(severity_color(diagnostic, palette));
        let chevron = match fold {
            Some(0) => text("▾")
                .font(metrics::mono())
                .color(palette.fold)
                .w(12.0)
                .align(Align::Center),
            Some(_) => text("▸")
                .font(metrics::mono())
                .color(palette.fold)
                .w(12.0)
                .align(Align::Center),
            None => el().w(12.0),
        };
        row()
            .w(GUTTER_W)
            .h(ROW_H)
            .items_center()
            .px(6.0)
            .gap(4.0)
            .child(number)
            .child(bar)
            .child(dot)
            .child(chevron)
    })
}

fn text_view(
    ui: &EditorUi,
    frame: &EditorFrame,
    line_row: &EditorRow,
    measured: &RowMetrics,
    palette: EditorPalette,
) -> El {
    // An unknown fingerprint (an older daemon) falls back to the document
    // version, which rebuilds the row on every edit but never shows a stale one.
    let identity = if line_row.fingerprint == 0 {
        (0, frame.doc_version)
    } else {
        (line_row.fingerprint, 0)
    };
    let version = (identity, line_row.fold, palette.rev);
    let source = line_row.spans.clone();
    let content = measured.text.clone();
    let folded = line_row.fold.filter(|hidden| *hidden > 0);
    let truncated = line_row.truncated;
    let builds = &ui.row_builds;
    memo(format!("text:{}", line_row.line), version, move || {
        builds.set(builds.get() + 1);
        let mut spans = Vec::with_capacity(source.len());
        let mut start = 0usize;
        for span in source.iter() {
            let end = start + span.text.len();
            if span.scope != EditorScope::Plain {
                spans.push(Span::new(start, end).color(palette.scope(span.scope)));
            }
            start = end;
        }
        let mut shown = row()
            .grow(1.0)
            .min_w(0.0)
            .pl(TEXT_PAD)
            .items_center()
            .child(
                rich(content.to_string(), spans)
                    .font(metrics::mono())
                    .color(palette.foreground)
                    .selectable(false)
                    .inert(),
            );
        if truncated {
            shown = shown.child(text(" …").font(metrics::mono()).color(palette.fold));
        }
        if let Some(hidden) = folded {
            shown = shown.child(
                text(format!("  … {hidden} lines"))
                    .font(metrics::mono())
                    .color(palette.fold),
            );
        }
        shown
    })
}

impl AppState {
    pub fn editor_drag(&mut self, line: u32, foldable: bool, drag: &mut Drag) {
        if drag.cancelled() {
            return;
        }
        if drag.x < GUTTER_W {
            if foldable && drag.phase == Phase::Down && drag.x >= GUTTER_W - FOLD_HIT_W {
                self.editor.queue(EditorInputEvent::ToggleFold { line });
            }
            return;
        }
        let Some(frame) = &self.editor.frame else {
            return;
        };
        let Some(row) = frame.rows.iter().find(|row| row.line == line) else {
            return;
        };
        let column = self
            .editor
            .metrics
            .borrow_mut()
            .row(row)
            .x_to_column(drag.x - GUTTER_W - TEXT_PAD);
        let kind = match drag.phase {
            Phase::Down => EditorPointer::Down,
            Phase::Move => EditorPointer::Drag,
            Phase::Up => EditorPointer::Up,
        };
        self.editor.queue(EditorInputEvent::Pointer {
            kind,
            line,
            column,
            modifiers: 0,
        });
    }
}

fn mark_code(mark: Option<EditorMark>) -> u8 {
    match mark {
        Some(EditorMark::Added) => 1,
        Some(EditorMark::Modified) => 2,
        Some(EditorMark::Deleted) => 3,
        None => 0,
        _ => 4,
    }
}

fn severity_code(severity: Option<EditorSeverity>) -> u8 {
    match severity {
        Some(EditorSeverity::Error) => 1,
        Some(EditorSeverity::Warning) => 2,
        Some(EditorSeverity::Info) => 3,
        None => 0,
        _ => 4,
    }
}

fn mark_color(mark: Option<EditorMark>, palette: EditorPalette) -> Color {
    match mark {
        Some(EditorMark::Added) => palette.git_added,
        Some(EditorMark::Modified) => palette.git_modified,
        Some(EditorMark::Deleted) => palette.git_deleted,
        _ => Color::TRANSPARENT,
    }
}

fn severity_color(severity: Option<EditorSeverity>, palette: EditorPalette) -> Color {
    match severity {
        Some(EditorSeverity::Error) => palette.git_deleted,
        Some(EditorSeverity::Warning) => palette.git_modified,
        Some(EditorSeverity::Info) => palette.property,
        _ => Color::TRANSPARENT,
    }
}

fn decoration_color(kind: EditorDecorationKind, palette: EditorPalette) -> Color {
    match kind {
        EditorDecorationKind::ActiveMatch => palette.search_match_selected,
        EditorDecorationKind::Bracket => palette.matching_bracket,
        _ => palette.search_match,
    }
}
