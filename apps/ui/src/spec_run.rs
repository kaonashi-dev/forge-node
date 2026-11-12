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
use domain::{AgentDescriptor, AgentProfileId, AgentProviderId, WorkspaceId};
use rune::{El, TextEdit, col, text};
use rune_kit::{Button, ButtonVariant, Kit, SegmentedControl, TextArea, TextInput};

use crate::agent_ops::Choice;
use crate::app::AppState;

/// The planner is Claude, at high effort when it offers it.
const PLANNER: &str = "claude";
/// A Claude coder, under any profile, codes at the `claude` preset's medium
/// effort; any other provider is the `mix` preset with that coder.
const CLAUDE_PRESET: &str = "claude";
/// The last coder and planner rows a run started with.
const CODER_PREF: &str = "ui.spec_run.coder";
const PLANNER_PREF: &str = "ui.spec_run.planner";
pub const DEFAULT_ROUNDS: u32 = 3;
const ROUNDS: [u32; 5] = [1, 2, 3, 4, 5];

/// What the dialog asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecRequest {
    /// A path (relative to the checkout, absolute or `~/`), a link, or the spec's text.
    pub spec: String,
    pub coder: SpecAgent,
    pub planner_profile: Option<AgentProfileId>,
    /// A new worktree on this branch for the coder, instead of the planner's checkout.
    pub worktree: Option<String>,
    pub rounds: u32,
}

/// An agent and the account it runs under.
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
    /// Anywhere else, already expanded.
    Elsewhere(PathBuf),
}

#[derive(Debug, PartialEq, Eq)]
pub struct PlannerLaunch {
    pub provider: AgentProviderId,
    pub profile: Option<AgentProfileId>,
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
    let provider = request.coder.provider.clone();
    let effort = if provider == CLAUDE_PRESET {
        Preset::Claude.coder().1.map(str::to_owned)
    } else {
        None
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
            profile: request.coder.profile.map(|id| id.to_string()),
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
        profile: request.planner_profile,
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
    /// [`Choice`] keys.
    pub coder: Option<String>,
    pub planner: Option<String>,
    pub worktree: TextEdit,
    pub rounds: u32,
    pub starting: bool,
    pub error: Option<String>,
}

impl SpecDialog {
    #[must_use]
    pub fn new(workspace: WorkspaceId, spec: &str, store: Option<&client::Store>) -> Self {
        let mut edit = TextEdit::new(spec);
        edit.focused = true;
        let remembered = |rows: Vec<Choice>, pref: &str| {
            remembered_row(&rows, store.and_then(|store| store.app_state_value(pref)))
        };
        Self {
            workspace,
            spec: edit,
            coder: store.and_then(|store| remembered(coder_rows(store), CODER_PREF)),
            planner: store.and_then(|store| remembered(planner_rows(store), PLANNER_PREF)),
            worktree: TextEdit::new(""),
            rounds: DEFAULT_ROUNDS,
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
            worktree: Some(self.worktree.text.trim().to_owned()).filter(|b| !b.is_empty()),
            rounds: self.rounds,
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

/// The planner is Claude; its rows are Claude and each Claude profile.
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
        self.spec_dialog = Some(SpecDialog::new(workspace, &on_screen, self.store.as_ref()));
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
        "The planner is Claude, and Claude is not installed.",
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
        .child(field("Planner", planner))
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
    let blocked = s
        .store
        .as_ref()
        .and_then(|store| dialog.request(store))
        .is_none();
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
            coder: SpecAgent {
                provider: coder.into(),
                profile: None,
            },
            planner_profile: None,
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

    #[test]
    fn profiles_reach_the_planner_launch_and_the_coder_spawn_by_uuid() {
        let (work, personal) = (AgentProfileId::new(), AgentProfileId::new());
        let mut asked = request("Add a cache", "claude");
        asked.coder.profile = Some(work);
        asked.planner_profile = Some(personal);
        let launch = launch(&asked, &[]).expect("launch");
        assert_eq!(launch.profile, Some(personal));
        assert!(
            launch.brief.contains(&format!(
                "--provider claude --profile {work} --role executor --effort medium"
            )),
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
        let dialog = SpecDialog::new(WorkspaceId::new(), "spec.md", Some(&store));
        assert_eq!(dialog.planner, Some(remembered));
        let request = dialog.request(&store).expect("a request");
        assert_eq!(request.planner_profile, Some(work.id));
        assert_eq!(request.coder.provider, "claude");
    }
}
