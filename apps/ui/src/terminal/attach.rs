use domain::TerminalId;
use rune::{Key, KeyEvent};

/// The O1 attach gate: a pane starts Attaching, becomes Live only once a snapshot
/// has been applied **and** the pane has been measured, and returns to Attaching
/// for a resync. Input in that window is held, never written at an unknown size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AttachState {
    #[default]
    Attaching,
    Live {
        size: (u16, u16),
    },
}

/// One held user gesture. Keys keep their modifiers so they are encoded with the
/// modes from the snapshot that eventually arrives, not with an assumed default.
#[derive(Clone, Debug, PartialEq)]
pub enum HeldInput {
    Key(KeyEvent),
    Paste(String),
}

impl HeldInput {
    #[must_use]
    pub fn cost(&self) -> usize {
        match self {
            HeldInput::Key(ev) => {
                16 + match &ev.key {
                    Key::Text(text) => text.len(),
                    _ => 0,
                }
            }
            HeldInput::Paste(text) => text.len(),
        }
    }
}

/// A hard cap on what an attaching pane may hold. A refusal is visible and
/// logged; the buffer never grows past this.
pub const MAX_HELD_BYTES: usize = 64 * 1024;

#[derive(Debug, Default)]
pub struct InputHold {
    events: Vec<HeldInput>,
    bytes: usize,
    refused: u64,
}

impl InputHold {
    #[must_use]
    pub fn push_key(&mut self, ev: &KeyEvent) -> bool {
        let event = HeldInput::Key(ev.clone());
        self.push(event)
    }

    #[must_use]
    pub fn push_paste(&mut self, text: &str) -> bool {
        self.push(HeldInput::Paste(text.to_string()))
    }

    fn push(&mut self, event: HeldInput) -> bool {
        let cost = event.cost();
        if self.bytes.saturating_add(cost) > MAX_HELD_BYTES {
            self.refused = self.refused.saturating_add(1);
            return false;
        }
        self.bytes += cost;
        self.events.push(event);
        true
    }

    pub fn take(&mut self) -> Vec<HeldInput> {
        self.bytes = 0;
        std::mem::take(&mut self.events)
    }

    pub fn clear(&mut self) {
        self.events.clear();
        self.bytes = 0;
        self.refused = 0;
    }

    #[must_use]
    pub fn bytes(&self) -> usize {
        self.bytes
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    #[must_use]
    pub fn refused(&self) -> u64 {
        self.refused
    }
}

/// The attachment state for the one visible terminal. It owns the target identity
/// so a terminal switch or generation change can drop the old queue in one place.
#[derive(Debug, Default)]
pub struct Attachment {
    state: AttachState,
    terminal: Option<TerminalId>,
    generation: u64,
    snapshot_size: Option<(u16, u16)>,
    measured: Option<(u16, u16)>,
    measure_floor: Option<u64>,
    hold: InputHold,
}

impl Attachment {
    #[cfg(test)]
    #[must_use]
    pub fn state(&self) -> AttachState {
        self.state
    }

    #[must_use]
    pub fn state_name(&self) -> &'static str {
        match self.state {
            AttachState::Attaching => "attaching",
            AttachState::Live { .. } => "live",
        }
    }

    #[must_use]
    pub fn is_live(&self) -> bool {
        matches!(self.state, AttachState::Live { .. })
    }

    #[must_use]
    pub fn terminal(&self) -> Option<TerminalId> {
        self.terminal
    }

    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Point at a terminal for a generation. A new target or generation is a
    /// failure boundary: held bytes belong to the old attachment and are dropped.
    pub fn begin(&mut self, generation: u64, terminal: TerminalId) {
        if self.terminal != Some(terminal) || self.generation != generation {
            self.hold.clear();
            self.snapshot_size = None;
            self.measured = None;
            self.measure_floor = None;
            self.state = AttachState::Attaching;
        }
        self.terminal = Some(terminal);
        self.generation = generation;
    }

    /// A disconnect or failed attach: drop what was held and wait for a new
    /// snapshot. An uncertain write is never replayed.
    pub fn fail(&mut self) {
        self.hold.clear();
        self.snapshot_size = None;
        self.measured = None;
        self.measure_floor = None;
        self.state = AttachState::Attaching;
    }

    /// A generation change is a new connection: no old key may cross it.
    pub fn new_generation(&mut self, generation: u64) {
        self.hold.clear();
        self.snapshot_size = None;
        self.measured = None;
        self.measure_floor = None;
        self.state = AttachState::Attaching;
        self.generation = generation;
        self.terminal = None;
    }

    /// `TerminalResync` re-enters Attaching without a new generation. Held input
    /// stays queued because the resync is this attachment, not a replacement,
    /// but the measurement that unlocks it must render **after** the resync.
    pub fn resync(&mut self, measure_epoch: u64) {
        self.state = AttachState::Attaching;
        self.snapshot_size = None;
        self.measured = None;
        self.measure_floor = Some(measure_epoch);
    }

    pub fn on_snapshot(&mut self, generation: u64, terminal: TerminalId, size: (u16, u16)) -> bool {
        if self.terminal != Some(terminal) || self.generation != generation {
            return false;
        }
        self.snapshot_size = Some(size);
        self.maybe_live()
    }

    /// A compositor measurement counts only if it rendered after the resync.
    pub fn on_measured(&mut self, measure_epoch: u64, size: (u16, u16)) -> bool {
        if self
            .measure_floor
            .is_some_and(|floor| measure_epoch <= floor)
        {
            return false;
        }
        self.measured = Some(size);
        self.maybe_live()
    }

    fn maybe_live(&mut self) -> bool {
        if self.state == AttachState::Attaching
            && let (Some(_), Some(measured)) = (self.snapshot_size, self.measured)
        {
            self.state = AttachState::Live { size: measured };
            return true;
        }
        false
    }

    #[must_use]
    pub fn hold_key(&mut self, ev: &KeyEvent) -> bool {
        self.hold.push_key(ev)
    }

    #[must_use]
    pub fn hold_paste(&mut self, text: &str) -> bool {
        self.hold.push_paste(text)
    }

    pub fn take_held(&mut self) -> Vec<HeldInput> {
        self.hold.take()
    }

    #[must_use]
    pub fn held_bytes(&self) -> usize {
        self.hold.bytes()
    }

    #[must_use]
    pub fn held_len(&self) -> usize {
        self.hold.len()
    }

    #[must_use]
    pub fn refused(&self) -> u64 {
        self.hold.refused()
    }

    /// The banner the view draws while attaching or after a refusal.
    #[must_use]
    pub fn notice(&self) -> Option<String> {
        if self.hold.refused() > 0 {
            return Some(format!(
                "{} input(s) refused: {} KiB hold buffer full",
                self.hold.refused(),
                MAX_HELD_BYTES / 1024
            ));
        }
        if self.state == AttachState::Attaching {
            return Some(if self.hold.is_empty() {
                "Attaching terminal…".to_string()
            } else {
                format!(
                    "Attaching terminal — {} bytes held ({})",
                    self.hold.bytes(),
                    self.hold.len()
                )
            });
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rune::Mods;

    fn key(text: &str) -> KeyEvent {
        KeyEvent {
            key: Key::Text(text.to_string()),
            mods: Mods::default(),
        }
    }

    #[test]
    fn input_is_held_until_snapshot_and_measure() {
        let mut attach = Attachment::default();
        let terminal = TerminalId::new();
        attach.begin(1, terminal);
        assert_eq!(attach.state(), AttachState::Attaching);
        assert!(attach.hold_key(&key("hi")));
        assert_eq!(attach.held_len(), 1);
        assert!(!attach.on_snapshot(1, terminal, (80, 24)), "no measure yet");
        assert!(attach.on_measured(1, (120, 40)));
        assert_eq!(attach.state(), AttachState::Live { size: (120, 40) });
        let held = attach.take_held();
        assert_eq!(held.len(), 1);
        assert!(attach.take_held().is_empty(), "flushed exactly once");
    }

    #[test]
    fn resync_holds_and_releases() {
        let mut attach = Attachment::default();
        let terminal = TerminalId::new();
        attach.begin(1, terminal);
        attach.on_snapshot(1, terminal, (80, 24));
        attach.on_measured(1, (80, 24));
        assert!(attach.is_live());
        attach.resync(4);
        assert_eq!(attach.state(), AttachState::Attaching);
        assert!(attach.hold_key(&key("after-resync")));
        assert!(!attach.is_live());
        assert!(
            !attach.on_measured(4, (80, 24)),
            "a measure from before the resync cannot release it"
        );
        attach.on_snapshot(1, terminal, (80, 24));
        assert!(attach.on_measured(5, (80, 24)));
        assert!(attach.is_live());
        assert_eq!(attach.take_held().len(), 1);
    }

    #[test]
    fn failure_drops_held_input() {
        let mut attach = Attachment::default();
        let terminal = TerminalId::new();
        attach.begin(1, terminal);
        assert!(attach.hold_key(&key("lost")));
        attach.fail();
        assert_eq!(attach.state(), AttachState::Attaching);
        assert_eq!(attach.held_len(), 0);
        assert_eq!(attach.held_bytes(), 0);
        assert!(attach.take_held().is_empty());
    }

    #[test]
    fn a_generation_change_drops_held_input() {
        let mut attach = Attachment::default();
        let terminal = TerminalId::new();
        attach.begin(1, terminal);
        assert!(attach.hold_key(&key("gen1")));
        attach.new_generation(2);
        assert_eq!(attach.held_len(), 0);
        attach.begin(2, TerminalId::new());
        assert_eq!(attach.state(), AttachState::Attaching);
    }

    #[test]
    fn a_terminal_switch_drops_held_input() {
        let mut attach = Attachment::default();
        attach.begin(1, TerminalId::new());
        assert!(attach.hold_key(&key("old")));
        attach.begin(1, TerminalId::new());
        assert_eq!(attach.held_len(), 0);
    }

    #[test]
    fn held_buffer_is_bounded() {
        let mut attach = Attachment::default();
        attach.begin(1, TerminalId::new());
        let chunk = "x".repeat(1024);
        for _ in 0..64 {
            assert!(attach.hold_paste(&chunk));
        }
        assert_eq!(attach.held_bytes(), MAX_HELD_BYTES);
        assert!(!attach.hold_paste(&chunk), "the excess is refused");
        assert_eq!(attach.held_bytes(), MAX_HELD_BYTES);
        assert_eq!(attach.refused(), 1);
        assert!(attach.notice().unwrap().contains("64 KiB"));
        assert_eq!(attach.held_len(), 64);
    }
}
