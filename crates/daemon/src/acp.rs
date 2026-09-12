//! Agent Client Protocol worker transport (Phase 4 skeleton).
//!
//! The full JSON-RPC session loop (`initialize` → `session/new` →
//! `session/prompt` → `session/update`) lands when harness jobs prefer ACP
//! over CLI. What exists today:
//!
//! - [`domain::AcpSpec`] / [`domain::AcpPermissionPolicy`] on the descriptor
//! - [`domain::AcpPermissionPolicy::decide`] — the allow/deny matrix a future
//!   `session/request_permission` handler will call
//! - Claude's `acp` and Grok's `agent stdio` entries in `agents::builtins`
//!
//! Until spawn is wired, harness jobs keep using CLI with
//! `--permission-mode acceptEdits` as the same posture on a different wire.

#![allow(dead_code)] // Phase 4 spawn will call these; the matrix is already tested in domain.

use std::path::{Path, PathBuf};

use domain::{AcpPermissionDecision, AcpPermissionPolicy, AcpToolKind};

/// Decide a harness ACP permission request.
///
/// `cwd` is the worktree the job runs in; `extra_roots` is typically the
/// canonical harness state directory outside that worktree.
#[must_use]
pub fn decide_permission(
    policy: &AcpPermissionPolicy,
    kind: AcpToolKind,
    path: Option<&Path>,
    cwd: &Path,
    extra_roots: &[PathBuf],
) -> AcpPermissionDecision {
    policy.decide(kind, path, cwd, extra_roots)
}
