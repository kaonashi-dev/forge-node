//! Run Spec… and Draft Spec…: the dialog that starts a daemon spec run.
//!
//! Whether a spec path names a file is asked of the daemon when the file is in
//! the checkout (ADR-012), on the connection worker, never here.

use std::path::{Component, Path, PathBuf};

use domain::spec_run::model::{
    AgentPick, Delivery, MAX_ROUNDS, MIN_ROUNDS, Placement, SpecOptions, SpecRunSource,
};
use domain::spec_run::{
    MAX_INLINE_SPEC_BYTES, Preset, SpecSource, agent_refusal, classify, planner_effort,
};
use domain::{AgentDescriptor, AgentProfileId, AgentProviderId, SpecRunId, WorkspaceId};
use rune::{El, TextEdit, col, text};
use rune_kit::{Button, ButtonVariant, Checkbox, Kit, SegmentedControl, TextArea, TextInput};

use crate::agent_ops::Choice;
use crate::app::AppState;

const PLANNER: &str = "claude";
/// A Claude coder, under any profile, codes at the `claude` preset's medium
/// effort; any other provider runs at its own default.
const CLAUDE_PRESET: &str = "claude";
/// The last coder and planner rows a run started with.
const CODER_PREF: &str = "ui.spec_run.coder";
const PLANNER_PREF: &str = "ui.spec_run.planner";
pub const DEFAULT_ROUNDS: u32 = 3;
pub const ROUNDS: [u32; 5] = [1, 2, 3, 4, 5];
const NAME_WORDS: usize = 6;
const NAME_CHARS: usize = 48;
const BRANCH_SLUG_CHARS: usize = 40;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecMode {
    Run,
    Draft,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRequest {
    /// A path (relative to the checkout, absolute or `~/`), a link, an issue, or text.
    pub spec: String,
    pub coder: SpecAgent,
    pub planner_profile: Option<AgentProfileId>,
    pub worktree: bool,
    /// Empty picks `spec/<slug of the name>`.
    pub branch: Option<String>,
    pub rounds: u32,
    pub publish_spec: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecAgent {
    pub provider: String,
    pub profile: Option<AgentProfileId>,
}

impl From<&Choice> for SpecAgent {
    fn from(row: &Choice) -> Self {
        Self {
            provider: row.provider.to_string(),
            profile: row.profile,
        }
    }
}

/// Where a spec path points, for the one question asked about it: is it a file?
#[derive(Debug, PartialEq, Eq)]
pub enum SpecPath<'a> {
    /// Relative and inside the checkout: the daemon answers.
    InCheckout(&'a Path),
    /// Anywhere else, as an absolute path.
    Elsewhere(PathBuf),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecLaunch {
    pub name: String,
    pub source: SpecRunSource,
    pub planner: AgentPick,
    pub coder: AgentPick,
    pub options: SpecOptions,
}

/// A `../` path is resolved against `checkout`: the connection worker's cwd
/// is not the checkout, and the daemon refuses a relative spec path.
fn place<'a>(raw: &'a Path, checkout: &Path, home: Option<&Path>) -> SpecPath<'a> {
    if let (Ok(rest), Some(home)) = (raw.strip_prefix("~"), home) {
        return SpecPath::Elsewhere(home.join(rest));
    }
    let escapes = raw
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    if raw.is_absolute() {
        SpecPath::Elsewhere(raw.to_path_buf())
    } else if escapes {
        SpecPath::Elsewhere(normalized(&checkout.join(raw)))
    } else {
        SpecPath::InCheckout(raw)
    }
}

/// `..` and `.` folded lexically: canonicalizing would read the disk from the GUI (ADR-012).
fn normalized(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::ParentDir => {
                if !out.pop() && !out.has_root() {
                    out.push(part);
                }
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// `#123`, `123`, or an https link to an issue.
#[must_use]
pub fn issue_reference(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let link = raw.starts_with("https://")
        && !raw.chars().any(char::is_whitespace)
        && raw
            .split_once("/issues/")
            .is_some_and(|(_, rest)| digits(rest.split(['/', '#', '?']).next().unwrap_or("")));
    (digits(raw.strip_prefix('#').unwrap_or(raw)) || link).then(|| raw.to_owned())
}

/// A short name for the run, from what it starts from.
#[must_use]
pub fn run_name(source: &SpecRunSource) -> String {
    let name = match source {
        SpecRunSource::File { path } => Path::new(path)
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned()),
        SpecRunSource::Issue { reference } => reference
            .rsplit(|c: char| !c.is_ascii_digit())
            .find(|part| !part.is_empty())
            .map(|number| format!("issue {number}")),
        SpecRunSource::Inline { text } => Some(
            text.split_whitespace()
                .take(NAME_WORDS)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        SpecRunSource::ClaudeArtifact { url } | SpecRunSource::Link { url } => url
            .split_once("://")
            .and_then(|(_, rest)| rest.split(['?', '#']).next())
            .and_then(|rest| rest.trim_end_matches('/').split_once('/'))
            .and_then(|(_, path)| path.rsplit('/').next())
            .map(str::to_owned),
        _ => None,
    };
    let name: String = name
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "spec".to_owned())
        .chars()
        .take(NAME_CHARS)
        .collect();
    name.trim_end().to_owned()
}

/// `spec/<slug>-<suffix>`, for a worktree placement with no branch typed. The suffix keeps two
/// runs with one name off each other's branch, as `forgectl` does.
#[must_use]
pub fn default_branch(name: &str) -> String {
    let mut slug = String::new();
    for c in name.chars().flat_map(char::to_lowercase) {
        if c.is_ascii_alphanumeric() {
            slug.push(c);
        } else if !slug.is_empty() && !slug.ends_with('-') {
            slug.push('-');
        }
        if slug.len() >= BRANCH_SLUG_CHARS {
            break;
        }
    }
    let slug = slug.trim_end_matches('-');
    let slug = if slug.is_empty() { "run" } else { slug };
    let unique = SpecRunId::new().to_string();
    format!("spec/{slug}-{}", &unique[unique.len() - 6..])
}

/// The spec run `request` starts in `checkout`, or why it would fail.
///
/// # Errors
/// A sentence for the person: an empty spec, a path that is not a file, a
/// pasted spec over the cap, or a planner or coder the providers cannot run.
pub fn spec_launch(
    providers: &[AgentDescriptor],
    checkout: &Path,
    request: &SpecRequest,
    home: Option<&Path>,
    is_file: impl Fn(SpecPath<'_>) -> bool,
) -> Result<SpecLaunch, String> {
    let source = match issue_reference(&request.spec) {
        Some(reference) => SpecRunSource::Issue { reference },
        None => match classify(&request.spec, |raw| is_file(place(raw, checkout, home)))
            .map_err(|error| error.to_string())?
        {
            SpecSource::File(raw) => SpecRunSource::File {
                path: match place(&raw, checkout, home) {
                    SpecPath::InCheckout(relative) => checkout.join(relative),
                    SpecPath::Elsewhere(path) => path,
                }
                .to_string_lossy()
                .into_owned(),
            },
            SpecSource::Inline(text) if text.len() > MAX_INLINE_SPEC_BYTES => {
                return Err(format!(
                    "a pasted spec is capped at {} KiB; save it to a file",
                    MAX_INLINE_SPEC_BYTES / 1024
                ));
            }
            SpecSource::Inline(text) => SpecRunSource::Inline { text },
            SpecSource::ClaudeArtifact(url) => SpecRunSource::ClaudeArtifact { url },
            SpecSource::Link(url) => SpecRunSource::Link { url },
        },
    };
    let coder_effort = (request.coder.provider == CLAUDE_PRESET)
        .then(|| Preset::Claude.coder().1.map(str::to_owned))
        .flatten();
    check(
        providers,
        &request.coder.provider,
        coder_effort.as_deref(),
        "coder",
    )?;
    let planner = descriptor(providers, PLANNER, "planner")?;
    let planner_effort = planner_effort(planner);
    check(providers, PLANNER, planner_effort.as_deref(), "planner")?;
    let name = run_name(&source);
    let placement = if request.worktree {
        Placement::Worktree {
            branch: request
                .branch
                .as_deref()
                .map(str::trim)
                .filter(|branch| !branch.is_empty())
                .map_or_else(|| default_branch(&name), str::to_owned),
            base: None,
        }
    } else {
        Placement::Checkout
    };
    Ok(SpecLaunch {
        name,
        source,
        planner: AgentPick {
            provider: planner.id.clone(),
            profile: request.planner_profile,
            effort: planner_effort,
            model: None,
        },
        coder: AgentPick {
            provider: AgentProviderId::new(&request.coder.provider),
            profile: request.coder.profile,
            effort: coder_effort,
            model: None,
        },
        options: SpecOptions {
            placement,
            delivery: Delivery::NoCommit,
            rounds: request.rounds.clamp(MIN_ROUNDS, MAX_ROUNDS),
            forge_review: true,
            session_per_stage: false,
            publish_spec: request.publish_spec,
            publish_report: false,
            checks_offline: false,
        },
    })
}

fn descriptor<'a>(
    providers: &'a [AgentDescriptor],
    id: &str,
    role: &str,
) -> Result<&'a AgentDescriptor, String> {
    providers
        .iter()
        .find(|descriptor| descriptor.id.as_str() == id)
        .ok_or_else(|| format!("the {role} provider {id} is not available"))
}

fn check(
    providers: &[AgentDescriptor],
    id: &str,
    effort: Option<&str>,
    role: &str,
) -> Result<(), String> {
    match agent_refusal(descriptor(providers, id, role)?, effort, None) {
        Some(reason) => Err(format!("the {role} provider {id} {reason}")),
        None => Ok(()),
    }
}

pub struct SpecDialog {
    pub mode: SpecMode,
    pub workspace: WorkspaceId,
    pub spec: TextEdit,
    pub coder: Option<String>,
    pub planner: Option<String>,
    pub worktree: bool,
    pub branch: TextEdit,
    pub rounds: u32,
    pub publish_spec: bool,
    pub starting: bool,
    pub error: Option<String>,
}

impl SpecDialog {
    #[must_use]
    pub fn new(
        mode: SpecMode,
        workspace: WorkspaceId,
        spec: &str,
        store: Option<&client::Store>,
    ) -> Self {
        let mut edit = TextEdit::new(spec);
        edit.focused = true;
        let remembered = |rows: Vec<Choice>, pref: &str| {
            remembered_row(&rows, store.and_then(|store| store.app_state_value(pref)))
        };
        Self {
            mode,
            workspace,
            spec: edit,
            coder: store.and_then(|store| remembered(coder_rows(store), CODER_PREF)),
            planner: store.and_then(|store| remembered(planner_rows(store), PLANNER_PREF)),
            worktree: true,
            branch: TextEdit::new(""),
            rounds: DEFAULT_ROUNDS,
            publish_spec: false,
            starting: false,
            error: None,
        }
    }

    #[must_use]
    pub fn request(&self, store: &client::Store) -> Option<SpecRequest> {
        let spec = self.spec.text.trim();
        let pick = |rows: Vec<Choice>, key: &Option<String>| {
            rows.into_iter()
                .find(|row| key.as_deref() == Some(row.key.as_str()))
        };
        let coder = pick(coder_rows(store), &self.coder)?;
        let planner = pick(planner_rows(store), &self.planner)?;
        (!spec.is_empty() && !self.starting).then(|| SpecRequest {
            spec: spec.to_owned(),
            coder: SpecAgent::from(&coder),
            planner_profile: planner.profile,
            worktree: self.worktree,
            branch: Some(self.branch.text.trim().to_owned()).filter(|b| !b.is_empty()),
            rounds: self.rounds,
            publish_spec: self.publish_spec,
        })
    }
}

/// Every agent and visible profile that takes a prompt, Claude's first; a
/// Claude row codes at the preset's medium effort and says so.
#[must_use]
pub fn coder_rows(store: &client::Store) -> Vec<Choice> {
    let mut rows = crate::agent_ops::choices(store, true);
    rows.sort_by_key(|row| row.provider.as_str() != CLAUDE_PRESET);
    for row in &mut rows {
        if row.provider.as_str() == CLAUDE_PRESET {
            row.label.push_str(" · medium");
        }
    }
    rows
}

/// The drafter and planner are Claude; its rows are Claude and each Claude profile.
#[must_use]
pub fn planner_rows(store: &client::Store) -> Vec<Choice> {
    crate::agent_ops::choices(store, true)
        .into_iter()
        .filter(|row| row.provider.as_str() == PLANNER)
        .collect()
}

/// The row a run last started with, when it is still offered, else the first.
#[must_use]
pub fn remembered_row(rows: &[Choice], remembered: Option<&str>) -> Option<String> {
    rows.iter()
        .find(|row| Some(row.key.as_str()) == remembered)
        .or_else(|| rows.first())
        .map(|row| row.key.clone())
}

impl AppState {
    /// Opens the dialog for the window's checkout; Run Spec fills in the
    /// Markdown file on screen.
    pub fn open_run_spec(&mut self, mode: SpecMode) {
        self.sync_workbench_workspace();
        let Some(workspace) = self.workbench.workspace else {
            self.toasts.neutral(
                "Open a checkout first",
                Some("A spec run starts in one.".into()),
            );
            return;
        };
        let on_screen = self
            .active_editor_state()
            .map(|state| state.path.clone())
            .filter(|path| {
                mode == SpecMode::Run
                    && Path::new(path)
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .is_some_and(|ext| matches!(ext, "md" | "markdown" | "mdx"))
            })
            .unwrap_or_default();
        self.spec_dialog = Some(SpecDialog::new(
            mode,
            workspace,
            &on_screen,
            self.store.as_ref(),
        ));
    }

    pub fn start_spec_run(&mut self) {
        let (Some(dialog), Some(store)) = (self.spec_dialog.as_mut(), &self.store) else {
            return;
        };
        let Some(request) = dialog.request(store) else {
            return;
        };
        let workspace = dialog.workspace;
        let remembered = [
            (CODER_PREF, dialog.coder.clone()),
            (PLANNER_PREF, dialog.planner.clone()),
        ];
        let Some(checkout) = store
            .workspaces
            .iter()
            .find(|row| row.id == workspace)
            .map(|row| row.path.clone())
        else {
            dialog.error = Some("This checkout is no longer listed.".into());
            return;
        };
        let providers = store
            .providers
            .iter()
            .map(|info| info.descriptor.clone())
            .collect();
        let Some(connection) = &self.connection else {
            dialog.error = Some("Not connected to the daemon.".into());
            return;
        };
        match connection.run_spec(workspace, checkout, providers, request) {
            Ok(()) => {
                for (pref, key) in remembered {
                    if let Some(key) = key {
                        let _ = connection.set_app_state(pref, &key);
                    }
                }
                dialog.starting = true;
                dialog.error = None;
            }
            Err(error) => dialog.error = Some(error),
        }
    }

    /// The daemon's answer to Run Spec: the drafter opens as a tab, or the
    /// dialog says why it did not start.
    pub fn spec_run_result(&mut self, ok: bool, message: &str) {
        if ok {
            self.spec_dialog = None;
        } else if let Some(dialog) = self.spec_dialog.as_mut() {
            dialog.starting = false;
            dialog.error = Some(message.to_owned());
        }
    }
}

pub fn dialog(s: &AppState, k: &Kit, layer: &mut Vec<El>) {
    let Some(dialog) = &s.spec_dialog else {
        return;
    };
    let offered = s.store.as_ref().map(coder_rows).unwrap_or_default();
    let planners = s.store.as_ref().map(planner_rows).unwrap_or_default();
    let picker = |rows: &[Choice],
                  chosen: &Option<String>,
                  label: &'static str,
                  empty: &'static str,
                  set: fn(&mut SpecDialog, String)| {
        if rows.is_empty() {
            return text(empty).font(k.ty.text_xsmall).color(k.c.warning.fg);
        }
        let labels: Vec<&str> = rows.iter().map(|row| row.label.as_str()).collect();
        let picks: Vec<String> = rows.iter().map(|row| row.key.clone()).collect();
        SegmentedControl::new(
            &labels,
            rows.iter()
                .position(|row| chosen.as_deref() == Some(row.key.as_str())),
        )
        .label(label)
        .width(None)
        .on_pick(move |s: &mut AppState, index: usize, _| {
            if let (Some(dialog), Some(key)) = (s.spec_dialog.as_mut(), picks.get(index)) {
                set(dialog, key.clone());
            }
        })
        .build(k)
        .key(format!("spec-{}", label.to_ascii_lowercase()))
    };
    let coder = picker(
        &offered,
        &dialog.coder,
        "Coder",
        "No installed agent takes a prompt.",
        |dialog, key| dialog.coder = Some(key),
    );
    let planner = picker(
        &planners,
        &dialog.planner,
        "Planner",
        "The drafter and planner are Claude, and Claude is not installed.",
        |dialog, key| dialog.planner = Some(key),
    );
    let round_labels = ROUNDS.map(|count| count.to_string());
    let rounds = SegmentedControl::new(
        &round_labels,
        ROUNDS.iter().position(|count| *count == dialog.rounds),
    )
    .label("Review rounds")
    .fit()
    .on_pick(|s: &mut AppState, index: usize, _| {
        if let (Some(dialog), Some(count)) = (s.spec_dialog.as_mut(), ROUNDS.get(index)) {
            dialog.rounds = *count;
        }
    })
    .build(k)
    .key("spec-rounds");
    let placement = SegmentedControl::new(
        &["New worktree", "This checkout"],
        Some(usize::from(!dialog.worktree)),
    )
    .label("Where the coder works")
    .fit()
    .on_pick(|s: &mut AppState, index: usize, _| {
        if let Some(dialog) = s.spec_dialog.as_mut() {
            dialog.worktree = index == 0;
        }
    })
    .build(k)
    .key("spec-placement");
    let field = |words: &'static str, control: El| {
        col()
            .w_full()
            .gap(crate::ds::SPACE_3)
            .child(
                text(words)
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted)
                    .a11y_hidden(),
            )
            .child(control)
    };
    let drafting = dialog.mode == SpecMode::Draft;
    let note = dialog.error.clone().unwrap_or_else(|| {
        if drafting {
            "A Claude drafter turns the idea, issue or file into a spec with AC-n criteria. \
             You pass three gates: the spec, the plan with the checks Forge will run, and the \
             result. Nothing is committed or pushed unless you choose it at the plan gate."
        } else {
            "A Claude drafter checks the spec and writes it with AC-n criteria. You pass three \
             gates: the spec, the plan with the checks Forge will run, and the result. Nothing \
             is committed or pushed unless you choose it at the plan gate."
        }
        .into()
    });
    let mut body = col()
        .w_full()
        .gap(crate::ds::SPACE_6)
        .child(field(
            if drafting { "Idea or issue" } else { "Spec" },
            TextArea::new(&dialog.spec)
                .label(if drafting { "Idea or issue" } else { "Spec" })
                .placeholder(if drafting {
                    "What to build, #123 or an issue link, a file, or a Claude artifact link"
                } else {
                    "docs/spec.md, a Claude artifact link, or the spec itself"
                })
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_dialog.as_mut() {
                    Some(dialog) => &mut dialog.spec,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-text")
                .h(96.0),
        ))
        .child(field("Planner", planner))
        .child(field("Coder", coder))
        .child(field("Where the coder works", placement));
    if dialog.worktree {
        body = body.child(field(
            "Worktree branch",
            TextInput::new(&dialog.branch)
                .label("Worktree branch")
                .placeholder("Empty: spec/<the run's name>")
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_dialog.as_mut() {
                    Some(dialog) => &mut dialog.branch,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-worktree"),
        ));
    }
    let publish = dialog.publish_spec;
    let body = body
        .child(field("Review rounds", rounds))
        .child(
            Checkbox::new(publish)
                .label("Also publish the spec as a Claude artifact")
                .on_toggle(move |s: &mut AppState| {
                    if let Some(dialog) = s.spec_dialog.as_mut() {
                        dialog.publish_spec = !publish;
                    }
                })
                .build(k)
                .key("spec-publish"),
        )
        .child(
            text(note)
                .key("spec-note")
                .role("status")
                .font(k.ty.text_xsmall)
                .color(if dialog.error.is_some() {
                    k.c.warning.fg
                } else {
                    k.c.text_faint
                })
                .wrap()
                .pb(crate::ds::SPACE_4),
        );
    let blocked = s
        .store
        .as_ref()
        .and_then(|store| dialog.request(store))
        .is_none();
    crate::modal::Modal::new(
        "run-spec",
        if drafting { "Draft Spec" } else { "Run a spec" },
    )
    .width(crate::modal::WIDTH_STANDARD)
    .on_close(|s: &mut AppState| s.spec_dialog = None)
    .child(body)
    .footer([
        Button::new("Cancel")
            .variant(ButtonVariant::GhostMuted)
            .on_press(|s: &mut AppState| s.spec_dialog = None)
            .build(k)
            .key("spec-cancel"),
        Button::new(if dialog.starting {
            "Starting…"
        } else {
            "Start"
        })
        .variant(ButtonVariant::Contrast)
        .disabled(blocked)
        .on_press(|s: &mut AppState| s.start_spec_run())
        .build(k)
        .key("spec-start"),
    ])
    .build(s, k, layer);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn providers() -> Vec<AgentDescriptor> {
        ["claude", "cursor", "opencode"]
            .iter()
            .map(|id| agents::builtins::builtin(id).expect("builtin"))
            .collect()
    }

    fn request(spec: &str, coder: &str) -> SpecRequest {
        SpecRequest {
            spec: spec.into(),
            coder: SpecAgent {
                provider: coder.into(),
                profile: None,
            },
            planner_profile: None,
            worktree: true,
            branch: None,
            rounds: DEFAULT_ROUNDS,
            publish_spec: false,
        }
    }

    /// A default branch ends in a random suffix; the slug before it is what a test can name.
    fn unsuffixed(placement: &Placement) -> Placement {
        match placement {
            Placement::Worktree { branch, base } => Placement::Worktree {
                branch: branch
                    .rsplit_once('-')
                    .map_or_else(|| branch.clone(), |(slug, _)| slug.to_owned()),
                base: base.clone(),
            },
            other => other.clone(),
        }
    }

    fn launch(request: &SpecRequest, files: &[&str]) -> Result<SpecLaunch, String> {
        spec_launch(
            &providers(),
            Path::new("/work/repo"),
            request,
            Some(Path::new("/home/me")),
            |path| match path {
                SpecPath::InCheckout(relative) => files.contains(&relative.to_str().unwrap_or("")),
                SpecPath::Elsewhere(path) => files.contains(&path.to_str().unwrap_or("")),
            },
        )
    }

    #[test]
    fn a_checkout_file_starts_a_run_in_a_new_worktree_by_default() {
        let launch = launch(
            &request("docs/auth-flow.md", "claude"),
            &["docs/auth-flow.md"],
        )
        .expect("launch");
        assert_eq!(launch.name, "auth-flow");
        assert_eq!(
            launch.source,
            SpecRunSource::File {
                path: "/work/repo/docs/auth-flow.md".into()
            }
        );
        assert_eq!(launch.planner.provider.as_str(), "claude");
        assert_eq!(launch.planner.effort.as_deref(), Some("high"));
        assert_eq!(launch.coder.effort.as_deref(), Some("medium"));
        assert_eq!(
            SpecOptions {
                placement: unsuffixed(&launch.options.placement),
                ..launch.options.clone()
            },
            SpecOptions {
                placement: Placement::Worktree {
                    branch: "spec/auth-flow".into(),
                    base: None
                },
                delivery: Delivery::NoCommit,
                rounds: DEFAULT_ROUNDS,
                forge_review: true,
                session_per_stage: false,
                publish_spec: false,
                publish_report: false,
                checks_offline: false,
            }
        );
    }

    #[test]
    fn a_home_or_escaping_path_is_looked_up_where_it_points() {
        let checkout = Path::new("/work/repo");
        assert_eq!(
            place(
                Path::new("~/specs/x.md"),
                checkout,
                Some(Path::new("/home/me"))
            ),
            SpecPath::Elsewhere("/home/me/specs/x.md".into())
        );
        assert_eq!(
            place(Path::new("../other/./x.md"), checkout, None),
            SpecPath::Elsewhere("/work/other/x.md".into())
        );
        assert_eq!(
            place(Path::new("../../../../x.md"), checkout, None),
            SpecPath::Elsewhere("/x.md".into()),
            "climbing past the root stays at it"
        );
        let launch_of =
            |spec: &str, files: &[&str]| launch(&request(spec, "claude"), files).map(|l| l.source);
        assert_eq!(
            launch_of("~/specs/x.md", &["/home/me/specs/x.md"]),
            Ok(SpecRunSource::File {
                path: "/home/me/specs/x.md".into()
            })
        );
        assert_eq!(
            launch_of("../other/x.md", &["/work/other/x.md"]),
            Ok(SpecRunSource::File {
                path: "/work/other/x.md".into()
            }),
            "the daemon is sent an absolute path, never one relative to the GUI"
        );
        assert_eq!(
            launch_of("docs/../specs/x.md", &["/work/repo/specs/x.md"]),
            Ok(SpecRunSource::File {
                path: "/work/repo/specs/x.md".into()
            }),
        );
    }

    #[test]
    fn another_coder_runs_at_its_own_effort_and_an_unknown_one_is_refused() {
        let launch_for = |coder| launch(&request("Add a health endpoint", coder), &[]);
        let opencode = launch_for("opencode").expect("launch");
        assert_eq!(opencode.coder.provider.as_str(), "opencode");
        assert_eq!(opencode.coder.effort, None);
        assert_eq!(
            launch_for("missing"),
            Err("the coder provider missing is not available".to_owned())
        );
    }

    #[test]
    fn a_missing_spec_file_is_refused_rather_than_sent_as_text() {
        assert_eq!(
            launch(&request("docs/missing.md", "claude"), &[]),
            Err("spec file not found: docs/missing.md".to_owned())
        );
    }

    #[test]
    fn a_typed_branch_the_checkout_and_the_rounds_reach_the_options() {
        let mut asked = request("Add a cache to the session list", "claude");
        asked.branch = Some("  spec/cache ".into());
        asked.rounds = 9;
        asked.publish_spec = true;
        let launch = launch(&asked, &[]).expect("launch");
        assert_eq!(launch.name, "Add a cache to the session");
        assert_eq!(
            launch.source,
            SpecRunSource::Inline {
                text: "Add a cache to the session list".into()
            }
        );
        assert_eq!(
            launch.options.placement,
            Placement::Worktree {
                branch: "spec/cache".into(),
                base: None
            }
        );
        assert_eq!(launch.options.rounds, MAX_ROUNDS, "clamped to the offer");
        assert!(launch.options.publish_spec);

        asked.worktree = false;
        asked.rounds = 0;
        let launch = self::launch(&asked, &[]).expect("launch");
        assert_eq!(launch.options.placement, Placement::Checkout);
        assert_eq!(launch.options.rounds, MIN_ROUNDS);
    }

    #[test]
    fn an_issue_is_a_number_or_an_issue_link() {
        for issue in [
            "#123",
            "123",
            " https://github.com/acme/app/issues/7 ",
            "https://github.com/acme/app/issues/7#issuecomment-1",
        ] {
            assert_eq!(
                issue_reference(issue).as_deref(),
                Some(issue.trim()),
                "{issue}"
            );
        }
        for not in [
            "#12a",
            "fix #12 please",
            "https://github.com/acme/app/pull/7",
            "http://github.com/acme/app/issues/7",
            "https://github.com/acme/app/issues/",
            "#",
        ] {
            assert_eq!(issue_reference(not), None, "{not}");
        }
        let launch = launch(&request("#42", "claude"), &[]).expect("launch");
        assert_eq!(
            launch.source,
            SpecRunSource::Issue {
                reference: "#42".into()
            }
        );
        assert_eq!(launch.name, "issue 42");
        assert_eq!(
            unsuffixed(&launch.options.placement),
            Placement::Worktree {
                branch: "spec/issue-42".into(),
                base: None
            }
        );
    }

    #[test]
    fn links_are_named_by_their_last_segment_and_branches_are_slugs() {
        let named = |url: &str| {
            run_name(&SpecRunSource::Link {
                url: url.to_owned(),
            })
        };
        assert_eq!(named("https://example.com/docs/plan-v2/"), "plan-v2");
        assert_eq!(named("https://example.com"), "spec");
        let branch = default_branch("Add OAuth: login & logout!");
        assert_eq!(
            branch.rsplit_once('-').map(|(slug, _)| slug),
            Some("spec/add-oauth-login-logout")
        );
        assert_ne!(branch, default_branch("Add OAuth: login & logout!"));
        assert!(default_branch("¿?").starts_with("spec/run-"));
    }

    #[test]
    fn profiles_reach_the_planner_and_the_coder() {
        let (work, personal) = (AgentProfileId::new(), AgentProfileId::new());
        let mut asked = request("Add a cache", "claude");
        asked.coder.profile = Some(work);
        asked.planner_profile = Some(personal);
        let launch = launch(&asked, &[]).expect("launch");
        assert_eq!(launch.planner.profile, Some(personal));
        assert_eq!(launch.coder.profile, Some(work));
        assert_eq!(
            launch.coder.effort.as_deref(),
            Some("medium"),
            "a Claude coder keeps the preset's effort under a profile"
        );
    }

    #[test]
    fn rows_offer_every_profile_with_claude_first_and_the_planner_only_claude() {
        use crate::test_support::provider;
        let account = |provider: &str, name: &str| domain::AgentProfile {
            id: AgentProfileId::new(),
            provider_id: AgentProviderId::new(provider),
            name: name.into(),
            executable: None,
            config_dir: None,
            args: vec![],
            created_at: domain::Timestamp::now(),
        };
        let mut store = client::Store::new();
        store.providers = vec![
            provider("opencode", true, true, false),
            provider("claude", true, true, true),
        ];
        let work = account("claude", "Work");
        store.agent_profiles = vec![account("opencode", "Personal"), work.clone()];
        let labels = |rows: Vec<Choice>| rows.into_iter().map(|row| row.label).collect::<Vec<_>>();
        assert_eq!(
            labels(coder_rows(&store)),
            [
                "claude · medium",
                "claude · Work · medium",
                "opencode",
                "opencode · Personal"
            ]
        );
        let planners = planner_rows(&store);
        assert_eq!(labels(planners.clone()), ["claude", "claude · Work"]);

        let remembered = format!("profile:{}", work.id);
        assert_eq!(
            remembered_row(&planners, Some(&remembered)),
            Some(remembered.clone())
        );
        assert_eq!(
            remembered_row(&planners, Some("profile:gone")),
            Some("provider:claude".into()),
            "a forgotten profile falls back to the bare provider"
        );
        store.app_state = vec![(PLANNER_PREF.into(), remembered.clone())];
        let dialog = SpecDialog::new(SpecMode::Draft, WorkspaceId::new(), "spec.md", Some(&store));
        assert_eq!(dialog.planner, Some(remembered));
        let request = dialog.request(&store).expect("a request");
        assert_eq!(request.planner_profile, Some(work.id));
        assert_eq!(request.coder.provider, "claude");
        assert!(request.worktree, "a run gets its own worktree by default");
        assert_eq!(request.branch, None);
    }
}
