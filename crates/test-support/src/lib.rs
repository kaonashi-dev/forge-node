//! Shared test fixtures: fake PTY, fake agent binaries, throwaway git repos.
//!
//! Unit tests drive [`FakePtyBackend`] with scripted bytes — no process, no
//! threads. Integration tests use real PTYs and [`TempRepo`]. Dev-only;
//! keep the dependency set small.

pub mod env;
pub mod fake_agent;
pub mod fake_pty;
pub mod temp_repo;

pub use env::{resolved_env, resolved_env_with};
pub use fake_agent::{fake_agent_in_tempdir, write_fake_agent, write_slow_fake_agent, FakeAgent};
pub use fake_pty::{FakePtyBackend, FakePtyHandle, DEFAULT_FAKE_PID};
pub use temp_repo::{init_repo, TempRepo};
