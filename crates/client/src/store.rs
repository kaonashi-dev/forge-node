//! The passive GUI-side replica of daemon state (§11.5, ADR-011).
//!
//! ADR-011 makes the daemon the single source of truth for every terminal grid:
//! it runs the only `TerminalEngine`, sends a [`TerminalSnapshot`] on attach and
//! [`TerminalDelta`]s afterwards. The GUI never emulates — it keeps a *replica
//! of cells* ([`CellGrid`]) plus the plain domain lists, and renders from them
//! (`apps/tauri` depends on `client`, never on `terminal-core`; §17).
//!
//! [`Store`] holds that replica. It is deliberately free of any GUI type:
//! the GUI reads its public fields and drives it from protocol messages
//! ([`Store::apply_snapshot`], [`Store::apply_event`]). The sequence rules of
//! §10.5 live entirely in [`CellGrid::apply_delta`], which returns a
//! [`DeltaOutcome`] telling the caller when a re-attach is required.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::Arc;

use domain::{
    AgentProfile, Cursor, ExternalAgentSession, Job, JobId, Project, ProjectGroup, ProviderUsage,
    PtySize, PullRequestState, Row, ScrollbackRows, Session, ShareRule, TermModes, TerminalDelta,
    TerminalId, TerminalSnapshot, Workspace,
};
use protocol::{DaemonEvent, ProviderInfo, Response};

/// Outcome of feeding one [`TerminalDelta`] to a [`CellGrid`], implementing the
/// per-terminal sequence rules of §10.5.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DeltaOutcome {
    /// `delta.seq == last_seq + 1`: the delta was applied and `last_seq`
    /// advanced.
    Applied,
    /// `delta.seq <= last_seq`: a duplicate/reordered delta, discarded (§10.5).
    Stale,
    /// `delta.seq > last_seq + 1`: a gap in the stream. The delta was *not*
    /// applied; the GUI must re-attach the terminal to obtain a fresh snapshot
    /// (§10.5). This should not normally happen and is logged as a bug.
    NeedsResync,
}

/// Outcome of applying one [`DaemonEvent`] to a [`Store`].
///
/// The only actionable variant is [`EventOutcome::NeedsResync`]: a terminal
/// delta arrived with a sequence gap and the GUI must re-`AttachTerminal`
/// (§10.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventOutcome {
    /// The event mutated replica state (or was a benign no-op such as a notice).
    Applied,
    /// A [`DaemonEvent::TerminalDelta`] could not be applied because of a
    /// sequence gap; the GUI must re-attach the named terminal.
    NeedsResync {
        /// The terminal that must be re-attached.
        terminal_id: TerminalId,
    },
    /// The event referenced a terminal with no local [`CellGrid`] (the GUI is
    /// not attached to it); it was ignored.
    Ignored,
}

/// The GUI-side replica of daemon state (§11.5).
///
/// Every field is public: the GUI reads them directly to render the sidebar,
/// session lists and terminals. Mutation goes exclusively through
/// [`Store::apply_snapshot`] and [`Store::apply_event`] so the replica always
/// mirrors the authoritative daemon (ADR-011).
/// How many lines of a running job's stream a replica keeps.
///
/// Enough to see what the agent is doing right now, short of holding a whole
/// run in memory in every client.
const JOB_TAIL_LINES: usize = 400;

#[derive(Clone, Debug, Default)]
pub struct Store {
    /// Organizational groups shown above projects in the sidebar.
    pub project_groups: Vec<ProjectGroup>,
    /// All known projects (§7.1).
    pub projects: Vec<Project>,
    /// All known workspaces (§7.2).
    pub workspaces: Vec<Workspace>,
    /// All known sessions (§7.3).
    pub sessions: Vec<Session>,
    /// Read-only agent sessions discovered on disk (e.g. Claude Code
    /// transcripts) that the daemon did not launch. Refreshed on each snapshot.
    pub external_agents: Vec<ExternalAgentSession>,
    /// Agent providers with their detection state (§13.1).
    pub providers: Vec<ProviderInfo>,
    /// Saved launch profiles (§13.4), in provider then name order.
    pub agent_profiles: Vec<AgentProfile>,
    /// Every project's file-sharing rules (§14.2), in application order.
    pub worktree_shares: Vec<ShareRule>,
    /// Opaque app-state key/value pairs (§15.2).
    pub app_state: Vec<(String, String)>,
    /// Latest usage per *account* (§16.2, §13.4): a provider reports one
    /// reading for its default login and one per profile that moved its config
    /// directory. Empty until a provider that declares a usage source reports.
    pub usage: Vec<ProviderUsage>,
    /// Complete cached pull-request state from the daemon.
    pub pull_requests: PullRequestState,
    /// The tail of each running job's event stream, newest last.
    ///
    /// Bounded, and dropped when the job leaves the list: a step can stream for
    /// minutes and nobody scrolls back through a finished one here — the whole
    /// transcript is a file the daemon serves with `ReadJobLog`. What this is
    /// for is watching the step that is running *now*, which is the one
    /// question the Feature tab could not answer before.
    pub job_output: HashMap<JobId, VecDeque<String>>,
    /// Headless agent runs the daemon has started, oldest first (§ jobs).
    ///
    /// Held whole rather than by id because the list *is* the view: a job's
    /// interest is mostly "what is running now, and how did the last ones
    /// end". The output itself is not here — that is a file the daemon writes
    /// and a client follows through `JobOutput` or reads with `ReadJobLog`.
    pub jobs: Vec<Job>,
    /// Cell replicas for the terminals the GUI is attached to, keyed by id.
    pub terminals: HashMap<TerminalId, CellGrid>,
    /// Terminals that rang the bell while the GUI was looking somewhere else.
    ///
    /// The flag cannot live in [`CellGrid`]: `terminals` only holds the
    /// *attached* replica, so a bell stored there could only ever describe the
    /// session already on screen — the one that by definition is not asking to
    /// be found. `TerminalBell` is broadcast for every terminal (`core.rs`
    /// `broadcast_domain`), so keeping the set beside the replicas is what lets
    /// the rail say *which* agent stopped to ask.
    pending_bell: HashSet<TerminalId>,
    /// Terminals that produced output while the GUI was looking somewhere else.
    ///
    /// Same reason as [`Store::pending_bell`]: `TerminalActivity` is only sent
    /// to clients that are *not* subscribed to that terminal, so storing it on
    /// the attached grid meant the unread mark could never be set for anyone.
    pending_activity: HashSet<TerminalId>,
}

impl Store {
    /// Create an empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Populate the domain lists from a [`Response::Snapshot`] (answering
    /// `GetSnapshot`, §10.2). Any other response variant is ignored.
    /// Extend one job's tail with lines read back from its log.
    ///
    /// Used when a step is opened after it has already been streaming: the
    /// events only carry what arrived while somebody was listening.
    pub fn seed_job_output(&mut self, job_id: JobId, lines: Vec<String>) {
        let tail = self.job_output.entry(job_id).or_default();
        if !tail.is_empty() {
            return;
        }
        let start = lines.len().saturating_sub(JOB_TAIL_LINES);
        tail.extend(lines.into_iter().skip(start));
    }

    /// The tail of one job's stream, oldest first.
    #[must_use]
    pub fn job_output(&self, job_id: JobId) -> Vec<String> {
        self.job_output
            .get(&job_id)
            .map(|tail| tail.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn apply_snapshot(&mut self, response: Response) {
        match response {
            Response::Snapshot {
                project_groups,
                projects,
                workspaces,
                sessions,
                providers,
                agent_profiles,
                worktree_shares,
                app_state,
                external_agents,
                pull_requests,
                jobs,
                usage,
            } => {
                self.project_groups = project_groups;
                self.projects = projects;
                self.workspaces = workspaces;
                self.sessions = sessions;
                self.providers = providers;
                self.agent_profiles = agent_profiles;
                self.worktree_shares = worktree_shares;
                self.app_state = app_state;
                self.external_agents = external_agents;
                self.pull_requests = pull_requests;
                self.jobs = jobs;
                // Usage rides in the snapshot from the daemon's cache (L1); a
                // later `ProviderUsageChanged` refreshes it in place.
                self.usage = usage;
                self.prune_pending();
            }
            _ => {
                tracing::warn!("apply_snapshot called with a non-Snapshot response");
            }
        }
    }

    /// Forget attention marks for terminals no longer named by any session.
    ///
    /// A session that exits takes its terminal with it, and a mark for a
    /// terminal nothing can navigate to would keep the rail's count above the
    /// number of rows it can show.
    fn prune_pending(&mut self) {
        let live: HashSet<TerminalId> = self
            .sessions
            .iter()
            .filter_map(|session| session.terminal_id)
            .collect();
        self.pending_bell.retain(|id| live.contains(id));
        self.pending_activity.retain(|id| live.contains(id));
    }

    /// Apply one unsolicited [`DaemonEvent`] to the replica (§10.3).
    ///
    /// Domain events (`Project*`/`Workspace*`/`Session*`) add, update or remove
    /// entries by id. Terminal events route to the matching [`CellGrid`]. The
    /// returned [`EventOutcome`] tells the caller whether a terminal re-attach is
    /// required (§10.5).
    #[must_use]
    pub fn apply_event(&mut self, event: &DaemonEvent) -> EventOutcome {
        match event {
            DaemonEvent::FactoryReset => {
                self.project_groups.clear();
                self.projects.clear();
                self.workspaces.clear();
                self.sessions.clear();
                self.external_agents.clear();
                self.agent_profiles.clear();
                self.worktree_shares.clear();
                self.app_state.clear();
                self.usage.clear();
                self.pull_requests = PullRequestState::default();
                self.job_output.clear();
                self.jobs.clear();
                self.terminals.clear();
                self.pending_bell.clear();
                self.pending_activity.clear();
                EventOutcome::Applied
            }
            DaemonEvent::ProjectGroupCreated(group) | DaemonEvent::ProjectGroupUpdated(group) => {
                upsert_by(&mut self.project_groups, group.clone(), |x| {
                    x.id == group.id
                });
                EventOutcome::Applied
            }
            DaemonEvent::ProjectGroupRemoved { project_group_id } => {
                self.project_groups.retain(|x| x.id != *project_group_id);
                for project in &mut self.projects {
                    if project.project_group_id == Some(*project_group_id) {
                        project.project_group_id = None;
                    }
                }
                EventOutcome::Applied
            }
            DaemonEvent::ProjectAdded(p) | DaemonEvent::ProjectUpdated(p) => {
                upsert_by(&mut self.projects, p.clone(), |x| x.id == p.id);
                EventOutcome::Applied
            }
            DaemonEvent::ProjectRemoved { project_id } => {
                self.projects.retain(|x| x.id != *project_id);
                EventOutcome::Applied
            }
            DaemonEvent::WorkspaceCreated(w) | DaemonEvent::WorkspaceUpdated(w) => {
                upsert_by(&mut self.workspaces, w.clone(), |x| x.id == w.id);
                EventOutcome::Applied
            }
            DaemonEvent::WorkspaceRemoved { workspace_id } => {
                self.workspaces.retain(|x| x.id != *workspace_id);
                EventOutcome::Applied
            }
            DaemonEvent::SessionCreated(s) | DaemonEvent::SessionUpdated(s) => {
                upsert_by(&mut self.sessions, s.clone(), |x| x.id == s.id);
                EventOutcome::Applied
            }
            DaemonEvent::SessionRemoved { session_id } => {
                if let Some(terminal_id) = self
                    .sessions
                    .iter()
                    .find(|session| session.id == *session_id)
                    .and_then(|session| session.terminal_id)
                {
                    self.terminals.remove(&terminal_id);
                    self.pending_bell.remove(&terminal_id);
                    self.pending_activity.remove(&terminal_id);
                }
                self.sessions.retain(|x| x.id != *session_id);
                self.prune_pending();
                EventOutcome::Applied
            }
            DaemonEvent::TerminalDelta { terminal_id, delta } => {
                match self.terminals.get_mut(terminal_id) {
                    Some(grid) => match grid.apply_delta(delta) {
                        DeltaOutcome::Applied | DeltaOutcome::Stale => EventOutcome::Applied,
                        DeltaOutcome::NeedsResync => EventOutcome::NeedsResync {
                            terminal_id: *terminal_id,
                        },
                    },
                    None => EventOutcome::Ignored,
                }
            }
            DaemonEvent::TerminalResync {
                terminal_id,
                snapshot,
            } => {
                self.terminals
                    .entry(*terminal_id)
                    .and_modify(|g| g.apply_resync(snapshot))
                    .or_insert_with(|| CellGrid::from_snapshot(snapshot));
                EventOutcome::Applied
            }
            // Both notes describe a terminal the user is *not* watching, so
            // they are recorded beside the replicas rather than in one: the
            // attached grid is the single terminal for which neither can mean
            // anything. Setting the grid flag too keeps the field honest for a
            // reader that already holds a `CellGrid`.
            DaemonEvent::TerminalActivity { terminal_id } => {
                match self.terminals.get_mut(terminal_id) {
                    Some(grid) => grid.activity = true,
                    None => {
                        self.pending_activity.insert(*terminal_id);
                    }
                }
                EventOutcome::Applied
            }
            DaemonEvent::TerminalBell { terminal_id } => {
                match self.terminals.get_mut(terminal_id) {
                    Some(grid) => grid.bell = true,
                    None => {
                        self.pending_bell.insert(*terminal_id);
                    }
                }
                EventOutcome::Applied
            }
            DaemonEvent::ProviderUsageChanged { usage } => {
                // The event carries the whole set, so replacing is what keeps
                // the picture consistent; merging would strand a provider that
                // stopped reporting.
                self.usage = usage.clone();
                EventOutcome::Applied
            }
            DaemonEvent::AgentProfilesChanged { profiles } => {
                // Whole-set replacement, for the same reason as the usage
                // event: a merge would strand a profile that was deleted.
                self.agent_profiles = profiles.clone();
                EventOutcome::Applied
            }
            DaemonEvent::ProjectSharesChanged { project_id, rules } => {
                // One project's whole set: drop what it had, take what it has.
                // A merge would strand a rule the user just removed.
                self.worktree_shares
                    .retain(|rule| rule.project_id != *project_id);
                self.worktree_shares.extend(rules.iter().cloned());
                EventOutcome::Applied
            }
            // Provisioning results are read where they are shown, like a diff
            // or a rebase state: nothing here caches them.
            DaemonEvent::SharesApplied { .. } => EventOutcome::Applied,
            DaemonEvent::PullRequestsUpdated { state } => {
                self.pull_requests = state.clone();
                EventOutcome::Applied
            }
            // Harness rows are not replicated in the store — the Feature tab
            // asks for the one it shows — so this is `Ignored` here and
            // handled by whoever is watching that feature.
            DaemonEvent::HarnessFeatureChanged { .. } => EventOutcome::Applied,
            DaemonEvent::JobUpdated(job) => {
                match self.jobs.iter_mut().find(|row| row.id == job.id) {
                    Some(row) => *row = (**job).clone(),
                    None => self.jobs.push((**job).clone()),
                }
                EventOutcome::Applied
            }
            DaemonEvent::JobOutput { job_id, lines, .. } => {
                let tail = self.job_output.entry(*job_id).or_default();
                for line in lines {
                    tail.push_back(line.clone());
                }
                // A stream can run to megabytes; only the recent past is worth
                // holding in a replica, and the file has the rest.
                while tail.len() > JOB_TAIL_LINES {
                    tail.pop_front();
                }
                EventOutcome::Applied
            }
            DaemonEvent::AgentDetectionChanged { results } => {
                for result in results {
                    if let Some(provider) = self
                        .providers
                        .iter_mut()
                        .find(|p| p.descriptor.id == result.provider_id)
                    {
                        provider.detection = result.clone();
                    }
                }
                EventOutcome::Applied
            }
            // Notices and shutdown carry no replica state; the GUI reacts to them
            // directly. The catch-all also covers variants added by a newer peer.
            _ => EventOutcome::Applied,
        }
    }

    /// Insert (or replace) a [`CellGrid`] for a terminal from an `AttachAck`
    /// snapshot (§10.5). Call this when a `Response::AttachAck` arrives.
    pub fn attach_terminal(&mut self, terminal_id: TerminalId, snapshot: &TerminalSnapshot) {
        // Arriving *is* reading it: whatever the terminal rang or printed while
        // the user was elsewhere has now been looked at, so the marks are spent
        // here rather than waiting for something to clear them later.
        self.pending_bell.remove(&terminal_id);
        self.pending_activity.remove(&terminal_id);
        self.terminals
            .insert(terminal_id, CellGrid::from_snapshot(snapshot));
    }

    /// Whether a terminal asked for the user while they were looking elsewhere.
    ///
    /// This is the "needs you" signal of §16.3: agent CLIs ring the bell when
    /// they stop to ask a question, and the daemon broadcasts it for every
    /// terminal. The attached terminal is never included — the user is already
    /// looking at it.
    #[must_use]
    pub fn wants_attention(&self, terminal_id: &TerminalId) -> bool {
        self.pending_bell.contains(terminal_id)
    }

    /// Whether a terminal produced output while the user was looking elsewhere.
    #[must_use]
    pub fn has_unread(&self, terminal_id: &TerminalId) -> bool {
        self.pending_activity.contains(terminal_id)
    }

    /// Drop the local replica for a terminal after `DetachTerminal`.
    pub fn detach_terminal(&mut self, terminal_id: &TerminalId) {
        self.terminals.remove(terminal_id);
    }

    /// Merge a fetched block of scrollback into a terminal's cache
    /// (`FetchScrollback` flow, §11.5). No-op if the terminal is not attached.
    pub fn merge_scrollback(&mut self, terminal_id: &TerminalId, block: &ScrollbackRows) {
        if let Some(grid) = self.terminals.get_mut(terminal_id) {
            grid.merge_scrollback(block);
        }
    }

    /// Read-only access to a terminal replica, if attached.
    #[must_use]
    pub fn terminal(&self, terminal_id: &TerminalId) -> Option<&CellGrid> {
        self.terminals.get(terminal_id)
    }

    /// Value of a persisted GUI preference from the last snapshot (§15.2);
    /// `None` if the key is absent. The daemon stays authoritative — writes go
    /// through `Client::set_app_state`, not this replica.
    #[must_use]
    pub fn app_state_value(&self, key: &str) -> Option<&str> {
        self.app_state
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, value)| value.as_str())
    }
}

/// Replace the first element matching `is_match`, or push `value` if none does.
fn upsert_by<T>(items: &mut Vec<T>, value: T, is_match: impl Fn(&T) -> bool) {
    if let Some(pos) = items.iter().position(is_match) {
        items[pos] = value;
    } else {
        items.push(value);
    }
}

/// Maximum scrollback rows a [`CellGrid`] keeps cached (§11.5).
///
/// The daemon holds up to `MAX_SCROLLBACK_LINES` (100 000) per terminal and
/// serves any of them through `FetchScrollback`, so the GUI only needs enough
/// around the viewport to scroll smoothly. At 80 columns a cached row costs a
/// few KiB, so an unbounded cache reached hundreds of MiB per attached terminal.
pub const MAX_SCROLLBACK_CACHE_ROWS: usize = 5_000;

/// A passive terminal replica (ADR-011); history access uses negative viewport
/// offsets, with `-1` immediately above the visible grid.
#[derive(Clone, Debug)]
pub struct CellGrid {
    /// The `rows` visible lines of the grid.
    pub visible: Vec<Row>,
    // Absolute indices avoid rebuilding up to 5,000 keys on each scroll delta.
    scrollback_cache: BTreeMap<i64, Row>,
    /// Total number of lines currently in the daemon's scrollback.
    pub scrollback_len: u64,
    pub scrollback_generation: u64,
    /// Cursor position and shape at `last_seq`.
    pub cursor: Cursor,
    /// Terminal modes (alt screen, mouse, ...) at `last_seq`.
    pub modes: TermModes,
    /// The sequence of the last snapshot/delta applied (§10.5).
    pub last_seq: u64,
    /// The PTY size the grid was last sized to.
    pub size: PtySize,
    /// The latest terminal-reported (OSC 0/2) title, if any.
    pub title: Option<String>,
    /// Set when the terminal rang the bell; the GUI clears it once surfaced.
    pub bell: bool,
    /// Set when the terminal produced output while unfocused; used for unread
    /// badges. The GUI clears it once surfaced.
    pub activity: bool,
}

impl CellGrid {
    /// Build a grid from an attach/resync [`TerminalSnapshot`] (§10.5).
    ///
    /// The snapshot's `scrollback_tail` is seeded into the cache at the correct
    /// negative offsets: its last row (nearest the viewport) at `-1`, the one
    /// before it at `-2`, and so on.
    #[must_use]
    pub fn from_snapshot(snapshot: &TerminalSnapshot) -> Self {
        let mut grid = Self {
            visible: snapshot.visible.clone(),
            scrollback_cache: BTreeMap::new(),
            scrollback_len: snapshot.scrollback_len,
            scrollback_generation: snapshot.scrollback_generation,
            cursor: snapshot.cursor,
            modes: snapshot.modes,
            last_seq: snapshot.seq,
            size: snapshot.size,
            title: snapshot.title.clone(),
            bell: false,
            activity: false,
        };
        grid.seed_scrollback_tail(snapshot);
        grid
    }

    /// Apply one [`TerminalDelta`], enforcing the §10.5 sequence rules exactly.
    ///
    /// - `seq <= last_seq`  → [`DeltaOutcome::Stale`] (discarded).
    /// - `seq == last_seq+1` → [`DeltaOutcome::Applied`]: damaged rows overwrite
    ///   `visible` by index, `scrollback_len`/`scrollback_generation` take the
    ///   delta's values, and cursor/modes/`last_seq` update.
    /// - `seq > last_seq+1`  → [`DeltaOutcome::NeedsResync`]: a gap; nothing is
    ///   applied and the caller must re-attach.
    /// - a damaged row outside the replica's current geometry →
    ///   [`DeltaOutcome::NeedsResync`]: a resize raced the replica; applying only
    ///   the rows that still fit would leave a permanently torn frame.
    #[must_use]
    pub fn apply_delta(&mut self, delta: &TerminalDelta) -> DeltaOutcome {
        if delta.seq <= self.last_seq {
            return DeltaOutcome::Stale;
        }
        if delta.seq > self.last_seq + 1 {
            return DeltaOutcome::NeedsResync;
        }

        if let Some((index, _)) = delta
            .rows
            .iter()
            .find(|(index, _)| usize::from(*index) >= self.visible.len())
        {
            tracing::warn!(
                index = *index,
                rows = self.visible.len(),
                seq = delta.seq,
                "terminal delta does not match replica geometry; requesting resync"
            );
            return DeltaOutcome::NeedsResync;
        }

        if delta.patches.iter().any(|patch| {
            self.visible.get(usize::from(patch.line)).is_none_or(|row| {
                usize::from(patch.first)
                    .checked_add(patch.row.cells.len())
                    .is_none_or(|end| end > row.cells.len())
            })
        }) {
            return DeltaOutcome::NeedsResync;
        }
        if delta.scrollback_generation != self.scrollback_generation {
            self.scrollback_cache.clear();
        }
        for (index, row) in &delta.rows {
            let i = *index as usize;
            self.visible[i] = row.clone();
        }
        for patch in &delta.patches {
            let row = &mut self.visible[usize::from(patch.line)];
            let first = usize::from(patch.first);
            Arc::make_mut(&mut row.cells)[first..first + patch.row.cells.len()]
                .clone_from_slice(&patch.row.cells);
            row.wrapped = patch.row.wrapped;
        }
        self.scrollback_len = delta.scrollback_len;
        self.scrollback_generation = delta.scrollback_generation;
        self.cursor = delta.cursor;
        self.modes = delta.modes;
        self.last_seq = delta.seq;
        DeltaOutcome::Applied
    }

    /// Replace the whole grid from a fresh [`TerminalSnapshot`] after a resync
    /// (§10.5). Resets `last_seq`, the scrollback cache and the transient
    /// bell/activity flags.
    pub fn apply_resync(&mut self, snapshot: &TerminalSnapshot) {
        self.visible = snapshot.visible.clone();
        self.scrollback_cache.clear();
        self.scrollback_len = snapshot.scrollback_len;
        self.scrollback_generation = snapshot.scrollback_generation;
        self.cursor = snapshot.cursor;
        self.modes = snapshot.modes;
        self.last_seq = snapshot.seq;
        self.size = snapshot.size;
        self.title = snapshot.title.clone();
        self.bell = false;
        self.activity = false;
        self.seed_scrollback_tail(snapshot);
    }

    /// Merge fetched history by absolute index (0 = oldest), keeping the newest
    /// [`MAX_SCROLLBACK_CACHE_ROWS`] cached rows.
    pub fn merge_scrollback(&mut self, block: &ScrollbackRows) {
        if let Some(snapshot) = &block.snapshot {
            if snapshot.seq < self.last_seq || snapshot.scrollback_generation != block.generation {
                return;
            }
            if self.scrollback_generation != block.generation {
                self.apply_resync(snapshot);
            }
        }
        if block.generation != self.scrollback_generation {
            return;
        }
        let skip = block.rows.len().saturating_sub(MAX_SCROLLBACK_CACHE_ROWS);
        for (i, row) in block.rows.iter().enumerate().skip(skip) {
            let Some(absolute) = block.from_line.checked_add(i as i64) else {
                break;
            };
            self.scrollback_cache.insert(absolute, row.clone());
            self.trim_scrollback_cache();
        }
    }

    /// Cached history at a negative viewport offset, or `None` if unavailable.
    #[must_use]
    pub fn scrollback_row(&self, offset: i64) -> Option<&Row> {
        if offset >= 0 {
            return None;
        }
        let base = i64::try_from(self.scrollback_len).ok()?;
        self.scrollback_cache.get(&base.checked_add(offset)?)
    }

    /// Whether the scrollback row at `offset` (negative offset from the viewport
    /// top) is present in the cache, so the GUI can decide when to
    /// `FetchScrollback`.
    #[must_use]
    pub fn is_scrollback_cached(&self, offset: i64) -> bool {
        self.scrollback_row(offset).is_some()
    }

    /// The `(min, max)` negative offsets currently held in the cache, or `None`
    /// if empty. The GUI uses this to size its virtual scrollbar and to decide
    /// which block to fetch next (§11.5).
    #[must_use]
    pub fn scrollback_cached_extent(&self) -> Option<(i64, i64)> {
        let min = *self.scrollback_cache.keys().next()?;
        let max = *self.scrollback_cache.keys().next_back()?;
        let base = i64::try_from(self.scrollback_len).ok()?;
        Some((min.checked_sub(base)?, max.checked_sub(base)?))
    }

    /// Take and clear the bell flag.
    #[must_use]
    pub fn take_bell(&mut self) -> bool {
        std::mem::replace(&mut self.bell, false)
    }

    /// Take and clear the activity flag.
    #[must_use]
    pub fn take_activity(&mut self) -> bool {
        std::mem::replace(&mut self.activity, false)
    }

    fn seed_scrollback_tail(&mut self, snapshot: &TerminalSnapshot) {
        let tail_len = snapshot.scrollback_tail.len() as i64;
        let base = self.scrollback_len as i64 - tail_len;
        let skip = snapshot
            .scrollback_tail
            .len()
            .saturating_sub(MAX_SCROLLBACK_CACHE_ROWS);
        for (i, row) in snapshot.scrollback_tail.iter().enumerate().skip(skip) {
            self.scrollback_cache.insert(base + i as i64, row.clone());
        }
    }

    fn trim_scrollback_cache(&mut self) {
        while self.scrollback_cache.len() > MAX_SCROLLBACK_CACHE_ROWS {
            self.scrollback_cache.pop_first();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{
        AgentProviderId, Project, ProjectGroup, ProjectGroupId, ProjectId, PullRequest,
        PullRequestRelations, PullRequestState, ReviewDecision, Session, SessionId, SessionKind,
        SessionRole, SessionState, SessionTitle, Timestamp, WorkspaceId,
    };
    use std::path::PathBuf;

    fn sample_usage(provider: &str, used_percent: u8) -> ProviderUsage {
        ProviderUsage {
            provider_id: AgentProviderId::new(provider),
            profile_id: None,
            windows: vec![domain::UsageWindow {
                used_percent,
                window: "5h".to_string(),
                resets_at: None,
            }],
            collected_at: Timestamp::now(),
        }
    }

    fn sample_project(name: &str) -> Project {
        Project {
            id: ProjectId::new(),
            project_group_id: None,
            name: name.to_string(),
            icon: None,
            root_path: PathBuf::from(format!("/home/dev/{name}")),
            git_root: None,
            created_at: Timestamp::now(),
            last_opened_at: Timestamp::now(),
        }
    }

    fn sample_session(workspace_id: WorkspaceId, state: SessionState) -> Session {
        let id = SessionId::new();
        Session {
            id,
            workspace_id,
            kind: SessionKind::Shell,
            role: SessionRole::Generic,
            parent_session_id: None,
            root_session_id: id,
            terminal_id: None,
            agent_provider_id: None,
            agent_profile_id: None,
            title: SessionTitle::default(),
            state,
            created_at: Timestamp::now(),
            launch_command: None,
            last_activity_at: Timestamp::now(),
            ended_at: None,
            base_commit: None,
        }
    }

    fn sample_pull_request_state(title: &str) -> PullRequestState {
        let now = Timestamp::now();
        PullRequestState {
            pull_requests: vec![PullRequest {
                project_id: Some(ProjectId::new()),
                repository: "forge/forge-node".to_string(),
                host: "github.com".to_string(),
                number: 42,
                title: title.to_string(),
                body: "Description".to_string(),
                body_truncated: false,
                url: "https://github.com/forge/forge-node/pull/42".to_string(),
                author: "octocat".to_string(),
                base_ref: "main".to_string(),
                head_ref: "feature/pull-requests".to_string(),
                is_draft: false,
                review_decision: Some(ReviewDecision::Approved),
                labels: vec![],
                assignees: vec![],
                review_requests: vec![],
                additions: 10,
                deletions: 2,
                changed_files: 3,
                comment_count: 1,
                created_at: now,
                updated_at: now,
                relations: PullRequestRelations {
                    assigned: false,
                    review_requested: true,
                    authored: false,
                },
            }],
            refreshed_at: Some(now),
            ..PullRequestState::default()
        }
    }

    fn snapshot(
        seq: u64,
        cols: u16,
        rows: u16,
        scrollback_len: u64,
        tail: usize,
    ) -> TerminalSnapshot {
        TerminalSnapshot {
            scrollback_generation: 0,
            seq,
            size: PtySize {
                cols,
                rows,
                pixel_width: 0,
                pixel_height: 0,
            },
            visible: vec![Row::blank(cols); rows as usize],
            scrollback_tail: vec![Row::blank(cols); tail],
            scrollback_len,
            cursor: Cursor::default(),
            modes: TermModes::default(),
            title: None,
        }
    }

    fn delta(seq: u64, rows: Vec<(u16, Row)>, scrollback_len: u64) -> TerminalDelta {
        TerminalDelta {
            patches: Vec::new(),
            scrollback_len,
            scrollback_generation: 0,
            seq,
            rows,
            scrolled_lines: 0,
            cursor: Cursor::default(),
            modes: TermModes::default(),
        }
    }

    #[test]
    fn snapshot_populates_domain_lists() {
        let mut store = Store::new();
        let project = sample_project("forge");
        let pull_requests = sample_pull_request_state("Snapshot PR");
        let response = Response::Snapshot {
            project_groups: vec![],
            projects: vec![project.clone()],
            workspaces: vec![],
            sessions: vec![],
            providers: vec![],
            agent_profiles: vec![],
            worktree_shares: vec![],
            app_state: vec![("sidebar_width".to_string(), "280".to_string())],
            external_agents: vec![],
            pull_requests: pull_requests.clone(),
            jobs: vec![],
            usage: vec![sample_usage("claude", 40)],
        };
        store.apply_snapshot(response);
        assert_eq!(store.projects, vec![project]);
        assert_eq!(store.app_state.len(), 1);
        assert_eq!(store.pull_requests, pull_requests);
        // Usage rides in the snapshot from the daemon's cache (L1): applying one
        // populates it without a separate `list_provider_usage` round trip.
        assert_eq!(store.usage.len(), 1);
        assert_eq!(store.usage[0].windows[0].used_percent, 40);
    }

    #[test]
    fn app_state_value_reads_snapshot_preferences() {
        let mut store = Store::new();
        store.apply_snapshot(Response::Snapshot {
            project_groups: vec![],
            projects: vec![],
            workspaces: vec![],
            sessions: vec![],
            providers: vec![],
            agent_profiles: vec![],
            worktree_shares: vec![],
            app_state: vec![
                ("sidebar_width".to_string(), "280".to_string()),
                ("theme".to_string(), "dark".to_string()),
            ],
            external_agents: vec![],
            pull_requests: PullRequestState::default(),
            jobs: vec![],
            usage: vec![],
        });

        assert_eq!(store.app_state_value("sidebar_width"), Some("280"));
        assert_eq!(store.app_state_value("theme"), Some("dark"));
        assert_eq!(store.app_state_value("never_written"), None);
        assert_eq!(Store::new().app_state_value("sidebar_width"), None);
    }

    #[test]
    fn factory_reset_discards_the_replica_state() {
        let mut store = Store::new();
        let terminal_id = TerminalId::new();
        let mut session = sample_session(WorkspaceId::new(), SessionState::Running);
        session.terminal_id = Some(terminal_id);
        store.projects.push(sample_project("forge"));
        store.sessions.push(session);
        store
            .app_state
            .push(("ui.theme_base".into(), "light".into()));
        store.usage.push(sample_usage("claude", 40));
        store.pull_requests = sample_pull_request_state("Reset me");
        store.attach_terminal(terminal_id, &snapshot(1, 4, 3, 0, 0));

        assert_eq!(
            store.apply_event(&DaemonEvent::FactoryReset),
            EventOutcome::Applied
        );

        assert!(store.projects.is_empty());
        assert!(store.sessions.is_empty());
        assert!(store.app_state.is_empty());
        assert!(store.usage.is_empty());
        assert_eq!(store.pull_requests, PullRequestState::default());
        assert!(store.terminal(&terminal_id).is_none());
    }

    #[test]
    fn project_events_add_update_and_remove_by_id() {
        let mut store = Store::new();
        let mut project = sample_project("forge");

        assert_eq!(
            store.apply_event(&DaemonEvent::ProjectAdded(project.clone())),
            EventOutcome::Applied
        );
        assert_eq!(store.projects.len(), 1);

        // A second, distinct project appends.
        let other = sample_project("other");
        let _ = store.apply_event(&DaemonEvent::ProjectAdded(other.clone()));
        assert_eq!(store.projects.len(), 2);

        // Updating by the same id replaces in place (no growth).
        project.name = "renamed".to_string();
        let _ = store.apply_event(&DaemonEvent::ProjectUpdated(project.clone()));
        assert_eq!(store.projects.len(), 2);
        assert_eq!(
            store
                .projects
                .iter()
                .find(|p| p.id == project.id)
                .unwrap()
                .name,
            "renamed"
        );

        // Remove by id.
        let _ = store.apply_event(&DaemonEvent::ProjectRemoved {
            project_id: project.id,
        });
        assert_eq!(store.projects.len(), 1);
        assert_eq!(store.projects[0].id, other.id);
    }

    #[test]
    fn project_group_events_add_update_and_remove_by_id() {
        let mut store = Store::new();
        let mut group = ProjectGroup {
            id: ProjectGroupId::new(),
            name: "Product X".to_string(),
            created_at: Timestamp::now(),
        };

        let _ = store.apply_event(&DaemonEvent::ProjectGroupCreated(group.clone()));
        assert_eq!(store.project_groups, vec![group.clone()]);
        let mut project = sample_project("grouped");
        project.project_group_id = Some(group.id);
        store.projects.push(project);

        group.name = "Product X Platform".to_string();
        let _ = store.apply_event(&DaemonEvent::ProjectGroupUpdated(group.clone()));
        assert_eq!(store.project_groups, vec![group.clone()]);

        let _ = store.apply_event(&DaemonEvent::ProjectGroupRemoved {
            project_group_id: group.id,
        });
        assert!(store.project_groups.is_empty());
        assert_eq!(store.projects[0].project_group_id, None);
    }

    #[test]
    fn session_events_create_and_update_by_id() {
        let mut store = Store::new();
        let workspace_id = WorkspaceId::new();
        let session = sample_session(workspace_id, SessionState::Starting);

        let _ = store.apply_event(&DaemonEvent::SessionCreated(session.clone()));
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].state, SessionState::Starting);

        let mut updated = session.clone();
        updated.state = SessionState::Running;
        let _ = store.apply_event(&DaemonEvent::SessionUpdated(updated));
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].state, SessionState::Running);
    }

    #[test]
    fn session_removed_drops_the_replica_entry() {
        let mut store = Store::new();
        let workspace_id = WorkspaceId::new();
        let kept = sample_session(workspace_id, SessionState::Running);
        let mut closed = sample_session(workspace_id, SessionState::Orphaned);
        let terminal_id = TerminalId::new();
        closed.terminal_id = Some(terminal_id);

        let _ = store.apply_event(&DaemonEvent::SessionCreated(kept.clone()));
        let _ = store.apply_event(&DaemonEvent::SessionCreated(closed.clone()));
        store.attach_terminal(terminal_id, &snapshot(1, 4, 3, 0, 0));
        assert_eq!(store.sessions.len(), 2);
        assert!(store.terminal(&terminal_id).is_some());

        // `CloseSession` deletes the session daemon-side; without this event the
        // sidebar would keep a ghost until the next snapshot.
        assert_eq!(
            store.apply_event(&DaemonEvent::SessionRemoved {
                session_id: closed.id,
            }),
            EventOutcome::Applied
        );
        assert_eq!(store.sessions.len(), 1);
        assert_eq!(store.sessions[0].id, kept.id);
        assert!(store.terminal(&terminal_id).is_none());
    }

    /// The regression this guards is the reason the feature could not exist:
    /// `TerminalBell` is broadcast for every terminal, but `terminals` only
    /// ever holds the attached one, so routing the flag into a `CellGrid`
    /// dropped it for every session the user was not already looking at — the
    /// only sessions that can meaningfully ask to be found.
    #[test]
    fn a_bell_from_an_unattached_terminal_is_what_asks_for_the_user() {
        let mut store = Store::new();
        let watched = TerminalId::new();
        let elsewhere = TerminalId::new();
        store.attach_terminal(watched, &snapshot(5, 4, 3, 0, 0));

        let _ = store.apply_event(&DaemonEvent::TerminalBell {
            terminal_id: elsewhere,
        });
        let _ = store.apply_event(&DaemonEvent::TerminalActivity {
            terminal_id: elsewhere,
        });
        assert!(store.wants_attention(&elsewhere));
        assert!(store.has_unread(&elsewhere));

        // The terminal on screen never asks: the user is already reading it.
        let _ = store.apply_event(&DaemonEvent::TerminalBell {
            terminal_id: watched,
        });
        assert!(!store.wants_attention(&watched));
        assert!(store.terminal(&watched).is_some_and(|grid| grid.bell));

        // Arriving spends both marks.
        store.attach_terminal(elsewhere, &snapshot(1, 4, 3, 0, 0));
        assert!(!store.wants_attention(&elsewhere));
        assert!(!store.has_unread(&elsewhere));
    }

    /// A mark for a terminal no session names any more would keep the rail's
    /// "needs you" count above the number of rows it can draw.
    #[test]
    fn closing_a_session_forgets_its_attention_mark() {
        let mut store = Store::new();
        let terminal_id = TerminalId::new();
        let mut session = sample_session(WorkspaceId::new(), SessionState::Running);
        session.terminal_id = Some(terminal_id);

        let _ = store.apply_event(&DaemonEvent::SessionCreated(session.clone()));
        let _ = store.apply_event(&DaemonEvent::TerminalBell { terminal_id });
        assert!(store.wants_attention(&terminal_id));

        let _ = store.apply_event(&DaemonEvent::SessionRemoved {
            session_id: session.id,
        });
        assert!(!store.wants_attention(&terminal_id));
    }

    #[test]
    fn terminal_delta_event_needs_resync_on_gap() {
        let mut store = Store::new();
        let terminal_id = TerminalId::new();
        store.attach_terminal(terminal_id, &snapshot(5, 4, 3, 0, 0));

        // In-order delta applies.
        let outcome = store.apply_event(&DaemonEvent::TerminalDelta {
            terminal_id,
            delta: delta(6, vec![], 0),
        });
        assert_eq!(outcome, EventOutcome::Applied);

        // A gap asks the GUI to re-attach.
        let outcome = store.apply_event(&DaemonEvent::TerminalDelta {
            terminal_id,
            delta: delta(9, vec![], 0),
        });
        assert_eq!(outcome, EventOutcome::NeedsResync { terminal_id });

        // A delta for an unattached terminal is ignored.
        let outcome = store.apply_event(&DaemonEvent::TerminalDelta {
            terminal_id: TerminalId::new(),
            delta: delta(1, vec![], 0),
        });
        assert_eq!(outcome, EventOutcome::Ignored);
    }

    #[test]
    fn cellgrid_sequence_rules_match_10_5() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 0, 0));
        assert_eq!(grid.last_seq, 5);

        // seq <= last_seq → stale, nothing changes.
        assert_eq!(grid.apply_delta(&delta(5, vec![], 0)), DeltaOutcome::Stale);
        assert_eq!(grid.last_seq, 5);

        // seq > last_seq + 1 → gap, not applied.
        assert_eq!(
            grid.apply_delta(&delta(7, vec![], 0)),
            DeltaOutcome::NeedsResync
        );
        assert_eq!(grid.last_seq, 5);

        // seq == last_seq + 1 → applied: damaged row at index 1, history length 2.
        let mut damaged = Row::blank(4);
        Arc::make_mut(&mut damaged.cells)[0].text = "X".into();
        assert_eq!(
            grid.apply_delta(&delta(6, vec![(1u16, damaged)], 2)),
            DeltaOutcome::Applied
        );
        assert_eq!(grid.last_seq, 6);
        assert_eq!(grid.scrollback_len, 2);
        assert_eq!(grid.visible[1].cells[0].text.as_str(), "X");
        // Untouched rows stay blank.
        assert_eq!(grid.visible[0].cells[0].text.as_str(), " ");
    }

    #[test]
    fn cellgrid_geometry_mismatch_requests_an_atomic_resync() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 7, 0));
        let before = grid.clone();
        let mut damaged = Row::blank(4);
        Arc::make_mut(&mut damaged.cells)[0].text = "new geometry".into();

        assert_eq!(
            grid.apply_delta(&delta(6, vec![(3, damaged)], 7)),
            DeltaOutcome::NeedsResync
        );
        assert_eq!(grid.last_seq, before.last_seq);
        assert_eq!(grid.scrollback_len, before.scrollback_len);
        assert_eq!(grid.visible, before.visible);
        assert_eq!(grid.scrollback_cache, before.scrollback_cache);
    }

    #[test]
    fn cellgrid_resync_replaces_state_and_resets_seq() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 0, 0));
        let _ = grid.apply_delta(&delta(6, vec![], 0));
        assert_eq!(grid.last_seq, 6);

        grid.bell = true;
        grid.apply_resync(&snapshot(20, 4, 3, 100, 5));
        assert_eq!(grid.last_seq, 20);
        assert_eq!(grid.scrollback_len, 100);
        assert!(!grid.bell);
        // Tail of 5 seeded at negative offsets -5..=-1.
        assert_eq!(grid.scrollback_cache.len(), 5);
        assert!(grid.is_scrollback_cached(-1));
        assert!(grid.is_scrollback_cached(-5));
        assert!(!grid.is_scrollback_cached(-6));
    }

    #[test]
    fn seeded_scrollback_shifts_when_lines_scroll_in() {
        // Tail of 2 seeds offsets -1 and -2.
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 10, 2));
        assert!(grid.is_scrollback_cached(-1) && grid.is_scrollback_cached(-2));

        // Absolute indices stay put when generation does not change, so a length
        // bump shifts offsets. The newly scrolled-off line is a hole until fetch.
        let _ = grid.apply_delta(&delta(6, vec![], 11));
        assert_eq!(grid.scrollback_len, 11);
        assert_eq!(grid.scrollback_cache.len(), 2);
        assert!(!grid.is_scrollback_cached(-1));
        assert!(grid.is_scrollback_cached(-2));
        assert!(grid.is_scrollback_cached(-3));
    }

    #[test]
    #[ignore = "manual scrollback hot-path timing"]
    fn scrollback_delta_timing() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 200, 50, 5_000, 5_000));
        let started = std::time::Instant::now();
        for seq in 6..10_006 {
            assert_eq!(
                grid.apply_delta(&delta(seq, vec![], 5_000 + (seq - 5))),
                DeltaOutcome::Applied
            );
        }
        eprintln!("10,000 scroll deltas: {:?}", started.elapsed());
        assert_eq!(grid.scrollback_cache.len(), MAX_SCROLLBACK_CACHE_ROWS);
    }

    #[test]
    fn scrollback_cache_is_bounded_and_evicts_the_furthest_rows() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 10_000, 0));

        // Merge well past the cap in blocks, as the GUI would while scrolling up.
        let block = MAX_SCROLLBACK_CACHE_ROWS / 4;
        for i in 0..6 {
            grid.merge_scrollback(&ScrollbackRows {
                snapshot: None,
                generation: 0,
                from_line: (i * block) as i64,
                rows: vec![Row::blank(4); block],
            });
        }

        assert!(
            grid.scrollback_cache.len() <= MAX_SCROLLBACK_CACHE_ROWS,
            "cache grew to {} rows",
            grid.scrollback_cache.len()
        );
        // The rows nearest the viewport (largest offsets) are the ones kept.
        let (min, max) = grid.scrollback_cached_extent().unwrap();
        assert!(min < max);
        assert!(grid.is_scrollback_cached(max));
    }

    #[test]
    fn merge_scrollback_maps_absolute_indices_to_offsets() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 100, 0));
        // Fetch rows 90..=93 (absolute). With scrollback_len 100 these map to
        // offsets -10..=-7.
        grid.merge_scrollback(&ScrollbackRows {
            snapshot: None,
            generation: 0,
            from_line: 90,
            rows: vec![Row::blank(4); 4],
        });
        assert!(grid.is_scrollback_cached(-10));
        assert!(grid.is_scrollback_cached(-7));
        assert!(!grid.is_scrollback_cached(-6));
    }

    #[test]
    fn history_contents_survive_scroll_fetch_and_resync() {
        let mut snap = snapshot(5, 4, 3, 10, 2);
        Arc::make_mut(&mut snap.scrollback_tail[0].cells)[0].text = "old".into();
        Arc::make_mut(&mut snap.visible[0].cells)[0].text = "top".into();
        let mut grid = CellGrid::from_snapshot(&snap);
        assert_eq!(
            grid.apply_delta(&delta(6, vec![], 12)),
            DeltaOutcome::Applied
        );
        assert_eq!(grid.scrollback_row(-4).unwrap().cells[0].text, "old");
        assert!(grid.scrollback_row(-1).is_none());

        // A coalesced scroll does not reconstruct missing history from the old viewport.
        assert_eq!(
            grid.apply_delta(&delta(7, vec![], 17)),
            DeltaOutcome::Applied
        );
        assert_eq!(grid.scrollback_row(-9).unwrap().cells[0].text, "old");
        assert!(grid.scrollback_row(-1).is_none());
        grid.merge_scrollback(&ScrollbackRows {
            snapshot: None,
            generation: 0,
            from_line: 16,
            rows: vec![snap.visible[0].clone()],
        });
        assert_eq!(grid.scrollback_row(-1).unwrap().cells[0].text, "top");
        assert_eq!(grid.scrollback_cached_extent(), Some((-9, -1)));
        assert!(grid.scrollback_row(0).is_none());
        assert!(grid.scrollback_row(i64::MIN).is_none());

        grid.apply_resync(&snap);
        assert_eq!(grid.scrollback_cached_extent(), Some((-2, -1)));
        assert_eq!(grid.scrollback_row(-2).unwrap().cells[0].text, "old");
        assert!(grid.scrollback_row(-9).is_none());
    }

    #[test]
    fn a_generation_change_drops_cached_history() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 10, 2));
        assert!(grid.is_scrollback_cached(-1));
        let mut next = delta(6, vec![], 10);
        next.scrollback_generation = 1;
        assert_eq!(grid.apply_delta(&next), DeltaOutcome::Applied);
        assert!(grid.scrollback_cache.is_empty());
        grid.merge_scrollback(&ScrollbackRows {
            snapshot: None,
            generation: 0,
            from_line: 8,
            rows: vec![Row::blank(4)],
        });
        assert!(grid.scrollback_cache.is_empty());
        grid.merge_scrollback(&ScrollbackRows {
            snapshot: None,
            generation: 1,
            from_line: 8,
            rows: vec![Row::blank(4)],
        });
        assert!(grid.is_scrollback_cached(-2));
    }

    #[test]
    fn a_column_patch_updates_only_the_named_cells() {
        let mut grid = CellGrid::from_snapshot(&snapshot(5, 4, 3, 0, 0));
        let mut patch_row = Row::blank(2);
        Arc::make_mut(&mut patch_row.cells)[0].text = "A".into();
        Arc::make_mut(&mut patch_row.cells)[1].text = "B".into();
        let mut next = delta(6, vec![], 0);
        next.patches = vec![domain::CellPatch {
            line: 1,
            first: 1,
            row: patch_row,
        }];
        assert_eq!(grid.apply_delta(&next), DeltaOutcome::Applied);
        assert_eq!(grid.visible[1].cells[0].text.as_str(), " ");
        assert_eq!(grid.visible[1].cells[1].text.as_str(), "A");
        assert_eq!(grid.visible[1].cells[2].text.as_str(), "B");
        assert_eq!(grid.visible[1].cells[3].text.as_str(), " ");
    }

    #[test]
    fn oversized_snapshot_tail_keeps_only_newest_cached_rows() {
        let tail = MAX_SCROLLBACK_CACHE_ROWS + 100;
        let snap = snapshot(5, 4, 3, tail as u64, tail);
        let mut grid = CellGrid::from_snapshot(&snap);
        assert_eq!(grid.scrollback_cache.len(), MAX_SCROLLBACK_CACHE_ROWS);
        assert_eq!(
            grid.scrollback_cached_extent(),
            Some((-(MAX_SCROLLBACK_CACHE_ROWS as i64), -1))
        );
        grid.apply_resync(&snap);
        assert_eq!(grid.scrollback_cache.len(), MAX_SCROLLBACK_CACHE_ROWS);
    }

    /// The usage event carries the whole set, so applying it must *replace*.
    /// Merging would strand a provider that stopped reporting at whatever
    /// number it last showed, which is worse than showing nothing.
    #[test]
    fn provider_usage_is_replaced_wholesale_not_merged() {
        let mut store = Store::new();
        let outcome = store.apply_event(&DaemonEvent::ProviderUsageChanged {
            usage: vec![sample_usage("claude", 40), sample_usage("codex", 10)],
        });
        assert_eq!(outcome, EventOutcome::Applied);
        assert_eq!(store.usage.len(), 2);

        let outcome = store.apply_event(&DaemonEvent::ProviderUsageChanged {
            usage: vec![sample_usage("claude", 55)],
        });
        assert_eq!(outcome, EventOutcome::Applied);
        assert_eq!(store.usage.len(), 1);
        assert_eq!(store.usage[0].windows[0].used_percent, 55);
    }

    #[test]
    fn pull_request_state_is_replaced_wholesale_not_merged() {
        let mut store = Store::new();
        let first = sample_pull_request_state("First refresh");
        let second = sample_pull_request_state("Second refresh");

        assert_eq!(
            store.apply_event(&DaemonEvent::PullRequestsUpdated {
                state: first.clone(),
            }),
            EventOutcome::Applied
        );
        assert_eq!(store.pull_requests, first);

        assert_eq!(
            store.apply_event(&DaemonEvent::PullRequestsUpdated {
                state: second.clone(),
            }),
            EventOutcome::Applied
        );
        assert_eq!(store.pull_requests, second);
        assert_eq!(store.pull_requests.pull_requests.len(), 1);
    }
}
