//! ACP permission policy seam; harness jobs currently launch through the CLI transport.

#![allow(dead_code)] // The ACP session transport is not wired to job launch yet.

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
