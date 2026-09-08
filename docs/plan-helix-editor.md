# Plan — Helix-model editing, keyboard tree and filesystem mutations (§17)

**Status:** research only (2026-08-29). Nothing here is implemented and no code
was written for it. This document revises [`plan-editor.md`](./plan-editor.md),
which delivered the in-app editor but left three things out: the tree is
mouse-only, there is no keyboard editing model, and the filesystem is
read-plus-overwrite with no create, rename or delete.

The goal, in the user's words: keep the ordinary copy/paste and pointer
navigation of Zed or VS Code, but gain modal editing and a light, keyboard-first
way of seeing and organising files — closer to LazyVim's navigation layer than
to a full IDE.

## 0. Relationship to the existing plan

Everything in phases 1-3 sits inside ADR-012's envelope (the daemon owns the
filesystem, the GUI is a replica) and needs **no new ADR**. Phase 4b — owning
the text element — would need one, and would reopen the decision
`plan-editor.md` §1.2 closed.

`plan-editor.md` §4 lists "no multi-cursor" as out of scope. §6 below reopens
that as an explicit question, because in the Helix model multi-cursor is not a
feature bolted on top: it *is* the model.

## 1. What was measured and discarded: `helix-core` as a dependency

`helix-core` was the leading candidate for the editing engine — MPL-2.0, which
`deny.toml` allows in writing ("file-level copyleft and safe to link into a
shipped binary as long as we do not modify the crate's own sources"). It was
measured before being adopted, and it fails on four counts. Two of them are
fatal on their own.

| # | Finding | Evidence |
| --- | --- | --- |
| 1 | **It is not published.** `helix-core` on crates.io is a placeholder from 2021 at version `0.0.0`, described verbatim as "This package is a placeholder for the future helix-editor crate". The real crate exists only as a workspace member of `helix-editor/helix`, reachable as a git dependency pinned by rev. | crates.io API, `updated_at` 2021-06-02 |
| 2 | **`ropey` collides, and this one is fatal.** The workspace resolves `ropey 2.0.0-beta.1` (pulled in by the UI kit). `helix-core` pins `ropey 1.6.1`. Two majors of the same crate coexist in a lockfile happily, but they are two *different types*: the `Rope` that `InputState::text()` returns is not the `Rope` any `helix-core` function accepts. The whole idea — "let helix-core compute ranges over the buffer we already have" — is unimplementable, not merely awkward. | `Cargo.lock`, helix `Cargo.toml` |
| 3 | **tree-sitter collides too.** `helix-core` no longer uses the `tree-sitter` crate; it uses `tree-house`, whose `tree-house-bindings` compiles its **own vendored copy** of the tree-sitter C runtime — `cc` over `vendor/src/lib.c`, emitted as a static library literally named `tree-sitter`. The workspace already builds `tree-sitter 0.25.10`, which compiles that same C runtime. Two static libraries exporting `ts_parser_new` and the rest of the C API is a duplicate-symbol link error, which is the exact failure the root `Cargo.toml` already documents for `tree-sitter-sequel`. | `tree-house/bindings/build.rs` |
| 4 | **It drags half an application.** `helix-core` depends on `helix-stdx`, `helix-loader` and `helix-parsec` by path. `helix-loader` fetches and compiles grammars at runtime and reads Helix's own configuration directories. None of that belongs in Forge, and a git dependency takes all of it. | helix `helix-core/Cargo.toml` |

**Verdict:** there is no version of "link `helix-core`" that works. Findings 2
and 3 are independent and each is sufficient.

## 2. What survives

### 2.1 `nucleo` — the one crate that enters cleanly

`nucleo` 0.5.0 / `nucleo-matcher` 0.3.1 (helix-editor, **MPL-2.0**, allowed by
`deny.toml`) is Helix's fuzzy matcher, and unlike `helix-core` it is genuinely
published and self-contained.

`fs-service` today ranks nothing: `fuzzy_match` (`crates/fs-service/src/lib.rs:508`)
is a case-insensitive subsequence test returning `bool`, so the file palette
shows hits in listing order rather than by relevance. Swapping in a scoring
matcher is a small, contained change with a visible payoff. It is independent
of everything else here and can land whenever.

### 2.2 The editing model — as a design, not as code

The model is documented behaviour, not source. Reimplementing it copies
nothing, and it is the part actually worth having:

```
Helix:  selection → operator          w selects a word; d deletes the selection
Vim:    operator → motion → range     d waits for w to tell it what to delete
```

This is the whole argument for preferring Helix over Vim here, and it is an
argument about *cost*, not taste. Vim's shape forces an operator-pending state
machine threading counts, registers and a pending operator through every
motion; that machinery is why Zed's `vim` crate is **25,856 lines**. In the
Helix shape the motion has already produced a selection and the operator is a
pure function of it. There is no `dw` because there is nothing left for it to
mean. The state machine disappears, and with it most of the code.

Zed's `vim` crate is in any case closed to us: **GPL-3.0-or-later**, the same
wall as its `editor` crate (`plan-editor.md` §1.1).

## 3. The ceiling: `InputState::selected_range`

`the UI kit`'s `InputState` keeps its selection as
`pub(super) selected_range: Selection` (`input/state.rs:273`) — **private, and a
single range**. The public surface is `text() -> &Rope` (`state.rs:797`),
`cursor()` (1498), `set_cursor_position()` (810), `insert()` (633), `replace()`
(648) and `unselect()` (1644), plus the action set at `state.rs:45`
(`MoveUp`, `SelectLeft`, `Copy`, `Undo`, …), which is public and dispatchable
from outside the crate against the `"Input"` key context (`input.rs:277`).

Two consequences, and they set the shape of phase 4:

- Without an upstream change, a selection cannot be **set** from outside the
  crate — only nudged, one dispatched action at a time.
- Even *with* a getter and setter, multi-cursor stays impossible while the field
  holds one range.

So: the Helix model with a single selection can run on `InputState`. The Helix
model as it is actually meant to work cannot. The upstream PR (roughly ten lines
in an Apache-2.0 crate) unblocks the first and not the second, and it should be
opened early — upstream latency is not ours to control.

For scale, should owning the element ever be on the table: the UI kit's
input is **9,084 lines** — `element.rs` 1,696, `state.rs` 2,178,
`text_wrapper.rs` 814, `rope_ext.rs` 699.

## 4. What we build

### 4.1 Phase 1 — the file tree, keyboard-first

Scope is deliberately the tree alone. No palette, no grep, no editing.

`file_tree()` (`apps/tauri is called from
`right_panel_pane` (`apps/tauri which is a render path.
Every frame it rebuilds the whole hierarchy through `tree_rows` → `flatten`
(one `String` per row), renders **every** row with no virtualisation, and builds
each element id as `SharedString::from(format!("file-tree-{path}"))`. On a
monorepo listing of 50k paths that is 50k elements per frame. The last of those
is a direct breach of C7 (`harness/CHECKPOINTS.md:151`: "Element ids are not
built with `SharedString::from(format!(…))` per row and per frame"), and the
second breaches "nothing derivable from the store is recomputed per frame".

Note the contrast: the UI kit's own `tree.rs` virtualises through
`uniform_list` (`tree.rs:351`), and the editor element already virtualises by
`visible_range`. The buffer is already light. The tree is the part that is not.

Work:

- Virtualise with `uniform_list`.
- Move row construction out of render into the `RuntimeUpdate::State` handler.
- Stable element ids, no per-frame `format!`.
- Keyboard: `j`/`k` move, `h`/`l` collapse/expand, `Enter` opens, with a
  selection index in the panel's state.

No new dependencies, no protocol change, no ADR.

### 4.2 Phase 2 — create, rename, delete

Additive requests; every protocol enum is `#[non_exhaustive]`, so this costs
nothing to existing clients. Local and synchronous, like `ListFiles` and
`GetWorkspaceDiff` — resolved in `fs-service` outside the core lock.

```
CreatePath { workspace_id, path, kind: File | Directory }        -> Ack
RenamePath { workspace_id, from, to }                            -> Ack
DeletePath { workspace_id, path, expected_revision: Option<..> } -> Ack
```

Six design points that are not obvious and that must be settled before any code
is written:

1. **An empty directory is invisible.** `list_files` runs
   `git ls-files --cached --others --exclude-standard`
   (`crates/fs-service/src/lib.rs:300`) and returns **files only** — the doc
   comment at `lib.rs:89` says so, and git does not track directories at all.
   `FileKind::Directory` exists in `domain` but the listing never emits it. So
   a freshly created empty folder cannot appear in the tree. Options: (a) the
   client remembers just-created empty directories until something lands in
   them; (b) `list_files` gains a directory walk; (c) "new folder" only exists
   as "new `folder/file`". **(a) is recommended** — it touches no service and
   is honest about what git can see.
2. **Directories have no `revision`.** The content hash that guards `WriteFile`
   has no meaning for a directory. Simplest honest rule: refuse to delete a
   non-empty directory without an explicit recursive flag.
3. **A rename must never overwrite.** An agent may have created the destination
   between the read and the write. `fs::rename` on Unix replaces silently. The
   atomic primitives are `renameat2(RENAME_NOREPLACE)` on Linux and
   `renamex_np(RENAME_EXCL)` on macOS — `nix` is already a workspace
   dependency. A `try_exists` check first leaves a small TOCTOU window; that is
   probably an acceptable trade, but it must be a written decision rather than
   an oversight.
4. **Canonicalise the parent, not the target.** `canonicalize()` fails on a path
   that does not exist yet, so create and rename must canonicalise the parent
   directory and then join the new name. This is also what closes the escape
   through a symlinked directory pointing outside the checkout.
5. **`fs::rename`, not `git mv`.** Git detects renames by content similarity at
   diff time, and the editor deliberately does no staging (`plan-editor.md` §4).
   None of this belongs in `git-service`.
6. **Refresh by re-reading.** After a mutation the GUI re-issues `ListFiles`,
   consistent with "the view is asked for, not remembered".

With these three primitives in place, an oil.nvim-style directory buffer — edit
the listing as text, apply on save — reduces to diffing the listing into a set
of mutations. That is a later step and depends on nothing else here.

### 4.3 Phase 3 — `crates/text-model`

A pure crate: no GUI toolkit, no tokio, no state, no I/O. The same shape as
`git-service` and `fs-service`, and testable with `cargo test -p text-model`
without opening a window.

```
Range { anchor, head }
Selection            n ranges, one primary
movement::{next_word_start, prev_word_start, line, ...}
textobject::{inside, around}      brackets, quotes, word, paragraph
Transaction                       changes, and how they carry the selection
```

This is the load-bearing decision of the plan: **the same crate serves phase 4a
and phase 4b**, so the model work is not thrown away whichever way the wiring
question is answered later.

### 4.4 Phase 4 — wiring (deliberately deferred)

- **4a — drive `InputState` from `text-model`, single selection.** Requires the
  upstream `selected_range` accessor from §3. Delivers the daily feel: motions
  that select as they move, `d`/`c`/`y` over a selection, text objects, `;`/`,`.
- **4b — own the text element, real multi-selection.** Only if 4a proves the
  appetite. This is the path `plan-editor.md` §1.2 rejected, and it is still
  months — but now with a measured reference number (§3) and with the model
  already written and unit-tested.

## 5. Delivery order

Each step leaves the tree green under `scripts/dev check` and is useful alone.

| # | Delivery | Verified with |
| --- | --- | --- |
| 1 | Tree virtualised, rows out of render, stable ids | the app; C7 re-read |
| 2 | Tree keyboard navigation | the app |
| 3 | `nucleo` ranking in `fs-service` name search | `cargo test -p fs-service` |
| 4 | `CreatePath` / `RenamePath` / `DeletePath` in `fs-service` | `cargo test -p fs-service` |
| 5 | Those three through `protocol` + `daemon` + `client` | `cargo test -p daemon --test integration` |
| 6 | Tree affordances for new / rename / delete | the app |
| 7 | `crates/text-model` with its tests | `cargo test -p text-model` |
| 8 | Upstream PR for `selected_range` | opened in parallel with step 1 |
| 9 | Phase 4a, behind an opt-in preference | the app |

Steps 1-6 are self-contained and answer no open question. Step 7 depends on §6.1.
Step 9 depends on step 8 landing upstream.

## 6. Open decisions

1. **Helix muscle memory, or modal editing in general?** They are not the same
   target. Helix's object→action order will fight a Vim user's hands, and the
   answer shapes `text-model` from its first commit.
2. **Is multi-cursor a requirement or a luxury?** A requirement makes phase 4a a
   throwaway bridge and puts 4b on the budget from the start. A luxury lets 4a
   be the destination, and the plan gets substantially shorter.

Both gate phase 3 and beyond. Neither gates phases 1 and 2.

## 7. Out of scope

No LSP (unchanged from `plan-editor.md` §4). No macros or registers. No
`:`-commands. No embedded Neovim: it was considered — a PTY session running
real `nvim` would be nearly free, given the terminal already passes the
`vim`/`htop` smokes (`crates/daemon/tests/integration.rs:1036`) — but it writes
to disk unmediated, which is precisely the single-owner invariant ADR-012
exists to state. `nvim-rs`, the obvious in-process client, is **LGPL-3.0** and
rejected by `deny.toml` regardless.

## 8. Gates this plan must clear

- **`deny.toml`** — GPL closed (Zed's `vim`), LGPL closed (`nvim-rs`), MPL-2.0
  open (`nucleo`). Any new dependency re-runs `cargo deny check`.
- **ADR-002** — dependencies are reviewed by rev; consuming an
  upstream `selected_range` change means a deliberate rev bump, and a fork
  would be a change of policy, not a patch.
- **ADR-012** — the daemon owns the filesystem. The phase 2 mutations keep that
  true; anything writing to the checkout behind the daemon's back does not.
- **C7** — phase 1 exists partly to *fix* a C7 breach; it must not introduce
  another. The key handler must not allocate per keystroke.
- **C4** — `text-model` must be a leaf: no `ui`, no `theme tokens`, no
  `client`, no GUI toolkit. `fs-service` is the precedent to copy — it depends only on
  `git-service`, `thiserror` and `tracing`, and defines its own result types
  rather than reaching for `domain`.
