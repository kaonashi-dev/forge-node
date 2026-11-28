//! The branch compare behind "Preview PR": the working tree, uncommitted
//! changes included, against the branch a pull request would target.
//!
//! Every read is a daemon request on the worker (`GetBranchCompare`,
//! `ListBranches`); this side parses the answer into one `DiffState` once,
//! and never runs git again for a layout change.

use domain::{BranchCompare, DiffStatus};
use rune::{El, ScrollSpec, Tx, col, el, icon, row, text};
use rune_kit::diff::{DiffMode, DiffState, DiffView};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Select};

use super::Scope;
use crate::app::AppState;
use crate::file_icons::FileIcon;

/// Bounds the base picker; a repository with hundreds of stale remote
/// branches is not a menu.
pub const MAX_BASE_OPTIONS: usize = 40;

/// Refs a pull request from `head` could target, spelled as git resolves them.
///
/// The default branch comes first because it is the usual answer, then remote
/// refs before local ones: a pull request is opened against what the forge
/// has, not against a local branch that may be behind it.
#[must_use]
pub fn base_options(
    branches: Option<&client::Branches>,
    head: Option<&str>,
    current: Option<&str>,
) -> Vec<String> {
    let mut refs: Vec<String> = Vec::new();
    let mut add = |reference: Option<&str>| {
        if let Some(reference) = reference.filter(|reference| !reference.is_empty())
            && Some(reference) != head
            && !refs.iter().any(|known| known == reference)
        {
            refs.push(reference.to_string());
        }
    };
    add(current);
    if let Some(default) = branches.and_then(|branches| branches.default_branch.as_deref()) {
        add(Some(&format!("origin/{default}")));
    }
    if let Some(branches) = branches {
        for branch in &branches.branches {
            if let domain::RefScope::Remote { remote } = &branch.scope {
                add(Some(&format!("{remote}/{}", branch.name)));
            }
        }
        for branch in &branches.branches {
            if branch.scope == domain::RefScope::Local {
                add(Some(&branch.name));
            }
        }
    }
    refs.truncate(MAX_BASE_OPTIONS);
    refs
}

pub struct CompareFile {
    pub path: String,
    pub status: DiffStatus,
    pub additions: u32,
    pub deletions: u32,
    pub icon: FileIcon,
    /// Its first entry in `Compare::state`, when its patch parsed to any.
    first_diff: Option<usize>,
}

#[derive(Default)]
pub struct Compare {
    pub data: Option<BranchCompare>,
    pub error: Option<String>,
    /// The base the last request asked for; `None` is the daemon's default.
    pub requested: Option<String>,
    /// A read waiting for the next `take_reads`.
    needs: Option<Option<String>>,
    /// The request the answer must carry to be the current one.
    asking: Option<Option<String>>,
    /// The rail's file: the one last picked, or the one scrolled to the top.
    pub selected: Option<String>,
    pub show_commits: bool,
    pub picker_open: bool,
    files: Vec<CompareFile>,
    pub state: DiffState,
    /// The `files` index of each `state` file, in order.
    owners: Vec<usize>,
}

impl Compare {
    #[must_use]
    pub fn loading(&self) -> bool {
        self.asking.is_some()
    }

    pub fn request(&mut self, base: Option<String>) {
        self.requested = base.clone();
        self.needs = Some(base);
        self.asking = None;
        self.error = None;
    }

    pub fn reload(&mut self) {
        let base = self
            .requested
            .clone()
            .or_else(|| self.data.as_ref().and_then(|data| data.base_ref.clone()));
        self.request(base);
    }

    /// The base the request for the next read names, once.
    pub fn take_request(&mut self) -> Option<Option<String>> {
        let base = self.needs.take()?;
        self.asking = Some(base.clone());
        Some(base)
    }

    pub fn apply(
        &mut self,
        scope: Scope,
        requested: Option<String>,
        result: Result<BranchCompare, String>,
    ) {
        if self.asking.as_ref() != Some(&requested) {
            return;
        }
        self.asking = None;
        match result {
            Ok(data) if data.workspace_id == scope.workspace => {
                self.error = None;
                self.requested = requested;
                self.build(&data);
                self.data = Some(data);
            }
            Ok(_) => {}
            Err(error) => self.error = Some(error),
        }
    }

    pub fn set_split(&mut self, split: bool) {
        self.state.set_mode(if split {
            DiffMode::Split
        } else {
            DiffMode::Unified
        });
    }

    /// Scrolls the diff to `path`'s patch, or to the top for `None`.
    pub fn select(&mut self, path: Option<String>, tx: &mut Tx) {
        let target = match &path {
            Some(path) => self
                .files
                .iter()
                .find(|file| &file.path == path)
                .and_then(|file| file.first_diff),
            None => Some(0),
        };
        if let Some(index) = target {
            self.state.scroll_to_file(index, tx);
        }
        self.selected = path;
    }

    /// The diff scrolled a new file to its top.
    pub fn follow(&mut self, diff_index: usize) {
        if let Some(file) = self
            .owners
            .get(diff_index)
            .and_then(|at| self.files.get(*at))
        {
            self.selected = Some(file.path.clone());
        }
    }

    pub fn toggle_commits(&mut self) {
        self.show_commits = !self.show_commits;
    }

    /// The base the header names: the chosen one while it is still loading,
    /// else what git resolved.
    #[must_use]
    pub fn base_label(&self) -> Option<&str> {
        self.requested
            .as_deref()
            .or_else(|| self.data.as_ref().and_then(|data| data.base_ref.as_deref()))
    }

    #[must_use]
    pub fn totals(&self) -> (u32, u32) {
        self.data.as_ref().map_or((0, 0), |data| data.diff.totals())
    }

    fn build(&mut self, data: &BranchCompare) {
        let mut parsed = Vec::new();
        self.owners.clear();
        self.files = data
            .diff
            .files
            .iter()
            .enumerate()
            .map(|(at, file)| {
                let diffs = rune_kit::diff::parse(&file.patch);
                let first_diff = (!diffs.is_empty()).then_some(parsed.len());
                self.owners.extend(std::iter::repeat_n(at, diffs.len()));
                parsed.extend(diffs);
                CompareFile {
                    path: file.path.clone(),
                    status: file.status,
                    additions: file.additions,
                    deletions: file.deletions,
                    icon: crate::file_icons::for_path(&file.path),
                    first_diff,
                }
            })
            .collect();
        self.state.set_files(parsed);
        let stale = self
            .selected
            .as_ref()
            .is_some_and(|selected| !self.files.iter().any(|file| &file.path == selected));
        if stale {
            self.selected = None;
        }
    }
}

fn status_letter(status: DiffStatus) -> &'static str {
    status.letter()
}

fn status_color(k: &Kit, status: DiffStatus) -> rune::Color {
    let git = crate::theme::roles(k).git;
    match status {
        DiffStatus::Added => git.added,
        DiffStatus::Deleted => git.deleted,
        DiffStatus::Conflicted => git.conflict,
        _ => git.modified,
    }
}

fn notice(k: &Kit, key: &'static str, message: &str) -> El {
    col().key(key).w_full().grow(1.0).padding(24.0).child(
        text(message.to_string())
            .font(k.ty.small)
            .color(k.c.text_muted),
    )
}

fn head(s: &AppState, k: &Kit, layer: &mut Vec<El>) -> El {
    let compare = &s.workbench.compare;
    let branch = compare
        .data
        .as_ref()
        .and_then(|data| data.diff.branch.clone())
        .or_else(|| {
            let workspace = s.workbench.workspace?;
            s.store
                .as_ref()?
                .workspaces
                .iter()
                .find(|workspace_row| workspace_row.id == workspace)?
                .branch
                .clone()
        })
        .unwrap_or_else(|| "detached".to_string());
    let project = s.workbench.workspace.and_then(|workspace| {
        s.store
            .as_ref()?
            .workspaces
            .iter()
            .find(|row| row.id == workspace)
            .map(|row| row.project_id)
    });
    let branches = project.and_then(|project| s.workbench.branches_for(project));
    let options = base_options(branches, Some(&branch), compare.base_label());
    let select = Select::new("compare-base", &options)
        .current(compare.base_label())
        .placeholder("base branch")
        .label("Base branch")
        .width(220.0)
        .open(compare.picker_open)
        .on_toggle(|s: &mut AppState| {
            s.workbench.compare.picker_open = !s.workbench.compare.picker_open;
        })
        .on_close(|s: &mut AppState| s.workbench.compare.picker_open = false)
        .on_choose(|s: &mut AppState, base, _| {
            s.workbench.compare.picker_open = false;
            s.choose_compare_base(base);
        })
        .build(k, layer);
    let (additions, deletions) = compare.totals();
    let files = compare
        .data
        .as_ref()
        .map_or(0, |data| data.diff.files.len());
    let commits = compare.data.as_ref().map_or(0, |data| data.commit_count);
    let mut bar = row()
        .key("compare-head")
        .w_full()
        .h(36.0)
        .px(12.0)
        .gap(crate::ds::SPACE_4)
        .items_center()
        .child(text(branch).font(k.ty.small_medium).color(k.c.text_base))
        .child(text("→").font(k.ty.small).color(k.c.text_faint))
        .child(select);
    if compare.data.is_some() {
        bar = bar
            .child(
                text(format!(
                    "{commits} commit{} · {files} file{}",
                    if commits == 1 { "" } else { "s" },
                    if files == 1 { "" } else { "s" }
                ))
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint),
            )
            .child(
                text(format!("+{additions}"))
                    .font(k.ty.text_xsmall)
                    .color(crate::theme::roles(k).git.added),
            )
            .child(
                text(format!("−{deletions}"))
                    .font(k.ty.text_xsmall)
                    .color(crate::theme::roles(k).git.deleted),
            );
    }
    let layout = SegmentedControl::new(
        &["Unified", "Split"],
        Some(if s.diff_split { 1 } else { 0 }),
    )
    .label("Patch layout")
    .width(Some(150.0))
    .on_pick(|s: &mut AppState, index, _| s.set_diff_split(index == 1));
    bar.child(el().grow(1.0))
        .children((!s.diff_split).then(|| super::view::diff_wrap_switch(s, k)))
        .child(layout.build(k))
        .child(
            Button::new("Reload")
                .variant(ButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .on_press(|s: &mut AppState| s.workbench.compare.reload())
                .build(k)
                .key("compare-reload"),
        )
}

fn body(s: &AppState, k: &Kit) -> El {
    let compare = &s.workbench.compare;
    let Some(data) = &compare.data else {
        return if compare.loading() {
            notice(k, "compare-loading", "Comparing branches…")
        } else {
            notice(k, "compare-empty", "Nothing compared yet.")
        };
    };
    if data.base_ref.is_none() {
        return notice(
            k,
            "compare-no-base",
            "No base branch resolves here: no origin default, main or master. Pick one above.",
        );
    }
    if data.diff.files.is_empty() {
        return notice(k, "compare-clean", "No changes against this base.");
    }
    let mut rail = col()
        .key("compare-rail")
        .w(230.0)
        .h_full()
        .min_h(0.0)
        .gap(1.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        });
    let all_selected = compare.selected.is_none();
    rail = rail.child(
        row()
            .key("compare-file-all")
            .role("button")
            .label("All files")
            .w_full()
            .h(26.0)
            .px(8.0)
            .items_center()
            .radius(k.radius.sm)
            .fill(if all_selected {
                k.c.surface_hover
            } else {
                rune::Color::TRANSPARENT
            })
            .hover(k.c.hover)
            .on_press_tx(|s: &mut AppState, tx| s.workbench.compare.select(None, tx))
            .child(text("All files").font(k.ty.small).color(k.c.text_base)),
    );
    for file in &compare.files {
        let path = file.path.clone();
        let selected = compare.selected.as_deref() == Some(file.path.as_str());
        rail = rail.child(
            row()
                .key(format!("compare-file-{}", file.path))
                .role("button")
                .label(file.path.clone())
                .w_full()
                .h(26.0)
                .px(8.0)
                .gap(crate::ds::SPACE_3)
                .items_center()
                .radius(k.radius.sm)
                .fill(if selected {
                    k.c.surface_hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .hover(k.c.hover)
                .on_press_tx(move |s: &mut AppState, tx| {
                    s.workbench.compare.select(Some(path.clone()), tx);
                })
                .child(icon(file.icon.icon(!k.dark), 13.0))
                .child(
                    text(status_letter(file.status))
                        .font(k.ty.mono)
                        .color(status_color(k, file.status))
                        .w(12.0),
                )
                .child(
                    text(
                        file.path
                            .rsplit('/')
                            .next()
                            .unwrap_or(&file.path)
                            .to_string(),
                    )
                    .font(k.ty.small)
                    .color(k.c.text_base)
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
    let mut diff = col().key("compare-diff").grow(1.0).min_w(0.0).min_h(0.0);
    if !data.commits.is_empty() {
        let label = format!(
            "Commits since {}",
            data.merge_base.as_deref().unwrap_or("the merge base")
        );
        diff = diff.child(
            row()
                .key("compare-commits-toggle")
                .role("button")
                .label(label.clone())
                .w_full()
                .h(26.0)
                .px(8.0)
                .items_center()
                .radius(k.radius.sm)
                .hover(k.c.hover)
                .on_press(|s: &mut AppState| s.workbench.compare.toggle_commits())
                .child(text(label).font(k.ty.text_xsmall).color(k.c.text_muted)),
        );
    }
    if compare.show_commits {
        let mut commits = col()
            .key("compare-commits")
            .w_full()
            .max_h(COMMITS_MAX_H)
            .px(8.0)
            .pb(crate::ds::SPACE_4)
            .gap(crate::ds::SPACE_1)
            .scroll(ScrollSpec {
                stick_bottom: false,
            });
        for commit in &data.commits {
            commits = commits.child(
                row()
                    .key(format!("compare-commit-{}", commit.short_id))
                    .w_full()
                    .gap(crate::ds::SPACE_4)
                    .child(
                        text(commit.short_id.clone())
                            .font(k.ty.mono)
                            .color(k.c.text_faint),
                    )
                    .child(
                        text(commit.subject.clone())
                            .font(k.ty.small)
                            .color(k.c.text_base)
                            .grow(1.0)
                            .min_w(0.0)
                            .truncate(),
                    ),
            );
        }
        let shown = data.commits.len() as u32;
        if data.commit_count > shown {
            commits = commits.child(
                text(format!("{} more", data.commit_count - shown))
                    .key("compare-commits-more")
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_faint),
            );
        }
        diff = diff.child(commits);
    }
    let mut scroller = DiffView::new(
        |s: &AppState| &s.workbench.compare.state,
        |s: &mut AppState| &mut s.workbench.compare.state,
    )
    .on_visible_file(|s: &mut AppState, index| s.workbench.compare.follow(index))
    .key("compare-scroll")
    .build(s, k)
    .grow(1.0)
    .min_h(0.0);
    if data.diff.truncated {
        scroller = col().w_full().grow(1.0).min_h(0.0).child(scroller).child(
            text("Some files were over budget and are not listed.")
                .key("compare-truncated")
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .padding(8.0),
        );
    }
    diff = diff.child(scroller);
    row()
        .key("compare-body")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(rail)
        .child(diff)
}

const COMMITS_MAX_H: f32 = 180.0;

pub fn view(s: &AppState, k: &Kit, layer: &mut Vec<El>) -> El {
    if s.workbench.workspace.is_none() {
        return notice(k, "compare-no-checkout", "No checkout in this window.");
    }
    let mut view = col()
        .key("compare")
        .w_full()
        .h_full()
        .min_h(0.0)
        .child(head(s, k, layer));
    if let Some(error) = &s.workbench.compare.error {
        view = view.child(
            text(format!("Could not compare: {error}"))
                .key("compare-error")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(k.c.warning.fg)
                .padding(12.0),
        );
    }
    view.child(body(s, k))
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{BranchRef, CommitLine, DiffFile, RefScope, Remote, WorkspaceDiff, WorkspaceId};
    use rune_kit::diff::DiffMode;

    fn branch(name: &str, scope: RefScope) -> BranchRef {
        BranchRef {
            name: name.into(),
            scope,
            upstream: None,
            committed_at: None,
            subject: None,
            checked_out_in: None,
        }
    }

    fn branches() -> client::Branches {
        client::Branches {
            branches: vec![
                branch("main", RefScope::Local),
                branch("feature/x", RefScope::Local),
                branch(
                    "main",
                    RefScope::Remote {
                        remote: "origin".into(),
                    },
                ),
                branch(
                    "stale",
                    RefScope::Remote {
                        remote: "origin".into(),
                    },
                ),
            ],
            remotes: vec![Remote {
                name: "origin".into(),
                url: "git@example.invalid:o/r".into(),
            }],
            default_branch: Some("main".into()),
        }
    }

    #[test]
    fn the_default_remote_leads_then_remotes_then_locals() {
        assert_eq!(
            base_options(Some(&branches()), Some("feature/x"), None),
            ["origin/main", "origin/stale", "main"]
        );
        assert_eq!(
            base_options(None, None, Some("release/1")),
            ["release/1"],
            "a resolved base still shows before branches arrive"
        );
    }

    #[test]
    fn a_base_never_offers_the_branch_it_compares() {
        let options = base_options(Some(&branches()), Some("main"), Some("origin/main"));
        assert!(!options.iter().any(|option| option == "main"));
        assert_eq!(options[0], "origin/main");
    }

    #[test]
    fn the_picker_is_bounded() {
        let mut many = branches();
        for index in 0..200 {
            many.branches
                .push(branch(&format!("branch-{index}"), RefScope::Local));
        }
        assert_eq!(
            base_options(Some(&many), None, None).len(),
            MAX_BASE_OPTIONS
        );
    }

    fn file(path: &str, hunks: &str) -> DiffFile {
        DiffFile {
            path: path.into(),
            status: DiffStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: format!("diff --git a/{path} b/{path}\n--- a/{path}\n+++ b/{path}\n{hunks}"),
            binary: false,
            truncated: false,
        }
    }

    fn compare_data(files: Vec<DiffFile>) -> BranchCompare {
        BranchCompare {
            workspace_id: WorkspaceId::new(),
            base_ref: Some("origin/main".into()),
            merge_base: Some("4f2b1ac".into()),
            commits: vec![CommitLine {
                short_id: "abc1234".into(),
                subject: "Add the compare view".into(),
            }],
            commit_count: 1,
            diff: WorkspaceDiff {
                workspace_id: WorkspaceId::new(),
                branch: Some("feature/x".into()),
                files,
                truncated: false,
            },
        }
    }

    fn scope(workspace: WorkspaceId) -> Scope {
        Scope {
            workspace,
            visit: 3,
        }
    }

    #[test]
    fn an_answer_lands_only_for_the_request_it_answers() {
        let mut compare = Compare::default();
        compare.request(Some("origin/main".into()));
        assert_eq!(compare.take_request(), Some(Some("origin/main".into())));
        let data = compare_data(vec![file("a.rs", "@@ -1 +1 @@\n-a\n+b\n")]);
        compare.apply(
            scope(data.workspace_id),
            Some("origin/release".into()),
            Ok(data.clone()),
        );
        assert!(compare.data.is_none(), "an older request is dropped");
        compare.apply(
            scope(data.workspace_id),
            Some("origin/main".into()),
            Ok(data),
        );
        assert_eq!(compare.data.as_ref().map(|data| data.commit_count), Some(1));
        assert!(!compare.loading());
    }

    #[test]
    fn a_switched_layout_is_a_redraw_without_reading_git_again() {
        let mut compare = Compare::default();
        let data = compare_data(vec![file("a.rs", "@@ -1,1 +1,2 @@\n-a\n+b\n+c\n")]);
        compare.request(None);
        let _ = compare.take_request();
        compare.apply(scope(data.workspace_id), None, Ok(data));
        assert_eq!(compare.state.files().len(), 1);
        assert_eq!(compare.state.mode(), DiffMode::Unified);
        compare.set_split(true);
        assert_eq!(compare.state.mode(), DiffMode::Split);
        assert_eq!(compare.base_label(), Some("origin/main"));
        assert_eq!(compare.take_request(), None, "a layout is not a read");
    }

    #[test]
    fn the_rail_follows_the_diff_and_a_vanished_pick_clears() {
        let mut compare = Compare::default();
        let data = compare_data(vec![
            file("a.rs", "@@ -1 +1 @@\n-a\n+b\n"),
            file("b.rs", "@@ -2 +2 @@\n-c\n+d\n"),
        ]);
        compare.request(None);
        let _ = compare.take_request();
        compare.apply(scope(data.workspace_id), None, Ok(data));
        assert_eq!(compare.state.files().len(), 2, "every file in one diff");
        compare.follow(1);
        assert_eq!(compare.selected.as_deref(), Some("b.rs"));

        let replaced = compare_data(vec![file("a.rs", "@@ -1 +1 @@\n-a\n+b\n")]);
        compare.request(None);
        let _ = compare.take_request();
        compare.apply(scope(replaced.workspace_id), None, Ok(replaced));
        assert_eq!(compare.selected, None, "b.rs is no longer in the compare");
    }

    #[test]
    fn a_layout_change_never_touches_the_working_tree_diff() {
        let mut workbench = super::super::Workbench::default();
        workbench.set_split(true);
        assert_eq!(
            workbench.changes.state.mode(),
            DiffMode::Split,
            "the Changes diff follows the same preference"
        );
        workbench.set_split(false);
        assert_eq!(workbench.changes.state.mode(), DiffMode::Unified);
    }
}
