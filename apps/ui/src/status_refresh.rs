//! When to re-read a workspace's git status that nothing in the app changed:
//! a command typed into one of its terminals once that terminal's output has
//! settled, an agent turn that ended, or the window coming back to the front.
//! The daemon learns of a `git checkout` in a shell only when asked.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use domain::WorkspaceId;

/// Output this long quiet reads as the command having finished.
const QUIET: Duration = Duration::from_millis(800);
/// A command still printing after this (a dev server) is measured anyway.
const CAP: Duration = Duration::from_secs(10);
/// The daemon acks a second request for one workspace within 2 s without
/// measuring, so a later one waits rather than being lost.
const DAEMON_THROTTLE: Duration = Duration::from_millis(2100);

#[derive(Default)]
pub struct StatusRefresh {
    pending: HashMap<WorkspaceId, Pending>,
    sent: HashMap<WorkspaceId, Instant>,
}

struct Pending {
    armed: Instant,
    quiet_from: Instant,
}

impl StatusRefresh {
    pub fn command_sent(&mut self, workspace: WorkspaceId, now: Instant) {
        self.pending
            .entry(workspace)
            .and_modify(|pending| pending.quiet_from = now)
            .or_insert(Pending {
                armed: now,
                quiet_from: now,
            });
    }

    /// Measured as soon as the daemon will, with no output to wait for.
    pub fn request(&mut self, workspace: WorkspaceId, now: Instant) {
        let settled = now.checked_sub(QUIET).unwrap_or(now);
        self.pending
            .entry(workspace)
            .and_modify(|pending| pending.quiet_from = pending.quiet_from.min(settled))
            .or_insert(Pending {
                armed: settled,
                quiet_from: settled,
            });
    }

    /// Output only postpones a measurement already armed; it never arms one.
    pub fn output(&mut self, workspace: WorkspaceId, now: Instant) {
        if let Some(pending) = self.pending.get_mut(&workspace) {
            pending.quiet_from = now;
        }
    }

    #[must_use]
    pub fn armed(&self) -> bool {
        !self.pending.is_empty()
    }

    #[must_use]
    pub fn next_due(&self) -> Option<Instant> {
        self.pending
            .iter()
            .map(|(workspace, pending)| self.due(*workspace, pending))
            .min()
    }

    pub fn take_due(&mut self, now: Instant) -> Vec<WorkspaceId> {
        let due: Vec<WorkspaceId> = self
            .pending
            .iter()
            .filter(|(workspace, pending)| self.due(**workspace, pending) <= now)
            .map(|(workspace, _)| *workspace)
            .collect();
        self.sent
            .retain(|_, at| now.saturating_duration_since(*at) < DAEMON_THROTTLE);
        for workspace in &due {
            self.pending.remove(workspace);
            self.sent.insert(*workspace, now);
        }
        due
    }

    fn due(&self, workspace: WorkspaceId, pending: &Pending) -> Instant {
        let settled = (pending.quiet_from + QUIET).min(pending.armed + CAP);
        self.sent
            .get(&workspace)
            .map_or(settled, |at| settled.max(*at + DAEMON_THROTTLE))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_measured_once_its_output_settles() {
        let ws = WorkspaceId::new();
        let t0 = Instant::now();
        let mut refresh = StatusRefresh::default();
        refresh.command_sent(ws, t0);
        refresh.output(ws, t0 + Duration::from_millis(500));
        assert!(refresh.take_due(t0 + QUIET).is_empty(), "still printing");
        assert_eq!(
            refresh.take_due(t0 + Duration::from_millis(500) + QUIET),
            vec![ws]
        );
        assert!(!refresh.armed());
    }

    #[test]
    fn output_without_a_command_measures_nothing() {
        let ws = WorkspaceId::new();
        let mut refresh = StatusRefresh::default();
        refresh.output(ws, Instant::now());
        assert_eq!(refresh.next_due(), None);
    }

    #[test]
    fn a_command_that_never_goes_quiet_is_measured_at_the_cap() {
        let ws = WorkspaceId::new();
        let t0 = Instant::now();
        let mut refresh = StatusRefresh::default();
        refresh.command_sent(ws, t0);
        let mut now = t0;
        while now < t0 + CAP {
            now += Duration::from_millis(100);
            refresh.output(ws, now);
        }
        assert_eq!(refresh.take_due(t0 + CAP), vec![ws]);
    }

    #[test]
    fn a_checkout_right_after_a_focus_refresh_waits_out_the_daemon_throttle() {
        let ws = WorkspaceId::new();
        let t0 = Instant::now();
        let mut refresh = StatusRefresh::default();
        refresh.request(ws, t0);
        assert_eq!(refresh.take_due(t0), vec![ws]);

        let typed = t0 + Duration::from_millis(300);
        refresh.command_sent(ws, typed);
        assert!(
            refresh.take_due(typed + QUIET).is_empty(),
            "the daemon would ack it without measuring"
        );
        assert_eq!(refresh.next_due(), Some(t0 + DAEMON_THROTTLE));
        assert_eq!(refresh.take_due(t0 + DAEMON_THROTTLE), vec![ws]);
    }
}
