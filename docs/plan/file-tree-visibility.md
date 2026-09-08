# Plan — Files that never appear (Tauri file tree + editor)

Status: **diagnosed, not yet fixed.** This document is the plan only; no code
in it has been applied.

Scope: the `Files` tab of the right panel (`FileTreePanel`) and the centre
editor (`EditorView`) in `apps/tauri`, plus the listing contract they read from
(`fs-service` → `domain::FileTree` → `workbench:file_tree`).

---

## 1. Symptom

Opening the `Files` tab of the inspector shows one of three wrong things,
depending on timing:

- a listing containing **only the files at the root of the checkout**
  (`README.md`, `Cargo.toml`, `Makefile`, …) and no directories at all —
  everything under `apps/`, `crates/`, `docs/`, `scripts/` is absent and there
  is no row to click that would reveal it;
- the copy **"No checkout selected."** while a session is plainly open;
- the copy **"Reading the checkout…"** that never resolves.

Opening a file that *is* visible works once. Opening a second file and then
switching back to the first editor tab shows **"Nothing open."** for a file that
is on disk.

## 2. Root cause

the GUI built the directory hierarchy **in the GUI** from a flat,
files-only listing. The Solid port kept the flat listing but dropped the
building step, so the view now expects a shape the daemon has never sent.

### F1 — the daemon never sends a directory entry (blocking)

`fs-service` only ever constructs `EntryKind::File`:

- `crates/fs-service/src/lib.rs:296` `list_via_git` — `git ls-files --cached
  --others --exclude-standard -z`, and every pushed entry is
  `kind: EntryKind::File` (`:322`). Paths ending in `/` are explicitly skipped.
- `crates/fs-service/src/lib.rs:329` `list_via_walk` / `:337` `walk` — recurses
  *through* directories and pushes only files (`:373`).
- `EntryKind::Directory` (`:74`) is declared and never constructed anywhere in
  the workspace. `crates/daemon/src/core.rs:1921` maps it to
  `domain::FileKind::Directory` for a case that cannot occur.

Both the service and the domain type say so in their own doc comments:
`crates/fs-service/src/lib.rs:88` and `crates/domain/src/file.rs:35` —
*"directories are inferred by the GUI when it builds the tree"*.

The Solid view does not infer them. `apps/tauri/src/workbench/filetree.ts`
`visibleRows` iterates `tree.entries` verbatim and hides any entry whose every
ancestor is not present in the `expanded` set (`ancestorsExpanded`). With no
directory entries in the listing, no directory row can be rendered, so no
directory can ever be expanded, so **every path containing a `/` is filtered
out permanently.** What survives is exactly the depth-0 files.

The reference implementation that must be ported is
`apps/tauri (`tree_rows` / `flatten`): it splits each
path, synthesises the `Node` hierarchy, collapses only-child directory chains
into a single row (`apps/tauri panel shows a nested path as one
row, not three), and resolves the fold state at build time.

### F2 — fold semantics are inverted against parity

the shell holds a `collapsed: HashSet<String>` — the tree opens **expanded**, and a
folded directory is the exception. `FileTreePanel.tsx:17` holds
`expanded: Set<string>` starting empty — the tree opens **fully folded**. Even
after F1 is fixed, the first paint of a checkout would show only top-level
directory names. Fixing F1 without F2 trades "no files" for "no files until you
click", which is still not parity.

### F3 — the listing is requested once, on mount, and never again

`FileTreePanel.tsx:72-102`: the `loadFileTree` call sits inside `onMount`,
guarded by `if (workspace && !workbenchStore.tree)`.

The workspace id is not known at that moment in the common case. `RightPanel`
resolves it in a `createEffect` (`RightPanel.tsx:21-24`) from
`runtimeStore.activeSession` / `forgeStore.workspaces`, which are only populated
by `runtime:connected`. A panel mounted before the daemon answers reads
`workbenchStore.workspace === null`, skips the request, and — having no
reactive dependency on `workbenchStore.workspace` — never retries. Result:
**"No checkout selected."** under a live session.

The same gap fires on every session switch: `focusWorkspace`
(`store/workbenchStore.ts:54`) clears `tree` to `null` by design, and nothing
asks for the new one while the panel stays mounted.

### F4 — the editor holds one file and re-reads nothing

`workbenchStore.file` is a single slot. `EditorView` (`EditorView.tsx:21`)
renders only when `workbenchStore.file?.path === props.path`, and the only
`ReadFile` for the initial paint is the one `FileTreePanel.open()` fires at
click time (`FileTreePanel.tsx:56-58`). `EditorView` never requests its own
content on mount or when `props.path` changes.

So any editor tab that is *re-focused* — a second tab, a tab restored from
`viewsStore.byWorkspace` after coming back to a checkout — shows
**"Nothing open."** indefinitely.

### F5 — a failed request leaves the panel spinning

`setLoading("tree", true)` is set before the call, and `loading` is only ever
cleared by an incoming `workbench:file_tree` / `workbench:file_tree_failed`
event (`runtime/events.ts:52,58`). The call sites swallow the rejection:
`.catch(() => undefined)` at `FileTreePanel.tsx:58,101` and
`EditorView.tsx:47,55`. If `invoke("send_workbench_command")` itself rejects —
worker gone, workspace id unknown to the daemon — no event is ever emitted and
the surface is stuck on **"Reading the checkout…"** with no error text.

### F6 — the listing is not virtualised, and the panel has no scroll box

`<For each={rows()}>` (`FileTreePanel.tsx:120`) renders every row.
`fs_service::MAX_TREE_ENTRIES` is `10_000`, so a monorepo expanded to its leaves
is ten thousand live DOM nodes — the cost `docs/performance.md` and invariant C7
exist to forbid; an earlier list used virtualisation for exactly this reason
(`apps/tauri and the comment above it).

Layout, separately: `.inspector-body` (`styles.css:1089`) is `padding` only —
no `flex: 1`, no `min-height: 0`, no `overflow`. `.file-tree` (`:695`) sets
`min-height: 0; overflow-y: auto` but never receives a bounded height, so it
grows instead of scrolling and the whole `.inspector` scrolls with it, carrying
the tab strip off screen.

### F7 — the tests are green because the fixtures are wrong

`apps/tauri/src/workbench/filetree.test.ts` builds its fixture with explicit
`{ kind: "Directory" }` entries, and `crates/protocol/src/bin/export_fixtures.rs:227`
`sample_file_tree` does the same (`"src"` as `Directory`, then `"src/main.rs"`).
Neither shape is producible by `fs-service`. The unit tests assert the view
against a listing the daemon cannot emit, which is why F1 survived review.

---

## 3. Decision

**Synthesise the hierarchy in the GUI**; do not teach `fs-service` to emit
directory rows.

Reasons: it is what both doc comments already promise, it keeps the wire flat
and bounded by one budget instead of two, it keeps `git ls-files` as the single
source of ignore rules, and it is a straight port of code that already shipped
in `apps/tauri `EntryKind::Directory` stays in the type (it is
`#[non_exhaustive]`-adjacent domain vocabulary and the walk fallback may want it
later) but the GUI must not depend on receiving one.

---

## 4. Fixes, in order

Each step is independently committable and leaves the tree building.

### Step 1 — rebuild `filetree.ts` around a files-only listing

`apps/tauri/src/workbench/filetree.ts`, 

- New internal `Node = { dirs: Map<string, Node>; files: string[] }`; build it
  by splitting each `entry.path` on `/`. Tolerate a `Directory` entry if one
  ever arrives (create the node, push no file) so the view is correct under
  both shapes.
- Sort directories before files, each case-insensitively by name, matching the
  `BTreeMap` + push order of the reference.
- Collapse only-child directory chains: a directory with no files and exactly
  one subdirectory renders as one row labelled `a/b/c` whose `path` is the deep
  path.
- Change the row type to carry `label` (what is painted) separately from `path`
  (identity for fold/open/`data-path`), and `folded` resolved at build time.
- Change the API from `visibleRows(tree, expanded)` to
  `treeRows(tree, collapsed)` — **collapsed** set, default open (F2).
- Rewrite `collapseTarget` / `expandTarget` against the new row shape and the
  inverted set. Behaviour stays: `h` folds an open directory else selects the
  parent; `l` unfolds a folded one else steps to the first child.

### Step 2 — make the panel reactive and honest

`apps/tauri/src/panels/FileTreePanel.tsx`.

- Replace the `expanded` signal with `collapsed`; rename `fold`/`unfold` to
  add/remove from it.
- Replace the `onMount` request with a `createEffect` on
  `workbenchStore.workspace`: when it is non-null and `workbenchStore.tree` is
  null and `loading.tree` is not already set, request the listing. This closes
  F3 for both the late-connect and the session-switch case.
- Keep the keyboard registration in `onMount` — it is correct there.
- Give the request a real failure path (F5):
  `loadFileTree(ws).catch((e) => { setLoading("tree", false); setWorkbenchStore("treeError", String(e)); })`.
  Same treatment for `openFile` in `open()`.
- Distinguish the two empty states in the fallback copy: "No checkout selected."
  only when `workbenchStore.workspace === null`; otherwise "Reading the
  checkout…" while loading and "This checkout has no files." when the answer
  arrived empty.

### Step 3 — let the editor ask for its own file

`apps/tauri/src/workbench/EditorView.tsx`.

- Add a `createEffect` on `props.path` + `workbenchStore.workspace`: when the
  store's `file.path` is not `props.path` and no read is in flight, fire
  `openFile(workspace, props.path)` and set `loading.file`.
- Guard it against the dirty-draft rule already in place: the effect requests,
  the existing effect at `:28` decides whether the answer replaces the draft.
  Reset `draft`/`dirty`/`conflict` when `props.path` itself changes, so a
  reused component instance does not carry another file's buffer.
- Same `.catch` treatment as Step 2 (F5).

### Step 4 — surface transport failures once, centrally

`apps/tauri/src/workbench/api.ts`: let `send()` reject with a normalised
`Error`, and add a small helper the panels use so each surface's
`*Error` + `loading` are cleared together. No `.catch(() => undefined)` should
remain in `FileTreePanel` or `EditorView` after this step.

### Step 5 — bound the list and the box

- `FileTreePanel.tsx`: windowed rendering over `rows()` — render a slice around
  the scroll offset with a spacer above and below, sized by
  `var(--forge-row-h)`. Row height is already fixed (`styles.css:899`), so a
  fixed-height window is enough; no dependency needed.
- `styles.css`: `.inspector-body { flex: 1; min-height: 0; overflow: hidden; }`
  and let `.panel-body` / `.file-tree` own the scroll
  (`flex: 1; min-height: 0; overflow-y: auto`). Drop the redundant
  `--depth: 0` reset on `.tree-row.file` (`:696`) — the inline custom property
  is the only source of indent.

### Step 6 — fix the fixtures, then the tests

- `crates/protocol/src/bin/export_fixtures.rs:227`: make `sample_file_tree`
  files-only (`"README.md"`, `"src/main.rs"`, `"src/util/mod.rs"`) so the
  exported fixture matches what `fs-service` can produce. Regenerate with
  `make fixtures`, which rewrites `apps/tauri/tests/fixtures` and the generated
  TS in `apps/tauri/src/runtime/generated/fixtures.ts`; `runtime/generated.test.ts`
  must stay green.
- `apps/tauri/src/workbench/filetree.test.ts`: rewrite the fixture as a flat
  files-only listing and port the four Rust cases as the regression set —
  *synthesises directories and collapses only-child chains*, *a collapsed
  directory hides its children*, *a folded row carries its own state*, *files
  are never folded* — plus the one that would have caught this bug:
  **"a files-only listing renders its nested files"**.
- Add a panel-level test that a workspace arriving *after* mount triggers the
  listing request (F3), if the harness allows mounting `FileTreePanel` with a
  stubbed `invoke`.
- `crates/fs-service`: add a test asserting `list_files` returns nested paths
  and that no entry is `EntryKind::Directory`, pinning the contract the GUI now
  relies on.

---

## 5. Verification

1. `make check` — `cargo fmt --all -- --check`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo test --workspace`, then the frontend
   gate (`pnpm -C apps/tauri test` + `cargo test -p forge-tauri`).
2. `make fixtures` must produce no diff after Step 6 has been committed.
3. Manual, `make run-tauri`, against this repository as the checkout:
   - `Cmd/Ctrl+Shift+F` opens the `Files` tab with the tree already expanded and
     `apps/`, `crates/`, `docs/`, `scripts/` present with their children;
   - `j`/`k`/`h`/`l` walk, fold and unfold; `Enter` opens a file into the centre;
   - open two files, switch between their tabs — both show their text, neither
     shows "Nothing open.";
   - switch sessions to another checkout and back — the tree reloads without a
     tab change, and the panel never sticks on "Reading the checkout…";
   - a checkout with >5 000 files scrolls without the tab strip leaving the
     panel, and typing in the terminal stays responsive while it is open.

## 6. Out of scope

The `Features` and `Lieutenant` tabs (Phase 5, unported by design), the file
palette (`palette/entries.ts` `fileEntries`, which reads `SearchFiles` and is a
separate path), and any change to the `ListFiles` request or its budgets.
