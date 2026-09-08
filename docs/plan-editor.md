# Plan — File editor in the core (§17, new)

The user's goal: to be able to **see the directory tree, navigate it, open a
file, edit it and search it** without leaving Forge. This document is the
up-front research and the delivery plan.

**Partly superseded (2026-08-29).** Keyboard-driven editing in the Helix model,
a virtualised and keyboard-navigable tree, and filesystem mutations (create,
rename, delete) are planned in
[`plan-helix-editor.md`](./plan-helix-editor.md), which reopens two items §4
below puts out of scope. This document stays the reference for what was
actually delivered.

## 0. Precondition: this contradicts the written plan

`plan.md:166` left "Code editor" out of the MVP and `plan.md:1307` repeated it
as a non-goal ("No editor"), in line with P7 ("no attempt to rebuild an IDE").
The diff viewer was on that same list and is implemented today, so it reads as a
phase boundary and not as a prohibition — but going ahead requires **rewriting
those two lines and adding ADR-012**. Otherwise, `harness/CHECKPOINTS.md` C4-C5
rejects the work against the invariants.

## 1. What was discarded, and why

### 1.1 Zed's core: closed by licence

Read from the checkout the workspace already pins
(`~/.cargo/git/checkouts/zed-*/69e2130`, rev `69e2130295c2`):

| Zed crate | Licence |
| --- | --- |
| editor support crates | Apache-2.0 |
| `editor`, `rope`, `text`, `language`, `multi_buffer`, `project`, `worktree`, `fuzzy`, `settings`, `theme`, `workspace` | **GPL-3.0-or-later** |

Everything that *is* the editor is GPL. `deny.toml` already forbids it in
writing: "No copyleft that would infect a distributed binary — in particular no
GPL crate copied out of Zed (§32, ADR-002)". Forge ships as a binary under
`MIT OR Apache-2.0`, so that route does not exist without relicensing the whole
project.

Even without the licence problem it would be unfeasible:
`crates/editor/Cargo.toml` depends on `lsp`, `dap`, `client`, `db`, `assets`,
`workspace`, `project`, `multi_buffer`, `markdown`, `file_icons`… it is half of
the Zed application, not a component.

### 1.2 Writing an editor from scratch: unnecessary

Rope, wrapping, undo, selections, IME, text measurement. Months of work to end
up where we already are without writing anything — see 1.3.

### 1.3 What we do have: the UI kit already ships the core

`the UI kit` 0.5.1 (Apache-2.0, already pinned by rev in the manifest)
includes a complete code editor in `apps/tauri **~9k lines** that we
use today only as a single-line text field:

| Piece | File | What it brings |
| --- | --- | --- |
| `InputMode::CodeEditor` | `input/mode.rs` | line numbers, indent guides, language, highlighter, diagnostics |
| Buffer | `input/state.rs` | `ropey::Rope`, not `String` |
| Render | `input/element.rs` | **virtualised** by `visible_range`: a long file does not cost per line |
| Undo/redo | `history.rs`, `input/change.rs` | already done |
| Wrapping / indentation | `input/text_wrapper.rs`, `input/indent.rs` | already done |
| Search inside the file | `input/search.rs` | `SearchMatcher` + `SearchPanel` |
| LSP (optional) | `input/lsp/` | hover, completions, definitions, code actions |
| Tree | `tree.rs` | `TreeState`/`TreeItem`, expand/collapse, keyboard |

And the point that closed this door until now: **`LanguageRegistry::register` is
public** (`highlighter/registry.rs:480`). Without the `tree-sitter-languages`
feature the registry starts with JSON only (`highlighter/languages.rs`,
`#[cfg(not(feature = ...))] enum Language { Json }`), so we register by hand the
six grammars the workspace already compiles for the diff. The 31 MB cost the
manifest documents and rejects does not apply.

`tree-sitter` resolves to **a single 0.25.10** in the lock and the UI kit
asks for `0.25.4`: same major, a single universe of types,
`LanguageConfig.language: tree_sitter::Language` is assignable.

**API at the pinned rev:** `InputState::code_editor(lang)`, not the `EditorState`
from upstream documentation. The live reference example is
`the UI kit` → `crates/story/examples/editor.rs` (tree + editor + grammar
registration).

## 1.4 Research: who already does this?

| Reference | What it brings | What it does not |
| --- | --- | --- |
| **the UI kit `story/examples/editor.rs`** (Apache-2.0, free) | Tree + `InputState::code_editor` + `LanguageRegistry::register` + in-file search. It is the UI reference. | No daemon, no remote FS, no conflict with agents. |
| **VS Code / Cursor** | The "the file changed on disk" dialog (reload vs keep). The UX pattern for `FileChanged` + a dirty buffer. | Electron / proprietary editor; not a reusable component. |
| **OpenCode** | Diff viewer + path tree (already imitated in §16.7). Directory sync. | No embedded file editor; the editing is done by the agent. |
| **Aider / Continue / Cline** | Editing through an agent + git. | No tree/editor UI of their own; last-write-wins between human and agent. |
| **Lapce / Helix** | Native Rust editors. | A different stack (or GPL-adjacent); does not embed into this shell. |
| **Zed `editor`** | A gold-standard native editor. | GPL — closed by §1.1. |

Conclusion: **there is no free crate that already is "tree + editor + FS through
a daemon + optimistic revision against agents"**. The closest and most reusable
thing is the the UI kit example plus VS Code's conflict pattern. Forge
already isolates agents in worktrees; the revision covers the same-checkout case
(a human edits while an agent writes).

## 2. What we build in Forge

### Guiding principle

**The daemon owns the filesystem; the GUI never does `std::fs`.** That is
ADR-012 (an extension of ADR-011 to files): a single owner, the GUI as a
replica. It is also what leaves the door open to a remote daemon without redoing
the layer.

### 2.1 `crates/fs-service` (new)

Sibling of `git-service`: pure functions, no GUI toolkit, no tokio, no state.

| Operation | Implementation | New deps |
| --- | --- | --- |
| List the tree | `git ls-files --cached --others --exclude-standard -z` — one subprocess, respects `.gitignore` for free (ADR-008). A recursive `read_dir` as the fallback outside a repo | 0 |
| Read | `std::fs` + a byte cap + binary detected by a `NUL` in the first 8 KB | 0 |
| Write | temporary file + atomic `rename`, preserving permissions | 0 |
| Search content | `git grep -n -I --no-color -z` with a result cap; fallback to walk + scan outside a repo | 0 |
| Search by name | fuzzy (case-insensitive subsequence) over the path list, in the daemon | 0 |

`list_files` returns **flat relative file paths**; the tree with directories is
built by the GUI (the same single-child collapsing as `diff_view`). There is no
`dir` parameter in v1: one complete listing feeds both the panel and `cmd-p`.

Caps with the same honesty as `git-service::diff`: `MAX_FILE_BYTES` (2 MiB) and
`MAX_SEARCH_RESULTS` / `MAX_TREE_ENTRIES`, each answer with `truncated: bool`. A
file above the cap is not opened halfway: it is rejected and the reason is
stated.

The `revision` is a hex hash of the content (`DefaultHasher` → 16 hex). It is
not cryptographic: it only detects "the disk is no longer what you read".

### 2.2 The real blocker: agents edit the same files

This is what tells Forge apart from any other editor. While you have a buffer
open there is an agent writing to that same path. If `WriteFile` just dumps
text, the agent's work disappears silently and the bug is irreproducible.

`ReadFile` returns a `revision`; `WriteFile` demands it back. If it does not
match the disk, the daemon **rejects** with `ErrorCode::PreconditionFailed` (the
same code as `CloseSession` over a live session). The GUI re-reads with
`ReadFile` and says "this file changed while you were editing it" — the content
is not embedded in the error (the error channel is a message; the content
travels through the synchronous read that already exists).

In the other direction, an open buffer has to find out. **v1:** when the window
regains focus, `stat` + compare the revision of every open buffer (zero new
dependencies). **v1.1 / step 7:** a `notify` watcher (CC0-1.0, already in the
lock through the UI kit) per workspace, ~300 ms debounce, a `FileChanged`
event. A dirty buffer is never overwritten on its own: it is marked as a
conflict and the user chooses to reload or save-over.

### 2.3 `domain` — `src/file.rs` (new)

`FileTree { workspace_id, entries, truncated }`,
`FileEntry { path, kind: File | Directory }`,
`FileContents { path, text, revision, language, binary, too_large }`,
`SearchResults { matches, truncated }`,
`SearchMatch { path, line, column, text }`,
`SearchKind { Name, Content }`.

Runtime-only, like `WorkspaceDiff` and `PullRequest`: **no column, no migration,
no field in `Store`**.

### 2.4 `protocol`

Local synchronous requests, modelled on `GetWorkspaceDiff` (`core.rs`,
`client/src/ipc.rs`): they answer directly, they do not do ack-then-event, they
do not broadcast, they do not persist. They do not open a socket, so the "ack
now, event later" invariant does **not** apply.

```
ListFiles   { workspace_id }                                    -> FileTree
ReadFile    { workspace_id, path }                              -> FileContents
WriteFile   { workspace_id, path, text, expected_revision }     -> Ack
SearchFiles { workspace_id, query, kind: Name|Content, limit }  -> SearchResults
```

One new event, this one broadcast: `DaemonEvent::FileChanged { workspace_id,
path }`. The enum is `#[non_exhaustive]`, so adding it is additive. In v1 the
event may not be emitted yet (focus-re-stat); the variant is left ready.

### 2.5 `daemon`

`core.rs`: `list_files` / `read_file` / `write_file` / `search_files` resolve the
path with `workspace_path` (already present) and call `fs-service` **outside the
core lock**, like every git path. Each path is canonicalised and checked to fall
inside the workspace before anything is touched: a path with `..` that escapes
the checkout is an `InvalidRequest`, not a file that gets read.

### 2.6 `client`

`Client::list_files`, `read_file`, `write_file`, `search_files`. No state in
`Store`: a file is a view you ask for, not shared state that gets broadcast.
`FileChanged` does go through the event channel once the watcher exists.

### 2.7 `theme tokens`

- In `init`, after `the text-input control register the grammars with
  `LanguageRegistry::register` and the queries exported by the crates
  (`HIGHLIGHTS_QUERY` / `HIGHLIGHT_QUERY`). TypeScript concatenates JS+TS as
  `highlight.rs` already does for the diff. It requires `tree-sitter` as a direct
  dependency of the crate.
- In `theme::apply`, populate `Theme::global_mut(cx).highlight_theme` from
  Forge's palette (`syntax_*`, `editor()`, active line). **That line does not
  exist today**, so a borrowed editor would paint with the UI kit's colours.
  The AGENTS.md rule still stands: no colour is written outside `theme`.

### 2.8 `ui`

- `file_tree.rs` (new): a tree fed by `ListFiles`, with the same single-child
  directory collapsing `diff_view.rs` already uses. Home: `RightPanel::Files`,
  next to History and Pull Requests.
- `editor_view.rs` (new): `InputState::code_editor(lang)` plus dirty / conflict
  state. An open file is one more `ViewTab` (`session_tabs.rs`), with its `x`,
  like the diff and the PR.
- `runtime.rs`: `RuntimeCommand::{LoadFileTree, OpenFile, SaveFile, SearchFiles}`
  and their `RuntimeUpdate`s. Same path as `LoadDiff`: through the runtime
  thread, never through the render one — that channel also carries
  `RuntimeCommand::Input`.
- Command palette: file finder scope for fuzzy open
  (`cmd-p`).
- Content search: a results panel in the right panel or an overlay reusing the
  `pr_panel.rs` layout.
- Search **inside** the file: the UI kit's (`SearchPanel`), not ours.

`highlight.rs` (the diff's highlighter) is **not touched**. They are two
consumers with different caches; unifying them is a later refactor.

## 3. Delivery order

Every step leaves the tree green under `scripts/dev check` and is usable on its
own.

| # | Delivery | Verified with |
| --- | --- | --- |
| 1 | `fs-service` with its tests | `cargo test -p fs-service` |
| 2 | Requests in `protocol` + `daemon` + `client` | `cargo test -p daemon --test integration` / scenario |
| 3 | Read-only file tree in the right panel | the app |
| 4 | Open a file → read-only editor with colour | the app |
| 5 | Editing and saving with a revision precondition | conflict E2E |
| 6 | Search by name (`cmd-p`) and by content | the app |
| 7 | Watcher (`notify`) + `FileChanged` — or focus-re-stat if deferred | E2E / manual |

Steps 1-4 are self-contained and break nothing that already exists. Step 5 is
where the real risk lives, and step 7 is the one that closes it.

## 4. Out of scope

No LSP (the UI kit's scaffolding is there, but each server is another process
to manage, and that really is building an IDE), no multi-cursor, no inline git
blame, no staging from the editor, and no opening files outside the active
workspace. Which files were left open is not persisted either: like the diff,
the view is asked for, not remembered.
