//! Read and write the subagent harness state under `<project>/harness/`.
//!
//! The daemon is the only caller; the GUI never opens these paths itself
//! (ADR-012). Paths are resolved from the project's *repository* root
//! (`Daemon::harness_root_for`), not from an isolated worktree checkout and
//! not from the directory the project happens to be registered at — that one
//! may sit below the repository root and hold no `harness/` at all.
//!
//! # One state machine per repository, several features at once
//!
//! A worktree is a second checkout of the same repository, so every checkout
//! of a project shares this one `features.json` and one id sequence — which
//! is why the path comes from the project root and never from the workspace
//! the agent happens to run in. Concurrency is expressed the other way round:
//! each feature records the checkout it occupies
//! ([`HarnessFeature::workspace_id`]), and [`register_feature`] refuses a
//! second *open* feature in a checkout that already has one. Two features in
//! two different worktrees of the same project are therefore fine; two in the
//! same worktree are not, because they would be editing the same files.
//!
//! The per-feature artefacts (`specs/<id>-<slug>/`, `progress/*_<id>.md`,
//! `progress/events_<id>.jsonl`) are already keyed by id and need no further
//! separation.

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

use domain::{
    HarnessAdvanceAction, HarnessArtifactKind, HarnessEvent, HarnessFeature, HarnessFeatureList,
    HarnessStep, SessionId, WorkspaceId,
};
use nix::fcntl::{Flock, FlockArg};
use serde::Deserialize;
use thiserror::Error;

pub mod transition;

pub use transition::{
    allowed_from, attempts_of, live_attempt, step_of_status, transition, HarnessRules,
    HarnessTrigger, Transitioned,
};

const FEATURES: &str = "harness/features.json";
const FEATURES_TMP: &str = "features.json.tmp";
const FEATURES_LOCK: &str = "features.json.lock";
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_POLL: Duration = Duration::from_millis(10);

static PROCESS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Debug, Error)]
pub enum HarnessError {
    #[error("harness not initialized in this repository")]
    NotInitialized,
    #[error("feature {0} not found")]
    NotFound(u32),
    #[error("invalid harness state: {0}")]
    Invalid(String),
    #[error("{0}")]
    Conflict(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// `features.json` as parsed for editing.
///
/// The daemon owns exactly two things in that document — the feature rows and,
/// for display, the project name. Everything else in it (`description`, and
/// above all `rules`, which the harness CLI validates against) belongs to the
/// repository, so the whole parsed document is carried along in `raw` and
/// given back on the next write. Modelling only the known keys and
/// reserialising the struct would silently delete the rest: registering one
/// feature from the GUI used to wipe `rules` and leave `scripts/harness gate`
/// failing on a `valid_status` that no longer existed.
#[derive(Clone, Debug)]
struct FeaturesFile {
    features: Vec<HarnessFeature>,
    project: Option<String>,
    /// The document as it was read, kept so a write can put back every key
    /// this crate does not model — per feature as well as at the top level.
    raw: serde_json::Value,
}

struct LockedFeaturesFile {
    path: PathBuf,
    file: FeaturesFile,
    _process_guard: MutexGuard<'static, ()>,
    _file_lock: Option<FeaturesFileLock>,
}

struct FeaturesFileLock {
    _lock: Flock<File>,
}

impl LockedFeaturesFile {
    fn save(&self) -> Result<(), HarnessError> {
        save(&self.path, &self.file)
    }
}

impl FeaturesFile {
    fn parse(raw: &str) -> Result<Self, HarnessError> {
        let raw: serde_json::Value = serde_json::from_str(raw)?;
        let features = match raw.get("features") {
            Some(value) => serde_json::from_value(value.clone())?,
            None => Vec::new(),
        };
        let project = raw
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        Ok(Self {
            features,
            project,
            raw,
        })
    }

    /// The document to write: `raw` with the feature rows folded back in.
    ///
    /// Each row is merged *over* the object it came from rather than replacing
    /// it, so a key this crate does not know about survives a status change.
    fn to_document(&self) -> Result<serde_json::Value, HarnessError> {
        let mut doc = match self.raw.clone() {
            serde_json::Value::Object(map) => map,
            _ => serde_json::Map::new(),
        };
        let previous: Vec<serde_json::Value> = doc
            .get("features")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut rows = Vec::with_capacity(self.features.len());
        for feature in &self.features {
            let mut row = previous
                .iter()
                .find(|row| {
                    row.get("id").and_then(serde_json::Value::as_u64) == Some(feature.id.into())
                })
                .and_then(|row| row.as_object().cloned())
                .unwrap_or_default();
            let serde_json::Value::Object(known) = serde_json::to_value(feature)? else {
                unreachable!("a struct serializes as an object");
            };
            row.extend(known);
            rows.push(serde_json::Value::Object(row));
        }
        doc.insert("features".into(), serde_json::Value::Array(rows));
        Ok(serde_json::Value::Object(doc))
    }
}

fn harness_root(project_root: &Path) -> PathBuf {
    project_root.join("harness")
}

fn features_path(project_root: &Path) -> PathBuf {
    project_root.join(FEATURES)
}

fn progress_dir(project_root: &Path) -> PathBuf {
    harness_root(project_root).join("progress")
}

fn specs_dir(project_root: &Path) -> PathBuf {
    harness_root(project_root).join("specs")
}

pub fn is_initialized(project_root: &Path) -> bool {
    features_path(project_root).is_file()
}

pub fn list_features(project_root: &Path) -> Result<HarnessFeatureList, HarnessError> {
    let path = features_path(project_root);
    if !path.is_file() {
        return Ok(HarnessFeatureList {
            initialized: false,
            ..HarnessFeatureList::default()
        });
    }
    let raw = fs::read_to_string(path)?;
    let file = FeaturesFile::parse(&raw)?;
    Ok(HarnessFeatureList {
        project: file.project,
        features: file.features,
        initialized: true,
    })
}

fn load_mut(project_root: &Path) -> Result<LockedFeaturesFile, HarnessError> {
    let path = features_path(project_root);
    if !path.is_file() {
        return Err(HarnessError::NotInitialized);
    }
    let process_guard = PROCESS_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let file_lock = acquire_features_lock(&path)?;
    let raw = fs::read_to_string(&path)?;
    Ok(LockedFeaturesFile {
        path,
        file: FeaturesFile::parse(&raw)?,
        _process_guard: process_guard,
        _file_lock: file_lock,
    })
}

fn acquire_features_lock(path: &Path) -> Result<Option<FeaturesFileLock>, HarnessError> {
    let lock_path = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(FEATURES_LOCK);
    let mut file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock_path)?;
    let deadline = Instant::now() + LOCK_WAIT;
    loop {
        match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => return Ok(Some(FeaturesFileLock { _lock: lock })),
            Err((returned, nix::errno::Errno::EWOULDBLOCK)) => {
                file = returned;
                if Instant::now() >= deadline {
                    tracing::warn!(
                        path = %lock_path.display(),
                        "could not acquire harness features lock before deadline; writing without the file lock"
                    );
                    return Ok(None);
                }
                std::thread::sleep(LOCK_POLL);
            }
            Err((_returned, error)) => return Err(HarnessError::Io(std::io::Error::from(error))),
        }
    }
}

fn save(path: &Path, file: &FeaturesFile) -> Result<(), HarnessError> {
    let text = serde_json::to_string_pretty(&file.to_document()?)?;
    let tmp = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(FEATURES_TMP);
    {
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp)?;
        out.write_all(text.as_bytes())?;
        out.write_all(b"\n")?;
        out.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    if let Some(parent) = path.parent() {
        if let Ok(dir) = File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

fn feature_mut(file: &mut FeaturesFile, id: u32) -> Result<&mut HarnessFeature, HarnessError> {
    file.features
        .iter_mut()
        .find(|f| f.id == id)
        .ok_or(HarnessError::NotFound(id))
}

pub fn get_feature(project_root: &Path, id: u32) -> Result<HarnessFeature, HarnessError> {
    let list = list_features(project_root)?;
    list.features
        .into_iter()
        .find(|f| f.id == id)
        .ok_or(HarnessError::NotFound(id))
}

pub fn read_timeline(project_root: &Path, id: u32) -> Result<Vec<HarnessEvent>, HarnessError> {
    let path = progress_dir(project_root).join(format!("events_{id}.jsonl"));
    if !path.is_file() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_str(line)?;
        let ts = value
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let kind = value
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let mut detail_obj = value.as_object().cloned().unwrap_or_default();
        detail_obj.remove("ts");
        detail_obj.remove("type");
        let job = detail_obj
            .get("job")
            .and_then(|v| v.as_str())
            .and_then(|id| id.parse().ok());
        let detail = if detail_obj.is_empty() {
            String::new()
        } else {
            serde_json::to_string(&detail_obj)?
        };
        out.push(HarnessEvent {
            ts,
            kind,
            detail,
            job,
        });
    }
    Ok(out)
}

/// The last step this feature entered, and whether anything settled it.
///
/// Read from the event log rather than from `status`, because status cannot
/// answer the question: a spec step runs while the feature is still `pending`,
/// which is also what a feature nobody has started looks like. Only the log
/// distinguishes "a step was entered and never came back" from "nothing has
/// happened yet".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StepTrace {
    /// The `*_started` event that has no matching settling event after it.
    pub unsettled: Option<StartedStep>,
    /// The kind of the last event of any sort, for display.
    pub last_kind: String,
    /// Its timestamp.
    pub last_ts: String,
}

/// A step that was entered, as its start event recorded it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StartedStep {
    /// `spec_started`, `impl_started`, `review_started`.
    pub kind: String,
    pub ts: String,
    /// The job that ran it, when the event names one. Older logs do not.
    pub job_id: Option<String>,
    pub provider: Option<String>,
}

/// Events that close whatever step was open.
const SETTLING: &[&str] = &[
    "human_gate_opened",
    "impl_done",
    "review_verdict",
    "feature_done",
    "feature_blocked",
];

/// Read the event log and say what, if anything, is still in flight.
pub fn read_step_trace(project_root: &Path, id: u32) -> Result<StepTrace, HarnessError> {
    let path = progress_dir(project_root).join(format!("events_{id}.jsonl"));
    if !path.is_file() {
        return Ok(StepTrace::default());
    }
    let mut trace = StepTrace::default();
    for line in fs::read_to_string(path)?.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = value.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let ts = value
            .get("ts")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        trace.last_kind = kind.to_owned();
        trace.last_ts = ts.clone();
        if kind.ends_with("_started") {
            trace.unsettled = Some(StartedStep {
                kind: kind.to_owned(),
                ts,
                job_id: value
                    .get("job")
                    .or_else(|| value.get("job_id"))
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
                provider: value
                    .get("provider")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned),
            });
        } else if SETTLING.contains(&kind) {
            trace.unsettled = None;
        }
    }
    Ok(trace)
}

pub fn append_event(
    project_root: &Path,
    id: u32,
    kind: &str,
    data: serde_json::Map<String, serde_json::Value>,
) -> Result<(), HarnessError> {
    let progress = progress_dir(project_root);
    fs::create_dir_all(&progress)?;
    let path = progress.join(format!("events_{id}.jsonl"));
    let ts = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    let mut row = serde_json::Map::new();
    row.insert("ts".into(), ts.into());
    row.insert("type".into(), kind.into());
    for (k, v) in data {
        row.insert(k, v);
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(file, "{}", serde_json::to_string(&row)?)?;
    Ok(())
}

pub fn read_artifact(
    project_root: &Path,
    id: u32,
    kind: HarnessArtifactKind,
) -> Result<String, HarnessError> {
    let feature = get_feature(project_root, id)?;
    let path = match kind {
        HarnessArtifactKind::Gate => progress_dir(project_root).join(format!("gate_{id}.md")),
        HarnessArtifactKind::Context => progress_dir(project_root).join(format!("context_{id}.md")),
        HarnessArtifactKind::Impl => progress_dir(project_root).join(format!("impl_{id}.md")),
        HarnessArtifactKind::Review => progress_dir(project_root).join(format!("review_{id}.md")),
        // Per feature, not per repository: two features running in two
        // worktrees would otherwise overwrite each other's scratch note. The
        // shared file is still read when no per-feature one exists, so state
        // written before the split is not lost.
        HarnessArtifactKind::Current => {
            let per_feature = progress_dir(project_root).join(format!("current_{id}.md"));
            if per_feature.is_file() {
                per_feature
            } else {
                progress_dir(project_root).join("current.md")
            }
        }
        HarnessArtifactKind::Requirements => specs_dir(project_root)
            .join(format!("{}-{}", id, feature.slug))
            .join("requirements.md"),
        HarnessArtifactKind::Design => specs_dir(project_root)
            .join(format!("{}-{}", id, feature.slug))
            .join("design.md"),
        HarnessArtifactKind::Tasks => specs_dir(project_root)
            .join(format!("{}-{}", id, feature.slug))
            .join("tasks.md"),
        HarnessArtifactKind::Unknown => {
            return Err(HarnessError::Invalid("unknown artifact kind".into()));
        }
        _ => {
            return Err(HarnessError::Invalid("unknown artifact kind".into()));
        }
    };
    if !path.is_file() {
        return Ok(String::new());
    }
    Ok(fs::read_to_string(path)?)
}

pub struct RegisterInput {
    pub spec_raw: String,
    pub title: Option<String>,
    pub source_issue: Option<u32>,
    /// The checkout the feature will be implemented in, when Forge knows it.
    pub workspace_id: Option<WorkspaceId>,
    /// That checkout's path, recorded for whoever reads the file by hand.
    pub workspace_path: Option<String>,
}

/// The open feature already occupying `workspace`, if there is one.
///
/// Only [`HarnessFeature::is_open`] rows count: a checkout whose last feature
/// is `done` or `blocked` is free again.
fn open_feature_in(file: &FeaturesFile, workspace: WorkspaceId) -> Option<&HarnessFeature> {
    file.features
        .iter()
        .find(|f| f.workspace_id == Some(workspace) && f.is_open())
}

pub fn register_feature(
    project_root: &Path,
    input: RegisterInput,
) -> Result<HarnessFeature, HarnessError> {
    if !is_initialized(project_root) {
        return Err(HarnessError::NotInitialized);
    }
    let mut locked = load_mut(project_root)?;
    // One open feature per checkout, not per repository: the limit exists
    // because two features in one working tree would edit the same files,
    // which says nothing about a second worktree.
    if let Some(workspace) = input.workspace_id {
        if let Some(busy) = open_feature_in(&locked.file, workspace) {
            return Err(HarnessError::Conflict(format!(
                "feature {} ({}) is still open in this checkout — finish or block it, \
                 or start the new one in another worktree",
                busy.id, busy.slug
            )));
        }
    }
    let id = locked.file.features.iter().map(|f| f.id).max().unwrap_or(0) + 1;
    let slug = slug_from_title(
        input.title.as_deref().unwrap_or(&input.spec_raw),
        id,
        input.source_issue,
    );
    let feature = HarnessFeature {
        id,
        slug,
        title: input.title,
        spec_raw: Some(input.spec_raw),
        status: "pending".into(),
        review_rounds: Some(0),
        gate_attempts: Some(0),
        source_issue: input.source_issue,
        created_at: Some(time::OffsetDateTime::now_utc().date().to_string()),
        workspace_id: input.workspace_id,
        workspace_path: input.workspace_path,
        ..HarnessFeature::default()
    };
    locked.file.features.push(feature.clone());
    locked.save()?;
    let mut data = serde_json::Map::new();
    data.insert("slug".into(), feature.slug.clone().into());
    if let Some(title) = &feature.title {
        data.insert("title".into(), title.clone().into());
    }
    append_event(project_root, id, "feature_registered", data)?;
    Ok(feature)
}

fn slug_from_title(title: &str, id: u32, issue: Option<u32>) -> String {
    if let Some(n) = issue {
        return format!("issue-{n}");
    }
    let slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let slug = slug.trim_matches('-').chars().take(48).collect::<String>();
    if slug.is_empty() {
        format!("feature-{id}")
    } else {
        slug
    }
}

pub fn link_orchestrator(
    project_root: &Path,
    id: u32,
    session_id: SessionId,
) -> Result<(), HarnessError> {
    let mut locked = load_mut(project_root)?;
    let feature = feature_mut(&mut locked.file, id)?;
    feature.orchestrator_session_id = Some(session_id);
    locked.save()
}

/// The rules `features.json` states for this repository.
pub fn rules_of(project_root: &Path) -> Result<HarnessRules, HarnessError> {
    let path = features_path(project_root);
    if !path.is_file() {
        return Ok(HarnessRules::default());
    }
    let file = FeaturesFile::parse(&fs::read_to_string(path)?)?;
    Ok(HarnessRules::from_json(file.raw.get("rules")))
}

/// The outcome of a decision that has already been written.
#[derive(Clone, Debug)]
pub struct Applied {
    /// The row as it now stands on disk.
    pub feature: HarnessFeature,
    /// The step the coordinator must start next, if any.
    pub next_step: Option<HarnessStep>,
    /// `false` when the caller's `revision` was stale and nothing was written.
    pub applied: bool,
}

/// Apply one trigger to one feature, atomically.
///
/// The only writer of `status` in this crate. Loads under the advisory lock,
/// runs the pure [`transition`] table, writes the row and appends the events it
/// produced — in that order, so a crash leaves a row whose events are behind
/// rather than a trail that describes a state nothing reached.
///
/// `expected_revision` is Orca's idempotent mutation: a caller that read the
/// row at revision *n* and decided on it passes *n*, and a row that has moved
/// on since answers with its current self and does nothing. `None` means "I did
/// not look" and always applies, which is what the coordinator's own triggers
/// use — they are reacting to a job that just ended, not to a stale view.
///
/// # Errors
///
/// [`HarnessError::Invalid`] when the transition table refuses the trigger,
/// [`HarnessError::NotFound`] for an unknown feature, and the IO and JSON
/// errors of reading and writing the state.
pub fn apply(
    project_root: &Path,
    id: u32,
    expected_revision: Option<u64>,
    trigger: &HarnessTrigger,
) -> Result<Applied, HarnessError> {
    let mut locked = load_mut(project_root)?;
    let rules = HarnessRules::from_json(locked.file.raw.get("rules"));
    let current = feature_mut(&mut locked.file, id)?;
    if let Some(expected) = expected_revision {
        if current.revision.unwrap_or(0) != expected {
            let feature = current.clone();
            return Ok(Applied {
                feature,
                next_step: None,
                applied: false,
            });
        }
    }
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned());
    let decided = transition(current, &rules, trigger, &now)?;
    *current = decided.feature.clone();
    // Events first, row second. `validate.ts` reads the trail as the
    // justification for the status — "`done` needs a `review_verdict`" — so a
    // crash between the two must leave the trail *ahead* of the row, which
    // validates, and never behind it, which does not. It is also what a client
    // polling the status assumes: by the time the row says `done`, the event
    // that says why is already there.
    for (kind, data) in decided.events {
        append_event(project_root, id, &kind, data)?;
    }
    locked.save()?;
    Ok(Applied {
        feature: decided.feature,
        next_step: decided.next_step,
        applied: true,
    })
}

/// Record the provider session id on the live attempt that names this job.
///
/// The stream names the id after `StartStep` wrote the attempt; without this
/// patch a RetryStep has nothing to pass as `resume_from`.
pub fn attach_provider_session(
    project_root: &Path,
    feature_id: u32,
    job: &str,
    provider_session_id: &str,
) -> Result<(), HarnessError> {
    let mut locked = load_mut(project_root)?;
    let current = feature_mut(&mut locked.file, feature_id)?;
    let Some(attempts) = current.attempts.as_mut() else {
        return Ok(());
    };
    let Some(attempt) = attempts
        .iter_mut()
        .rev()
        .find(|attempt| attempt.job.as_deref() == Some(job))
    else {
        return Ok(());
    };
    if attempt.provider_session_id.as_deref() == Some(provider_session_id) {
        return Ok(());
    }
    attempt.provider_session_id = Some(provider_session_id.to_owned());
    current.revision = Some(current.revision.unwrap_or(0) + 1);
    locked.save()?;
    Ok(())
}

/// A human decision from a client, as one [`HarnessTrigger`].
///
/// The wire vocabulary and the table's vocabulary are deliberately separate:
/// `StartImplement` and `StartReview` are older spellings of "run this step",
/// and they now go through the same gate as everything else rather than
/// setting a status by hand.
pub fn advance(
    project_root: &Path,
    id: u32,
    revision: Option<u64>,
    action: HarnessAdvanceAction,
) -> Result<Applied, HarnessError> {
    let trigger = match action {
        HarnessAdvanceAction::ApproveSpec => HarnessTrigger::ApproveSpec,
        HarnessAdvanceAction::ReviseSpec => HarnessTrigger::ReviseSpec { reason: None },
        HarnessAdvanceAction::Block { reason } => HarnessTrigger::Block { reason },
        HarnessAdvanceAction::RetryStep => HarnessTrigger::RetryStep,
        HarnessAdvanceAction::Reopen => HarnessTrigger::Reopen,
        HarnessAdvanceAction::StartImplement => HarnessTrigger::StartStep {
            step: HarnessStep::Implement,
            job: None,
            provider: None,
            transport: None,
            force: false,
        },
        HarnessAdvanceAction::StartReview => HarnessTrigger::StartStep {
            step: HarnessStep::Review,
            job: None,
            provider: None,
            transport: None,
            force: false,
        },
        _ => {
            return Err(HarnessError::Invalid(
                "unsupported harness advance action".into(),
            ));
        }
    };
    apply(project_root, id, revision, &trigger)
}

/// Runs `bun harness/src/validate.ts` when present; returns stdout+stderr.
pub fn run_validate(project_root: &Path) -> Result<String, HarnessError> {
    let script = project_root.join("harness/src/validate.ts");
    if !script.is_file() {
        return Err(HarnessError::NotInitialized);
    }
    let output = std::process::Command::new("bun")
        .arg(script)
        .current_dir(project_root)
        .output()?;
    let text = String::from_utf8_lossy(&output.stdout).into_owned()
        + &String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        Ok(text)
    } else {
        Err(HarnessError::Invalid(text))
    }
}

#[derive(Debug, Deserialize)]
struct GhIssue {
    title: String,
    body: Option<String>,
    number: u32,
}

pub fn register_from_issue(
    project_root: &Path,
    issue_number: u32,
    workspace_id: Option<WorkspaceId>,
    workspace_path: Option<String>,
) -> Result<HarnessFeature, HarnessError> {
    let output = std::process::Command::new("gh")
        .args([
            "issue",
            "view",
            &issue_number.to_string(),
            "--json",
            "title,body,number",
        ])
        .current_dir(project_root)
        .output()
        .map_err(|e| HarnessError::Invalid(format!("gh not available: {e}")))?;
    if !output.status.success() {
        return Err(HarnessError::Invalid(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    let issue: GhIssue = serde_json::from_slice(&output.stdout)?;
    let spec_raw = issue
        .body
        .filter(|b| !b.trim().is_empty())
        .unwrap_or_else(|| issue.title.clone());
    let feature = register_feature(
        project_root,
        RegisterInput {
            spec_raw,
            title: Some(issue.title),
            source_issue: Some(issue.number),
            workspace_id,
            workspace_path,
        },
    )?;
    let mut data = serde_json::Map::new();
    data.insert("issue".into(), issue.number.into());
    append_event(project_root, feature.id, "feature_from_issue", data)?;
    Ok(feature)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn register_and_timeline() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join(FEATURES),
            r#"{"project":"t","features":[]}"#,
        )
        .unwrap();
        let f = register_feature(
            dir.path(),
            RegisterInput {
                spec_raw: "do the thing".into(),
                title: Some("Do Thing".into()),
                source_issue: None,
                workspace_id: None,
                workspace_path: None,
            },
        )
        .unwrap();
        assert_eq!(f.id, 1);
        let events = read_timeline(dir.path(), 1).unwrap();
        assert!(!events.is_empty());
    }

    /// The step's job id is lifted out of the event, because it is what still
    /// points at a transcript after the daemon that ran it is gone.
    #[test]
    fn a_started_event_carries_the_job_it_started() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join("harness/progress/events_3.jsonl"),
            "{\"ts\":\"t\",\"type\":\"spec_started\",\"job\":\"01a04ee6-1b6a-7682-be7f-3ca103a23ef0\"}\n\
             {\"ts\":\"t\",\"type\":\"human_gate_opened\",\"gate\":\"spec_approval\"}\n",
        )
        .unwrap();
        let events = read_timeline(dir.path(), 3).unwrap();
        assert_eq!(
            events[0].job.map(|id| id.to_string()).as_deref(),
            Some("01a04ee6-1b6a-7682-be7f-3ca103a23ef0")
        );
        assert!(events[0].detail.contains("job"));
        assert_eq!(events[1].job, None);

        let trace = read_step_trace(dir.path(), 3).unwrap();
        assert_eq!(trace.unsettled, None);
    }

    #[test]
    fn step_trace_reads_the_job_key_the_runner_writes() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join("harness/progress/events_3.jsonl"),
            "{\"ts\":\"t\",\"type\":\"impl_started\",\"job\":\"01a04ee6-1b6a-7682-be7f-3ca103a23ef0\",\"provider\":\"claude\"}\n",
        )
        .unwrap();

        let trace = read_step_trace(dir.path(), 3).unwrap();
        let unsettled = trace.unsettled.unwrap();
        assert_eq!(unsettled.kind, "impl_started");
        assert_eq!(
            unsettled.job_id.as_deref(),
            Some("01a04ee6-1b6a-7682-be7f-3ca103a23ef0")
        );
        assert_eq!(unsettled.provider.as_deref(), Some("claude"));
    }

    /// A repository with two worktrees: each checkout may hold one open
    /// feature, and the second attempt in an occupied one is refused rather
    /// than silently queued behind the first.
    #[test]
    fn one_open_feature_per_checkout_but_several_per_repository() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join(FEATURES),
            r#"{"project":"t","features":[]}"#,
        )
        .unwrap();
        let main = WorkspaceId::new();
        let worktree = WorkspaceId::new();
        let input = |workspace: WorkspaceId| RegisterInput {
            spec_raw: "do the thing".into(),
            title: Some("Do Thing".into()),
            source_issue: None,
            workspace_id: Some(workspace),
            workspace_path: Some("/tmp/checkout".into()),
        };

        let first = register_feature(dir.path(), input(main)).unwrap();
        assert_eq!(first.workspace_id, Some(main));

        // Another worktree of the same repository: allowed, and it shares the
        // id sequence rather than starting its own.
        let second = register_feature(dir.path(), input(worktree)).unwrap();
        assert_eq!(second.id, first.id + 1);

        // The checkout the first one holds: refused.
        let err = register_feature(dir.path(), input(main)).unwrap_err();
        assert!(
            matches!(&err, HarnessError::Conflict(msg) if msg.contains(&first.slug)),
            "expected a conflict naming feature {}, got {err:?}",
            first.id
        );

        // Closing it frees the checkout again.
        apply(
            dir.path(),
            first.id,
            None,
            &HarnessTrigger::Block {
                reason: "abandoned".into(),
            },
        )
        .unwrap();
        let third = register_feature(dir.path(), input(main)).unwrap();
        assert_eq!(third.workspace_id, Some(main));
    }

    /// Writing a feature gives back the keys this crate does not model.
    ///
    /// `rules` is the one that matters: the harness CLI validates against it,
    /// and a register that dropped it left `scripts/harness gate` failing on a
    /// `valid_status` that no longer existed.
    #[test]
    fn a_write_keeps_the_rest_of_the_document() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join(FEATURES),
            r#"{
              "project": "t",
              "description": "state of the harness",
              "rules": { "valid_status": ["pending", "done"], "max_review_rounds": 2 },
              "features": [
                { "id": 1, "slug": "old", "status": "done", "notes_from_the_cli": "keep me" }
              ]
            }"#,
        )
        .unwrap();
        register_feature(
            dir.path(),
            RegisterInput {
                spec_raw: "do the thing".into(),
                title: None,
                source_issue: None,
                workspace_id: None,
                workspace_path: None,
            },
        )
        .unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(dir.path().join(FEATURES)).unwrap()).unwrap();
        assert_eq!(written["description"], "state of the harness");
        assert_eq!(written["rules"]["max_review_rounds"], 2);
        assert_eq!(written["rules"]["valid_status"][1], "done");
        // Per feature as well as at the top level.
        assert_eq!(written["features"][0]["notes_from_the_cli"], "keep me");
        assert_eq!(written["features"][1]["id"], 2);
    }

    /// A feature registered outside Forge has no checkout to occupy, so it
    /// neither takes a slot nor blocks one.
    #[test]
    fn a_feature_without_a_checkout_blocks_nothing() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join(FEATURES),
            r#"{"project":"t","features":[]}"#,
        )
        .unwrap();
        let free = RegisterInput {
            spec_raw: "from the cli".into(),
            title: None,
            source_issue: None,
            workspace_id: None,
            workspace_path: None,
        };
        register_feature(dir.path(), free).unwrap();
        let bound = RegisterInput {
            spec_raw: "from forge".into(),
            title: None,
            source_issue: None,
            workspace_id: Some(WorkspaceId::new()),
            workspace_path: None,
        };
        assert!(register_feature(dir.path(), bound).is_ok());
    }

    #[test]
    fn concurrent_review_round_bumps_are_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("harness/progress")).unwrap();
        fs::write(
            dir.path().join(FEATURES),
            r#"{
              "project": "t",
              "rules": { "max_review_rounds": 200 },
              "features": [
                { "id": 1, "slug": "rounds", "status": "in_review", "review_rounds": 0 }
              ]
            }"#,
        )
        .unwrap();

        let root = std::sync::Arc::new(dir.path().to_path_buf());
        let mut threads = Vec::new();
        for _ in 0..2 {
            let root = std::sync::Arc::clone(&root);
            threads.push(std::thread::spawn(move || {
                for _ in 0..50 {
                    apply(
                        &root,
                        1,
                        None,
                        &HarnessTrigger::Block {
                            reason: "contention".into(),
                        },
                    )
                    .unwrap();
                }
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }

        // `revision` is the read-modify-write this file is full of: read the
        // row, add one, write the whole document back. Without the lock and the
        // atomic rename, two threads read the same number and one of the two
        // writes is simply gone.
        let feature = get_feature(dir.path(), 1).unwrap();
        assert_eq!(feature.revision, Some(100));
    }
}
