//! The daemon's stop signal: one flag that sweeper threads sleep on and the
//! async accept loop awaits, so neither wakes up only to look at it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct Shutdown {
    flag: AtomicBool,
    lock: Mutex<()>,
    wake: Condvar,
    notify: tokio::sync::Notify,
}

impl Shutdown {
    pub(crate) fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    pub(crate) fn trigger(&self) {
        {
            // Stored under the lock so a sleeper between its check and its
            // wait cannot miss the wake.
            let _guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
            self.flag.store(true, Ordering::SeqCst);
        }
        self.wake.notify_all();
        self.notify.notify_waiters();
    }

    /// Block for `period` or until shutdown, whichever comes first; returns
    /// whether the daemon is stopping.
    pub(crate) fn sleep(&self, period: Duration) -> bool {
        let guard = self.lock.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = self
            .wake
            .wait_timeout_while(guard, period, |()| !self.is_set())
            .unwrap_or_else(PoisonError::into_inner);
        self.is_set()
    }

    pub(crate) async fn wait(&self) {
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // Registered before the check, so a trigger in between still wakes it.
            notified.as_mut().enable();
            if self.is_set() {
                return;
            }
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use super::Shutdown;

    #[test]
    fn a_sleeper_wakes_on_the_trigger_not_at_its_period() {
        let shutdown = Arc::new(Shutdown::default());
        let sleeper = Arc::clone(&shutdown);
        let started = Instant::now();
        let thread = std::thread::spawn(move || sleeper.sleep(Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(20));
        shutdown.trigger();
        assert!(thread.join().unwrap());
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            shutdown.sleep(Duration::from_secs(60)),
            "a set flag never sleeps"
        );
    }

    #[tokio::test]
    async fn the_async_wait_returns_once_triggered() {
        let shutdown = Arc::new(Shutdown::default());
        let waiter = Arc::clone(&shutdown);
        let task = tokio::spawn(async move { waiter.wait().await });
        tokio::task::yield_now().await;
        shutdown.trigger();
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("woken")
            .unwrap();
    }
}
