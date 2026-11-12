//! The editor pane: a scroll container as tall as the document's rows, the
//! mounted window inside it, and quads for caret, selection and decorations.
//!
//! Row text and gutters are memoised on what they paint (the row fingerprint,
//! the mark, the fold), so a caret move or a scroll rebuilds no text.

use domain::{
    EditorDecorationKind, EditorFind, EditorFindCommand, EditorFrame, EditorInputEvent, EditorMark,
    EditorPlace, EditorPointer, EditorRow, EditorScope, EditorSeverity, SessionId,
};
use rune::{
    Align, Color, Cursor, Drag, El, Phase, ScrollSpec, Span, Tx, col, el, icon, memo, rich, row,
    text,
};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, TextInput};

use super::aria;
use super::metrics::{self, RowMetrics};
use super::theme::{self, EditorPalette};
use super::{
    EditorUi, FOLD_HIT_W, GUTTER_PAD, MARK_GAP, MARK_W, ROW_H, SCROLL_KEY, TEXT_PAD, conflict,
    content_rows, find, visual_base,
};
use crate::app::AppState;

const HEADER_H: f32 = 30.0;
const STATUS_H: f32 = 24.0;
const COMPARE_H: f32 = 260.0;
/// A comparison is a read, not a second editor: past this the rest is said
/// to be left out rather than mounted as thousands of rows.
const MAX_COMPARE_ROWS: usize = 2_000;

pub fn pane(s: &AppState, k: &Kit) -> El {
    let base = s.theme_base(k.dark);
    let palette = theme::for_base(base, k.dark);
    let ui = &s.editor;
    let editor = s.active_editor_state();
    let path = editor.map_or("editor", |state| state.path.as_str());
    let aria = aria::editor_aria(editor);
    let announcement = aria::editor_announcement(editor);
    // The label already names the file; the header would say it twice.
    let mut header = row()
        .key("editor-header")
        .w_full()
        .h(HEADER_H)
        .items_center()
        .px(12.0)
        .gap(10.0)
        .child(
            text(path.to_string())
                .font(k.ty.small_medium)
                .color(palette.foreground)
                .a11y_hidden(),
        )
        .child(
            text(aria::editor_mark(editor))
                .font(k.ty.text_xsmall)
                .color(if editor.is_some_and(|state| state.dirty) {
                    palette.git_modified
                } else {
                    palette.gutter_foreground
                })
                .a11y_hidden(),
        )
        .child(
            text(aria::editor_details(editor, path))
                .font(k.ty.text_xsmall)
                .color(palette.gutter_foreground)
                .grow(1.0)
                .a11y_hidden(),
        );
    if let Some(state) = editor.filter(|state| {
        crate::workbench::preview_kind(&state.path)
            .is_some_and(crate::workbench::PreviewKind::has_source)
    }) {
        let path = state.path.clone();
        header = header.child(
            crate::ds::icon_button(
                &crate::glyphs::EYE,
                "Show Rendered",
                crate::ds::IconButtonSize::Mini,
            )
            .help("Show Rendered")
            .on_press(move |s: &mut AppState| s.preview_file(&path))
            .build(k)
            .key("editor-preview"),
        );
    }
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
        .gap(crate::ds::SPACE_6)
        .fill(palette.active_line)
        .child(
            text(match caret {
                Some(caret) => format!("Ln {}, Col {}", caret.line + 1, caret.column + 1),
                None => String::new(),
            })
            .font(k.ty.text_xsmall)
            .color(palette.gutter_foreground)
            .a11y_hidden(),
        )
        // The editor's answer to the last gesture, which a person not looking
        // at the status row would otherwise never hear.
        .child(
            text(announcement)
                .key("editor-announce")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(if editor.is_some_and(|state| state.conflict) {
                    palette.git_deleted
                } else {
                    palette.gutter_foreground
                }),
        );
    let mut pane = col()
        .key("editor-pane")
        // Rune gives text-field roles only to its own text inputs, so the
        // surface is a document; the caret is told through the description.
        .role("document")
        .label(path.to_string())
        .description(format!("{} {}", aria.line, aria.status))
        .a11y_value(ui.a11y_text.to_string())
        .read_only(aria.read_only)
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(palette.background)
        .focusable(true)
        .no_focus_ring()
        .on_key(|s: &mut AppState, ev, tx| s.editor_key(ev, tx))
        .child(header);
    if let Some(session) = s.active_line_editor() {
        pane = pane.on_secondary_press(move |s: &mut AppState, point, _| {
            crate::menus::open(s, crate::menus::Target::Editor(session), point);
        });
        if editor.is_some_and(|state| state.conflict) {
            pane = pane.child(conflict_note(ui, session, palette, k));
            if let Some(held) = ui.conflicts.get(&session).filter(|held| held.comparing) {
                pane = pane.child(compare_view(held, palette, k));
            }
        }
        if let Some(open) = editor.and_then(|state| state.find.as_deref()) {
            pane = pane.child(find_panel(ui, open, palette, k));
        }
    }
    pane.child(body).child(status)
}

fn conflict_note(ui: &EditorUi, session: SessionId, palette: EditorPalette, k: &Kit) -> El {
    let comparing = ui
        .conflicts
        .get(&session)
        .is_some_and(|held| held.comparing);
    let button = |label: &'static str,
                  key: &'static str,
                  chosen: bool,
                  act: fn(&mut AppState, SessionId)| {
        Button::new(label)
            .variant(if chosen {
                ButtonVariant::Neutral
            } else {
                ButtonVariant::GhostMuted
            })
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| act(s, session))
            .build(k)
            .key(key)
    };
    row()
        .key("editor-conflict")
        .w_full()
        .items_center()
        .px(12.0)
        .py(4.0)
        .gap(crate::ds::SPACE_4)
        .fill(palette.git_deleted.alpha(0.12))
        .child(
            el().size(6.0, 6.0)
                .radius(3.0)
                .fill(palette.git_deleted)
                .a11y_hidden(),
        )
        .child(
            text("This file changed on disk while you were editing it.")
                .font(k.ty.text_xsmall)
                .color(palette.foreground)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
        .child(button(
            "Keep mine",
            "editor-conflict-keep",
            false,
            AppState::keep_mine,
        ))
        .child(button(
            "Take disk",
            "editor-conflict-take",
            false,
            AppState::take_disk,
        ))
        .child(
            button(
                "Compare",
                "editor-conflict-compare",
                comparing,
                AppState::toggle_compare,
            )
            .checked(comparing),
        )
}

/// Disk against mine as one read-only list; a side is kept with the note's
/// buttons, never by editing here.
fn compare_view(held: &conflict::Conflict, palette: EditorPalette, k: &Kit) -> El {
    let mut list = col().key("editor-compare-rows").w_full();
    if let Some(error) = &held.error {
        list = list.child(
            text(format!("Could not read the conflict: {error}"))
                .key("editor-compare-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(palette.git_deleted)
                .padding(8.0),
        );
    } else if held.loading && held.rows.is_none() {
        list = list.child(
            text("Reading both sides…")
                .font(k.ty.text_xsmall)
                .color(palette.gutter_foreground)
                .padding(8.0),
        );
    }
    if let Some(rows) = &held.rows {
        for (index, line) in rows.iter().take(MAX_COMPARE_ROWS).enumerate() {
            list = list.child(compare_row(index, line, palette));
        }
        if rows.len() > MAX_COMPARE_ROWS {
            list = list.child(
                text(format!(
                    "{} more lines are not drawn here.",
                    rows.len() - MAX_COMPARE_ROWS
                ))
                .font(k.ty.text_xsmall)
                .color(palette.gutter_foreground)
                .padding(8.0),
            );
        }
    }
    col()
        .key("editor-compare")
        .role("region")
        .label("Disk against your copy")
        .w_full()
        .h(COMPARE_H)
        .min_h(0.0)
        .fill(palette.active_line)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list)
}

fn compare_row(index: usize, line: &conflict::Row, palette: EditorPalette) -> El {
    let (fill, mark) = match line.change {
        conflict::Change::Added => (palette.git_added.alpha(0.14), "+"),
        conflict::Change::Removed => (palette.git_deleted.alpha(0.14), "−"),
        conflict::Change::Equal => (Color::TRANSPARENT, ""),
    };
    let number = |n: Option<u32>| {
        text(n.map_or_else(String::new, |n| n.to_string()))
            .font(metrics::mono(metrics::MONO_SIZE))
            .color(palette.gutter_foreground)
            .w(44.0)
            .align(Align::End)
            .truncate()
    };
    row()
        .key(format!("compare:{index}"))
        .w_full()
        .h(ROW_H)
        .items_center()
        .gap(crate::ds::SPACE_4)
        .px(6.0)
        .fill(fill)
        .child(number(line.left))
        .child(number(line.right))
        .child(
            text(mark)
                .font(metrics::mono(metrics::MONO_SIZE))
                .color(palette.gutter_foreground)
                .w(10.0),
        )
        .child(
            text(line.text.clone())
                .font(metrics::mono(metrics::MONO_SIZE))
                .color(palette.foreground)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
}

/// Find in the file, as a bar under the header: the field, the editor's
/// count, the three toggles, previous, next and close.
fn find_panel(ui: &EditorUi, open: &EditorFind, palette: EditorPalette, k: &Kit) -> El {
    let typed = ui.find.edit.text.as_str();
    let refused = open.error.is_some() || find::too_long(typed);
    let field = TextInput::new(&ui.find.edit)
        .label("Find")
        .placeholder("Find")
        .leading_icon(&crate::glyphs::SEARCH)
        .invalid(refused)
        .width(Some(220.0))
        .on_edit(|s: &mut AppState| &mut s.editor.find.edit)
        .on_change(|s: &mut AppState, _| s.find_typed())
        .on_key(|s: &mut AppState, ev, tx| s.find_key(ev, tx))
        .build(k)
        .key("editor-find-input");
    let mut toggles = row()
        .key("editor-find-toggles")
        .role("group")
        .label("Search options")
        .gap(crate::ds::SPACE_1);
    for (flag, glyph, label) in find::TOGGLES {
        let on = find::flag(open, flag);
        toggles = toggles.child(
            Button::new(glyph)
                .variant(if on {
                    ButtonVariant::Neutral
                } else {
                    ButtonVariant::GhostMuted
                })
                .control_size(ControlSize::Small)
                .help(label)
                .on_press(move |s: &mut AppState| s.find_toggle(flag))
                .build(k)
                .key(format!("editor-find-{}", glyph_key(flag)))
                .label(label)
                .checked(on),
        );
    }
    let step = |glyph: &'static rune::Icon,
                label: &'static str,
                key: &'static str,
                command: fn() -> EditorFindCommand| {
        IconButton::new(glyph)
            .variant(IconButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .label(label)
            .disabled(open.total == 0)
            .on_press(move |s: &mut AppState| s.send_editor_find(command()))
            .build(k)
            .key(key)
    };
    row()
        .key("editor-find")
        .role("search")
        .label("Find in file")
        .focus_return("editor-pane")
        .w_full()
        .items_center()
        .justify_end()
        .px(12.0)
        .py(4.0)
        .gap(crate::ds::SPACE_3)
        .fill(palette.active_line)
        .child(field)
        .child(
            text(find::counter(open, typed))
                .key("editor-find-count")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(if refused {
                    palette.git_deleted
                } else {
                    palette.gutter_foreground
                })
                .w(110.0)
                .truncate(),
        )
        .child(toggles)
        .child(step(
            &crate::glyphs::CHEVRON_UP,
            "Previous match",
            "editor-find-previous",
            || EditorFindCommand::Previous,
        ))
        .child(step(
            &crate::glyphs::CHEVRON_DOWN,
            "Next match",
            "editor-find-next",
            || EditorFindCommand::Next,
        ))
        .child(
            IconButton::new(&crate::glyphs::CLOSE)
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label("Close find")
                .on_press(|s: &mut AppState| s.send_editor_find(EditorFindCommand::Close))
                .build(k)
                .key("editor-find-close"),
        )
}

fn glyph_key(flag: find::Flag) -> &'static str {
    match flag {
        find::Flag::Case => "case",
        find::Flag::Word => "word",
        find::Flag::Regex => "regex",
    }
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

    let row_h = ui.row_h;
    let content_height = content_rows(frame) as f32 * row_h;
    let top = visual_base(frame) as f32 * row_h;
    let bottom = (content_height - top - frame.rows.len() as f32 * row_h).max(0.0);
    let mut content = col()
        .key("editor-content")
        .w_full()
        .h(content_height.max(top + frame.rows.len() as f32 * row_h));
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
        .on_wheel(|s: &mut crate::app::AppState, ev, _tx| crate::pages::sidebar_swipe(s, &ev))
        // Rows, gutters and spans are paint: the pane's value already carries
        // the text, and hundreds of nodes per window would bury it.
        .child(content.cursor(Cursor::Text).a11y_hidden())
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
    let ty = Ty::of(ui);
    let row_h = ty.row_h;
    let mut view = el()
        .key(format!("row:{line}"))
        .w_full()
        .h(row_h)
        .items_center()
        .on_drag(move |s: &mut AppState, drag: &mut Drag, tx: &mut Tx| {
            s.editor_drag(line, foldable, drag, tx.mods());
        });
    if frame.caret.line == line {
        view = view.child(
            el().key(format!("aline:{line}"))
                .absolute(0.0, 0.0)
                .w_full()
                .h(row_h)
                .instant_changes()
                .fill(palette.active_line),
        );
    }
    let text_x = ty.gutter_w + TEXT_PAD;
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
                .h(row_h)
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
                    .h(row_h - 2.0)
                    .radius(2.0)
                    .instant_changes()
                    .fill(decoration_color(decoration.kind, palette)),
            );
        }
    }
    view = view.child(gutter_view(line_row, frame.caret.line == line, palette, ty));
    view = view.child(text_view(ui, frame, line_row, measured, palette, ty));
    let carets = std::iter::once(frame.caret).chain(frame.extra_carets.iter().copied());
    for (index, place) in carets.enumerate().filter(|(_, place)| place.line == line) {
        let mut caret = el()
            .key(if index == 0 {
                "caret".to_string()
            } else {
                format!("caret:{index}")
            })
            .absolute(text_x + measured.column_to_x(place.column), 1.0)
            .size(1.5, row_h - 2.0)
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
                ty,
            ));
        }
    }
    view
}

/// The type rows are drawn with, copied into their memoised builders.
#[derive(Clone, Copy)]
struct Ty {
    row_h: f32,
    size: f32,
    gutter_w: f32,
    number_w: f32,
    rev: u32,
}

impl Ty {
    fn of(ui: &EditorUi) -> Self {
        Ty {
            row_h: ui.row_h,
            size: ui.font_size,
            gutter_w: ui.gutter_w(),
            number_w: ui.number_w(),
            rev: ui.type_rev,
        }
    }

    fn mono(self) -> rune::tok::TextStyle {
        metrics::mono(self.size)
    }
}

/// The composition, over the text at the caret: marked text the editor has
/// not seen, underlined the way every input method shows it.
fn preedit_view(preedit: &str, x: f32, palette: EditorPalette, ty: Ty) -> El {
    let underline = Span::new(0, preedit.len()).underline(palette.caret);
    el().key("editor-preedit")
        .absolute(x, 0.0)
        .h(ty.row_h)
        .items_center()
        .fill(palette.background)
        .instant_changes()
        .child(
            rich(preedit.to_string(), vec![underline])
                .font(ty.mono())
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

fn gutter_view(line_row: &EditorRow, active: bool, palette: EditorPalette, ty: Ty) -> El {
    let line = line_row.line;
    let (mark, diagnostic, fold) = (line_row.mark, line_row.diagnostic, line_row.fold);
    let version = (
        mark_code(mark),
        severity_code(diagnostic),
        fold,
        active,
        palette.rev,
        ty.rev,
    );
    memo(format!("gutter:{line}"), version, move || {
        let number = text(format!("{}", line + 1))
            .font(ty.mono())
            .color(if active {
                palette.gutter_active_foreground
            } else {
                palette.gutter_foreground
            })
            .w(ty.number_w)
            .align(Align::End)
            .truncate();
        // A whole-row bar, so consecutive changed lines read as one strip, and
        // a deletion a wedge on the row's top edge, where the lines went.
        let bar = if mark == Some(EditorMark::Deleted) {
            col().w(MARK_W).h(ty.row_h).child(
                el().w(MARK_W * 2.0)
                    .h(MARK_W)
                    .radius(MARK_W / 2.0)
                    .fill(mark_color(mark, palette)),
            )
        } else {
            el().w(MARK_W).h(ty.row_h).fill(mark_color(mark, palette))
        };
        let bar = if mark.is_some() {
            bar.cursor(Cursor::Pointer)
        } else {
            bar
        };
        let dot = el()
            .size(6.0, 6.0)
            .radius(3.0)
            .fill(severity_color(diagnostic, palette));
        let chevron = match fold {
            Some(hidden) => el().w(12.0).center().child(
                icon(
                    if hidden == 0 {
                        &crate::glyphs::CHEVRON_DOWN
                    } else {
                        &crate::glyphs::CHEVRON_RIGHT
                    },
                    12.0,
                )
                .color(palette.fold),
            ),
            None => el().w(12.0),
        };
        row()
            .w(ty.gutter_w)
            .h(ty.row_h)
            .items_center()
            .px(GUTTER_PAD)
            .child(number)
            .child(bar.ml(MARK_GAP))
            .child(dot.ml(crate::ds::SPACE_2))
            .child(chevron.ml(crate::ds::SPACE_2))
    })
}

fn text_view(
    ui: &EditorUi,
    frame: &EditorFrame,
    line_row: &EditorRow,
    measured: &RowMetrics,
    palette: EditorPalette,
    ty: Ty,
) -> El {
    // An unknown fingerprint (an older daemon) falls back to the document
    // version, which rebuilds the row on every edit but never shows a stale one.
    let identity = if line_row.fingerprint == 0 {
        (0, frame.doc_version)
    } else {
        (line_row.fingerprint, 0)
    };
    let version = (identity, line_row.fold, palette.rev, ty.rev);
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
                    .font(ty.mono())
                    .color(palette.foreground)
                    .selectable(false)
                    .inert(),
            );
        if truncated {
            shown = shown.child(text(" …").font(ty.mono()).color(palette.fold));
        }
        if let Some(hidden) = folded {
            shown = shown.child(
                text(format!("  … {hidden} lines"))
                    .font(ty.mono())
                    .color(palette.fold),
            );
        }
        shown
    })
}

impl AppState {
    pub fn editor_drag(&mut self, line: u32, foldable: bool, drag: &mut Drag, mods: rune::Mods) {
        if drag.cancelled() {
            return;
        }
        let gutter_w = self.editor.gutter_w();
        if drag.x < gutter_w {
            if drag.phase != Phase::Down {
                return;
            }
            if foldable && drag.x >= gutter_w - FOLD_HIT_W {
                self.editor.queue(EditorInputEvent::ToggleFold { line });
            } else if crate::editor::on_mark(drag.x, self.editor.number_w()) && self.marked(line) {
                self.open_line_diff(line);
            }
            return;
        }
        let Some(frame) = &self.editor.frame else {
            return;
        };
        let Some(start) = frame.rows.iter().position(|row| row.line == line) else {
            return;
        };
        let row_h = self.editor.row_h.max(1.0);
        let steps = (drag.y / row_h).floor() as i64;
        let visual =
            (i64::from(crate::editor::visual_base(frame)) + start as i64 + steps).max(0) as u32;
        let target = crate::editor::document_line(frame, visual);
        let Some(row) = frame
            .rows
            .iter()
            .find(|row| row.line == target)
            .or_else(|| frame.rows.iter().find(|row| row.line == line))
        else {
            return;
        };
        let column = self
            .editor
            .metrics
            .borrow_mut()
            .row(row)
            .x_to_column((drag.x - gutter_w - TEXT_PAD).max(0.0));
        let kind = match drag.phase {
            Phase::Down => EditorPointer::Down,
            Phase::Move => EditorPointer::Drag,
            Phase::Up => EditorPointer::Up,
        };
        self.editor.queue(EditorInputEvent::Pointer {
            kind,
            line: target,
            column,
            modifiers: crate::editor::modifier_bits(mods),
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
