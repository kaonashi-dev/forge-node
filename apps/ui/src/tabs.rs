//! Session tab order and the Ctrl-Tab ring. Strip order is what a drag
//! decides and is persisted under `ui.tab_order`; the ring is focus
//! order, kept separately so a Ctrl-Tab returns where you just were.
//!
//! The functions that decide an order or an index are pure so the strip, the
//! rail and Settings can be tested without a window.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use domain::{SessionId, WorkspaceId};

use crate::app::SessionRow;

/// The `app_state` key for the per-checkout strip order.
pub const TAB_ORDER_KEY: &str = "ui.tab_order";
/// How long Control stays down before the list appears; a tap commits at once.
pub const SHOW_DELAY: Duration = Duration::from_millis(180);
const MRU_DEPTH: usize = 64;
/// Live sessions from other checkouts a hold may reach.
const FOREIGN_RECENT: usize = 3;

/// Move `id` to the front of the focus ring, dropping duplicates.
pub fn touch_mru(history: &mut Vec<SessionId>, id: SessionId) {
    history.retain(|item| *item != id);
    history.insert(0, id);
    history.truncate(MRU_DEPTH);
}

/// The open panes in focus order: the active one, the ring, then strip order.
#[must_use]
pub fn mru_tabs(
    history: &[SessionId],
    open: &[SessionId],
    active: Option<SessionId>,
) -> Vec<SessionId> {
    let mut remaining: HashSet<SessionId> = open.iter().copied().collect();
    let mut out = Vec::with_capacity(open.len());
    if let Some(active) = active.filter(|id| remaining.remove(id)) {
        out.push(active);
    }
    for id in history {
        if remaining.remove(id) {
            out.push(*id);
        }
    }
    for id in open {
        if remaining.remove(id) {
            out.push(*id);
        }
    }
    out
}

/// This checkout's tabs in focus order, then recent live sessions from other
/// checkouts. `foreign_at` is the first foreign index, or the length.
#[must_use]
pub fn switcher_ids(
    history: &[SessionId],
    local: &[SessionId],
    active: Option<SessionId>,
    live: &[SessionId],
) -> (Vec<SessionId>, usize) {
    let mut targets = mru_tabs(history, local, active);
    let foreign_at = targets.len();
    let local_set: HashSet<SessionId> = local.iter().copied().collect();
    let live_set: HashSet<SessionId> = live.iter().copied().collect();
    let mut foreign = Vec::new();
    for id in history {
        if foreign.len() >= FOREIGN_RECENT {
            break;
        }
        if local_set.contains(id) || !live_set.contains(id) || foreign.contains(id) {
            continue;
        }
        foreign.push(*id);
    }
    if foreign.len() < FOREIGN_RECENT {
        for id in live {
            if foreign.len() >= FOREIGN_RECENT {
                break;
            }
            if !local_set.contains(id) && !foreign.contains(id) {
                foreign.push(*id);
            }
        }
    }
    targets.extend(foreign);
    (targets, foreign_at)
}

/// The first step lands on the previous pane, or the last walking backward.
#[must_use]
pub fn initial_index(length: usize, delta: isize) -> usize {
    if length < 2 {
        0
    } else if delta >= 0 {
        1
    } else {
        length - 1
    }
}

#[must_use]
pub fn step_index(index: usize, length: usize, delta: isize) -> usize {
    if length == 0 {
        return 0;
    }
    (index as isize + delta).rem_euclid(length as isize) as usize
}

/// Read `ui.tab_order`: a map of checkout id to session ids.
#[must_use]
pub fn parse_order(raw: Option<&str>) -> BTreeMap<String, Vec<SessionId>> {
    let Some(raw) = raw else {
        return BTreeMap::new();
    };
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(raw) else {
        return BTreeMap::new();
    };
    map.into_iter()
        .filter_map(|(workspace, ids)| {
            let ids = ids.as_array()?;
            let ids: Vec<SessionId> = ids
                .iter()
                .filter_map(|id| id.as_str())
                .filter_map(|id| id.parse().ok())
                .collect();
            Some((workspace, ids))
        })
        .collect()
}

#[must_use]
pub fn serialize_order(order: &BTreeMap<String, Vec<SessionId>>) -> String {
    serde_json::to_string(order).unwrap_or_else(|_| "{}".to_string())
}

#[must_use]
pub fn is_shell(row: &SessionRow) -> bool {
    !row.agent && row.kind != domain::SessionKind::Editor
}

fn grouped(rows: Vec<&SessionRow>) -> Vec<&SessionRow> {
    let (shells, rest): (Vec<&SessionRow>, Vec<&SessionRow>) =
        rows.into_iter().partition(|row| is_shell(row));
    let mut out = shells;
    out.extend(rest);
    out
}

/// Apply a stored order over the strip's fallback order. A tab the store has
/// never seen keeps its creation order after the remembered ones, and an id
/// that no longer exists is ignored. Shells still lead.
#[must_use]
pub fn applied<'a>(base: &[&'a SessionRow], order: &[SessionId]) -> Vec<&'a SessionRow> {
    if order.is_empty() {
        return grouped(base.to_vec());
    }
    let mut by_id: BTreeMap<SessionId, &'a SessionRow> =
        base.iter().map(|row| (row.id, *row)).collect();
    let mut out = Vec::with_capacity(base.len());
    for id in order {
        if let Some(row) = by_id.remove(id) {
            out.push(row);
        }
    }
    for row in base {
        if let Some(row) = by_id.remove(&row.id) {
            out.push(row);
        }
    }
    grouped(out)
}

/// Keep a drag inside its kind: a shell does not land among agents.
#[must_use]
pub fn clamp_gap(base: &[&SessionRow], from: SessionId, gap: usize) -> usize {
    let Some(from_row) = base.iter().find(|row| row.id == from) else {
        return gap;
    };
    let dragging_shell = is_shell(from_row);
    let (mut start, mut end) = (0, base.len());
    for (index, row) in base.iter().enumerate() {
        if dragging_shell && !is_shell(row) {
            end = index;
            break;
        }
        if !dragging_shell && is_shell(row) {
            start = index + 1;
        }
    }
    gap.clamp(start, end)
}

/// Move a tab into a gap: 0 is before the first tab, length after the last.
#[must_use]
pub fn move_to_gap(order: &[SessionId], from: SessionId, gap: usize) -> Vec<SessionId> {
    let mut next = order.to_vec();
    let Some(from_index) = next.iter().position(|id| *id == from) else {
        return next;
    };
    let bounded = gap.min(next.len());
    let insert_at = if from_index < bounded {
        bounded - 1
    } else {
        bounded
    };
    if insert_at == from_index {
        return next;
    }
    next.remove(from_index);
    next.insert(insert_at, from);
    next
}

/// Which gap a pointer is in: the midpoint of each tab, then its slot.
#[must_use]
pub fn gap_at(rects: &[(SessionId, [f32; 4])], ids: &[SessionId], x: f32) -> usize {
    for (index, id) in ids.iter().enumerate() {
        if let Some((_, rect)) = rects.iter().find(|(candidate, _)| candidate == id)
            && x < rect[0] + rect[2] / 2.0
        {
            return index;
        }
    }
    ids.len()
}

#[derive(Clone, Debug)]
pub struct Gesture {
    pub targets: Vec<SessionId>,
    pub foreign_at: usize,
    pub index: usize,
    pub shown: bool,
    started: Instant,
}

#[derive(Clone, Debug)]
pub struct Drag {
    pub id: SessionId,
    pub gap: usize,
    original: Vec<SessionId>,
}

impl Drag {
    fn from_id(id: SessionId, gap: usize, original: Vec<SessionId>) -> Self {
        Self { id, gap, original }
    }
}

#[derive(Default)]
pub struct Tabs {
    order: BTreeMap<String, Vec<SessionId>>,
    seeded: bool,
    mru: Vec<SessionId>,
    pub gesture: Option<Gesture>,
    pub drag: Option<Drag>,
    /// Each tab's laid-out rect, refreshed by the strip's layout pass.
    pub rects: Vec<(SessionId, [f32; 4])>,
}

impl Tabs {
    pub fn seed(&mut self, raw: Option<&str>) {
        if self.seeded {
            return;
        }
        self.seeded = true;
        self.order = parse_order(raw);
    }

    #[must_use]
    pub fn order_for(&self, workspace: WorkspaceId) -> Option<&[SessionId]> {
        self.order.get(&workspace.to_string()).map(Vec::as_slice)
    }

    pub fn set_order(&mut self, workspace: WorkspaceId, ids: Vec<SessionId>) {
        self.order.insert(workspace.to_string(), ids);
    }

    /// Drops ids and checkouts the replica no longer has, so a removed tab
    /// cannot come back if one is made later with the same position.
    pub fn retain(&mut self, rows: &[SessionRow], workspaces: &HashSet<String>) {
        let live: HashSet<SessionId> = rows.iter().map(|row| row.id).collect();
        self.order.retain(|workspace, ids| {
            ids.retain(|id| live.contains(id));
            workspaces.contains(workspace)
        });
        self.mru.retain(|id| live.contains(id));
        self.rects.retain(|(id, _)| live.contains(id));
        if let Some(gesture) = &mut self.gesture {
            gesture.targets.retain(|id| live.contains(id));
            if gesture.targets.is_empty() {
                self.gesture = None;
            } else {
                gesture.index = gesture.index.min(gesture.targets.len() - 1);
            }
        }
        if self
            .drag
            .as_ref()
            .is_some_and(|drag| !live.contains(&drag.id))
        {
            self.drag = None;
        }
    }

    /// Persisted order with checkouts that are gone removed.
    #[must_use]
    pub fn serialized(&self, live_workspaces: &HashSet<String>) -> String {
        let pruned: BTreeMap<String, Vec<SessionId>> = self
            .order
            .iter()
            .filter(|(workspace, _)| live_workspaces.contains(*workspace))
            .map(|(workspace, ids)| (workspace.clone(), ids.clone()))
            .collect();
        serialize_order(&pruned)
    }

    #[must_use]
    pub fn mru(&self) -> &[SessionId] {
        &self.mru
    }

    pub fn touch(&mut self, id: SessionId) {
        touch_mru(&mut self.mru, id);
    }

    pub fn note_rect(&mut self, id: SessionId, rect: [f32; 4]) {
        match self
            .rects
            .iter_mut()
            .find(|(candidate, _)| *candidate == id)
        {
            Some((_, existing)) => *existing = rect,
            None => self.rects.push((id, rect)),
        }
    }

    pub fn begin_drag(&mut self, id: SessionId, gap: usize, original: Vec<SessionId>) {
        self.drag = Some(Drag::from_id(id, gap, original));
    }

    pub fn cancel_drag(&mut self) {
        self.drag = None;
    }

    /// The order a release would store, or `None` when the pointer is still
    /// in the gap the tab came from.
    #[must_use]
    pub fn dropped(&self) -> Option<Vec<SessionId>> {
        let drag = self.drag.as_ref()?;
        let moved = move_to_gap(&drag.original, drag.id, drag.gap);
        (moved != drag.original).then_some(moved)
    }

    pub fn start_gesture(&mut self, targets: Vec<SessionId>, foreign_at: usize, index: usize) {
        self.gesture = Some(Gesture {
            targets,
            foreign_at,
            index,
            shown: false,
            started: Instant::now(),
        });
    }

    pub fn step_gesture(&mut self, delta: isize) {
        if let Some(gesture) = &mut self.gesture {
            gesture.index = step_index(gesture.index, gesture.targets.len(), delta);
            gesture.shown = true;
        }
    }

    /// A hold that has lasted past `SHOW_DELAY` reveals the list.
    pub fn reveal(&mut self, now: Instant) -> bool {
        let Some(gesture) = &mut self.gesture else {
            return false;
        };
        if gesture.shown || now.saturating_duration_since(gesture.started) < SHOW_DELAY {
            return false;
        }
        gesture.shown = true;
        true
    }

    pub fn cancel_gesture(&mut self) {
        self.gesture = None;
    }

    #[must_use]
    pub fn commit_gesture(&mut self) -> Option<SessionId> {
        let gesture = self.gesture.take()?;
        gesture.targets.get(gesture.index).copied()
    }

    #[must_use]
    pub fn shown_gesture(&self) -> Option<(&[SessionId], usize, usize)> {
        let gesture = self.gesture.as_ref().filter(|gesture| gesture.shown)?;
        Some((&gesture.targets, gesture.index, gesture.foreign_at))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::connection::{Metrics, fixture_store};
    use crate::terminal::SharedTerminal;

    fn id(tail: u32) -> SessionId {
        format!("00000000-0000-7000-8000-{tail:0>12}")
            .parse()
            .expect("a fixture id")
    }

    fn state() -> crate::app::AppState {
        let store = fixture_store(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/snapshot.json"),
        )
        .expect("fixture");
        crate::app::AppState::for_bench(store, Arc::new(Metrics::default()), SharedTerminal::new())
    }

    #[test]
    fn the_ring_moves_a_session_to_the_front_and_forgets_duplicates() {
        let mut history = vec![id(1), id(2), id(3)];
        touch_mru(&mut history, id(2));
        assert_eq!(history, vec![id(2), id(1), id(3)]);
        touch_mru(&mut history, id(2));
        assert_eq!(history, vec![id(2), id(1), id(3)]);
    }

    #[test]
    fn focus_order_is_active_then_the_ring_then_strip_order() {
        let open = vec![id(1), id(2), id(3), id(4)];
        let history = vec![id(3), id(1)];
        assert_eq!(
            mru_tabs(&history, &open, Some(id(2))),
            vec![id(2), id(3), id(1), id(4)]
        );
    }

    #[test]
    fn the_switcher_appends_recent_foreign_sessions() {
        let local = vec![id(1), id(2)];
        let live = vec![id(9), id(8), id(2), id(7)];
        let history = vec![id(2), id(9), id(8), id(7)];
        let (targets, foreign_at) = switcher_ids(&history, &local, Some(id(2)), &live);
        assert_eq!(targets, vec![id(2), id(1), id(9), id(8), id(7)]);
        assert_eq!(foreign_at, 2);
    }

    #[test]
    fn a_first_step_is_the_previous_tab_and_backward_starts_at_the_end() {
        assert_eq!(initial_index(0, 1), 0);
        assert_eq!(initial_index(1, 1), 0);
        assert_eq!(initial_index(4, 1), 1);
        assert_eq!(initial_index(4, -1), 3);
        assert_eq!(step_index(3, 4, 1), 0);
        assert_eq!(step_index(0, 4, -1), 3);
    }

    #[test]
    fn a_stored_order_reconciles_with_a_snapshot() {
        let state = state();
        let base = crate::app::rows_in_strip(&state.rows, state.workspaces.first().map(|w| w.id));
        assert!(base.len() > 2);
        let ids: Vec<SessionId> = base.iter().map(|row| row.id).collect();
        let stored = vec![ids[1], ids[0], id(999)];
        let applied = applied(&base, &stored);
        assert_eq!(applied[0].id, ids[1]);
        assert_eq!(applied[1].id, ids[0]);
        assert_eq!(applied.len(), base.len(), "an unknown id is ignored");
    }

    #[test]
    fn a_tab_moves_into_the_gap_under_the_pointer() {
        let order = vec![id(1), id(2), id(3)];
        assert_eq!(move_to_gap(&order, id(1), 3), vec![id(2), id(3), id(1)]);
        assert_eq!(move_to_gap(&order, id(3), 0), vec![id(3), id(1), id(2)]);
        assert_eq!(move_to_gap(&order, id(2), 1), order);
        assert_eq!(move_to_gap(&order, id(2), 2), order);
    }

    #[test]
    fn a_drag_stays_inside_its_kind() {
        let state = state();
        let base = crate::app::rows_in_strip(&state.rows, state.workspaces.first().map(|w| w.id));
        let shells: Vec<&SessionRow> = base.iter().copied().filter(|row| is_shell(row)).collect();
        let agents: Vec<&SessionRow> = base.iter().copied().filter(|row| !is_shell(row)).collect();
        if let Some(shell) = shells.first().filter(|_| !agents.is_empty()) {
            let gap = clamp_gap(&base, shell.id, base.len());
            assert!(gap <= shells.len(), "a shell cannot pass the agent group");
        }
    }

    #[test]
    fn a_gap_is_the_midpoint_between_tabs() {
        let rects = [
            (id(1), [0.0, 0.0, 100.0, 30.0]),
            (id(2), [102.0, 0.0, 100.0, 30.0]),
        ];
        let ids = [id(1), id(2)];
        assert_eq!(gap_at(&rects, &ids, 10.0), 0);
        assert_eq!(gap_at(&rects, &ids, 60.0), 1);
        assert_eq!(gap_at(&rects, &ids, 500.0), 2);
    }

    #[test]
    fn a_hold_reveals_after_the_delay_and_commits_its_highlight() {
        let mut tabs = Tabs::default();
        tabs.start_gesture(vec![id(1), id(2), id(3)], 3, initial_index(3, 1));
        let started = Instant::now();
        assert!(!tabs.reveal(started + Duration::from_millis(100)));
        assert!(tabs.reveal(started + SHOW_DELAY));
        tabs.step_gesture(1);
        assert_eq!(tabs.commit_gesture(), Some(id(3)));
        assert!(tabs.gesture.is_none());
    }

    #[test]
    fn a_cancelled_gesture_commits_nothing() {
        let mut tabs = Tabs::default();
        tabs.start_gesture(vec![id(1), id(2)], 2, 1);
        tabs.cancel_gesture();
        assert_eq!(tabs.commit_gesture(), None);
    }
}
