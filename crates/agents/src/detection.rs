//! Provider detection (§13.1).
//!
//! For a descriptor we locate a candidate binary on the resolved PATH, run its
//! version probe under a hard timeout, and verify the output. The probe is
//! spawned with piped stdio and drained on dedicated threads; a
//! [`std::sync::mpsc`] channel with [`recv_timeout`](std::sync::mpsc::Receiver::recv_timeout)
//! bounds the wait so a hung probe can never block the caller.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use domain::{
    AgentDescriptor, DetectionResult, DetectionStatus, ResolvedEnvironment, Timestamp, VersionProbe,
};

/// Maximum bytes read from a version probe's stdout or stderr (§13.1).
const MAX_PROBE_OUTPUT: usize = 64 * 1024;

/// Detect a single provider following the §13.1 algorithm.
///
/// - `override_path`, when present, is used directly and skips the PATH search
///   (step 1); it is still verified by the version probe.
/// - Otherwise every candidate in [`AgentDescriptor::binary_candidates`] is
///   tried in order: a rejected candidate falls through to the next one, an
///   installed or timed-out candidate is terminal.
///
/// `checked_at` is stamped when detection finishes.
#[must_use]
pub fn detect(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    override_path: Option<&Path>,
) -> DetectionResult {
    let status = detect_status(descriptor, env, override_path);
    DetectionResult {
        provider_id: descriptor.id.clone(),
        status,
        checked_at: Timestamp::now(),
    }
}

fn detect_status(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    override_path: Option<&Path>,
) -> DetectionStatus {
    // Step 1: an explicit override skips the PATH search entirely.
    if let Some(over) = override_path {
        return probe_and_verify(descriptor, env, over, over.display().to_string());
    }

    // Steps 2-4: walk candidates in preference order.
    let mut last_rejected: Option<DetectionStatus> = None;
    for candidate in &descriptor.binary_candidates {
        let Some(program) = find_executable(candidate, &env.path_entries) else {
            continue;
        };
        match probe_and_verify(descriptor, env, &program, candidate.clone()) {
            rejected @ DetectionStatus::Rejected { .. } => {
                // Step 4: a rejected candidate falls through to the next one,
                // but we remember it in case none succeed.
                last_rejected = Some(rejected);
            }
            terminal => return terminal, // Installed or ProbeTimeout
        }
    }

    // Step 5: nothing usable. Report the last rejection if we have one (more
    // informative for the picker), otherwise NotFound.
    last_rejected.unwrap_or(DetectionStatus::NotFound)
}

/// Run the version probe for one program and turn its outcome into a status.
fn probe_and_verify(
    descriptor: &AgentDescriptor,
    env: &ResolvedEnvironment,
    program: &Path,
    candidate: String,
) -> DetectionStatus {
    match run_probe(program, &descriptor.version_probe, env) {
        ProbeOutcome::TimedOut => DetectionStatus::ProbeTimeout,
        ProbeOutcome::Failed(err) => DetectionStatus::Rejected {
            candidate,
            reason: format!("failed to run version probe: {err}"),
        },
        ProbeOutcome::Completed { stdout, stderr } => {
            let combined = combine_output(&stdout, &stderr);

            // Step 4: enforce the expected marker, case-insensitively.
            if let Some(expect) = descriptor.version_probe.expect_substring.as_deref() {
                let expect = expect.to_lowercase();
                let output_matches = combined.to_lowercase().contains(&expect);
                let unambiguous_name = program
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase().contains(&expect))
                    .unwrap_or(false);
                if !output_matches && !unambiguous_name {
                    return DetectionStatus::Rejected {
                        candidate,
                        reason: format!(
                            "version output did not contain the expected marker `{expect}`"
                        ),
                    };
                }
            }

            // Step 5: version = first non-empty line of the probe output.
            let version = combined
                .lines()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map(str::to_owned);
            DetectionStatus::Installed {
                executable: program.to_path_buf(),
                version,
            }
        }
    }
}

/// The result of running a version probe.
enum ProbeOutcome {
    Completed { stdout: Vec<u8>, stderr: Vec<u8> },
    TimedOut,
    Failed(std::io::Error),
}

/// Run a descriptor's version probe.
fn run_probe(program: &Path, probe: &VersionProbe, env: &ResolvedEnvironment) -> ProbeOutcome {
    run_args(program, &probe.args, u64::from(probe.timeout_ms), env)
}

/// Run `program` with `args` under the resolved login environment, capped by
/// `timeout_ms`, and return its captured stdout.
///
/// `None` covers every failure — spawn, timeout, non-zero exit — because the
/// only other caller, [`crate::usage`], treats them all the same way: no
/// reading rather than a wrong one.
pub(crate) fn run_to_completion(
    program: &Path,
    args: &[String],
    timeout_ms: u64,
    env: &ResolvedEnvironment,
) -> Option<Vec<u8>> {
    match run_args(program, args, timeout_ms, env) {
        ProbeOutcome::Completed { stdout, .. } => Some(stdout),
        ProbeOutcome::TimedOut | ProbeOutcome::Failed(_) => None,
    }
}

/// Spawn `program` with `args`, capturing stdout+stderr, and wait at most
/// `timeout_ms`. On timeout the child is killed and reaped.
///
/// stdout and stderr are drained on separate threads so a program that fills
/// one pipe while we block on the other cannot deadlock. The child stays in
/// this thread throughout so it can be killed on timeout; a notifier thread
/// reports completion through an mpsc channel and `recv_timeout` bounds the
/// wait.
fn run_args(
    program: &Path,
    args: &[String],
    timeout_ms: u64,
    env: &ResolvedEnvironment,
) -> ProbeOutcome {
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // The probe runs under the resolved login environment (§12), not the
    // daemon's own environment.
    cmd.env_clear();
    for (key, value) in &env.vars {
        cmd.env(key, value);
    }
    // Ensure a sane PATH even if the resolved environment lacks one, so the
    // probed binary can still find any helpers it shells out to.
    if !env.vars.iter().any(|(k, _)| k == "PATH") {
        if let Ok(joined) = std::env::join_paths(env.path_entries.iter()) {
            cmd.env("PATH", joined);
        }
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(err) => return ProbeOutcome::Failed(err),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let out_handle = thread::spawn(move || read_all(stdout));
    let err_handle = thread::spawn(move || read_all(stderr));

    // A notifier thread joins both readers and reports completion; both pipes
    // closing means the process has finished (or is finishing).
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let out = out_handle.join().unwrap_or_default();
        let err = err_handle.join().unwrap_or_default();
        let _ = tx.send((out, err));
    });

    let timeout = Duration::from_millis(timeout_ms);
    match rx.recv_timeout(timeout) {
        Ok((stdout, stderr)) => match child.wait() {
            Ok(status) if status.success() => ProbeOutcome::Completed { stdout, stderr },
            Ok(status) => ProbeOutcome::Failed(std::io::Error::other(format!(
                "version probe exited with {status}"
            ))),
            Err(error) => ProbeOutcome::Failed(error),
        },
        Err(RecvTimeoutError::Timeout) => {
            // A hung probe: killing the child closes the pipes so the reader
            // and notifier threads unwind on their own.
            let _ = child.kill();
            let _ = child.wait();
            ProbeOutcome::TimedOut
        }
        Err(RecvTimeoutError::Disconnected) => {
            let _ = child.kill();
            let _ = child.wait();
            ProbeOutcome::Failed(std::io::Error::other(
                "version probe reader thread disconnected",
            ))
        }
    }
}

/// Read a captured pipe into a buffer, tolerating read errors.
///
/// Capped at [`MAX_PROBE_OUTPUT`]: the probed program is whatever happens to sit
/// on `PATH` under a candidate name, and a misidentified binary that streams
/// would otherwise grow this buffer without bound inside the daemon. Only the
/// first non-empty line is ever used (§13.1), so the cap costs nothing.
fn read_all(stream: Option<impl Read>) -> Vec<u8> {
    let mut buf = Vec::new();
    if let Some(stream) = stream {
        let _ = stream.take(MAX_PROBE_OUTPUT as u64).read_to_end(&mut buf);
    }
    buf
}

/// Decode and concatenate stdout then stderr (lossily), separated by a newline.
fn combine_output(stdout: &[u8], stderr: &[u8]) -> String {
    let mut combined = String::from_utf8_lossy(stdout).into_owned();
    if !stderr.is_empty() {
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&String::from_utf8_lossy(stderr));
    }
    combined
}

/// Search `path_entries` in order for an executable file named `name` and
/// return the first hit as an absolute-when-the-entry-is path.
pub(crate) fn find_executable(name: &str, path_entries: &[PathBuf]) -> Option<PathBuf> {
    path_entries.iter().find_map(|dir| {
        let candidate = dir.join(name);
        is_executable_file(&candidate).then_some(candidate)
    })
}

/// Whether `path` is a regular file with an executable bit set (any bit on
/// unix; existence-as-file elsewhere).
fn is_executable_file(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::builtins;
    use crate::test_support::{echo_script, env_with_path, write_script};
    use domain::DetectionStatus;

    #[test]
    fn foreign_agent_alone_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        // On this machine `agent` on PATH is actually the Grok CLI.
        write_script(dir.path(), "agent", &echo_script("grok-cli 1.2"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("cursor").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Rejected { candidate, .. } => assert_eq!(candidate, "agent"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn rejects_foreign_agent_then_detects_cursor_agent() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "agent", &echo_script("grok-cli 1.2"));
        write_script(
            dir.path(),
            "cursor-agent",
            &echo_script("cursor agent 2026.1"),
        );
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("cursor").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Installed {
                executable,
                version,
            } => {
                // Detection fell through the rejected `agent` to `cursor-agent`.
                assert_eq!(executable.file_name().unwrap(), "cursor-agent");
                assert_eq!(version.as_deref(), Some("cursor agent 2026.1"));
            }
            other => panic!("expected Installed via cursor-agent, got {other:?}"),
        }
    }

    #[test]
    fn cursor_agent_name_is_enough_when_version_is_numeric() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "agent", &echo_script("grok-cli 1.2"));
        write_script(
            dir.path(),
            "cursor-agent",
            &echo_script("2026.08.04-aaa8809"),
        );
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("cursor").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Installed {
                executable,
                version,
            } => {
                assert_eq!(executable.file_name().unwrap(), "cursor-agent");
                assert_eq!(version.as_deref(), Some("2026.08.04-aaa8809"));
            }
            other => panic!("expected Installed via cursor-agent, got {other:?}"),
        }
    }

    #[test]
    fn falls_through_to_the_lower_priority_candidate() {
        // §7.5: `opencode2` is the lower-priority candidate; it is only reached
        // when `opencode` is absent from PATH.
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode2", &echo_script("opencode 2.0.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("opencode").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Installed { executable, .. } => {
                assert_eq!(executable.file_name().unwrap(), "opencode2");
            }
            other => panic!("expected Installed via opencode2, got {other:?}"),
        }
    }

    #[test]
    fn first_candidate_wins_when_both_are_present() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "opencode", &echo_script("opencode 1.0.0"));
        write_script(dir.path(), "opencode2", &echo_script("opencode 2.0.0"));
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("opencode").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Installed {
                executable,
                version,
            } => {
                assert_eq!(executable.file_name().unwrap(), "opencode");
                assert_eq!(version.as_deref(), Some("opencode 1.0.0"));
            }
            other => panic!("expected Installed via opencode, got {other:?}"),
        }
    }

    #[test]
    fn path_entries_are_searched_in_order() {
        // §13.1 step 2: the candidate is looked up along `path_entries`, first
        // match wins — the same precedence a shell gives PATH.
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_script(first.path(), "claude", &echo_script("Claude Code 1.0"));
        write_script(second.path(), "claude", &echo_script("Claude Code 2.0"));
        let env = env_with_path(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);

        let result = detect(&builtins::builtin("claude").unwrap(), &env, None);
        match result.status {
            DetectionStatus::Installed { version, .. } => {
                assert_eq!(version.as_deref(), Some("Claude Code 1.0"));
            }
            other => panic!("expected Installed, got {other:?}"),
        }
    }

    #[test]
    fn a_non_executable_file_is_not_a_candidate() {
        // A plain file named like the CLI must not be mistaken for it; without
        // the executable-bit check the probe would fail with EACCES and the
        // provider would be reported Rejected rather than NotFound.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("claude"), "not a program").unwrap();
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("claude").unwrap(), &env, None);
        assert!(matches!(result.status, DetectionStatus::NotFound));
    }

    #[test]
    fn not_found_when_no_candidate_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let env = env_with_path(vec![dir.path().to_path_buf()]);
        let result = detect(&builtins::builtin("claude").unwrap(), &env, None);
        assert!(matches!(result.status, DetectionStatus::NotFound));
    }

    #[test]
    fn probe_that_hangs_times_out() {
        let dir = tempfile::tempdir().unwrap();
        write_script(dir.path(), "claude", "#!/bin/sh\nsleep 3\n");
        // System dirs are on PATH so the script's own `sleep` resolves; the
        // candidate is still found in the temp dir first.
        let env = env_with_path(vec![
            dir.path().to_path_buf(),
            PathBuf::from("/usr/bin"),
            PathBuf::from("/bin"),
        ]);

        let mut descriptor = builtins::builtin("claude").unwrap();
        descriptor.version_probe.timeout_ms = 200;

        let result = detect(&descriptor, &env, None);
        assert!(matches!(result.status, DetectionStatus::ProbeTimeout));
    }

    #[test]
    fn a_nonzero_version_probe_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        write_script(
            dir.path(),
            "claude",
            "#!/bin/sh\necho 'Claude Code 1.0'\nexit 7\n",
        );
        let env = env_with_path(vec![dir.path().to_path_buf()]);

        let result = detect(&builtins::builtin("claude").unwrap(), &env, None);
        assert!(matches!(result.status, DetectionStatus::Rejected { .. }));
    }

    #[test]
    fn override_is_probed_directly_without_path_search() {
        let dir = tempfile::tempdir().unwrap();
        let over = write_script(dir.path(), "my-cursor", &echo_script("cursor build 9"));
        // Empty PATH: only the override should be consulted.
        let env = env_with_path(vec![]);

        let result = detect(&builtins::builtin("cursor").unwrap(), &env, Some(&over));
        match result.status {
            DetectionStatus::Installed {
                executable,
                version,
            } => {
                assert_eq!(executable, over);
                assert_eq!(version.as_deref(), Some("cursor build 9"));
            }
            other => panic!("expected Installed via override, got {other:?}"),
        }
    }
}
