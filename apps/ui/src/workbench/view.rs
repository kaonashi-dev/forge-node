//! The Changes and Files views, and the switch between them and the session.

use rune::{El, GrowthSpec, ScrollSpec, col, el, rich, row, text};
use rune_kit::diff::DiffView;
use rune_kit::syntax::SyntaxTheme;
use rune_kit::{
    Button, ButtonVariant, ControlSize, FileNode, Kit, Markdown, SegmentedControl, Tree, TreeItem,
    TreeStyle,
};

use super::{CenterMode, Preview};
use crate::app::AppState;

const MODES: [(&str, CenterMode); 5] = [
    ("Session", CenterMode::Session),
    ("Projects", CenterMode::Projects),
    ("Changes", CenterMode::Changes),
    ("Files", CenterMode::Files),
    ("Settings", CenterMode::Settings),
];
const TREE_W: f32 = 280.0;
const CODE_ROW_H: f32 = 18.0;

pub fn mode_switch(s: &AppState, k: &Kit) -> El {
    let labels: Vec<&str> = MODES.iter().map(|(label, _)| *label).collect();
    let selected = MODES.iter().position(|(_, mode)| *mode == s.workbench.mode);
    SegmentedControl::new(&labels, selected)
        .label("Centre view")
        .fit()
        .on_pick(|s: &mut AppState, index: usize, _tx| {
            if let Some((_, mode)) = MODES.get(index) {
                s.show_mode(*mode);
            }
        })
        .build(k)
        .key("centre-mode")
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

pub fn changes(s: &AppState, k: &Kit) -> El {
    let changes = &s.workbench.changes;
    let mut detail = match &changes.branch {
        Some(branch) => format!("{branch} · {} files", changes.file_count),
        None => format!("{} files", changes.file_count),
    };
    if changes.truncated {
        detail.push_str(" · list cut at the daemon's budget");
    }
    if !changes.without_patch.is_empty() {
        detail.push_str(&format!(
            " · no patch for {}",
            changes.without_patch.join(", ")
        ));
    }
    let body = if s.workbench.workspace.is_none() {
        notice(k, "changes-empty", "No workspace for this session.".into())
    } else if let Some(error) = &changes.error {
        notice(
            k,
            "changes-error",
            format!("Could not read the diff: {error}"),
        )
    } else if !changes.loaded {
        notice(k, "changes-loading", "Reading the working tree…".into())
    } else if changes.file_count == 0 {
        notice(k, "changes-clean", "No uncommitted changes.".into())
    } else {
        DiffView::new(
            |s: &AppState| &s.workbench.changes.state,
            |s: &mut AppState| &mut s.workbench.changes.state,
        )
        .build(s, k)
        .key("changes-diff")
        .grow(1.0)
        .min_h(0.0)
    };
    let project = s.store.as_ref().and_then(|store| {
        let workspace = s.workbench.workspace?;
        store
            .workspaces
            .iter()
            .find(|candidate| candidate.id == workspace)
            .map(|found| found.project_id)
    });
    if let Some(note) = project.and_then(|id| s.sync.fetched.get(&id)) {
        detail.push_str(" · ");
        detail.push_str(note);
    }
    let fetching = project.is_some_and(|id| s.sync.fetching.contains(&id));
    let mut header = bar(k, "Changes".into(), detail).child(
        Button::new(if fetching { "Fetching…" } else { "Fetch" })
            .variant(ButtonVariant::GhostMuted)
            .control_size(ControlSize::Small)
            .on_press(|s: &mut AppState| s.fetch())
            .build(k)
            .key("changes-fetch"),
    );
    // A pull request carries commits, so it is offered on a clean tree only;
    // with changes, the next step is the commit.
    if changes.loaded && changes.file_count == 0 && changes.error.is_none() {
        header = header.child(
            Button::new("Open PR…")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.open_pr_dialog())
                .build(k)
                .key("changes-open-pr"),
        );
    }
    if changes.loaded && changes.file_count > 0 {
        header = header.child(
            Button::new("Commit…")
                .variant(ButtonVariant::Outline)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.start_commit())
                .build(k)
                .key("changes-commit"),
        );
    }
    let mut view = col()
        .key("changes")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(header);
    if let Some(state) = &changes.rebase {
        view = view.child(stopped(state, changes.rebase_error.as_deref(), k));
    }
    view.child(body)
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
    .on_select(|s: &mut AppState, id: &str, _tx| s.workbench.select_file(id))
    .on_open(|s: &mut AppState, id: &str, _tx| s.open_in_editor(id))
    .build(s, k)
    .key("files-tree-view")
    .w(TREE_W)
    .h_full();
    let preview = match (&bench.files.preview, &bench.files.error) {
        (_, Some(error)) => notice(k, "files-error", format!("Could not read: {error}")),
        (Some(preview), None) => preview_view(preview, k),
        (None, None) => notice(k, "files-hint", "Select a file to read it.".into()),
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
fn stopped(state: &domain::RebaseState, error: Option<&str>, k: &Kit) -> El {
    use crate::workbench::RebaseStep;
    let what = match state.operation {
        Some(domain::SequencerOp::Rebase) => "Rebase",
        Some(domain::SequencerOp::Merge) => "Merge",
        Some(domain::SequencerOp::CherryPick) => "Cherry-pick",
        Some(domain::SequencerOp::Revert) => "Revert",
        _ => "Operation",
    };
    let mut title = format!("{what} stopped");
    if let (Some(step), Some(total)) = (state.step, state.total) {
        title.push_str(&format!(" at {step} of {total}"));
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
                )),
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
    let chosen = s.theme_base.as_deref();
    let option = |label: String, base: Option<&'static str>, selected: bool| {
        let id = base.map(str::to_string);
        Button::new(label)
            .variant(if selected {
                ButtonVariant::Outline
            } else {
                ButtonVariant::Ghost
            })
            .control_size(ControlSize::Regular)
            .on_press(move |s: &mut AppState| s.choose_theme(id.clone()))
            .build(k)
            .key(format!("theme-{}", base.unwrap_or("system")))
    };
    let mut list = col()
        .key("theme-options")
        .gap(4.0)
        .role("radiogroup")
        .label("Theme")
        .child(option("Follow the system".into(), None, chosen.is_none()));
    for base in crate::theme::bases() {
        let Some(theme) = crate::theme::theme(base) else {
            continue;
        };
        let label = if theme.light {
            format!("{} (light)", theme.label)
        } else {
            theme.label.clone()
        };
        list = list.child(option(
            label,
            Some(base.as_str()),
            chosen == Some(base.as_str()),
        ));
    }
    col()
        .key("settings")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(bar(k, "Settings".into(), "saved for this Mac".into()))
        .child(
            col()
                .key("settings-body")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(20.0)
                .gap(12.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .child(text("Theme").font(k.ty.body_medium).color(k.c.text_base))
                .child(list),
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
}
