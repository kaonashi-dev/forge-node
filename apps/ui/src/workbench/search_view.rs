//! The centre's project search: a query
//! field, then each file's hits with their context. A line opens the file at
//! that line.

use std::time::Instant;

use rune::{El, ScrollSpec, Span, col, icon, rich, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, TextInput};

use super::search::{ExcerptLine, FileHits, should_search};
use crate::app::AppState;

const LINE_H: f32 = 20.0;

fn note(k: &Kit, key: &'static str, message: &str) -> El {
    text(message.to_string())
        .key(key)
        .font(k.ty.small)
        .color(k.c.text_faint)
        .padding(16.0)
}

pub fn view(s: &AppState, k: &Kit) -> El {
    let search = &s.workbench.search;
    let field = TextInput::new(&search.edit)
        .placeholder("Search in files…")
        .label("Search in files")
        .leading_icon(k.glyph.search)
        .clear_button()
        .on_edit(|s: &mut AppState| &mut s.workbench.search.edit)
        .on_change(|s: &mut AppState, _| s.workbench.search.typed(Instant::now()))
        .on_submit(|s: &mut AppState, _| s.workbench.search.search_now(Instant::now()))
        .build(k)
        .key("project-search-field")
        .grow(1.0)
        .min_w(0.0);
    let summary = search
        .results
        .as_ref()
        .map(super::search::Results::summary)
        .unwrap_or_default();
    let has_files = search
        .results
        .as_ref()
        .is_some_and(|results| !results.files.is_empty());
    let bar = row()
        .key("project-search-bar")
        .w_full()
        .px(12.0)
        .py(8.0)
        .gap(10.0)
        .items_center()
        .child(field)
        .child(
            text(summary)
                .key("project-search-summary")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        )
        .children(has_files.then(|| {
            Button::new(if search.all_folded() {
                "Unfold all"
            } else {
                "Fold all"
            })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.workbench.search.toggle_all())
            .build(k)
            .key("project-search-fold")
        }))
        .child(
            Button::new("Search again")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.workbench.search.search_now(Instant::now()))
                .build(k)
                .key("project-search-again"),
        );
    let mut view = col()
        .key("project-search")
        .role("region")
        .label("Search in files")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(bar);
    if let Some(error) = &search.error {
        view = view.child(
            text(error.clone())
                .key("project-search-error")
                .role("status")
                .font(k.ty.small)
                .color(k.c.warning.fg)
                .px(16.0),
        );
    }
    if search.stale {
        view = view.child(note(
            k,
            "project-search-stale",
            "Files changed. Search again to refresh these results.",
        ));
    }
    let body = if s.workbench.workspace.is_none() {
        note(k, "project-search-empty", "No checkout selected.")
    } else if !should_search(search.query()) {
        note(
            k,
            "project-search-hint",
            "Type at least two characters to search the checkout.",
        )
    } else {
        match &search.results {
            None if search.searching => note(k, "project-search-busy", "Searching the checkout…"),
            None => note(k, "project-search-wait", ""),
            Some(results) if results.files.is_empty() => {
                note(k, "project-search-none", "Nothing matches that.")
            }
            Some(results) => {
                let mut list = col().key("project-search-files").w_full().gap(6.0).px(8.0);
                for file in &results.files {
                    list = list.child(file_group(s, k, file));
                }
                list
            }
        }
    };
    view.child(
        col()
            .key("project-search-scroll")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(body.pb(16.0)),
    )
}

fn file_group(s: &AppState, k: &Kit, file: &FileHits) -> El {
    let folded = s.workbench.search.folded.contains(&file.path);
    let cut = file.path.rfind('/').map_or(0, |slash| slash + 1);
    let path = file.path.clone();
    let head = row()
        .key(format!("search-file-{}", file.path))
        .role("button")
        .label(format!("{}, {} hits", file.path, file.hits))
        .expanded(!folded)
        .w_full()
        .h(26.0)
        .px(6.0)
        .gap(6.0)
        .radius(5.0)
        .items_center()
        .hover(k.c.hover)
        .on_press(move |s: &mut AppState| s.workbench.search.toggle(&path))
        .child(
            icon(
                if folded {
                    k.glyph.caret_right
                } else {
                    k.glyph.chevron_down
                },
                12.0,
            )
            .color(k.c.icon_muted),
        )
        .child(icon(file.icon.icon(!k.dark), 14.0))
        .child(
            text(file.path[cut..].to_string())
                .font(k.ty.small_medium)
                .color(k.c.text_base),
        )
        .child(
            text(file.path[..cut].to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
        .child(
            text(file.hits.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        );
    let mut group = col()
        .key(format!("search-group-{}", file.path))
        .w_full()
        .child(head);
    if folded {
        return group;
    }
    for (index, excerpt) in file.excerpts.iter().enumerate() {
        let mut lines = col()
            .key(format!("excerpt-{index}"))
            .w_full()
            .ml(18.0)
            .py(2.0)
            .radius(5.0)
            .fill(k.c.surface_weak);
        for line in excerpt {
            lines = lines.child(excerpt_line(k, &file.path, line));
        }
        group = group.child(lines);
    }
    group
}

fn excerpt_line(k: &Kit, path: &str, line: &ExcerptLine) -> El {
    let hit_bg = crate::theme::roles(k).git.modified.alpha(0.28);
    let spans: Vec<Span> = line
        .hits
        .iter()
        .map(|range| {
            let mut span = Span::new(range.start, range.end);
            span.bg = Some(hit_bg);
            span.color = Some(k.c.text_base);
            span
        })
        .collect();
    let path = path.to_string();
    let number = line.line;
    row()
        .key(format!("l{}", line.line))
        .role("button")
        .label(format!("{path}, line {number}"))
        .w_full()
        .h(LINE_H)
        .px(6.0)
        .items_center()
        .hover(k.c.hover)
        .on_press(move |s: &mut AppState| s.open_file(&path, Some(number)))
        .child(
            text(line.line.to_string())
                .font(k.ty.mono)
                .color(k.c.text_faint)
                .w(48.0)
                .pr(10.0)
                .align(rune::Align::End),
        )
        .child(
            rich(line.text.clone(), spans)
                .font(k.ty.mono)
                .color(if line.hit {
                    k.c.text_base
                } else {
                    k.c.text_muted
                })
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
}
