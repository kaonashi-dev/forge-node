//! A project's review knowledge base, on the review store's connection and
//! never under the core lock. Everything stored is redacted first.

use codebase_graph::{EdgeKind, Graph, NodeKind};
use domain::{KbFile, ProjectId, Timestamp, WorkspaceId};
use persistence::repositories::kb::{KbEntry, DAEMON, USER};
use protocol::{ProtocolError, Response};

use crate::core::{db_err, Daemon};

const PATTERN_DAYS: i64 = 90;

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

fn stamp(unix: i64) -> Option<Timestamp> {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .map(Timestamp::from_offset)
}

impl Daemon {
    fn kb_project(&self, workspace_id: WorkspaceId) -> Result<ProjectId, ProtocolError> {
        self.lock()
            .workspaces
            .get(&workspace_id)
            .map(|ws| ws.project_id)
            .ok_or_else(|| ProtocolError::not_found("workspace"))
    }

    /// The daemon's `architecture.md` drawn again from `graph`; a person's
    /// entry beside it is left alone.
    pub(crate) fn refresh_architecture(&self, project: ProjectId, graph: &Graph) {
        if !self.config.review.kb_enabled {
            return;
        }
        let files: Vec<(String, String)> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::File)
            .map(|node| (node.path.clone(), node.lang.clone()))
            .collect();
        let index = graph.index();
        let imports: Vec<(String, String)> = graph
            .edges
            .iter()
            .filter(|edge| edge.kind == EdgeKind::Imports)
            .filter_map(|edge| {
                Some((
                    index.get(&edge.src)?.path.clone(),
                    index.get(&edge.dst)?.path.clone(),
                ))
            })
            .collect();
        let text = review::redact(&knowledge_base::architecture(
            &files,
            &imports,
            &graph.git_oid,
        ))
        .0;
        if let Err(error) = self.review_store.set_kb_entry(
            project,
            knowledge_base::ARCHITECTURE,
            &text,
            DAEMON,
            now(),
        ) {
            tracing::warn!(%error, "could not store the architecture entry");
        }
    }

    /// A completed run's patterns, deduplicated against the live ones and kept
    /// 90 days; those over the knowledge base's caps are not stored.
    pub(crate) fn remember_patterns(&self, project: ProjectId, patterns: &[String]) {
        if patterns.is_empty() || !self.config.review.kb_enabled {
            return;
        }
        let redacted: Vec<String> = patterns.iter().map(|p| review::redact(p).0).collect();
        let at = now();
        if let Err(error) =
            self.review_store
                .add_kb_patterns(project, &redacted, at, at + PATTERN_DAYS * 86_400)
        {
            tracing::warn!(%error, "could not store review patterns");
        }
    }

    pub(crate) fn prune_kb(&self) {
        let at = now();
        if let Err(error) = self
            .review_store
            .prune_kb(at, at - knowledge_base::MAX_AGE_DAYS * 86_400)
        {
            tracing::warn!(%error, "could not prune the knowledge base");
        }
    }

    /// The four files: a person's `architecture.md` before the daemon's.
    pub(crate) fn kb_files(&self, project: ProjectId) -> Result<Vec<KbFile>, ProtocolError> {
        let entries = self
            .review_store
            .kb_entries(project, now())
            .map_err(db_err)?;
        let feedback = self.review_store.feedback(project).map_err(db_err)?;
        let suppressions = review::suppressions(&feedback, now());
        let dismissed = self.review_store.false_positives(project).map_err(db_err)?;
        let architecture = entries
            .iter()
            .filter(|e| e.file == knowledge_base::ARCHITECTURE)
            .min_by_key(|e| e.source != USER);
        let patterns: Vec<&KbEntry> = entries
            .iter()
            .filter(|e| e.file == knowledge_base::PATTERNS)
            .collect();
        let mut files = Vec::new();
        if let Some(entry) = architecture {
            files.push(KbFile {
                file: entry.file.clone(),
                content: entry.content.clone(),
                source: entry.source.clone(),
                updated_at: stamp(entry.created_at),
            });
        }
        files.push(KbFile {
            file: knowledge_base::PATTERNS.into(),
            content: knowledge_base::patterns_json(
                &patterns
                    .iter()
                    .map(|e| e.content.clone())
                    .collect::<Vec<_>>(),
            ),
            source: if patterns.iter().any(|e| e.source == USER) {
                USER
            } else {
                DAEMON
            }
            .into(),
            updated_at: patterns.iter().map(|e| e.created_at).max().and_then(stamp),
        });
        files.push(KbFile {
            file: knowledge_base::LEARNED_RULES.into(),
            content: knowledge_base::learned_rules(&suppressions),
            source: "derived".into(),
            updated_at: None,
        });
        files.push(KbFile {
            file: knowledge_base::FALSE_POSITIVES.into(),
            content: knowledge_base::false_positives(&dismissed),
            source: "derived".into(),
            updated_at: None,
        });
        Ok(files)
    }

    /// `<kb-N>` excerpts for a run's changed paths.
    pub(crate) fn kb_excerpts(
        &self,
        project: ProjectId,
        paths: &[String],
        budget: usize,
    ) -> Vec<(String, String)> {
        let files = match self.kb_files(project) {
            Ok(files) => files,
            Err(error) => {
                tracing::warn!(%error, "could not read the knowledge base");
                return Vec::new();
            }
        };
        let text = |name: &str| {
            files
                .iter()
                .find(|f| f.file == name)
                .map(|f| f.content.clone())
                .unwrap_or_default()
        };
        knowledge_base::excerpts(
            &text(knowledge_base::ARCHITECTURE),
            &text(knowledge_base::LEARNED_RULES),
            paths,
            budget,
        )
    }

    pub(crate) fn get_kb(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let project = self.kb_project(workspace_id)?;
        Ok(Response::Kb(self.kb_files(project)?))
    }

    pub(crate) fn export_kb(&self, workspace_id: WorkspaceId) -> Result<Response, ProtocolError> {
        let project = self.kb_project(workspace_id)?;
        Ok(Response::KbExport(knowledge_base::export(
            &self.kb_files(project)?,
        )))
    }

    pub(crate) fn update_kb_entry(
        &self,
        workspace_id: WorkspaceId,
        file: &str,
        content: &str,
    ) -> Result<Response, ProtocolError> {
        let project = self.kb_project(workspace_id)?;
        let content = review::redact(content).0;
        let others: usize = self
            .review_store
            .kb_entries(project, now())
            .map_err(db_err)?
            .iter()
            .filter(|e| e.file != file)
            .map(|e| e.content.len())
            .sum();
        knowledge_base::check_write(file, &content, others)
            .map_err(ProtocolError::invalid_request)?;
        if file == knowledge_base::PATTERNS {
            let patterns =
                knowledge_base::parse_patterns(&content).map_err(ProtocolError::invalid_request)?;
            self.review_store
                .replace_kb_items(project, file, &patterns, USER, now())
                .map_err(db_err)?;
        } else {
            self.review_store
                .set_kb_entry(project, file, &content, USER, now())
                .map_err(db_err)?;
        }
        Ok(Response::Ack)
    }

    pub(crate) fn clear_kb_entry(
        &self,
        workspace_id: WorkspaceId,
        file: &str,
    ) -> Result<Response, ProtocolError> {
        let project = self.kb_project(workspace_id)?;
        if !knowledge_base::STORED.contains(&file) {
            return Err(ProtocolError::invalid_request(format!(
                "{file} is not a stored knowledge-base file"
            )));
        }
        self.review_store
            .clear_kb_entry(project, file)
            .map_err(db_err)?;
        Ok(Response::Ack)
    }
}
