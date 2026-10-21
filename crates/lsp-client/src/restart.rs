use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// Crashes older than this no longer count against a server.
pub const CRASH_WINDOW: Duration = Duration::from_secs(5 * 60);
/// Crashes inside [`CRASH_WINDOW`] after which a server is not restarted.
pub const GIVE_UP_AFTER: usize = 3;
const MAX_DELAY: Duration = Duration::from_secs(30);

/// Restart policy for one language server: exponential backoff from 1 s,
/// and no restart at all once [`GIVE_UP_AFTER`] crashes fall inside
/// [`CRASH_WINDOW`].
#[derive(Clone, Debug, Default)]
pub struct CrashHistory {
    crashes: VecDeque<Instant>,
}

impl CrashHistory {
    pub fn record(&mut self, now: Instant) {
        self.prune(now);
        if self.crashes.len() == GIVE_UP_AFTER {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
    }

    /// How long to wait before the next start, or `None` to stay stopped.
    #[must_use]
    pub fn next_delay(&mut self, now: Instant) -> Option<Duration> {
        self.prune(now);
        let recent = self.crashes.len();
        if recent >= GIVE_UP_AFTER {
            return None;
        }
        let Some(exponent) = recent.checked_sub(1) else {
            return Some(Duration::ZERO);
        };
        let seconds = 1u64.checked_shl(u32::try_from(exponent).ok()?)?;
        Some(Duration::from_secs(seconds).min(MAX_DELAY))
    }

    fn prune(&mut self, now: Instant) {
        while self
            .crashes
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) > CRASH_WINDOW)
        {
            self.crashes.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_then_gives_up() {
        let start = Instant::now();
        let mut history = CrashHistory::default();
        assert_eq!(history.next_delay(start), Some(Duration::ZERO));
        history.record(start);
        assert_eq!(history.next_delay(start), Some(Duration::from_secs(1)));
        history.record(start + Duration::from_secs(10));
        assert_eq!(
            history.next_delay(start + Duration::from_secs(10)),
            Some(Duration::from_secs(2))
        );
        history.record(start + Duration::from_secs(20));
        assert_eq!(history.next_delay(start + Duration::from_secs(20)), None);
    }

    #[test]
    fn crashes_outside_the_window_are_forgiven() {
        let start = Instant::now();
        let mut history = CrashHistory::default();
        for offset in [0, 1, 2] {
            history.record(start + Duration::from_secs(offset));
        }
        assert_eq!(history.next_delay(start + Duration::from_secs(3)), None);
        let later = start + CRASH_WINDOW + Duration::from_secs(2);
        assert_eq!(history.next_delay(later), Some(Duration::from_secs(1)));
        let much_later = start + CRASH_WINDOW * 3;
        assert_eq!(history.next_delay(much_later), Some(Duration::ZERO));
    }
}
