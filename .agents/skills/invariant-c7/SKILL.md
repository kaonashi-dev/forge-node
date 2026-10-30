---
name: invariant-c7
description: Resource cost checklist (C7). Use when the diff touches terminal-core, delta path, Tauri render/store code, core lock, or Command handlers.
---

# Invariant C7 — resource cost

Read `docs/performance.md` if any box is unclear.

- [ ] Grid/scrollback on delta path uses `Arc`, not deep clone.
- [ ] No new unbounded queues; bounded + drop + resync pattern.
- [ ] Per-cell terminal path: no alloc, no locks, no `format!`; palette/theme cached before the cell loop.
- [ ] Store-derived Tauri UI state computed in store updates or memoized selectors, not per frame.
- [ ] Wire/subprocess/file sizes bounded **before** allocation. Result files and board values included.
- [ ] `ClientKind::Cli` skips terminal-activity, bell, clipboard, editor-frame, editor-focus, and usage broadcasts.
- [ ] Definition providers honour the query's deadline and cancel flag between files; grep-then-parse, never a whole-repository index up front; per-file caches keyed by (root, path, length, mtime) and bounded; LSP frames clamped on `Content-Length` before allocation; at most `max_servers` language servers.
- [ ] New `Inner` maps: one owner, one deletion path on close.
- [ ] Cross-session prompts are capped at 8 KiB before cleaning; one queued slot rejects replacement, belongs to a terminal generation, clears on exit/restart/removal, and never replays an uncertain write.
- [ ] No sleep-and-check unless waiting on non-wakeable condition.
- [ ] An action's output watch scans on its PTY thread after the pump, keeps a bounded tail, and is dropped once it settles; nothing action-related runs inside `pump_terminal_batch`. Manifests are `metadata()`-checked and `.take(cap)`-read (256 KiB, 64 KiB for `.forge/actions.toml`).
- [ ] Timeout commands: `.process_group(0)`, kill negative pgid.
- [ ] Every `spawn()` reaped; coalescing flags cleared on `Drop` failure paths.
- [ ] Id-minting mutations return the id; no snapshot reload to guess created ids.
- [ ] Directory watches stay connection-scoped and bounded (128 directories, 4096 bytes per path, `try_send` into a bounded queue); overflow is one empty-path `FileChanged`.
- [ ] A cache key is fingerprinted from its inputs, never the resolved output; a TTL shorter than the gap between two user actions is not a cache.
