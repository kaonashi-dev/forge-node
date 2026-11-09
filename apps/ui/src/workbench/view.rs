//! The Changes and Files views, and the switch between them and the session.

use rune::{El, GrowthSpec, ScrollSpec, col, el, rich, row, text};
use rune_kit::diff::DiffView;
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{
    Button, ButtonVariant, ControlSize, FileNode, Kit, Markdown, Tree, TreeItem, TreeStyle,
};

use super::Preview;
use crate::app::AppState;

const TREE_W: f32 = 280.0;
const CODE_ROW_H: f32 = 18.0;

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

/// A diff status as one letter, as Tauri's `statusLetter`.
fn status_letter(status: domain::DiffStatus) -> &'static str {
    match status {
        domain::DiffStatus::Added => "A",
        domain::DiffStatus::Modified => "M",
        domain::DiffStatus::Deleted => "D",
        domain::DiffStatus::Conflicted => "U",
        _ => "?",
    }
}

/// A diff status in words for the row's accessible name, as Tauri's `statusWord`.
fn status_word(status: domain::DiffStatus) -> &'static str {
    match status {
        domain::DiffStatus::Added => "added",
        domain::DiffStatus::Modified => "modified",
        domain::DiffStatus::Deleted => "deleted",
        domain::DiffStatus::Conflicted => "unresolved conflict",
        _ => "changed",
    }
}

pub fn changes(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    let mut detail = changes.branch.clone().unwrap_or_default();
    let project = s.store.as_ref().and_then(|store| {
        let workspace = s.workbench.workspace?;
        store
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|found| found.project_id)
    });
    if let Some(note) = project.and_then(|id| s.sync.fetched.get(&id)) {
        if !detail.is_empty() {
            detail.push_str(" · ");
        }
        detail.push_str(note);
    }
    let fetching = project.is_some_and(|id| s.sync.fetching.contains(&id));
    let mut header = bar(k, "Git".into(), detail).child(
        Button::new(if fetching { "Fetching…" } else { "Fetch" })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.fetch())
            .build(k)
            .key("changes-fetch"),
    );
    if changes.loaded && changes.error.is_none() {
        header = header.child(
            Button::new("Preview PR")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.open_pr_dialog())
                .build(k)
                .key("changes-open-pr"),
        );
    }

    let mut list = col().key("changed-files").w_full().gap(1.0).padding(6.0);
    if s.workbench.workspace.is_none() {
        list = list.child(
            text("No workspace for this session.")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    } else if let Some(error) = &changes.error {
        list = list.child(
            text(format!("Could not read the diff: {error}"))
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    } else if !changes.loaded {
        list = list.child(
            text("Reading the working tree…")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    } else if changes.files.is_empty() {
        list = list.child(
            text("Nothing uncommitted.")
                .key("changes-clean")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    }
    for (index, file) in changes.files.iter().enumerate() {
        let selected = changes.selected == Some(index);
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
                .gap(8.0)
                .radius(5.0)
                .items_center()
                .fill(if selected {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .on_press(move |s: &mut AppState| s.workbench.select_change(index))
                .child(
                    text(status_letter(file.status))
                        .font(k.ty.mono)
                        .color(k.c.text_muted)
                        .w(12.0),
                )
                .child(
                    text(file.path.clone())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .grow(1.0)
                        .truncate(),
                )
                .child(
                    text(format!("+{}", file.additions))
                        .font(k.ty.text_xsmall)
                        .color(k.c.success.fg),
                )
                .child(
                    text(format!("−{}", file.deletions))
                        .font(k.ty.text_xsmall)
                        .color(k.c.danger.fg),
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
    let mut rail = col()
        .key("changes-rail")
        .w(TREE_W + 40.0)
        .h_full()
        .min_h(0.0)
        .child(
            col()
                .key("changes-list-scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(list),
        );
    if changes.loaded && changes.error.is_none() {
        rail = rail.child(crate::commit::commit_box(s, k, changes.files.len()));
    }
    let patch = match changes.selected.and_then(|index| changes.files.get(index)) {
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
        None => el().grow(1.0),
    };
    let mut view = col()
        .key("changes")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header);
    if let Some(state) = &changes.rebase {
        view = view.child(stopped(state, changes.rebase_error.as_deref(), k));
    }
    view.child(
        row()
            .key("changes-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .child(rail)
            .child(el().w(1.0).h_full().fill(k.c.border_weak))
            .child(patch),
    )
}

pub fn files(s: &AppState, k: &Kit) -> El {
    let bench = &s.workbench;
    let tree = Tree::new(
        |s: &AppState| &s.workbench.files.tree,
        |s: &mut AppState| &mut s.workbench.files.tree,
        FileNode::label,
    )
    .row(|node: &FileNode<()>, r| TreeItem::file(node, r))
    .style(TreeStyle::from_kit(k).chevron_only_folders(true))
    .name("Files")
    // One click opens the file in an editor, as in Tauri's tree.
    .on_select(|s: &mut AppState, id: &str, _tx| s.open_in_editor(id))
    .on_open(|s: &mut AppState, id: &str, _tx| s.open_in_editor(id))
    .build(s, k)
    .key("files-tree-view")
    .w(TREE_W)
    .h_full();
    let preview = match (&bench.files.preview, &bench.files.error) {
        (_, Some(error)) => notice(k, "files-error", format!("Could not read: {error}")),
        (Some(preview), None) => preview_view(preview, k),
        (None, None) => notice(
            k,
            "files-hint",
            "Click a file to open it in an editor.".into(),
        ),
    };
    let title = bench
        .files
        .preview
        .as_ref()
        .map_or_else(|| "Files".to_string(), |preview| preview.path.clone());
    col()
        .key("files")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(bar(k, title, "read only".into()))
        .child(
            row()
                .key("files-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .child(tree)
                .child(el().w(1.0).h_full().fill(k.c.border_weak))
                .child(preview),
        )
}

fn preview_view(preview: &Preview, k: &Kit) -> El {
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
    if preview.is_markdown() {
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
/// The sequencer operation as a sentence names it, as Tauri's `operationName`:
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
    for conflict in &state.conflicts {
        let path = conflict.path.clone();
        panel = panel.child(
            row()
                .key(format!("conflict-{}", conflict.path))
                .gap(8.0)
                .items_center()
                .child(
                    text(conflict.code.clone())
                        .font(k.ty.mono)
                        .color(k.c.warning.fg)
                        .w(28.0),
                )
                .child(
                    text(conflict.path.clone())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .grow(1.0),
                )
                .child(
                    text(conflict.label())
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_faint),
                )
                .child(
                    Button::new("Mark resolved")
                        .variant(ButtonVariant::GhostMuted)
                        .control_size(ControlSize::Small)
                        .on_press(move |s: &mut AppState| {
                            s.rebase_step(RebaseStep::Resolve(vec![path.clone()]));
                        })
                        .build(k)
                        .key(format!("resolve-{}", conflict.path)),
                ),
        );
    }
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

pub fn settings(s: &AppState, k: &Kit) -> El {
    let rule = || el().h(1.0).w_full().fill(k.c.border_weak);
    col().key("settings").w_full().h_full().min_h(0.0).child(
        col()
            .key("settings-body")
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .padding(20.0)
            .gap(16.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .child(crate::appearance::section(s, k))
            .child(rule())
            .child(crate::storage::section(s, k))
            .child(rule())
            .child(crate::usage::section(s, k)),
    )
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
    use crate::workbench::CenterMode;

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
            s.show_mode(CenterMode::Files);
            let workspace = s.workbench.workspace.expect("a workspace");
            let code: String = (0..200).map(|i| format!("let x{i} = {i};\n")).collect();
            s.workbench
                .apply_file(workspace, Ok(file("a.rs", &code, "rust", workspace)));
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
            let workspace = s.workbench.workspace.expect("a workspace");
            s.workbench.apply_file(
                workspace,
                Ok(file(
                    "README.md",
                    "# Title\n\nBody.\n",
                    "markdown",
                    workspace,
                )),
            );
        });
        rt.frame(0.3);
        assert!(
            !rt.elements()
                .into_iter()
                .any(|e| e.key.as_deref() == Some("preview-lines"))
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
}
