//! The branch compare behind "Preview PR": committed work only, against the
//! branch a pull request would target.
//!
//! Every read is a daemon request on the worker (`GetBranchCompare`,
//! `ListBranches`); this side keeps the answer, splits its patches into rows
//! once, and never runs git again for a layout change.

use std::borrow::Cow;

use domain::{BranchCompare, DiffStatus};
use rune::{El, GrowthSpec, ScrollSpec, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, Kit, SegmentedControl, Select};

use super::Scope;
use super::patch::{self, Pair, Row};
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
    rows: Vec<Row>,
    pairs: Vec<Pair>,
}

#[derive(Clone, Copy)]
pub enum DisplayRow {
    Commit(usize),
    More(u32),
    File(usize),
    Line {
        file: usize,
        row: usize,
    },
    Split {
        file: usize,
        left: Option<usize>,
        right: Option<usize>,
    },
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
    pub selected: Option<String>,
    pub split: bool,
    pub show_commits: bool,
    pub picker_open: bool,
    files: Vec<CompareFile>,
    display: Vec<DisplayRow>,
    keys: Vec<String>,
    rev: u64,
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
        if self.split == split {
            return;
        }
        self.split = split;
        self.rebuild();
    }

    pub fn select(&mut self, path: Option<String>) {
        self.selected = path;
        self.rebuild();
    }

    pub fn toggle_commits(&mut self) {
        self.show_commits = !self.show_commits;
        self.rebuild();
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
        self.files = data
            .diff
            .files
            .iter()
            .map(|file| {
                let rows = patch::rows(&file.patch);
                let pairs = patch::pairs(&rows);
                CompareFile {
                    path: file.path.clone(),
                    status: file.status,
                    additions: file.additions,
                    deletions: file.deletions,
                    icon: crate::file_icons::for_path(&file.path),
                    rows,
                    pairs,
                }
            })
            .collect();
        let stale = self
            .selected
            .as_ref()
            .is_some_and(|selected| !self.files.iter().any(|file| &file.path == selected));
        if stale {
            self.selected = None;
        }
        self.rebuild();
    }

    fn rebuild(&mut self) {
        self.rev += 1;
        self.display.clear();
        self.keys.clear();
        if self.show_commits
            && let Some(data) = &self.data
        {
            for (index, commit) in data.commits.iter().enumerate() {
                self.display.push(DisplayRow::Commit(index));
                self.keys
                    .push(format!("compare-commit-{}", commit.short_id));
            }
            let shown = data.commits.len() as u32;
            if data.commit_count > shown {
                self.display
                    .push(DisplayRow::More(data.commit_count - shown));
                self.keys.push("compare-commits-more".into());
            }
        }
        let stale = self
            .selected
            .as_ref()
            .is_some_and(|selected| !self.files.iter().any(|file| &file.path == selected));
        if stale {
            self.selected = None;
        }
        for (index, file) in self.files.iter().enumerate() {
            if self
                .selected
                .as_ref()
                .is_some_and(|selected| selected != &file.path)
            {
                continue;
            }
            self.display.push(DisplayRow::File(index));
            self.keys.push(format!("compare-file-head-{}", file.path));
            if self.split {
                for (at, pair) in file.pairs.iter().enumerate() {
                    self.display.push(DisplayRow::Split {
                        file: index,
                        left: pair.left,
                        right: pair.right,
                    });
                    self.keys.push(format!("compare-split-{index}-{at}"));
                }
            } else {
                for at in 0..file.rows.len() {
                    self.display.push(DisplayRow::Line {
                        file: index,
                        row: at,
                    });
                    self.keys.push(format!("compare-line-{index}-{at}"));
                }
            }
        }
    }

    #[cfg(test)]
    pub fn visible_files(&self) -> impl Iterator<Item = &CompareFile> {
        self.files
            .iter()
            .filter(|file| self.selected.as_ref().is_none_or(|pick| pick == &file.path))
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

#[derive(Clone, Copy)]
struct Look {
    mono: rune::TextStyle,
    small: rune::TextStyle,
    xsmall: rune::TextStyle,
    text: rune::Color,
    muted: rune::Color,
    faint: rune::Color,
    added: rune::Color,
    deleted: rune::Color,
    hunk: rune::Color,
    added_bg: rune::Color,
    removed_bg: rune::Color,
    surface: rune::Color,
    conflict: rune::Color,
    dark: bool,
}

impl Look {
    fn new(k: &Kit) -> Look {
        let roles = crate::theme::roles(k);
        Look {
            mono: k.ty.mono,
            small: k.ty.small,
            xsmall: k.ty.text_xsmall,
            text: k.c.text_base,
            muted: k.c.text_muted,
            faint: k.c.text_faint,
            added: roles.git.added,
            deleted: roles.git.deleted,
            hunk: k.c.text_faint,
            added_bg: roles.diff_added_bg,
            removed_bg: roles.diff_removed_bg,
            surface: k.c.surface_weak,
            conflict: roles.git.conflict,
            dark: k.dark,
        }
    }
}

const ROW_H: f32 = 20.0;
const GUTTER_W: f32 = 44.0;

fn file_head(s: &AppState, look: &Look, index: usize) -> El {
    let file = &s.workbench.compare.files[index];
    let path = file.path.clone();
    let line = row()
        .role("button")
        .label(format!(
            "Open {} at line 1, {}",
            file.path,
            status_letter(file.status)
        ))
        .w_full()
        .h(ROW_H)
        .px(6.0)
        .gap(crate::ds::SPACE_3)
        .items_center()
        .fill(look.surface)
        .hover(look.surface)
        .on_press(move |s: &mut AppState| s.open_file(&path, Some(1)))
        .child(icon(file.icon.icon(!look.dark), 13.0))
        .child(
            text(status_letter(file.status))
                .font(look.mono)
                .color(status_color_of(look, file.status))
                .w(12.0),
        )
        .child(
            text(file.path.clone())
                .font(look.small)
                .color(look.text)
                .grow(1.0)
                .min_w(0.0)
                .truncate(),
        )
        .child(
            text(format!("+{}", file.additions))
                .font(look.xsmall)
                .color(look.added),
        )
        .child(
            text(format!("−{}", file.deletions))
                .font(look.xsmall)
                .color(look.deleted),
        );
    line.key(format!("compare-file-{}", file.path))
}

fn status_color_of(look: &Look, status: DiffStatus) -> rune::Color {
    match status {
        DiffStatus::Added => look.added,
        DiffStatus::Deleted => look.deleted,
        DiffStatus::Conflicted => look.conflict,
        _ => look.muted,
    }
}

fn cell(look: &Look, number: Option<u32>, patch_row: Option<&Row>, key: String) -> El {
    let (fill, ink) = match patch_row.map(|row| row.kind) {
        Some(patch::Kind::Added) => (look.added_bg, look.added),
        Some(patch::Kind::Removed) => (look.removed_bg, look.deleted),
        Some(patch::Kind::Hunk) => (look.surface, look.hunk),
        Some(patch::Kind::Meta) => (rune::Color::TRANSPARENT, look.faint),
        _ => (rune::Color::TRANSPARENT, look.text),
    };
    row()
        .key(key)
        .w(0.0)
        .grow(1.0)
        .min_w(0.0)
        .h(ROW_H)
        .gap(crate::ds::SPACE_2)
        .items_center()
        .fill(fill)
        .child(
            text(number.map_or_else(String::new, |number| number.to_string()))
                .font(look.mono)
                .color(look.faint)
                .w(GUTTER_W)
                .pr(4.0)
                .align(rune::Align::End),
        )
        .child(
            text(patch_row.map_or_else(String::new, |row| row.text.clone()))
                .font(look.mono)
                .color(ink)
                .truncate()
                .min_w(0.0),
        )
}

fn unified_row(s: &AppState, look: &Look, file: usize, at: usize) -> El {
    let source = &s.workbench.compare.files[file];
    let patch_row = &source.rows[at];
    let path = source.path.clone();
    let after = patch_row.after;
    let (fill, ink) = match patch_row.kind {
        patch::Kind::Added => (look.added_bg, look.added),
        patch::Kind::Removed => (look.removed_bg, look.deleted),
        patch::Kind::Hunk => (look.surface, look.hunk),
        patch::Kind::Meta => (rune::Color::TRANSPARENT, look.faint),
        _ => (rune::Color::TRANSPARENT, look.text),
    };
    let mut line = row()
        .key(format!("compare-unified-{file}-{at}"))
        .w_full()
        .h(ROW_H)
        .gap(crate::ds::SPACE_2)
        .items_center()
        .fill(fill);
    if let Some(after) = after {
        line = line.on_double_press(move |s: &mut AppState| {
            s.open_file(&path, Some(after));
        });
    }
    line.child(
        text(
            patch_row
                .before
                .map_or_else(String::new, |number| number.to_string()),
        )
        .font(look.mono)
        .color(look.faint)
        .w(GUTTER_W)
        .pr(4.0)
        .align(rune::Align::End),
    )
    .child(
        text(
            patch_row
                .after
                .map_or_else(String::new, |number| number.to_string()),
        )
        .font(look.mono)
        .color(look.faint)
        .w(GUTTER_W)
        .pr(4.0)
        .align(rune::Align::End),
    )
    .child(
        text(match patch_row.kind {
            patch::Kind::Added => "+",
            patch::Kind::Removed => "−",
            _ => "",
        })
        .font(look.mono)
        .color(ink)
        .w(10.0),
    )
    .child(
        text(patch_row.text.clone())
            .font(look.mono)
            .color(ink)
            .grow(1.0)
            .min_w(0.0)
            .truncate(),
    )
}

fn split_row(
    s: &AppState,
    look: &Look,
    file: usize,
    left: Option<usize>,
    right: Option<usize>,
) -> El {
    let source = &s.workbench.compare.files[file];
    let path = source.path.clone();
    let left_row = left.map(|at| &source.rows[at]);
    let right_row = right.map(|at| &source.rows[at]);
    let left_after = left_row.and_then(|row| row.after);
    let right_after = right_row.and_then(|row| row.after);
    let mut left_cell = cell(
        look,
        left_row.and_then(|row| row.before),
        left_row,
        format!("compare-cell-{file}-{left:?}-{right:?}-left"),
    );
    if let Some(after) = left_after {
        let path = path.clone();
        left_cell = left_cell.on_double_press(move |s: &mut AppState| {
            s.open_file(&path, Some(after));
        });
    }
    let mut right_cell = cell(
        look,
        right_row.and_then(|row| row.after),
        right_row,
        format!("compare-cell-{file}-{left:?}-{right:?}-right"),
    );
    if let Some(after) = right_after {
        right_cell = right_cell.on_double_press(move |s: &mut AppState| {
            s.open_file(&path, Some(after));
        });
    }
    row()
        .key(format!("compare-split-row-{file}-{left:?}-{right:?}"))
        .w_full()
        .h(ROW_H)
        .child(left_cell)
        .child(right_cell)
}

fn display_row(s: &AppState, look: &Look, index: usize) -> El {
    let compare = &s.workbench.compare;
    match compare.display[index] {
        DisplayRow::Commit(commit) => {
            let Some(data) = &compare.data else {
                return el();
            };
            let Some(commit) = data.commits.get(commit) else {
                return el();
            };
            row()
                .key(format!("compare-commit-{}", commit.short_id))
                .h(18.0)
                .px(8.0)
                .gap(crate::ds::SPACE_4)
                .items_center()
                .child(
                    text(commit.short_id.clone())
                        .font(look.mono)
                        .color(look.faint),
                )
                .child(
                    text(commit.subject.clone())
                        .font(look.small)
                        .color(look.text)
                        .truncate()
                        .min_w(0.0),
                )
        }
        DisplayRow::More(count) => text(format!("…and {count} more."))
            .key("compare-commits-more")
            .h(18.0)
            .px(8.0)
            .font(look.xsmall)
            .color(look.faint),
        DisplayRow::File(file) => file_head(s, look, file),
        DisplayRow::Line { file, row } => unified_row(s, look, file, row),
        DisplayRow::Split { file, left, right } => split_row(s, look, file, left, right),
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
    bar.child(el().grow(1.0)).child(layout.build(k)).child(
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
        return notice(
            k,
            "compare-clean",
            "No committed changes against this base.",
        );
    }
    let look = Look::new(k);
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
            .on_press(|s: &mut AppState| s.workbench.compare.select(None))
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
                .on_press(move |s: &mut AppState| {
                    s.workbench.compare.select(Some(path.clone()));
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
    let count = compare.display.len();
    let list = col()
        .key("compare-rows")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            count,
            ROW_H,
            |s: &AppState, index| Cow::Borrowed(s.workbench.compare.keys[index].as_str()),
            move |s: &AppState, index| display_row(s, &look, index),
        )
        .virtual_rows_version(compare.rev);
    let mut scroller = col()
        .key("compare-scroll")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec {
            stick_bottom: false,
        })
        .child(list);
    if data.diff.truncated {
        scroller = scroller.child(
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

    fn file(path: &str, patch: &str) -> DiffFile {
        DiffFile {
            path: path.into(),
            status: DiffStatus::Modified,
            additions: 1,
            deletions: 1,
            patch: patch.into(),
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
    fn a_switched_layout_pairs_rows_without_reading_git_again() {
        let mut compare = Compare::default();
        let data = compare_data(vec![file("a.rs", "@@ -1,1 +1,2 @@\n-a\n+b\n+c\n")]);
        compare.request(None);
        let _ = compare.take_request();
        compare.apply(scope(data.workspace_id), None, Ok(data));
        assert!(matches!(compare.display[1], DisplayRow::Line { .. }));
        assert_eq!(compare.data.as_ref().unwrap().diff.files.len(), 1);
        compare.set_split(true);
        assert!(matches!(compare.display[1], DisplayRow::Split { .. }));
        assert_eq!(compare.base_label(), Some("origin/main"));
        assert_eq!(compare.take_request(), None, "a layout is not a read");
    }

    #[test]
    fn selecting_a_file_keeps_only_its_rows_and_a_missing_one_clears() {
        let mut compare = Compare::default();
        let data = compare_data(vec![
            file("a.rs", "@@ -1 +1 @@\n-a\n+b\n"),
            file("b.rs", "@@ -2 +2 @@\n-c\n+d\n"),
        ]);
        compare.request(None);
        let _ = compare.take_request();
        compare.apply(scope(data.workspace_id), None, Ok(data));
        compare.select(Some("b.rs".into()));
        let files: Vec<&str> = compare
            .visible_files()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(files, ["b.rs"]);
        assert!(
            compare
                .display
                .iter()
                .all(|row| !matches!(row, DisplayRow::File(0)))
        );
        compare.select(Some("gone.rs".into()));
        assert_eq!(compare.visible_files().count(), 2, "a stale pick shows all");
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
