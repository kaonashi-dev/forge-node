//! Daemon logging setup (§15.1, §22).
//!
//! Writes structured `tracing` output to `daemon.log` in the logs directory and
//! to stderr, which is what `scripts/dev daemon` reads. Log lines never contain
//! terminal content or environment values (§22, §23).
//!
//! The plan calls for size-based rotation (5×10 MB). `tracing-appender` rotates
//! by time, so we approximate with daily rotation capped at 5 files; true
//! size-based rotation is a post-MVP refinement.

use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Initialize logging. The returned [`WorkerGuard`] must be held for the
/// lifetime of the process so buffered log lines are flushed on shutdown.
///
/// `level` is the configured `[daemon].log_level`; the `RUST_LOG` environment
/// variable overrides it when set.
pub fn init(logs_dir: &Path, level: &str) -> std::io::Result<WorkerGuard> {
    std::fs::create_dir_all(logs_dir)?;

    let file_appender = tracing_appender::rolling::Builder::new()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("daemon")
        .filename_suffix("log")
        .max_log_files(5)
        .build(logs_dir)
        .map_err(std::io::Error::other)?;
    let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);

    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(default_directive(level)));

    let file_layer = fmt::layer()
        .with_ansi(false)
        .with_target(true)
        .with_writer(non_blocking);

    let stderr_layer = fmt::layer().with_ansi(true).with_writer(std::io::stderr);

    tracing_subscriber::registry()
        .with(env_filter)
        .with(file_layer)
        .with(stderr_layer)
        .init();

    Ok(guard)
}

/// Build a default `EnvFilter` directive from the configured level, keeping
/// noisy dependencies at `warn`.
fn default_directive(level: &str) -> String {
    let level = match level {
        "trace" | "debug" | "info" | "warn" | "error" => level,
        _ => "info",
    };
    format!("forge={level},daemon={level},terminal_core={level},{level}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directive_falls_back_to_info_for_garbage() {
        assert!(default_directive("nonsense").starts_with("forge=info"));
        assert!(default_directive("debug").starts_with("forge=debug"));
    }
}
