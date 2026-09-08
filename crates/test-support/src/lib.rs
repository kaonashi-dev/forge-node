//! # test-support
//!
//! Deterministic test fixtures shared across the Forge (ForgeNode) workspace
//! (§21 "Determinismo"). The plan splits terminal testing into two tiers, and
//! this crate provides the building blocks for both:
//!
//! - **Unit/terminal tests** drive a fake [`PtyBackend`](terminal_core::PtyBackend)
//!   with predefined bytes: [`FakePtyBackend`] replays a scripted byte stream as
//!   the child's output, captures input, and lets the test control termination —
//!   no real process, no threads.
//! - **Integration tests** use real PTYs and real git repositories:
//!   [`TempRepo`] builds an actual `git` repo in a temp directory, and the
//!   [`fake_agent`] helpers write real executable probe scripts.
//!
//! It also exposes small builders for shared runtime types
//! ([`resolved_env`] for [`domain::ResolvedEnvironment`]).
//!
//! This crate is a dev-only dependency of the other backend crates; keep it
//! dependency-light (`std` plus `domain`, `terminal-core`, and `tempfile`).

pub mod env;
pub mod fake_agent;
pub mod fake_pty;
pub mod temp_repo;

pub use env::{resolved_env, resolved_env_with};
pub use fake_agent::{fake_agent_in_tempdir, write_fake_agent, write_slow_fake_agent, FakeAgent};
pub use fake_pty::{FakePtyBackend, FakePtyHandle, DEFAULT_FAKE_PID};
pub use temp_repo::{init_repo, TempRepo};
