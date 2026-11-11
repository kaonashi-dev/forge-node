//! Run Spec…: the planner launch `forgectl spec run` makes, from the palette.
//!
//! The brief is `domain::spec_run::compose_planner_prompt`, as on the command
//! line. Whether a spec path names a file is asked of the daemon when the file
//! is in the checkout (ADR-012), on the connection worker, never here.

use std::path::{Component, Path, PathBuf};

use domain::spec_run::{
    Coder, MAX_INLINE_SPEC_BYTES, Preset, SpecRun, SpecSource, agent_refusal, classify,
    compose_planner_prompt, planner_effort,
};
use domain::{AgentDescriptor, AgentProviderId, WorkspaceId};
use rune::{El, TextEdit, col, text};
use rune_kit::{Button, ButtonVariant, Kit, SegmentedControl, TextArea, TextInput};

use crate::app::AppState;

/// The planner is Claude, at high effort when it offers it.
const PLANNER: &str = "claude";
/// Claude as the coder is the `claude` preset, at medium effort; any other
/// provider is the `mix` preset with that coder.
const CLAUDE_PRESET: &str = "claude";
pub const DEFAULT_ROUNDS: u32 = 3;
const ROUNDS: [u32; 5] = [1, 2, 3, 4, 5];

/// What the dialog asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRequest {
    /// A path (relative to the checkout, absolute or `~/`), a link, or the spec's text.
    pub spec: String,
    pub coder: String,
    /// A new worktree on this branch for the coder, instead of the planner's checkout.
    pub worktree: Option<String>,
    pub rounds: u32,
}

/// Where a spec path points, for the one question asked about it: is it a file?
#[derive(Debug, PartialEq, Eq)]
pub enum SpecPath<'a> {
    /// Relative and inside the checkout: the daemon answers.
    InCheckout(&'a Path),
    /// Anywhere else, already expanded.
    Elsewhere(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlannerLaunch {
    pub provider: AgentProviderId,
    pub effort: Option<String>,
    pub title: String,
    pub brief: String,
}

fn place<'a>(raw: &'a Path, home: Option<&Path>) -> SpecPath<'a> {
    if let (Ok(rest), Some(home)) = (raw.strip_prefix("~"), home) {
        return SpecPath::Elsewhere(home.join(rest));
    }
    let escapes = raw
        .components()
        .any(|part| matches!(part, Component::ParentDir));
    if raw.is_absolute() || escapes {
        SpecPath::Elsewhere(raw.to_path_buf())
    } else {
        SpecPath::InCheckout(raw)
    }
}

/// The planner launch for `request` in `checkout`, or why it would fail.
///
/// # Errors
/// A sentence for the person: an empty spec, a path that is not a file, a
/// pasted spec over the cap, or a planner or coder the providers cannot run.
pub fn planner_launch(
    providers: &[AgentDescriptor],
    checkout: &Path,
    request: &SpecRequest,
    home: Option<&Path>,
    forgectl: PathBuf,
    is_file: impl Fn(SpecPath<'_>) -> bool,
) -> Result<PlannerLaunch, String> {
    let source = match classify(&request.spec, |raw| is_file(place(raw, home)))
        .map_err(|error| error.to_string())?
    {
        SpecSource::File(raw) => SpecSource::File(match place(&raw, home) {
            SpecPath::InCheckout(relative) => checkout.join(relative),
            SpecPath::Elsewhere(path) => path,
        }),
        SpecSource::Inline(text) if text.len() > MAX_INLINE_SPEC_BYTES => {
            return Err(format!(
                "a pasted spec is capped at {} KiB; save it to a file",
                MAX_INLINE_SPEC_BYTES / 1024
            ));
        }
        other => other,
    };
    let (provider, effort) = if request.coder == CLAUDE_PRESET {
        let (provider, effort) = Preset::Claude.coder();
        (provider.to_owned(), effort.map(str::to_owned))
    } else {
        (request.coder.clone(), None)
    };
    check(providers, &provider, effort.as_deref(), "coder")?;
    let planner = descriptor(providers, PLANNER, "planner")?;
    let planner_effort = planner_effort(planner);
    check(providers, PLANNER, planner_effort.as_deref(), "planner")?;
    let name = match &source {
        SpecSource::File(path) => path.file_stem().map_or_else(
            || "spec".to_owned(),
            |stem| stem.to_string_lossy().into_owned(),
        ),
        _ => "spec".to_owned(),
    };
    let run = SpecRun {
        source,
        coder: Coder {
            provider,
            effort,
            model: None,
        },
        forgectl,
        name: name.clone(),
        worktree: request
            .worktree
            .as_deref()
            .map(str::trim)
            .filter(|branch| !branch.is_empty())
            .map(str::to_owned),
        base: None,
        rounds: request
            .rounds
            .clamp(1, *ROUNDS.last().unwrap_or(&DEFAULT_ROUNDS)),
        planner_is_session: true,
    };
    Ok(PlannerLaunch {
        provider: planner.id.clone(),
        effort: planner_effort,
        title: format!("{name} planner"),
        brief: compose_planner_prompt(&run),
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

/// The `forgectl` the planner's brief names. Beside this binary in a bundle,
/// else beside the daemon; a bare name still works in a session, whose `PATH`
/// the daemon leads with its own directory.
#[must_use]
pub fn forgectl() -> PathBuf {
    let beside = |exe: Option<PathBuf>| {
        exe.map(|exe| exe.with_file_name("forgectl"))
            .filter(|path| path.is_file())
    };
    beside(std::env::current_exe().ok())
        .or_else(|| beside(std::env::var_os("FORGE_DAEMON_BIN").map(PathBuf::from)))
        .unwrap_or_else(|| PathBuf::from("forgectl"))
}

pub struct SpecDialog {
    pub workspace: WorkspaceId,
    pub spec: TextEdit,
    pub coder: String,
    pub worktree: TextEdit,
    pub rounds: u32,
    pub starting: bool,
    pub error: Option<String>,
}

impl SpecDialog {
    #[must_use]
    pub fn new(workspace: WorkspaceId, spec: &str) -> Self {
        let mut edit = TextEdit::new(spec);
        edit.focused = true;
        Self {
            workspace,
            spec: edit,
            coder: CLAUDE_PRESET.to_owned(),
            worktree: TextEdit::new(""),
            rounds: DEFAULT_ROUNDS,
            starting: false,
            error: None,
        }
    }

    #[must_use]
    pub fn request(&self) -> Option<SpecRequest> {
        let spec = self.spec.text.trim();
        (!spec.is_empty() && !self.starting).then(|| SpecRequest {
            spec: spec.to_owned(),
            coder: self.coder.clone(),
            worktree: Some(self.worktree.text.trim().to_owned()).filter(|b| !b.is_empty()),
            rounds: self.rounds,
        })
    }
}

/// The coders on offer: one row per installed provider that takes a prompt,
/// Claude first. The planner launches the coder by provider, never by profile.
#[must_use]
pub fn coders(store: &client::Store) -> Vec<(String, String)> {
    let mut rows: Vec<(String, String)> = store
        .providers
        .iter()
        .filter(|info| {
            matches!(
                info.detection.status,
                domain::DetectionStatus::Installed { .. }
            ) && info.descriptor.capabilities.supports_initial_prompt
        })
        .map(|info| {
            let id = info.descriptor.id.to_string();
            let label = if id == CLAUDE_PRESET {
                format!("{} · medium", info.descriptor.display_name)
            } else {
                info.descriptor.display_name.clone()
            };
            (id, label)
        })
        .collect();
    rows.sort_by_key(|(id, _)| id != CLAUDE_PRESET);
    rows
}

impl AppState {
    /// Opens the dialog for the window's checkout, with the Markdown file on
    /// screen already filled in.
    pub fn open_run_spec(&mut self) {
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
                Path::new(path)
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| matches!(ext, "md" | "markdown" | "mdx"))
            })
            .unwrap_or_default();
        self.spec_dialog = Some(SpecDialog::new(workspace, &on_screen));
    }

    pub fn start_spec_run(&mut self) {
        let Some(dialog) = self.spec_dialog.as_mut() else {
            return;
        };
        let Some(request) = dialog.request() else {
            return;
        };
        let workspace = dialog.workspace;
        let Some(store) = &self.store else {
            return;
        };
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
                dialog.starting = true;
                dialog.error = None;
            }
            Err(error) => dialog.error = Some(error),
        }
    }

    /// The daemon's answer to Run Spec: the planner opens as a tab, or the
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
    let offered = s.store.as_ref().map(coders).unwrap_or_default();
    let coder = if offered.is_empty() {
        text("No installed agent takes a prompt.")
            .font(k.ty.text_xsmall)
            .color(k.c.warning.fg)
    } else {
        let labels: Vec<&str> = offered.iter().map(|(_, label)| label.as_str()).collect();
        let picks: Vec<String> = offered.iter().map(|(id, _)| id.clone()).collect();
        SegmentedControl::new(
            &labels,
            offered.iter().position(|(id, _)| *id == dialog.coder),
        )
        .label("Coder")
        .width(None)
        .on_pick(move |s: &mut AppState, index: usize, _| {
            if let (Some(dialog), Some(id)) = (s.spec_dialog.as_mut(), picks.get(index)) {
                dialog.coder.clone_from(id);
            }
        })
        .build(k)
        .key("spec-coders")
    };
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
    let note = dialog.error.clone().unwrap_or_else(|| {
        "A Claude planner validates the spec, asks what it cannot answer, plans, starts the \
         coder under itself and reviews its work. It never commits or pushes."
            .into()
    });
    let body = col()
        .w_full()
        .gap(crate::ds::SPACE_6)
        .child(field(
            "Spec",
            TextArea::new(&dialog.spec)
                .label("Spec")
                .placeholder("docs/spec.md, a Claude artifact link, or the spec itself")
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_dialog.as_mut() {
                    Some(dialog) => &mut dialog.spec,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-text")
                .h(96.0),
        ))
        .child(field("Coder", coder))
        .child(field(
            "Worktree branch",
            TextInput::new(&dialog.worktree)
                .label("Worktree branch")
                .placeholder("Empty: the coder works in this checkout")
                .width(None)
                .on_edit(|s: &mut AppState| match s.spec_dialog.as_mut() {
                    Some(dialog) => &mut dialog.worktree,
                    None => &mut s.scratch_edit,
                })
                .build(k)
                .key("spec-worktree"),
        ))
        .child(field("Review rounds", rounds))
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
    let blocked = dialog.request().is_none() || offered.is_empty();
    crate::modal::Modal::new("run-spec", "Run a spec")
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
                "Start planner"
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
            coder: coder.into(),
            worktree: None,
            rounds: DEFAULT_ROUNDS,
        }
    }

    fn launch(request: &SpecRequest, files: &[&str]) -> Result<PlannerLaunch, String> {
        planner_launch(
            &providers(),
            Path::new("/work/repo"),
            request,
            Some(Path::new("/home/me")),
            PathBuf::from("forgectl"),
            |path| match path {
                SpecPath::InCheckout(relative) => files.contains(&relative.to_str().unwrap_or("")),
                SpecPath::Elsewhere(path) => files.contains(&path.to_str().unwrap_or("")),
            },
        )
    }

    #[test]
    fn a_checkout_path_is_asked_of_the_daemon_and_names_the_planner() {
        let launch = launch(
            &request("docs/auth-flow.md", "claude"),
            &["docs/auth-flow.md"],
        )
        .expect("launch");
        assert_eq!(launch.provider.as_str(), "claude");
        assert_eq!(launch.effort.as_deref(), Some("high"));
        assert_eq!(launch.title, "auth-flow planner");
        assert!(
            launch
                .brief
                .contains("The spec is the file /work/repo/docs/auth-flow.md")
        );
        assert!(
            launch
                .brief
                .contains("--provider claude --role executor --effort medium --parent self")
        );
    }

    #[test]
    fn a_home_or_escaping_path_is_looked_up_where_it_points() {
        assert_eq!(
            place(Path::new("~/specs/x.md"), Some(Path::new("/home/me"))),
            SpecPath::Elsewhere("/home/me/specs/x.md".into())
        );
        assert_eq!(
            place(Path::new("../other/x.md"), None),
            SpecPath::Elsewhere("../other/x.md".into())
        );
        let launch = launch(&request("~/specs/x.md", "claude"), &["/home/me/specs/x.md"]);
        assert!(
            launch
                .expect("launch")
                .brief
                .contains("The spec is the file /home/me/specs/x.md")
        );
    }

    #[test]
    fn another_coder_is_the_mix_preset_and_an_unknown_one_is_refused() {
        let brief = launch(&request("Add a health endpoint", "opencode"), &[])
            .expect("launch")
            .brief;
        assert!(brief.contains("--provider opencode --role executor --parent self"));
        assert_eq!(
            launch(&request("Add a health endpoint", "missing"), &[]),
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
    fn a_worktree_and_the_rounds_reach_the_brief() {
        let mut asked = request("Add a cache", "claude");
        asked.worktree = Some("  spec/cache ".into());
        asked.rounds = 9;
        let brief = launch(&asked, &[]).expect("launch").brief;
        assert!(brief.contains("--worktree spec/cache"));
        assert!(
            brief.contains("After 5 review round(s)"),
            "clamped to the offer"
        );
    }
}
