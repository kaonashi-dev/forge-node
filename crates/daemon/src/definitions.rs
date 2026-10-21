//! Go to definition for editor sessions.
//!
//! One lookup per editor at a time, on its own thread, so a slow provider
//! never holds the editor's control loop; a newer lookup from the same editor
//! cancels the older one. Places outside the checkout travel as opaque ids
//! minted here: an editor can follow a language server into
//! `~/.cargo/registry`, but it never sends a path back for the daemon to open.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::Duration;

use domain::{SessionId, WorkspaceId};
use fs_service::definition::DefinitionProvider;

use crate::config::DefinitionsConfig;

/// Lookups running at once across every editor. Past this a lookup is
/// answered as busy rather than queued behind the others.
const MAX_LOOKUPS: usize = 8;

/// External places remembered; the oldest id stops resolving past this.
const MAX_EXTERNAL: usize = 512;

const CTAGS_PROBE: Duration = Duration::from_secs(3);

/// What an editor asked, as the lookup thread needs it.
pub(crate) struct Request {
    pub request_id: u64,
    pub symbol: String,
    pub position: Option<editor_control::WirePosition>,
    /// The unsaved buffer, already checked against the document budget.
    pub text: Option<String>,
}

/// A file outside the checkout that a resolver named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ExternalPlace {
    pub id: u64,
    pub workspace_id: WorkspaceId,
    /// Canonical, absolute.
    pub path: PathBuf,
    /// What the editor and the tab show (`~/.cargo/registry/…`).
    pub display: String,
}

#[derive(Default)]
struct External {
    next: u64,
    places: VecDeque<ExternalPlace>,
}

/// The providers in front of the grep heuristic, and the lookups in flight.
pub(crate) struct Definitions {
    syntax: Option<Arc<syntax_index::SyntaxProvider>>,
    scip: Option<Arc<external_index::ScipProvider>>,
    ctags_executable: Option<String>,
    /// Probed on first use: a subprocess at daemon start for a feature most
    /// people leave off would be paid by everyone.
    ctags: OnceLock<Option<Arc<external_index::CtagsProvider>>>,
    timeout: Duration,
    lookups: Mutex<HashMap<SessionId, Arc<AtomicBool>>>,
    running: AtomicUsize,
    external: Mutex<External>,
}

impl Definitions {
    pub(crate) fn new(config: &DefinitionsConfig) -> Self {
        Self {
            syntax: config
                .syntax
                .then(|| Arc::new(syntax_index::SyntaxProvider::new())),
            scip: config.scip.then(|| {
                Arc::new(external_index::ScipProvider::new(
                    config.scip_indexes.clone(),
                ))
            }),
            ctags_executable: config.ctags.then(|| config.ctags_executable.clone()),
            ctags: OnceLock::new(),
            timeout: config.timeout(),
            lookups: Mutex::new(HashMap::new()),
            running: AtomicUsize::new(0),
            external: Mutex::new(External::default()),
        }
    }

    #[must_use]
    pub(crate) fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Start a lookup for `session`, cancelling the one it replaces.
    ///
    /// `None` when [`MAX_LOOKUPS`] are already running.
    pub(crate) fn begin(&self, session: SessionId) -> Option<Arc<AtomicBool>> {
        if self.running.fetch_add(1, Ordering::AcqRel) >= MAX_LOOKUPS {
            self.running.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        let flag = Arc::new(AtomicBool::new(false));
        let previous = self
            .lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(session, Arc::clone(&flag));
        if let Some(previous) = previous {
            previous.store(true, Ordering::Relaxed);
        }
        Some(flag)
    }

    /// A lookup thread is done, whether or not it was cancelled.
    pub(crate) fn finish(&self, session: SessionId, flag: &Arc<AtomicBool>) {
        self.running.fetch_sub(1, Ordering::AcqRel);
        let mut lookups = self.lookups.lock().unwrap_or_else(PoisonError::into_inner);
        if lookups
            .get(&session)
            .is_some_and(|current| Arc::ptr_eq(current, flag))
        {
            lookups.remove(&session);
        }
    }

    /// The editor is gone: whatever it asked is no longer worth answering.
    pub(crate) fn forget_session(&self, session: SessionId) {
        if let Some(flag) = self
            .lookups
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&session)
        {
            flag.store(true, Ordering::Relaxed);
        }
    }

    /// The providers after `first` (a language server, when one is ready to
    /// be asked), best first. The grep heuristic is always last.
    /// `path_entries` is the login shell's `PATH`, which is where a ctags the
    /// person installed lives.
    pub(crate) fn chain<'a>(
        &'a self,
        first: Option<&'a dyn DefinitionProvider>,
        path_entries: &[PathBuf],
    ) -> Vec<&'a dyn DefinitionProvider> {
        let mut chain: Vec<&dyn DefinitionProvider> = Vec::with_capacity(5);
        chain.extend(first);
        if let Some(scip) = &self.scip {
            chain.push(scip.as_ref());
        }
        if let Some(syntax) = &self.syntax {
            chain.push(syntax.as_ref());
        }
        if let Some(ctags) = self.ctags(path_entries) {
            chain.push(ctags);
        }
        chain.push(&fs_service::definition::GrepProvider);
        chain
    }

    fn ctags(&self, path_entries: &[PathBuf]) -> Option<&external_index::CtagsProvider> {
        let executable = self.ctags_executable.as_deref()?;
        self.ctags
            .get_or_init(|| {
                let resolved = if Path::new(executable).components().count() == 1 {
                    agents::detection::find_executable(executable, path_entries)
                        .unwrap_or_else(|| PathBuf::from(executable))
                } else {
                    PathBuf::from(executable)
                };
                match external_index::CtagsProvider::probe(&resolved, CTAGS_PROBE) {
                    Ok(provider) => Some(Arc::new(provider)),
                    Err(error) => {
                        tracing::warn!(%error, executable, "[definitions] ctags is not usable");
                        None
                    }
                }
            })
            .as_deref()
    }

    /// Remember a place outside the checkout and name it with an id.
    pub(crate) fn mint(&self, workspace_id: WorkspaceId, path: PathBuf) -> ExternalPlace {
        let mut external = self.external.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = external
            .places
            .iter()
            .find(|place| place.workspace_id == workspace_id && place.path == path)
        {
            return known.clone();
        }
        external.next += 1;
        let place = ExternalPlace {
            id: external.next,
            workspace_id,
            display: display_path(&path),
            path,
        };
        if external.places.len() >= MAX_EXTERNAL {
            external.places.pop_front();
        }
        external.places.push_back(place.clone());
        place
    }

    /// The place an id was minted for, only within the workspace it was minted in.
    pub(crate) fn external(&self, workspace_id: WorkspaceId, id: u64) -> Option<ExternalPlace> {
        self.external
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .places
            .iter()
            .find(|place| place.id == id && place.workspace_id == workspace_id)
            .cloned()
    }
}

/// `path` with the home directory written as `~`.
#[must_use]
pub(crate) fn display_path(path: &Path) -> String {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    match home
        .as_deref()
        .and_then(|home| path.strip_prefix(home).ok())
    {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn definitions() -> Definitions {
        Definitions::new(&DefinitionsConfig {
            syntax: false,
            ..DefinitionsConfig::default()
        })
    }

    #[test]
    fn a_newer_lookup_cancels_the_older_one_and_the_cap_refuses() {
        let defs = definitions();
        let session = SessionId::new();
        let first = defs.begin(session).unwrap();
        let second = defs.begin(session).unwrap();
        assert!(first.load(Ordering::Relaxed));
        assert!(!second.load(Ordering::Relaxed));
        defs.finish(session, &first);
        assert!(
            defs.lookups.lock().unwrap().contains_key(&session),
            "the finished older lookup must not unregister the newer one"
        );
        defs.finish(session, &second);
        let held: Vec<_> = (0..MAX_LOOKUPS)
            .map(|_| defs.begin(SessionId::new()).unwrap())
            .collect();
        assert!(defs.begin(SessionId::new()).is_none());
        assert_eq!(held.len(), MAX_LOOKUPS);
    }

    #[test]
    fn external_ids_are_stable_scoped_to_their_workspace_and_bounded() {
        let defs = definitions();
        let workspace = WorkspaceId::new();
        let path = PathBuf::from("/opt/registry/src/lib.rs");
        let place = defs.mint(workspace, path.clone());
        assert_eq!(defs.mint(workspace, path).id, place.id);
        assert_eq!(defs.external(workspace, place.id), Some(place.clone()));
        assert_eq!(defs.external(WorkspaceId::new(), place.id), None);
        for n in 0..MAX_EXTERNAL {
            defs.mint(workspace, PathBuf::from(format!("/x/{n}.rs")));
        }
        assert_eq!(defs.external(workspace, place.id), None);
    }

    #[test]
    fn the_home_directory_reads_as_a_tilde() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let inside = PathBuf::from(home).join(".cargo/registry/src/x.rs");
        assert_eq!(display_path(&inside), "~/.cargo/registry/src/x.rs");
        assert_eq!(display_path(Path::new("/etc/x")), "/etc/x");
    }
}
