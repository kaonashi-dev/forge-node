//! Idle-session policy: what to do with a session nobody is using.
//!
//! An agent CLI left running is not free. It holds a PTY, an OS thread, a VT
//! engine with up to `terminal.scrollback_lines` rows of grid behind it, and —
//! for the agents that poll — a live provider connection. A day of work leaves
//! a tail of sessions that finished their task hours ago and that nothing is
//! attached to. This module decides which ones to say something about and,
//! when the user has opted in, which ones to stop.
//!
//! The policy is deliberately a pure function of four inputs — the session, the
//! current time, whether a client is attached, and whether it was already
//! warned — so the interesting behaviour is unit-testable without a PTY. The
//! daemon's sweeper ([`crate::core::Daemon`]) supplies those inputs and
//! performs the effects.
//!
//! Two distinct clocks feed it, and conflating them would be wrong:
//!
//! - **Inactivity** ([`Session::idle_for`]) — no terminal output and no input.
//!   This is "nobody is using it". It resets whenever the session does
//!   anything.
//! - **Age** ([`Session::age`]) — time since the session was created. This is
//!   "it has been open for too long", and it keeps growing while the session is
//!   busy. A week-old orchestrator that is working right now is worth
//!   mentioning, but it is emphatically not worth killing.
//!
//! Only inactivity can lead to [`IdleAction::Stop`]; age never does.

use std::time::Duration;

use domain::{Session, SessionKind, Timestamp};

/// Why a session was flagged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleReason {
    /// No terminal input or output for this long.
    Inactive(Duration),
    /// Open for this long, regardless of how busy it has been.
    LongRunning(Duration),
}

impl IdleReason {
    /// The duration behind the verdict, for rendering.
    #[must_use]
    pub fn duration(self) -> Duration {
        match self {
            IdleReason::Inactive(d) | IdleReason::LongRunning(d) => d,
        }
    }
}

/// What the sweeper should do about one session on this pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdleAction {
    /// Nothing: the session is in use, or was already flagged.
    Keep,
    /// Surface it once. The session keeps running.
    Warn(IdleReason),
    /// Stop it: quiet past the shutdown threshold, and the user opted in.
    Stop(IdleReason),
}

/// Thresholds from `[sessions]` in `config.toml` (§15.4).
///
/// `None` means "off"; the config layer maps `0` to `None` so a user can
/// disable a rule without knowing a sentinel value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct IdlePolicy {
    /// Inactivity after which a live session is reported once.
    pub warn_after: Option<Duration>,
    /// Inactivity after which a live session is stopped. Off by default:
    /// killing a user's process is never something to infer.
    pub stop_after: Option<Duration>,
    /// Age after which a live session is reported once, however busy it is.
    pub long_running_after: Option<Duration>,
    /// Allow [`IdleAction::Stop`] even while a client is attached. Off by
    /// default — a terminal the user is looking at is in use by definition.
    pub stop_attached: bool,
    /// Apply the policy to shell sessions too. Off by default: a shell sitting
    /// at a prompt is the normal resting state of a terminal, not a leak.
    pub include_shells: bool,
}

impl IdlePolicy {
    /// Whether any rule is enabled at all. The daemon skips starting the
    /// sweeper thread when this is `false`.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.warn_after.is_some() || self.stop_after.is_some() || self.long_running_after.is_some()
    }

    /// Decide what to do about `session` at `now`.
    ///
    /// `attached` is whether any client is currently subscribed to the
    /// session's terminal; `warned` is whether a previous pass already reported
    /// it, which is what keeps a warning to exactly one notice per quiet spell.
    ///
    /// Terminal-state sessions (exited, failed, orphaned) always return
    /// [`IdleAction::Keep`]: they hold no process, so there is nothing to
    /// reclaim and nothing to warn about.
    #[must_use]
    pub fn evaluate(
        &self,
        session: &Session,
        now: Timestamp,
        attached: bool,
        warned: bool,
    ) -> IdleAction {
        if !session.state.is_active() {
            return IdleAction::Keep;
        }
        if session.kind == SessionKind::Shell && !self.include_shells {
            return IdleAction::Keep;
        }

        let idle_for = session.idle_for(now);

        // Stopping outranks warning: past the stop threshold there is no point
        // reporting a session we are about to end.
        if let Some(stop_after) = self.stop_after {
            if idle_for >= stop_after && (!attached || self.stop_attached) {
                return IdleAction::Stop(IdleReason::Inactive(idle_for));
            }
        }

        // One notice per quiet spell. `warned` is cleared by the caller as soon
        // as the session drops back below every threshold, so a session that
        // goes quiet, wakes up and goes quiet again is reported twice — which
        // is right, they are two different events.
        if warned {
            return IdleAction::Keep;
        }

        if let Some(warn_after) = self.warn_after {
            if idle_for >= warn_after {
                return IdleAction::Warn(IdleReason::Inactive(idle_for));
            }
        }

        // Age is checked last: when a session is both old and quiet, the quiet
        // is the more actionable half of the story.
        if let Some(long_running_after) = self.long_running_after {
            let age = session.age(now);
            if age >= long_running_after {
                return IdleAction::Warn(IdleReason::LongRunning(age));
            }
        }

        IdleAction::Keep
    }

    /// Whether a session has dropped back below every warning threshold, so the
    /// next quiet spell is reported again.
    ///
    /// Age never falls back below its threshold, so a long-running session
    /// stays flagged until it ends — reporting it every sweep would be noise.
    #[must_use]
    pub fn is_settled(&self, session: &Session, now: Timestamp) -> bool {
        if !session.state.is_active() {
            return true;
        }
        let below_idle = self
            .warn_after
            .is_none_or(|warn_after| session.idle_for(now) < warn_after);
        let below_age = self
            .long_running_after
            .is_none_or(|after| session.age(now) < after);
        below_idle && below_age
    }
}

/// Render a duration the way a notice should read: `2h 5m`, `45m`, `30s`.
///
/// Deliberately coarse — a notice about a session idle for two hours does not
/// improve by naming the seconds.
#[must_use]
pub fn humanize(d: Duration) -> String {
    let secs = d.as_secs();
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    match (h, m) {
        (0, 0) => format!("{s}s"),
        (0, _) => format!("{m}m"),
        (_, 0) => format!("{h}h"),
        _ => format!("{h}h {m}m"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use domain::{SessionId, SessionRole, SessionState, SessionTitle, WorkspaceId};

    fn at(secs_ago: u64) -> Timestamp {
        Timestamp::from_offset(
            Timestamp::now().as_offset() - time::Duration::seconds(secs_ago as i64),
        )
    }

    fn session(kind: SessionKind, idle_secs: u64, age_secs: u64) -> Session {
        let id = SessionId::new();
        Session {
            id,
            workspace_id: WorkspaceId::new(),
            kind,
            role: SessionRole::Generic,
            parent_session_id: None,
            root_session_id: id,
            terminal_id: None,
            agent_provider_id: None,
            agent_profile_id: None,
            title: SessionTitle::default(),
            state: SessionState::Running,
            created_at: at(age_secs),
            launch_command: None,
            last_activity_at: at(idle_secs),
            ended_at: None,
            base_commit: None,
        }
    }

    fn warn_only(secs: u64) -> IdlePolicy {
        IdlePolicy {
            warn_after: Some(Duration::from_secs(secs)),
            ..IdlePolicy::default()
        }
    }

    #[test]
    fn a_busy_agent_is_left_alone() {
        let policy = warn_only(1800);
        let s = session(SessionKind::Agent, 5, 10_000);
        assert_eq!(
            policy.evaluate(&s, Timestamp::now(), false, false),
            IdleAction::Keep
        );
    }

    #[test]
    fn a_quiet_agent_is_warned_once_per_quiet_spell() {
        let policy = warn_only(1800);
        let s = session(SessionKind::Agent, 3600, 3600);
        let now = Timestamp::now();
        assert!(matches!(
            policy.evaluate(&s, now, false, false),
            IdleAction::Warn(IdleReason::Inactive(_))
        ));
        // Already reported: the next sweep says nothing.
        assert_eq!(policy.evaluate(&s, now, false, true), IdleAction::Keep);
    }

    #[test]
    fn a_shell_is_exempt_unless_the_user_opts_in() {
        let mut policy = warn_only(1800);
        let s = session(SessionKind::Shell, 3600, 3600);
        let now = Timestamp::now();
        assert_eq!(policy.evaluate(&s, now, false, false), IdleAction::Keep);

        policy.include_shells = true;
        assert!(matches!(
            policy.evaluate(&s, now, false, false),
            IdleAction::Warn(_)
        ));
    }

    #[test]
    fn a_terminal_session_is_never_flagged() {
        let mut policy = warn_only(1);
        policy.stop_after = Some(Duration::from_secs(1));
        policy.long_running_after = Some(Duration::from_secs(1));
        for state in [
            SessionState::Exited {
                code: Some(0),
                signal: None,
            },
            SessionState::Failed { reason: "x".into() },
            SessionState::Orphaned,
        ] {
            let mut s = session(SessionKind::Agent, 10_000, 10_000);
            s.state = state;
            assert_eq!(
                policy.evaluate(&s, Timestamp::now(), false, false),
                IdleAction::Keep,
                "{:?} holds no process",
                s.state
            );
        }
    }

    #[test]
    fn stopping_needs_an_explicit_threshold_and_spares_attached_terminals() {
        let s = session(SessionKind::Agent, 7200, 7200);
        let now = Timestamp::now();

        // Warning alone never stops anything, however long the silence.
        assert!(matches!(
            warn_only(60).evaluate(&s, now, false, false),
            IdleAction::Warn(_)
        ));

        let policy = IdlePolicy {
            warn_after: Some(Duration::from_secs(60)),
            stop_after: Some(Duration::from_secs(3600)),
            ..IdlePolicy::default()
        };
        assert!(matches!(
            policy.evaluate(&s, now, false, false),
            IdleAction::Stop(IdleReason::Inactive(_))
        ));
        // A terminal the user is looking at is in use: warn, never stop.
        assert!(matches!(
            policy.evaluate(&s, now, true, false),
            IdleAction::Warn(_)
        ));

        let forceful = IdlePolicy {
            stop_attached: true,
            ..policy
        };
        assert!(matches!(
            forceful.evaluate(&s, now, true, false),
            IdleAction::Stop(_)
        ));
    }

    #[test]
    fn an_old_but_busy_session_is_reported_as_long_running_not_idle() {
        let policy = IdlePolicy {
            warn_after: Some(Duration::from_secs(1800)),
            long_running_after: Some(Duration::from_secs(8 * 3600)),
            ..IdlePolicy::default()
        };
        // Active five seconds ago, but open for a day.
        let s = session(SessionKind::Agent, 5, 86_400);
        assert!(matches!(
            policy.evaluate(&s, Timestamp::now(), false, false),
            IdleAction::Warn(IdleReason::LongRunning(_))
        ));
    }

    #[test]
    fn inactivity_outranks_age_when_both_apply() {
        let policy = IdlePolicy {
            warn_after: Some(Duration::from_secs(1800)),
            long_running_after: Some(Duration::from_secs(8 * 3600)),
            ..IdlePolicy::default()
        };
        let s = session(SessionKind::Agent, 7200, 86_400);
        assert!(
            matches!(
                policy.evaluate(&s, Timestamp::now(), false, false),
                IdleAction::Warn(IdleReason::Inactive(_))
            ),
            "the quiet is the actionable half"
        );
    }

    #[test]
    fn a_session_settles_once_it_is_active_again() {
        let policy = warn_only(1800);
        let now = Timestamp::now();
        assert!(!policy.is_settled(&session(SessionKind::Agent, 3600, 3600), now));
        assert!(policy.is_settled(&session(SessionKind::Agent, 5, 3600), now));
    }

    #[test]
    fn a_long_running_session_never_settles_while_it_lives() {
        let policy = IdlePolicy {
            long_running_after: Some(Duration::from_secs(3600)),
            ..IdlePolicy::default()
        };
        let now = Timestamp::now();
        assert!(
            !policy.is_settled(&session(SessionKind::Agent, 1, 7200), now),
            "age only grows, so re-reporting it every sweep would be noise"
        );
    }

    #[test]
    fn a_policy_with_no_thresholds_is_disabled() {
        assert!(!IdlePolicy::default().is_enabled());
        assert!(warn_only(1).is_enabled());
    }

    #[test]
    fn durations_read_the_way_a_notice_should() {
        assert_eq!(humanize(Duration::from_secs(30)), "30s");
        assert_eq!(humanize(Duration::from_secs(45 * 60)), "45m");
        assert_eq!(humanize(Duration::from_secs(2 * 3600)), "2h");
        assert_eq!(humanize(Duration::from_secs(2 * 3600 + 5 * 60)), "2h 5m");
    }
}
