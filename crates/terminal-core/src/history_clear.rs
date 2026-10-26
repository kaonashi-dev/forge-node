//! Spots the escapes that empty the scrollback (`ED 3`, `RIS`) in a PTY stream.
//!
//! The engine cannot infer a clear from the history length alone: a program that
//! clears and reprints in one frame (Claude Code on every resize, inside one
//! synchronized update) leaves the length unchanged or longer, and a replica
//! would keep its rows at addresses that now hold different lines.

const ESC: u8 = 0x1b;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Ground,
    Escape,
    Csi {
        first: u16,
        in_first: bool,
        marked: bool,
    },
}

/// Carries a partial escape across [`feed`](Self::feed) calls, because the PTY
/// splits reads wherever it likes.
///
/// It over-reports rather than under-reports: an escape inside an OSC payload
/// still counts, which only costs a replica a refetch.
#[derive(Debug, Default)]
pub(crate) struct HistoryClearScan {
    state: State,
}

impl HistoryClearScan {
    /// Whether `bytes` complete at least one history-clearing escape.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> bool {
        let mut cleared = false;
        for &byte in bytes {
            self.state = match (self.state, byte) {
                (_, ESC) => State::Escape,
                (State::Ground, _) | (_, CAN | SUB) => State::Ground,
                (State::Escape, b'[') => State::Csi {
                    first: 0,
                    in_first: true,
                    marked: false,
                },
                (State::Escape, b'c') => {
                    cleared = true;
                    State::Ground
                }
                (State::Escape, _) => State::Ground,
                (state @ State::Csi { .. }, 0x00..=0x1f) => state,
                (
                    State::Csi {
                        first,
                        in_first,
                        marked,
                    },
                    b'0'..=b'9',
                ) => State::Csi {
                    first: if in_first {
                        first
                            .saturating_mul(10)
                            .saturating_add(u16::from(byte - b'0'))
                    } else {
                        first
                    },
                    in_first,
                    marked,
                },
                (State::Csi { first, marked, .. }, b';' | b':') => State::Csi {
                    first,
                    in_first: false,
                    marked,
                },
                // A private marker or an intermediate makes it another
                // sequence: vte dispatches `ED` only with none (`CSI ? 3 J` is
                // DECSED, which it ignores).
                (
                    State::Csi {
                        first, in_first, ..
                    },
                    0x20..=0x2f | 0x3c..=0x3f,
                ) => State::Csi {
                    first,
                    in_first,
                    marked: true,
                },
                (State::Csi { first, marked, .. }, final_byte) => {
                    cleared |= final_byte == b'J' && first == 3 && !marked;
                    State::Ground
                }
            };
        }
        cleared
    }
}

#[cfg(test)]
mod tests {
    use super::HistoryClearScan;

    fn scan(bytes: &[u8]) -> bool {
        HistoryClearScan::default().feed(bytes)
    }

    #[test]
    fn finds_ed3_and_ris() {
        assert!(scan(b"\x1b[3J"));
        assert!(scan(b"\x1b[2J\x1b[3J\x1b[H"));
        assert!(scan(b"\x1b[3;1J"));
        assert!(scan(b"\x1b[003J"));
        assert!(scan(b"text\x1bc"));
    }

    #[test]
    fn ignores_clears_that_keep_history() {
        assert!(!scan(b"\x1b[J\x1b[0J\x1b[1J\x1b[2J"));
        assert!(!scan(b"\x1b[?3J"));
        assert!(!scan(b"\x1b[13J\x1b[1;3J"));
        assert!(!scan(b"\x1b[3K\x1b[3m"));
        assert!(!scan(b"plain 3J and c"));
        assert!(!scan(b"\x1b[3\x18J"));
    }

    #[test]
    fn an_escape_split_across_reads_still_counts() {
        let mut scan = HistoryClearScan::default();
        assert!(!scan.feed(b"out\x1b"));
        assert!(!scan.feed(b"[3"));
        assert!(scan.feed(b"J"));
        assert!(!scan.feed(b"J"));
    }
}
