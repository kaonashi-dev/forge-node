//! Readiness of a running action: an output pattern or a port.
//!
//! The output watch is owned by the action's PTY thread and scans each batch
//! *after* `pump_terminal_batch` returns, never under the core lock; it is
//! dropped the moment it has an answer, so a ready server costs nothing per
//! batch from then on. The port probe is its own thread, bounded by the
//! action's timeout and by the session leaving `Running`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use domain::{ActionReadiness, ActionReady, SessionId, SessionState};
use protocol::DaemonEvent;

use crate::core::Daemon;

/// Bytes of recent, escape-stripped output kept to find a match that straddles
/// two reads and the URL on its line.
const TAIL_BYTES: usize = 4096;
const PORT_RETRY: Duration = Duration::from_millis(500);

/// A literal, case-insensitive search of a terminal's output, ANSI stripped.
pub struct OutputWatch {
    session_id: SessionId,
    needle: Vec<u8>,
    port: Option<u16>,
    deadline: Instant,
    tail: Vec<u8>,
    escape: Escape,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Escape {
    Text,
    Esc,
    /// `ESC [` up to a final byte.
    Csi,
    /// `ESC ]` up to BEL or `ESC \`.
    Osc,
    OscEsc,
}

pub enum Scan {
    Pending,
    Ready(Option<String>),
    TimedOut,
}

impl OutputWatch {
    #[must_use]
    pub fn new(session_id: SessionId, pattern: &str, timeout_secs: u64, port: Option<u16>) -> Self {
        Self {
            session_id,
            needle: pattern.as_bytes().to_vec(),
            port,
            deadline: Instant::now() + Duration::from_secs(timeout_secs),
            tail: Vec::with_capacity(TAIL_BYTES),
            escape: Escape::Text,
        }
    }

    #[must_use]
    pub const fn session_id(&self) -> SessionId {
        self.session_id
    }

    #[must_use]
    pub const fn deadline(&self) -> Instant {
        self.deadline
    }

    /// Feed one batch. The search starts just before the new bytes, so a
    /// batch costs its own length, not the tail's.
    pub fn scan(&mut self, bytes: &[u8], now: Instant) -> Scan {
        let from = self
            .tail
            .len()
            .saturating_sub(self.needle.len().saturating_sub(1));
        for &byte in bytes {
            self.escape = match (self.escape, byte) {
                (Escape::Text, 0x1b) => Escape::Esc,
                (Escape::Text, b'\r') => Escape::Text,
                (Escape::Text, _) => {
                    self.tail.push(byte);
                    Escape::Text
                }
                (Escape::Esc, b'[') => Escape::Csi,
                (Escape::Esc, b']') => Escape::Osc,
                (Escape::Esc, _) => Escape::Text,
                (Escape::Csi, 0x40..=0x7e) => Escape::Text,
                (Escape::Csi, _) => Escape::Csi,
                (Escape::Osc, 0x07) => Escape::Text,
                (Escape::Osc, 0x1b) => Escape::OscEsc,
                (Escape::Osc, _) => Escape::Osc,
                (Escape::OscEsc, _) => Escape::Text,
            };
        }
        let found = self.tail[from.min(self.tail.len())..]
            .windows(self.needle.len())
            .position(|window| window.eq_ignore_ascii_case(&self.needle))
            .map(|at| at + from);
        if let Some(at) = found {
            return Scan::Ready(self.url_near(at).or_else(|| self.port.map(localhost)));
        }
        if self.tail.len() > TAIL_BYTES {
            let excess = self.tail.len() - TAIL_BYTES;
            self.tail.drain(..excess);
        }
        if now >= self.deadline {
            Scan::TimedOut
        } else {
            Scan::Pending
        }
    }

    /// The first `http(s)://` URL on the line holding the match.
    fn url_near(&self, at: usize) -> Option<String> {
        let start = self.tail[..at]
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i + 1);
        let end = self.tail[at..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(self.tail.len(), |i| at + i);
        let line = String::from_utf8_lossy(&self.tail[start..end]);
        let begin = line.find("http://").or_else(|| line.find("https://"))?;
        let url: String = line[begin..]
            .chars()
            .take_while(|c| !c.is_whitespace() && !c.is_control())
            .collect();
        let url = url.trim_end_matches(['.', ',', ')', ']', '\'', '"']);
        (url.len() > "http://".len()).then(|| url.to_owned())
    }
}

fn localhost(port: u16) -> String {
    format!("http://localhost:{port}")
}

impl Daemon {
    /// Begin a port probe. An output watch was already handed to the PTY
    /// thread at spawn, so it needs nothing here.
    pub(crate) fn start_readiness(
        self: &Arc<Self>,
        session_id: SessionId,
        ready: &ActionReady,
        port: Option<u16>,
        readiness: &ActionReadiness,
    ) {
        if *readiness != ActionReadiness::Probing {
            return;
        }
        if let (ActionReady::Port { timeout_secs }, Some(port)) = (ready, port) {
            let daemon = Arc::clone(self);
            let deadline = Instant::now() + Duration::from_secs(*timeout_secs);
            std::thread::spawn(move || daemon.probe_port(session_id, port, deadline));
        }
    }

    /// Connect until the server answers, the deadline passes, or the session
    /// stops running. A port that opens notifies nobody, which is what makes
    /// this sleep-and-check loop the exception rather than the defect.
    fn probe_port(&self, session_id: SessionId, port: u16, deadline: Instant) {
        loop {
            if !self.is_probing(session_id) {
                return;
            }
            if super::ports::answers(port) {
                self.set_readiness(
                    session_id,
                    ActionReadiness::Ready {
                        url: Some(localhost(port)),
                    },
                );
                return;
            }
            if Instant::now() >= deadline {
                self.set_readiness(session_id, ActionReadiness::TimedOut);
                return;
            }
            std::thread::sleep(PORT_RETRY);
        }
    }

    fn is_probing(&self, session_id: SessionId) -> bool {
        self.lock()
            .sessions
            .get(&session_id)
            .is_some_and(|session| {
                matches!(
                    session.state,
                    SessionState::Starting | SessionState::Running
                ) && session
                    .action
                    .as_ref()
                    .is_some_and(|run| run.readiness == ActionReadiness::Probing)
            })
    }

    /// Settle a probing session's readiness and broadcast it. A session that
    /// is no longer probing (restarted, untagged, already settled) is left alone.
    pub(crate) fn set_readiness(&self, session_id: SessionId, readiness: ActionReadiness) {
        let updated = {
            let mut inner = self.lock();
            let Some(session) = inner.sessions.get_mut(&session_id) else {
                return;
            };
            let Some(run) = session.action.as_mut() else {
                return;
            };
            if run.readiness != ActionReadiness::Probing || !session.state.is_active() {
                return;
            }
            run.readiness = readiness;
            let updated = session.clone();
            crate::actions::run::wake_waiters(&mut inner);
            updated
        };
        self.registry
            .broadcast_domain(DaemonEvent::SessionUpdated(updated));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(pattern: &str) -> OutputWatch {
        OutputWatch::new(SessionId::new(), pattern, 60, Some(5173))
    }

    fn ready(scan: Scan) -> Option<Option<String>> {
        match scan {
            Scan::Ready(url) => Some(url),
            _ => None,
        }
    }

    #[test]
    fn a_match_across_reads_and_colors_falls_back_to_the_port_without_a_url() {
        let mut w = watch("ready in");
        let now = Instant::now();
        assert!(ready(w.scan(b"\x1b[32mVITE\x1b[0m READ", now)).is_none());
        let url = ready(w.scan(b"Y in 300ms\r\n", now));
        assert_eq!(url, Some(Some("http://localhost:5173".to_owned())));
    }

    #[test]
    fn the_url_on_the_matched_line_wins_over_the_port() {
        let mut w = watch("localhost:");
        let url = ready(w.scan(
            b"  \x1b[32m\xe2\x9e\x9c\x1b[39m  Local:   \x1b[36mhttp://localhost:\x1b[1m5174\x1b[22m/\x1b[39m\r\n",
            Instant::now(),
        ));
        assert_eq!(url, Some(Some("http://localhost:5174/".to_owned())));
    }

    #[test]
    fn without_a_url_the_port_names_the_server() {
        let mut w = watch("listening");
        let url = ready(w.scan(b"Server LISTENING\n", Instant::now()));
        assert_eq!(url, Some(Some("http://localhost:5173".to_owned())));
    }

    #[test]
    fn osc_titles_do_not_match_and_the_deadline_times_out() {
        let mut w = OutputWatch::new(SessionId::new(), "ready", 1, None);
        let now = Instant::now();
        assert!(matches!(
            w.scan(b"\x1b]0;ready\x07working\n", now),
            Scan::Pending
        ));
        assert!(matches!(
            w.scan(b"", now + Duration::from_secs(2)),
            Scan::TimedOut
        ));
    }

    #[test]
    fn the_tail_stays_bounded() {
        let mut w = watch("never");
        let noise = vec![b'x'; 64 * 1024];
        for _ in 0..4 {
            assert!(matches!(w.scan(&noise, Instant::now()), Scan::Pending));
        }
        assert!(w.tail.len() <= TAIL_BYTES);
    }
}
