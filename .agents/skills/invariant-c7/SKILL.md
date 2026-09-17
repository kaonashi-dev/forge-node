---
name: invariant-c7
description: Resource cost checklist for harness reviewer (CHECKPOINTS C7). Use when the diff touches terminal-core, delta path, Tauri render/store code, core lock, or Command handlers.
---

# Invariant C7 — resource cost

Read `docs/performance.md` if any box is unclear.

- [ ] Grid/scrollback on delta path uses `Arc`, not deep clone.
- [ ] No new unbounded queues; bounded + drop + resync pattern.
- [ ] Per-cell terminal path: no alloc, no locks, no `format!`; palette/theme cached before the cell loop.
- [ ] Store-derived Tauri UI state computed in store updates or memoized selectors, not per frame.
- [ ] Wire/subprocess/file sizes bounded **before** allocation.
- [ ] New `Inner` maps: one owner, one deletion path on close.
- [ ] No sleep-and-check unless waiting on non-wakeable condition.
- [ ] Timeout commands: `.process_group(0)`, kill negative pgid.
- [ ] Every `spawn()` reaped; coalescing flags cleared on `Drop` failure paths.
- [ ] Id-minting mutations return the id; no snapshot reload to guess created ids.
