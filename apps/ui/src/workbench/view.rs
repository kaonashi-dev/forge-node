//! The Changes and Files views, and the switch between them and the session.

use rune::{El, GrowthSpec, ScrollSpec, col, el, icon, rich, row, text};
use rune_kit::diff::DiffView;
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{
    Button, ButtonVariant, ControlSize, FileNode, Kit, Markdown, SegmentedControl, TextInput, Tree,
    TreeItem, TreeStyle,
};

use super::decorations::Tone;
use super::{CenterMode, Preview, PreviewKind};
use crate::app::AppState;
use crate::file_icons::FileIcon;

const CODE_ROW_H: f32 = 18.0;
const CSV_ROW_H: f32 = 24.0;
const CSV_COLUMN_W: f32 = 160.0;

fn tone_color(k: &Kit, tone: Tone) -> rune::Color {
    let git = crate::theme::roles(k).git;
    match tone {
        Tone::Added => git.added,
        Tone::Modified => git.modified,
        Tone::Deleted => git.deleted,
        Tone::Conflict => git.conflict,
    }
}

fn bar(k: &Kit, title: String, detail: String) -> El {
    let refresh = Button::new("Refresh")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(|s: &mut AppState| s.workbench.refresh())
        .build(k)
        .key("workbench-refresh");
    row()
        .key("workbench-bar")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(10.0)
        .items_center()
        .child(text(title).font(k.ty.small_medium).color(k.c.text_base))
        .child(
            text(detail)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .grow(1.0),
        )
        .child(refresh)
}

fn notice(k: &Kit, key: &'static str, message: String) -> El {
    col()
        .key(key)
        .w_full()
        .grow(1.0)
        .padding(24.0)
        .child(text(message).font(k.ty.small).color(k.c.text_muted))
}

/// A diff status as one letter, as the original client's `statusLetter`.
pub(crate) fn status_letter(status: domain::DiffStatus) -> &'static str {
    match status {
        domain::DiffStatus::Added => "A",
        domain::DiffStatus::Modified => "M",
        domain::DiffStatus::Deleted => "D",
        domain::DiffStatus::Conflicted => "U",
        _ => "?",
    }
}

/// A diff status in words for the row's accessible name, as the original client's `statusWord`.
pub(crate) fn status_word(status: domain::DiffStatus) -> &'static str {
    match status {
        domain::DiffStatus::Added => "added",
        domain::DiffStatus::Modified => "modified",
        domain::DiffStatus::Deleted => "deleted",
        domain::DiffStatus::Conflicted => "unresolved conflict",
        _ => "changed",
    }
}

fn changed_rows(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    let mut list = col().key("changed-files").w_full().gap(1.0);
    let note = |message: String| text(message).font(k.ty.small).color(k.c.text_faint);
    if s.workbench.workspace.is_none() {
        return list.child(note("No checkout in this window.".into()));
    }
    if let Some(error) = &changes.error {
        return list.child(note(format!("Could not read the diff: {error}")));
    }
    if !changes.loaded {
        return list.child(note("Reading the working tree…".into()));
    }
    if changes.files.is_empty() {
        list = list.child(note("Nothing uncommitted.".into()).key("changes-clean"));
    }
    let shown = s.workbench.mode == CenterMode::Changes;
    for (index, file) in changes.files.iter().enumerate() {
        let selected = shown && changes.selected == Some(index);
        let name = file
            .path
            .rsplit('/')
            .next()
            .unwrap_or(&file.path)
            .to_string();
        let dir = file
            .path
            .strip_suffix(name.as_str())
            .unwrap_or_default()
            .trim_end_matches('/')
            .to_string();
        list = list.child(
            row()
                .key(format!("changed-{}", file.path))
                .role("button")
                .label(format!(
                    "{}, {}, {} added, {} removed",
                    file.path,
                    status_word(file.status),
                    file.additions,
                    file.deletions
                ))
                .w_full()
                .h(26.0)
                .px(8.0)
                .gap(6.0)
                .radius(5.0)
                .items_center()
                .fill(if selected {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press(move |s: &mut AppState| {
                    s.workbench.select_change(index);
                    s.workbench.mode = CenterMode::Changes;
                })
                .child(
                    text(status_letter(file.status))
                        .font(k.ty.mono)
                        .color(if file.status == domain::DiffStatus::Conflicted {
                            crate::theme::roles(k).git.conflict
                        } else {
                            k.c.text_muted
                        })
                        .w(12.0),
                )
                .child(icon(file.icon.icon(!k.dark), 14.0))
                .child(text(name).font(k.ty.small).color(k.c.text_base))
                .child(
                    text(dir)
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(
                    text(format!("+{}", file.additions))
                        .font(k.ty.text_xsmall)
                        .color(crate::theme::roles(k).git.added),
                )
                .child(
                    text(format!("−{}", file.deletions))
                        .font(k.ty.text_xsmall)
                        .color(crate::theme::roles(k).git.deleted),
                ),
        );
    }
    if changes.truncated {
        list = list.child(
            text("Some files were over budget and are not listed.")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    list
}

/// The sidebar's Git panel, as the original client's `GitPanel`: the branch and its sync,
/// the changed files, and the commit box. A file's diff opens in the centre.
pub fn git_panel(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    let project = s.store.as_ref().and_then(|store| {
        let workspace = s.workbench.workspace?;
        store
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|found| found.project_id)
    });
    let fetching = project.is_some_and(|id| s.sync.fetching.contains(&id));
    let mut actions = row().gap(4.0).flex_wrap().child(
        Button::new(if fetching { "Fetching…" } else { "Fetch" })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.fetch())
            .build(k)
            .key("changes-fetch"),
    );
    if changes.loaded && changes.error.is_none() {
        actions = actions
            .child(
                Button::new("Preview PR")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.open_compare_current())
                    .build(k)
                    .key("changes-preview-pr"),
            )
            .child(
                Button::new("Open PR…")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.open_pr_dialog())
                    .build(k)
                    .key("changes-open-pr"),
            );
    }
    let mut head = col().key("git-head").w_full().gap(4.0).child(
        row()
            .gap(6.0)
            .items_center()
            .child(icon(&crate::glyphs::GIT, 14.0).color(k.c.icon_muted))
            .child(
                text(changes.branch.clone().unwrap_or_else(|| "Git".into()))
                    .font(k.ty.small_medium)
                    .color(k.c.text_base)
                    .truncate()
                    .min_w(0.0),
            ),
    );
    if let Some(note) = project.and_then(|id| s.sync.fetched.get(&id)) {
        head = head.child(
            text(note.clone())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    head = head.child(actions);
    if let Some(state) = &changes.rebase {
        head = head.child(
            row()
                .key("git-stopped")
                .role("button")
                .label("Open the stopped operation")
                .w_full()
                .px(8.0)
                .py(6.0)
                .radius(6.0)
                .fill(k.c.warning.bg)
                .hover(k.c.hover)
                .on_press(|s: &mut AppState| s.workbench.mode = CenterMode::Changes)
                .child(
                    text(format!(
                        "{} stopped · {} conflicts",
                        operation_name(state.operation),
                        state.conflicts.len()
                    ))
                    .font(k.ty.small)
                    .color(k.c.warning.fg),
                ),
        );
    }
    let mut panel = col()
        .key("git-panel")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(8.0)
        .child(head)
        .child(
            col()
                .key("changes-list-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(changed_rows(s, k)),
        );
    if changes.loaded && changes.error.is_none() {
        panel = panel.child(crate::commit::commit_box(s, k, changes.files.len()));
    }
    panel
}

/// The centre's diff of the file picked in Git, under a stopped operation's
/// panel when there is one.
pub fn changes(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    let selected = changes.selected.and_then(|index| changes.files.get(index));
    let title = selected.map_or_else(|| "Git".to_string(), |file| file.path.clone());
    let detail = selected.map_or_else(String::new, |file| {
        format!(
            "{} · +{} −{}",
            status_word(file.status),
            file.additions,
            file.deletions
        )
    });
    let patch = match selected {
        Some(_) if changes.state.files().is_empty() => notice(
            k,
            "changes-no-patch",
            "No patch to show: a binary file, or one over the budget.".into(),
        ),
        Some(_) => DiffView::new(
            |s: &AppState| &s.workbench.changes.state,
            |s: &mut AppState| &mut s.workbench.changes.state,
        )
        .build(s, k)
        .key("changes-diff")
        .grow(1.0)
        .min_h(0.0),
        None => notice(
            k,
            "changes-hint",
            "Pick a changed file in Git to see its diff.".into(),
        ),
    };
    let mut view = col().key("changes").w_full().h_full().min_h(0.0).child(
        bar(k, title, detail).child(
            SegmentedControl::new(
                &["Unified", "Split"],
                Some(if s.diff_split { 1 } else { 0 }),
            )
            .label("Patch layout")
            .width(Some(150.0))
            .on_pick(|s: &mut AppState, index, _| s.set_diff_split(index == 1))
            .build(k),
        ),
    );
    if let Some(state) = &changes.rebase {
        view = view.child(stopped(state, changes.rebase_error.as_deref(), k));
        if !state.conflicts.is_empty() {
            return view.child(conflict_view(s, k));
        }
    }
    view.child(patch)
}

/// The sidebar's Files panel: the checkout's tree. A Markdown file opens
/// rendered in the centre, anything else in an editor, as the original client routes them.
pub fn files_panel(s: &AppState, k: &Kit) -> El {
    if s.workbench.workspace.is_none() {
        return text("No checkout in this window.")
            .key("files-empty")
            .font(k.ty.small)
            .color(k.c.text_faint);
    }
    let light = !k.dark;
    let decorations = std::rc::Rc::clone(&s.workbench.changes.decorations);
    let tones = [Tone::Added, Tone::Modified, Tone::Deleted, Tone::Conflict]
        .map(|tone| tone_color(k, tone));
    let tint = move |tone: Tone| tones[tone as usize];
    let query = s.workbench.files.filter_query().to_string();
    let tree = Tree::new(
        |s: &AppState| &s.workbench.files.tree,
        |s: &mut AppState| &mut s.workbench.files.tree,
        FileNode::label,
    )
    .row(move |node: &FileNode<FileIcon>, r| {
        let mark = match (node.data, r.expanded) {
            (Some(file), _) => Some(file.icon(light)),
            (None, Some(true)) => Some(&crate::glyphs::FOLDER_OPEN),
            (None, Some(false)) => Some(&crate::glyphs::FILES),
            (None, None) => None,
        };
        let mut item = TreeItem::new(node.name.clone()).highlight(query.clone());
        if let Some(mark) = mark {
            item = item.icon(mark);
        }
        if node.dir {
            if let Some(count) = decorations.folder(&node.path) {
                item = item
                    .badge(count.to_string())
                    .label_color(tint(Tone::Modified));
            }
        } else if let Some(mark) = decorations.file(&node.path) {
            item = item.badge(mark.letter).label_color(tint(mark.tone));
        }
        if node.path.is_empty() {
            item
        } else {
            item.trailing(crate::menus::tree_row_catcher(&node.path, node.dir))
        }
    })
    .style(TreeStyle::from_kit(k))
    .name("Files")
    .on_select(|s: &mut AppState, id: &str, _tx| s.open_in_editor(id))
    .on_open(|s: &mut AppState, id: &str, _tx| s.open_in_editor(id))
    .build(s, k)
    .key(crate::menus::FILES_TREE)
    .on_secondary_press(crate::menus::files_secondary)
    .w_full()
    .grow(1.0)
    .min_h(0.0);
    let filter = TextInput::new(&s.workbench.files.filter)
        .placeholder("Filter files…")
        .label("Filter files")
        .leading_icon(k.glyph.search)
        .clear_button()
        .on_edit(|s: &mut AppState| &mut s.workbench.files.filter)
        .on_change(|s: &mut AppState, _| s.workbench.filter_changed())
        .build(k)
        .key("files-filter");
    col()
        .key("files-panel")
        .w_full()
        .h_full()
        .min_h(0.0)
        .gap(4.0)
        .child(filter)
        .children(s.menus.file_error.as_ref().map(|error| {
            text(error.clone())
                .key("files-error-notice")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg)
        }))
        .children(s.workbench.files.watch_notice.as_ref().map(|notice| {
            text(notice.clone())
                .key("files-watch-notice")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
        }))
        .child(tree)
}

/// A rendered file in the centre: Markdown, a CSV table or an image, with
/// its source a toggle away; text when it is none of those.
pub fn preview(s: &AppState, k: &Kit) -> El {
    let bench = &s.workbench;
    let body = match (&bench.files.preview, &bench.files.error) {
        (_, Some(error)) => notice(k, "files-error", format!("Could not read: {error}")),
        (Some(preview), None) => preview_view(preview, k),
        (None, None) => notice(k, "files-hint", "Reading…".into()),
    };
    let shown = bench.files.preview.as_ref();
    let title = shown.map_or_else(|| "Preview".to_string(), |preview| preview.path.clone());
    let with_source = shown.filter(|preview| preview.kind.is_some_and(PreviewKind::has_source));
    let detail = match with_source {
        Some(preview) if preview.source => "source · read only",
        _ => "rendered · read only",
    };
    let mut head = bar(k, title, detail.into());
    if let Some(preview) = with_source {
        head = head
            .child(
                Button::new(if preview.source { "Rendered" } else { "Source" })
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.workbench.toggle_source())
                    .build(k)
                    .key("preview-source-switch"),
            )
            .child(
                Button::new("Edit")
                    .variant(ButtonVariant::Outline)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.edit_previewed())
                    .build(k)
                    .key("preview-edit"),
            );
    }
    col()
        .key("files")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(head)
        .child(body)
}

fn image_view(image: Option<&Result<rune::Image, String>>, k: &Kit) -> El {
    let image = match image {
        None => return notice(k, "preview-image-reading", "Reading…".into()),
        Some(Err(error)) => {
            return notice(k, "preview-image-error", format!("Could not show: {error}"));
        }
        Some(Ok(image)) => image,
    };
    if let Some(error) = image.error() {
        return notice(
            k,
            "preview-image-error",
            format!("Could not decode: {}", error.0),
        );
    }
    let Some([width, height]) = image.size().filter(|_| image.is_ready()) else {
        return notice(k, "preview-image-decoding", "Decoding…".into());
    };
    col()
        .key("preview-image")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .padding(20.0)
        .items_center()
        .justify_center()
        .child(
            el().key("preview-image-pixels")
                .aspect_fit(width as f32 / height.max(1) as f32)
                .max_w(width as f32)
                .max_h(height as f32)
                .pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        )
        .child(
            text(format!("{width} × {height}"))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .pt(8.0),
        )
}

fn csv_view(k: &Kit, table: &super::csv::CsvTable) -> El {
    if table.rows.is_empty() {
        return notice(k, "preview-csv-empty", "An empty table.".into());
    }
    let width = table.width.max(1) as f32 * CSV_COLUMN_W;
    let mono = k.ty.small;
    let border = k.c.border_weak;
    let header_fill = k.c.surface_weak;
    let base = k.c.text_base;
    let mut view = col()
        .key("preview-csv")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll_xy()
        .child(
            col()
                .key("preview-csv-rows")
                .w(width)
                .growth(GrowthSpec::default())
                .virtual_rows(
                    table.rows.len(),
                    CSV_ROW_H,
                    |_s: &AppState, index| format!("r{index}").into(),
                    move |s: &AppState, index| {
                        let Some(row_cells) = s
                            .workbench
                            .files
                            .preview
                            .as_ref()
                            .and_then(|preview| preview.table.as_ref())
                            .and_then(|table| table.rows.get(index))
                        else {
                            return el();
                        };
                        let mut line = row()
                            .h(CSV_ROW_H)
                            .border_sides([0.0, 0.0, 1.0, 0.0], border)
                            .items_center();
                        if index == 0 {
                            line = line.fill(header_fill);
                        }
                        for cell in row_cells {
                            line = line.child(
                                text(cell.replace('\n', " "))
                                    .font(mono)
                                    .color(base)
                                    .w(CSV_COLUMN_W)
                                    .px(8.0)
                                    .truncate(),
                            );
                        }
                        line
                    },
                ),
        );
    if table.truncated {
        view = view.child(
            text("Only the first rows and columns are shown.")
                .key("preview-csv-cut")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .padding(8.0),
        );
    }
    view
}

fn preview_view(preview: &Preview, k: &Kit) -> El {
    match preview.kind {
        Some(PreviewKind::Image) => return image_view(preview.image.as_ref(), k),
        Some(PreviewKind::Svg) if !preview.source => {
            return image_view(preview.image.as_ref(), k);
        }
        Some(PreviewKind::Csv) if !preview.source && !preview.too_large && !preview.binary => {
            if let Some(table) = &preview.table {
                return csv_view(k, table);
            }
        }
        _ => {}
    }
    if preview.too_large {
        return notice(
            k,
            "preview-large",
            "This file is larger than the daemon reads for a preview.".into(),
        );
    }
    if preview.binary {
        return notice(
            k,
            "preview-binary",
            "Binary file; nothing to show as text.".into(),
        );
    }
    let scroller = col()
        .key("preview-scroll")
        .grow(1.0)
        .h_full()
        .min_w(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    if preview.is_markdown() && !preview.source {
        return scroller.child(
            col()
                .padding(20.0)
                .child(Markdown::new(preview.text.to_string()).build(k)),
        );
    }
    let count = preview.lines.len() + usize::from(preview.cut);
    let theme = SyntaxTheme::from_kit(k);
    let mono = k.ty.mono;
    let faint = k.c.text_faint;
    let base = k.c.text_base;
    scroller.child(
        col()
            .key("preview-lines")
            .w_full()
            .growth(GrowthSpec::default())
            .virtual_rows(
                count,
                CODE_ROW_H,
                |_s: &AppState, index| format!("l{index}").into(),
                move |s: &AppState, index| {
                    let Some(preview) = &s.workbench.files.preview else {
                        return el();
                    };
                    let Some(range) = preview.lines.get(index) else {
                        return text(format!("… cut after {} lines", preview.lines.len()))
                            .font(mono)
                            .color(faint)
                            .h(CODE_ROW_H);
                    };
                    let spans = theme.spans(&preview.line_highlights(range), 0);
                    row()
                        .h(CODE_ROW_H)
                        .items_center()
                        .child(
                            text(format!("{}", index + 1))
                                .font(mono)
                                .color(faint)
                                .w(56.0)
                                .pr(12.0)
                                .align(rune::Align::End),
                        )
                        .child(
                            rich(preview.text[range.clone()].to_string(), spans)
                                .font(mono)
                                .color(base),
                        )
                },
            ),
    )
}

/// A rebase, merge or cherry-pick that git stopped, and its unmerged paths.
/// The sequencer operation as a sentence names it, as the original client's `operationName`:
/// unknown is a rebase, the only one older daemons reported.
fn operation_name(operation: Option<domain::SequencerOp>) -> String {
    match operation {
        None | Some(domain::SequencerOp::Rebase) => "Rebase".into(),
        Some(domain::SequencerOp::Merge) => "Merge".into(),
        Some(domain::SequencerOp::CherryPick) => "Cherry-pick".into(),
        Some(domain::SequencerOp::Revert) => "Revert".into(),
        Some(other) => format!("{other:?}"),
    }
}

/// `step 3 of 7`, as `rebaseSteps` says it: a step past the end is the last
/// one, and an unknown or empty replay says nothing.
fn step_label(step: Option<u32>, total: Option<u32>) -> Option<String> {
    let (step, total) = (step?, total?);
    (total > 0).then(|| format!("step {} of {total}", step.clamp(1, total)))
}

fn stopped(state: &domain::RebaseState, error: Option<&str>, k: &Kit) -> El {
    use crate::workbench::RebaseStep;
    let mut title = format!("{} stopped", operation_name(state.operation));
    if let Some(step) = step_label(state.step, state.total) {
        title.push_str(&format!(", {step}"));
    }
    if let Some(onto) = &state.onto {
        title.push_str(&format!(" onto {onto}"));
    }
    let count = state.conflicts.len();
    title.push_str(&match count {
        0 => " · no conflicts left".to_string(),
        1 => " · 1 conflict".to_string(),
        n => format!(" · {n} conflicts"),
    });
    let button = |label: &'static str, key: &'static str, variant, step: RebaseStep| {
        Button::new(label)
            .variant(variant)
            .control_size(ControlSize::Small)
            .on_press(move |s: &mut AppState| s.rebase_step(step.clone()))
            .build(k)
            .key(key)
    };
    let mut panel = col()
        .key("rebase-panel")
        .role("region")
        .label(title.clone())
        .w_full()
        .gap(6.0)
        .px(12.0)
        .py(8.0)
        .fill(k.c.warning.bg)
        .child(
            row()
                .gap(8.0)
                .items_center()
                .child(
                    text(title)
                        .font(k.ty.small_medium)
                        .color(k.c.warning.fg)
                        .grow(1.0),
                )
                .child(button(
                    "Continue",
                    "rebase-continue",
                    ButtonVariant::Outline,
                    RebaseStep::Continue,
                ))
                .child(button(
                    "Abort",
                    "rebase-abort",
                    ButtonVariant::Danger,
                    RebaseStep::Abort,
                ))
                .child(
                    Button::new("Resolve with AI")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(|s: &mut AppState| s.resolve_with_agent())
                        .build(k)
                        .key("rebase-resolve-ai"),
                ),
        );
    if state.truncated {
        panel = panel.child(
            text("More unmerged paths than are listed.")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    if let Some(error) = error {
        panel = panel.child(
            text(format!("git refused: {error}"))
                .key("rebase-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg),
        );
    }
    panel
}

/// One conflicted path at a time, read three ways.
///
/// The regions come from the working-tree patch, so what shows is what is on
/// disk now. Once no markers are left, whoever resolved the file — the person
/// or an agent — has only proposed a resolution: staging is what accepts it.
fn conflict_view(s: &AppState, k: &Kit) -> El {
    use crate::workbench::RebaseStep;
    use domain::conflict_prompt::{conflict_label, conflict_sides};
    let changes = &s.workbench.changes;
    let Some(state) = &changes.rebase else {
        return el();
    };
    let Some(focus) = changes.conflict_focus.as_deref() else {
        return el();
    };
    let index = state
        .conflicts
        .iter()
        .position(|conflict| conflict.path == focus)
        .unwrap_or(0);
    let conflict = &state.conflicts[index];
    let (ours, theirs) = conflict_sides(state);
    let path = conflict.path.clone();
    let blocks = changes.conflict_blocks.len();
    let open_path = path.clone();
    let open = Button::new("Open in editor")
        .variant(ButtonVariant::GhostMuted)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| s.open_file(&open_path, None))
        .build(k)
        .key("conflict-open");
    let mut hand = row()
        .key("conflict-hand")
        .w_full()
        .gap(8.0)
        .items_center()
        .child(
            text(path.clone())
                .font(k.ty.small_medium)
                .color(k.c.text_base)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        );
    if blocks > 0 {
        hand = hand.child(
            row()
                .key("conflict-state")
                .gap(5.0)
                .items_center()
                .px(6.0)
                .h(18.0)
                .radius(4.0)
                .fill(k.c.warning.bg)
                .child(el().size(6.0, 6.0).radius(3.0).fill(k.c.warning.fg))
                .child(
                    text(format!(
                        "{blocks} conflict{}",
                        if blocks == 1 { "" } else { "s" }
                    ))
                    .font(k.ty.text_xsmall)
                    .color(k.c.warning.fg),
                ),
        );
    } else if changes.conflict_readable {
        hand = hand.child(
            text("no markers left".to_string())
                .key("conflict-no-markers")
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        );
    } else {
        hand = hand.child(
            text(conflict_label(&conflict.code).to_string())
                .key("conflict-label")
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted),
        );
    }
    hand = hand.child(open);
    if state.conflicts.len() > 1 {
        hand = hand
            .child(
                Button::new("Previous file")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.workbench.changes.step_conflict(-1))
                    .build(k)
                    .key("conflict-prev"),
            )
            .child(
                Button::new("Next file")
                    .variant(ButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .on_press(|s: &mut AppState| s.workbench.changes.step_conflict(1))
                    .build(k)
                    .key("conflict-next"),
            );
    }
    if blocks > 1 {
        hand = hand.child(
            Button::new("Next conflict")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press_tx(|s: &mut AppState, tx| {
                    if let Some(index) = s.workbench.changes.next_conflict_region() {
                        tx.scroll_into_view(format!("conflict-region-{index}"));
                    }
                })
                .build(k)
                .key("conflict-region-next"),
        );
    }
    let sides = if blocks > 0 {
        row()
            .key("conflict-sides")
            .w_full()
            .gap(8.0)
            .child(side_head(k, "ours", &format!("Ours {ours}")))
            .child(side_head(k, "base", "Base"))
            .child(side_head(k, "theirs", &format!("Theirs {theirs}")))
    } else {
        row()
    };
    let mut regions = col().key("conflict-regions").w_full().gap(10.0);
    for (at, block) in changes.conflict_blocks.iter().enumerate() {
        let mut head = row()
            .gap(8.0)
            .items_center()
            .child(el().size(6.0, 6.0).radius(3.0).fill(k.c.warning.fg))
            .child(
                text(format!("Conflict {} of {blocks}", at + 1))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
            );
        if let Some(line) = block.line {
            head = head.child(
                text(format!("line {line}"))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
        }
        let open_path = path.clone();
        let line = block.line;
        head = head.child(el().grow(1.0)).child(
            Button::new("Resolve in editor")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(move |s: &mut AppState| {
                    s.open_file(&open_path, line);
                })
                .build(k)
                .key(format!("conflict-resolve-{at}")),
        );
        let mut sides = row().w_full().gap(8.0);
        sides = sides
            .child(side_column(k, at, "ours", &block.ours))
            .child(if let Some(base) = &block.base {
                side_column(k, at, "base", base)
            } else {
                col()
                    .w(0.0)
                    .grow(1.0)
                    .gap(3.0)
                    .padding(6.0)
                    .fill(k.c.surface_weak)
                    .child(
                        text(
                            "No base recorded. Git wrote this conflict without diff3 markers."
                                .to_string(),
                        )
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint),
                    )
            })
            .child(side_column(k, at, "theirs", &block.theirs));
        let mut article = col()
            .key(format!("conflict-region-{at}"))
            .role("group")
            .label(format!("Conflict {} of {blocks}", at + 1))
            .w_full()
            .gap(6.0)
            .child(head)
            .child(sides);
        if !block.complete {
            article = article.child(
                text(
                    "Part of this region is outside the patch's context. Open it in the editor to \
                     read all of it.",
                )
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
            );
        }
        regions = regions.child(article);
    }
    if blocks == 0 && changes.conflict_readable {
        regions = regions.child(
            col()
                .key("conflict-proposal")
                .w_full()
                .gap(6.0)
                .padding(8.0)
                .radius(6.0)
                .fill(k.c.surface_weak)
                .child(
                    text("Proposed resolution · not staged · you stage it".to_string())
                        .font(k.ty.small_medium)
                        .color(k.c.text_base),
                )
                .child(
                    text(
                        "No conflict markers are left in this file. Read the change below: staging \
                         it is what accepts it, whether you or an agent wrote it.",
                    )
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
                )
                .child(conflict_patch(s, k)),
        );
    }
    if !changes.conflict_readable {
        regions = regions.child(
            text(
                "There is no patch to read for this path: it is binary, over budget, or gone \
                 from one side. Resolve it in the editor or a terminal, then stage it.",
            )
            .key("conflict-unreadable")
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint),
        );
    }
    let stage = Button::new("Stage file")
        .variant(ButtonVariant::Contrast)
        .control_size(ControlSize::Small)
        .on_press(move |s: &mut AppState| {
            s.rebase_step(RebaseStep::Resolve(vec![path.clone()]));
        })
        .build(k)
        .key("conflict-stage");
    let footer = row()
        .key("conflict-foot")
        .w_full()
        .gap(8.0)
        .items_center()
        .child(
            text(format!(
                "Conflict file {} of {} left in the replay",
                index + 1,
                state.conflicts.len()
            ))
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint)
            .grow(1.0),
        )
        .child(stage);
    col()
        .key("conflict-view")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .gap(8.0)
        .padding(10.0)
        .fill(k.c.surface_weak)
        .child(hand)
        .child(sides)
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(regions),
        )
        .child(footer)
}

fn side_head(k: &Kit, side: &'static str, label: &str) -> El {
    row()
        .key(format!("conflict-side-head-{side}"))
        .w(0.0)
        .grow(1.0)
        .h(20.0)
        .px(6.0)
        .gap(5.0)
        .items_center()
        .radius(4.0)
        .fill(k.c.surface_hover)
        .child(el().size(6.0, 6.0).radius(3.0).fill(match side {
            "ours" => k.c.text_muted,
            "base" => k.c.text_faint,
            _ => k.c.icon_accent,
        }))
        .child(
            text(label.to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        )
}

fn side_column(k: &Kit, region: usize, side: &'static str, lines: &[String]) -> El {
    let mut column = col()
        .key(format!("conflict-side-{region}-{side}"))
        .w(0.0)
        .grow(1.0)
        .min_w(0.0)
        .gap(1.0)
        .padding(6.0)
        .radius(4.0)
        .fill(k.c.bg_base);
    if lines.is_empty() {
        return column.child(
            text("Empty on this side.".to_string())
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
        );
    }
    for (at, line) in lines.iter().enumerate() {
        column = column.child(
            row()
                .key(format!("conflict-line-{region}-{side}-{at}"))
                .w_full()
                .child(
                    text(if line.is_empty() {
                        " ".to_string()
                    } else {
                        line.clone()
                    })
                    .font(k.ty.mono)
                    .color(k.c.text_base)
                    .truncate()
                    .min_w(0.0),
                ),
        );
    }
    column
}

/// The proposed resolution as plain patch rows: no syntax colours, because
/// the point here is which lines are gone and which arrived.
fn conflict_patch(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    if changes.conflict_rows.is_empty() {
        return text("Nothing to show.".to_string())
            .font(k.ty.text_xsmall)
            .color(k.c.text_faint);
    }
    let mut rows = col().key("conflict-patch").w_full().gap(0.5);
    for (at, patch_row) in changes.conflict_rows.iter().enumerate() {
        let ink = match patch_row.kind {
            crate::workbench::patch::Kind::Added => crate::theme::roles(k).git.added,
            crate::workbench::patch::Kind::Removed => crate::theme::roles(k).git.deleted,
            crate::workbench::patch::Kind::Hunk => k.c.text_faint,
            crate::workbench::patch::Kind::Meta => k.c.text_faint,
            _ => k.c.text_base,
        };
        rows = rows.child(
            row()
                .key(format!("conflict-patch-{at}"))
                .w_full()
                .h(18.0)
                .child(
                    text(
                        patch_row
                            .before
                            .map_or_else(String::new, |line| line.to_string()),
                    )
                    .font(k.ty.mono)
                    .color(k.c.text_faint)
                    .w(40.0),
                )
                .child(
                    text(
                        patch_row
                            .after
                            .map_or_else(String::new, |line| line.to_string()),
                    )
                    .font(k.ty.mono)
                    .color(k.c.text_faint)
                    .w(40.0),
                )
                .child(
                    text(patch_row.text.clone())
                        .font(k.ty.mono)
                        .color(ink)
                        .truncate()
                        .min_w(0.0),
                ),
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use domain::FileContents;
    use rune::{Env, Runtime};

    use crate::app::AppState;
    use crate::connection::{Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    fn file(
        path: &str,
        text: &str,
        language: &str,
        workspace: domain::WorkspaceId,
    ) -> FileContents {
        FileContents {
            workspace_id: workspace,
            path: path.into(),
            text: text.into(),
            revision: String::new(),
            language: language.into(),
            binary: false,
            too_large: false,
        }
    }

    /// Code and markdown previews both mount; a virtual list without its
    /// growth spec panics at the first commit, so this is the guard.
    #[test]
    fn a_code_and_a_markdown_preview_render() {
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let state = AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.update(|s, _| {
            s.show_panel(crate::workbench::Panel::Files);
            s.workbench.preview("a.rs");
            let workspace = s.workbench.workspace.expect("a workspace");
            let scope = s.workbench.scope().expect("a scope");
            let code: String = (0..200).map(|i| format!("let x{i} = {i};\n")).collect();
            s.workbench
                .apply_file(scope, Ok(file("a.rs", &code, "rust", workspace)));
        });
        rt.frame(0.1);
        rt.frame(0.2);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|key| key == "preview-lines"));
        assert!(
            keys.iter().any(|key| key == "l0"),
            "the first line is mounted"
        );
        rt.update(|s, _| {
            s.workbench.preview("README.md");
            let workspace = s.workbench.workspace.expect("a workspace");
            let scope = s.workbench.scope().expect("a scope");
            s.workbench.apply_file(
                scope,
                Ok(file(
                    "README.md",
                    "# Title\n\nBody.\n",
                    "markdown",
                    workspace,
                )),
            );
        });
        rt.frame(0.3);
        let keys = |rt: &Runtime<AppState>| -> Vec<String> {
            rt.elements().into_iter().filter_map(|e| e.key).collect()
        };
        assert!(!keys(&rt).iter().any(|key| key == "preview-lines"));
        assert!(keys(&rt).iter().any(|key| key == "preview-source-switch"));
        rt.update(|s, _| s.workbench.toggle_source());
        rt.frame(0.4);
        assert!(
            keys(&rt).iter().any(|key| key == "preview-lines"),
            "the source is its lines"
        );
    }

    #[test]
    fn a_csv_table_and_an_image_render() {
        use base64::Engine as _;
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        state.images = Some(rune::ImageCache::new(Arc::new(|| {})));
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.update(|s, _| {
            s.show_panel(crate::workbench::Panel::Files);
            s.workbench.preview("data.csv");
            let workspace = s.workbench.workspace.expect("a workspace");
            let scope = s.workbench.scope().expect("a scope");
            s.workbench.apply_file(
                scope,
                Ok(file("data.csv", "name,size\na,1\nb,2\n", "", workspace)),
            );
        });
        rt.frame(0.1);
        rt.frame(0.2);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|key| key == "preview-csv"));
        assert!(keys.iter().any(|key| key == "r2"), "every row is mounted");
        let png = base64::engine::general_purpose::STANDARD
            .decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=")
            .expect("a png");
        rt.update(move |s, _| {
            s.workbench.preview("logo.png");
            let scope = s.workbench.scope().expect("a scope");
            s.apply_image(scope, "logo.png", Ok(("image/png".into(), Arc::from(png))));
            if let Some(Some(Ok(image))) = s
                .workbench
                .files
                .preview
                .as_ref()
                .map(|preview| preview.image.as_ref())
            {
                image.wait().expect("decodes");
            }
        });
        rt.frame(0.3);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(
            keys.iter().any(|key| key == "preview-image-pixels"),
            "{keys:?}"
        );
        assert!(
            !keys.iter().any(|key| key == "preview-source-switch"),
            "a raster has no source"
        );
    }

    #[test]
    fn git_words_follow_git_view() {
        use super::{operation_name, status_letter, status_word, step_label};
        use domain::{DiffStatus, SequencerOp};
        assert_eq!(
            [
                DiffStatus::Added,
                DiffStatus::Modified,
                DiffStatus::Deleted,
                DiffStatus::Conflicted
            ]
            .map(status_letter),
            ["A", "M", "D", "U"]
        );
        assert_eq!(status_word(DiffStatus::Conflicted), "unresolved conflict");
        assert_eq!(status_word(DiffStatus::Modified), "modified");
        assert_eq!(step_label(Some(3), Some(7)).as_deref(), Some("step 3 of 7"));
        assert_eq!(
            step_label(Some(50), Some(100)).as_deref(),
            Some("step 50 of 100")
        );
        assert_eq!(
            step_label(Some(9), Some(4)).as_deref(),
            Some("step 4 of 4"),
            "clamped"
        );
        assert_eq!(step_label(None, None), None);
        assert_eq!(step_label(Some(1), Some(0)), None);
        assert_eq!(operation_name(Some(SequencerOp::CherryPick)), "Cherry-pick");
        assert_eq!(operation_name(Some(SequencerOp::Merge)), "Merge");
        assert_eq!(operation_name(None), "Rebase");
    }

    fn conflicted(workspace: domain::WorkspaceId, patch: &str) -> domain::WorkspaceDiff {
        domain::WorkspaceDiff {
            workspace_id: workspace,
            branch: Some("feature/x".into()),
            files: vec![domain::DiffFile {
                path: "src/a.rs".into(),
                status: domain::DiffStatus::Conflicted,
                additions: 3,
                deletions: 0,
                patch: patch.into(),
                binary: false,
                truncated: false,
            }],
            truncated: false,
        }
    }

    fn stopped(workspace: domain::WorkspaceId) -> domain::RebaseState {
        domain::RebaseState {
            workspace_id: workspace,
            operation: Some(domain::SequencerOp::Merge),
            branch: Some("feature/x".into()),
            conflicts: vec![domain::ConflictFile {
                path: "src/a.rs".into(),
                code: "UU".into(),
            }],
            ..domain::RebaseState::default()
        }
    }

    #[test]
    fn a_stopped_operation_reads_its_markers_and_a_resolved_file_is_staged() {
        let store =
            fixture_store(&Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"))
                .expect("fixture snapshot decodes");
        let mut state =
            AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new());
        state.show_mode(crate::workbench::CenterMode::Changes);
        let workspace = state.workbench.workspace.expect("a workspace");
        let markers = concat!(
            "@@ -1,7 +1,7 @@\n",
            " <<<<<<< HEAD\n",
            " ours\n",
            " ||||||| 4f2b1ac\n",
            " base\n",
            " =======\n",
            " theirs\n",
            " >>>>>>> feature\n",
        );
        let mut rt = Runtime::new(
            state,
            crate::ui::view,
            (1180.0, 760.0),
            Env {
                system_dark: true,
                scale: 1.0,
            },
        );
        rt.update(|s, _| {
            let scope = s.workbench.scope().expect("a scope");
            s.workbench.apply_rebase(scope, Ok(stopped(workspace)));
            s.workbench
                .apply_diff(scope, Ok(conflicted(workspace, markers)));
            s.workbench.show(crate::workbench::CenterMode::Changes);
        });
        rt.frame(0.1);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(keys.iter().any(|key| key == "conflict-view"), "{keys:?}");
        assert!(keys.iter().any(|key| key == "conflict-region-0"));
        assert!(keys.iter().any(|key| key == "conflict-stage"));
        assert!(!keys.iter().any(|key| key == "conflict-no-markers"));
        let resolved = "@@ -1,2 +1,2 @@\n context\n-old\n+new\n";
        rt.update(|s, _| {
            let scope = s.workbench.scope().expect("a scope");
            s.workbench
                .apply_diff(scope, Ok(conflicted(workspace, resolved)));
        });
        rt.frame(0.2);
        let keys: Vec<String> = rt.elements().into_iter().filter_map(|e| e.key).collect();
        assert!(
            keys.iter().any(|key| key == "conflict-no-markers"),
            "a resolved file says so before it is staged: {keys:?}"
        );
        assert!(keys.iter().any(|key| key == "conflict-proposal"));
        assert!(keys.iter().any(|key| key == "conflict-stage"));
    }
}
