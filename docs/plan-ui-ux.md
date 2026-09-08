# Plan — UI/UX to the Zed / Orca / Superset bar

**Status:** executed (2026-09-01). Everything below is implemented except the
five items listed in §10, which records what was not done and why. Where the
implementation departed from the plan the reason is in §10 as well; the body of
this document is left as it was written, so the two can be read against each
other.
Scope is `apps/tauri` (Solid + Vite in a Tauri WebView). Four axes, in the
order the user named them: the **code editor**, **performance**, **usability**,
and **base component style**.

Read alongside [`architecture.md`](./architecture.md),
[`performance.md`](./performance.md) (the cost model this plan must not break),
[`theming.md`](./theming.md), [`plan-editor.md`](./plan-editor.md) (what was
delivered) and [`plan-helix-editor.md`](./plan-helix-editor.md) (written for
the shell; §2 below re-answers its questions for the web stack).

---

## 0. The bar, in one table

What the three references actually do that Forge does not yet, restricted to
things observable in their UIs. Nothing below proposes copying code.

| Quality | Zed | Orca | Superset | Forge today |
| --- | --- | --- | --- | --- |
| Editor | Virtualised, multi-cursor, selection-first, keyboard-complete, git gutter, folding, find/replace, go-to-line | Embedded editor over the worktree | Embedded editor + diff per worktree | `<textarea>` under a shiki `<pre>`; whole-file paint, 6 000-line cap, no line numbers, no find, no multi-cursor (`workbench/EditorView.tsx`) |
| Diff | Inline + split, hunk nav, intra-line, stage/unstage | Per-task diff | Per-workspace diff with review | Unified only, no hunk nav, no intra-line, one DOM row per line for the whole patch (`workbench/DiffView.tsx`) |
| Tree | Keyboard tree, git decorations, filter, context menu, rename/create/delete | Worktree tree | Worktree tree with change counts | Virtualised and keyboard-driven, but no ARIA, no git decoration, no filter, no context menu (`panels/FileTreePanel.tsx`) |
| Tabs | Preview tabs, pin, close others/right, reopen closed | — | Per-workspace tabs | Reorder + close only; no preview, no pin, no bulk close (`shell/SessionTabs.tsx`) |
| Keys | User keymap file, every action listed with its chord | Configurable | Configurable | One hard-coded table (`actions/actions.ts:261-335`), no override path |
| Theme | Light + dark, density, many themes | Light + dark | Light + dark | Three dark bases, no light, no density (`theme/ThemeProvider.tsx`) |
| Feedback | Toasts, progress, skeletons | Toasts, status pills | Status pills, attention | Inline `panel-error`/`empty-copy` only; `window.confirm()` for destructive actions |
| Density/polish | 26 px rows, 4 px grid, one shadow scale, one motion scale | — | — | Scales exist and are used in `ui/ui.css`; `styles.css` (3 356 lines, 94 % of the CSS) predates them |

The token foundation and the `src/ui/` layer are already at the bar (see §5).
The gap is concentrated in four places: the editor, the legacy stylesheet, the
keyboard/ARIA layer, and a handful of hot loops outside the terminal.

---

## 1. Principles that shape every item below

1. **The daemon owns the filesystem (ADR-012).** The editor stays a replica:
   text arrives by `ReadFile` with a revision, leaves by `WriteFile` conditioned
   on it. No editor library is allowed to touch disk.
2. **Compute on change, not on frame** (`performance.md` §CPU). Nothing that
   scales with the grid, the file, or the job log is rebuilt per event.
3. **One primitive per interaction.** Kobalte stays confined to `src/ui/`
   (`ui/index.ts:1-13`); anything outside that hand-rolls focus or keyboard
   semantics is a defect to fix, not a pattern to extend.
4. **Tokens are load-bearing.** A raw `px`, hex, or z-index in CSS is a lint
   failure once §5.1 lands.
5. **Every shortcut is discoverable.** If it has a chord it appears in the
   palette and in a tooltip; `EmptyCenter.tsx` already derives its hints from
   the binding table and that is the pattern to spread.
6. **Measure before and after.** Each workstream has a number it must move
   (§7). A change with no number is polish and goes last.

---

## 2. Workstream A — the code editor

### 2.1 Decision: replace the textarea/overlay with CodeMirror 6

The current design (`EditorView.tsx` header comment) was the right call for a
zero-dependency first cut, and it has hit its ceiling:

- **Whole-file paint.** Every token is a DOM span and the whole file is painted
  (`highlight.ts:19-27`, `MAX_PAINTED_LINES = 6_000`). Above the cap the file is
  plain; below it a 5 000-line file is tens of thousands of nodes rebuilt on
  every settle.
- **Two layers that must never drift.** Font, padding, tab size and wrapping are
  duplicated between `.editor-paint` and `.editor-area` (`styles.css:1152-1210`)
  and an IME or a ligature that changes advance width breaks the illusion.
- **No editor features.** No line numbers, no find/replace, no go-to-line, no
  bracket match, no fold, no multi-cursor, no indent-aware Enter, no comment
  toggle, no move-line. Each would be reimplemented on a `<textarea>` from
  scratch.
- **The Helix plan is stranded.** `plan-helix-editor.md` was written against
  `the UI kit::InputState` (single private range). That crate is gone.

CodeMirror 6 (MIT, ~150 kB gz for core + a language) answers every one of those:

| Need | CM6 answer |
| --- | --- |
| Virtualised rendering | `EditorView` renders the viewport only; 100k-line files are routine |
| Selection-first, multi-range model | `EditorSelection` is *n* ranges with one primary — the Helix model natively, so `plan-helix-editor.md` §4.3's `text-model` becomes a CM6 keymap, not a crate |
| Vim/Helix | `@replit/codemirror-vim` (MIT) for Vim users; a Helix keymap is a `keymap.of([...])` over `EditorSelection` transforms, buildable in-repo |
| Highlighting | Lezer grammars `@codemirror/lang-{javascript,rust,json,css,html,markdown,yaml}` (MIT); `@codemirror/legacy-modes` covers toml and shell. Shiki stays only for the diff view's read-only rendering if wanted, or is dropped |
| Theme | `EditorView.theme({...})` + `HighlightStyle` built from `--forge-*` / `--fg-*` tokens, one per base, switched with `Compartment` on `themeBase()` |
| Find/replace, go-to-line | `@codemirror/search` |
| Gutter, line numbers, folds, bracket match, active line, indent guides | `@codemirror/view`, `@codemirror/language`, community `indentation-markers` |
| Git gutter | `gutterLineClass` + `lineMarker` fed from `workbenchStore.diff` |
| Undo/redo, IME, platform caret | native to `EditorView`; no second layer |
| Diff | `@codemirror/merge` — unified and split, intra-line, virtualised, hunk gutter |

Bundle cost is paid once and lazily: the editor chunk loads on first "Code" tab,
exactly as shiki and the gallery do today (`App.tsx` lazy import).

**Rejected:** Monaco (2–3 MB, its own worker model, no selection-first model);
keeping the textarea and adding features one by one (each feature is a new way
for the two layers to drift); an embedded `nvim` PTY (writes to disk behind the
daemon, `plan-helix-editor.md` §7).

### 2.2 Editor deliverables

Ordered so each step ships alone.

| # | Deliverable | Detail |
| --- | --- | --- |
| A1 | `workbench/editor/` module: `createEditor(host, opts)` returns a handle; `EditorView.tsx` becomes a thin Solid wrapper | Doc is set from `FileContents.text`; dirty/conflict/save logic from today's component is kept verbatim. `save_file` still goes through `saveFile(workspace, path, text, revision)` |
| A2 | Theme bridge | `theme/editorTheme.ts` builds `EditorView.theme` + `HighlightStyle` from the same tokens `tokens.ts` emits, with a unit test that every scope colour clears 4:1 on `--forge-editor` (mirror of `theming.md` Legibility) |
| A3 | Language registry | `workbench/language.ts` keeps `grammarFor()`; the table maps to CM6 `LanguageSupport` loaders. Unknown stays plain, as now |
| A4 | Baseline features on by default | line numbers, active-line, bracket match, fold gutter, indent on Enter, `⌘/` comment, `⌘D` next occurrence, `⌥↑/↓` move line, `⌘⇧K` delete line, `⌘F` find, `⌥⌘F` replace, `⌃G` go-to-line, `⌘]`/`⌘[` indent, rectangular selection on `⌥`-drag |
| A5 | Git gutter | Added/modified/removed marks per line from `workbenchStore.diff` for the open path; click a mark to reveal the hunk in the Diff tab |
| A6 | Conflict UX | Replace the single "Discard mine and reload" banner with three choices: *Keep mine*, *Take disk*, *Compare* (opens a `@codemirror/merge` view of both). Dirty state shows on the Code tab label (`●`) |
| A7 | Tab-level polish | Preview tab on single click (italic label), pinned on double-click/edit; `⌘⇧T` reopens the last closed; breadcrumb of the path above the editor with a click-to-tree action |
| A8 | Autosave (opt-in setting) | On blur / after 1 s idle, only when not in conflict. Off by default: agents write the same files |
| A9 | Vim keymap (opt-in) | `@replit/codemirror-vim` behind a preference; a status-bar mode pill |
| A10 | Helix keymap (opt-in, after A9) | Own keymap over `EditorSelection`: `w/b/e` select, `x` line, `d/c/y` over selection, `mi(`/`ma"` objects, `;`/`,`, `C`/`⌥C` add cursor. Answers `plan-helix-editor.md` §6 without a Rust crate |
| A11 | File-tree mutations | `CreatePath`/`RenamePath`/`DeletePath` per `plan-helix-editor.md` §4.2 (that section is stack-independent and still correct); tree context menu items and `F2`/`⌫` bindings |

### 2.3 Diff view deliverables

| # | Deliverable |
| --- | --- |
| D1 | Replace `DiffView.tsx`'s row `<For>` with `@codemirror/merge` `unifiedMergeView` per file, read-only, lazily mounted when the file section is expanded |
| D2 | Split view toggle (persisted), intra-line change highlighting, `]c`/`[c` and `⌥F5`-style next/prev hunk, per-file collapse with the same twisty |
| D3 | Sticky file header while scrolling, `+/−` totals in the header, file glyph from `fileGlyph()` |
| D4 | Open-in-editor at the hunk's line from the diff; jump-to-diff from the editor gutter (pairs with A5) |

---

## 3. Workstream B — performance

The terminal path is already sound (canvas, rAF-coalesced dirty rows, 16 ms
floor on the Rust side, run-merged wire format, `bus.ts` bypassing Solid stores).
The findings are outside it.

### 3.1 Confirmed hot spots

| # | Location | Problem | Fix |
| --- | --- | --- | --- |
| P1 | `store/harnessStore.ts:162-172`, `store/lieutenantStore.ts:107-130` | Every `job_output` batch copies the whole tail (2 000 / 400 lines) and writes at absolute indexes, leaving sparse holes (dictionary-mode arrays in V8). Whole array reference replaced each batch | Ring buffer keyed by `fromLine`; append in place with `setStore("output", jobId, next.length, line)`; trim by slice only when over budget |
| P2 | `workbench/JobStreamView.tsx:60-71` | `<For>` over a value-keyed string array that is replaced wholesale; duplicate blank lines collide as keys | `<Index>`; the list only appends or slides |
| P3 | `workbench/JobStreamView.tsx:33-38` | Layout read + scroll write on every batch | Only when the tail grew and the user is pinned to the bottom |
| P4 | `terminal/renderer.ts:107-144` with `palette.ts:32-66` | Colour → hex string rebuilt per run per paint for indexed/truecolor cells; `font()` is cached but colour is not | 256-entry indexed cache + small LRU for truecolor |
| P5 | `terminal/renderer.ts:163-174` | `save()`/`restore()` per run regardless of need | Only around `DIM` runs |
| P6 | `src-tauri/.../cells.rs:195` | While scrolled into history every damaged frame is a full frame at up to 62 fps | Deliberate; document in `performance.md` and cap background repaint to 30 fps while `scroll_offset > 0` |
| P7 | `store/harnessStore.ts:123-129` | `mergeFeature` rebuilds the whole array per event | Indexed store write |
| P8 | `workbench/EditorView.tsx` + `highlight.ts` | Whole-file DOM paint per settle | Solved by A1 (virtualised) |
| P9 | `workbench/DiffView.tsx:66-84` | One DOM row per patch line for every expanded file | Solved by D1 |
| P10 | `palette/fuzzy.ts` + `fs-service::fuzzy_match` | Subsequence test, no scoring; hits in listing order | `nucleo-matcher` in the daemon (`plan-helix-editor.md` §2.1, still valid), scored results on the wire |

### 3.2 Startup and bundle

| # | Deliverable |
| --- | --- |
| P11 | Route-level chunks: editor (CM6 + grammars), diff (`@codemirror/merge`), settings, gallery. Measure with `vite build --report` and pin a budget: initial JS ≤ 350 kB gz |
| P12 | First paint: `TerminalPane` waits on `document.fonts.ready` (correct); preload `JetBrainsMono-Regular` with `<link rel=preload>` so the wait is ~0 |
| P13 | `styles.css` split (§5.2) also cuts unused CSS per route; measure style recalc with the Performance panel on a 500-row sidebar |

### 3.3 Budgets to add to `performance.md`

- Editor: keystroke → paint p95 ≤ 16 ms on a 20 000-line file; open ≤ 100 ms after `ReadFile` returns.
- Diff: expand a 2 000-line patch ≤ 50 ms.
- Job stream: 1 000 lines/s sustained with main-thread idle ≥ 70 %.
- File tree: 50 000 paths, scroll at 60 fps, filter keystroke ≤ 8 ms.
- Initial bundle ≤ 350 kB gz; editor chunk ≤ 250 kB gz.

---

## 4. Workstream C — usability, keyboard, navigation

### 4.1 Keyboard model

| # | Deliverable | Detail |
| --- | --- | --- |
| U1 | User keymap file | `~/…/Forge/keymap.json` read by the daemon (same path family as `theme.json`), merged over `defaultBindings()`; conflicts reported in Settings › Keyboard. `chordFor()` reads the merged table |
| U2 | Settings › Keyboard | Table of every action, its context and chord, inline rebind with conflict detection; "reset" per row |
| U3 | Missing chords | `⌘F` find in terminal, `⌘=`/`⌘-`/`⌘0` terminal zoom, `⌘⇧T` reopen closed tab, `⌘⇧G` Git tab, `⌘⇧E` History tab, `⌘⇧W` close others, `⌘⌥W` close to the right, `⌘\` toggle preview terminal, `⌥⌘←/→` previous/next Code tab |
| U4 | Sidebar tree keyboard | `Sidebar.tsx:447` declares `role="tree"` and the `SIDEBAR` context exists with zero bindings. Add roving tabindex, `↑/↓/←/→/Home/End`, type-ahead, `Enter`, `⌘↩` new terminal here, and a single tab stop |
| U5 | File tree ARIA | `FileTreePanel.tsx:248` has no role. Add `role="tree"`, `treeitem`, `aria-level`, `aria-expanded`, `aria-selected`, `aria-activedescendant` on the container so the existing `j/k/h/l` model becomes a compliant tree |
| U6 | Palette on Kobalte Combobox | `CommandPalette.tsx:88-175` and `BranchPicker.tsx:129-179` hand-roll combobox semantics twice. One `ui/Combobox.tsx` (Kobalte) used by both plus the file palette |
| U7 | Chord hints everywhere | `Tooltip` shows the chord (`<Kbd>`); palette rows show it; menus show it. All from the merged binding table, never a string literal |

### 4.2 Panels and lists

| # | Deliverable |
| --- | --- |
| U8 | File tree: in-panel filter (`⌘⇧F` when the panel has focus), git decorations from `workbenchStore.diff` (colour + `M/A/D/?` glyph, folder roll-up count), context menu (open, reveal in Finder/file manager, copy path, copy relative path, new file/folder, rename, delete), drag to reorder disabled |
| U9 | Inspector tab persisted (`inspectorStore.ts` → `app_state` key `ui.panel.tab`), tab order/fold state moved onto the same typed helpers as `layout.ts` |
| U10 | Tabs: preview tab, pin, close others/right/all, middle-click close, drag a Code tab out into its own session-level tab (later) |
| U11 | Destructive confirms through `AlertDialog`, not `window.confirm()` (`AppShell.tsx:221`, `sessionMenuItems.ts:8-13`, `GitPanel.tsx:82`) |
| U12 | Shared `FilterHeader` (scope radio + search) replacing the copies in `HistoryPanel.tsx:64-84`, `FeaturesPanel.tsx:105-138`, `FeatureView.tsx:301-315` |
| U13 | Shared `ListCard` replacing `history-card`, `feature-card`, `pr-card`; adopt `ui/Card.tsx` |
| U14 | One indent mechanism: `--depth` var everywhere; delete the inline `padding-left` arithmetic in `FeaturesPanel.tsx:210` and `FeatureView.tsx:214` |
| U15 | `ResizeHandle` double-click resets to the token default, not to drag-start width (`ResizeHandle.tsx:66-70`) |
| U16 | Toasts for background outcomes (save done, worktree created, PR opened, agent finished) via `ui/Toast.tsx` (Kobalte); attention stays in the AttentionBar |

### 4.3 Discoverability

| # | Deliverable |
| --- | --- |
| U17 | Replace every `title=` on chrome with `Tooltip` (`Sidebar.tsx`, `SessionTabs.tsx`, `StatusBar.tsx`, `GitPanel.tsx`, `FeaturesPanel.tsx`, `CenterStack.tsx`, `BranchPicker.tsx`, `TitleBar.tsx`, `FileTreePanel.tsx`); make `IconButton` wrap itself in `Tooltip` so keyboard focus shows it |
| U18 | Empty states for every panel derive their CTA and chord from the binding table, as `EmptyCenter.tsx` does |
| U19 | "What's here" first-run: a three-row hint in the empty centre (add project, new terminal, new agent) with chords; dismissible, persisted |

---

## 5. Workstream D — base components and style

### 5.1 Make the tokens load-bearing

Measured state of `src/styles.css` (3 356 lines):

| Metric | Count |
| --- | --- |
| raw spacing `px` (gap/padding/margin) | 221, zero through `--space-*` |
| raw `z-index` integers (1, 2, 3, 5, 15, 30) | 7, none on the `--z-*` ladder |
| raw `border-radius` literals | 10 (incl. `999px` ×3 that *is* `--radius-full`) |
| raw `font-size` | 2 + 5 with inline fallbacks |
| hex colours | 10, all in mask gradients and one stripe pattern |
| `var(--…)` | 556 |

`src/ui/ui.css` (867 lines) is already clean: 0 hex, 3 raw px.

| # | Deliverable |
| --- | --- |
| S1 | Lint gate: a vitest that greps the stylesheets for `\b\d+px\b` outside `tokens.css`/`@font-face`, raw `z-index: \d`, raw `#[0-9a-f]{3,6}` outside mask/stripe allowlist, and fails on any new occurrence (baseline file, ratchet down) |
| S2 | Migrate `styles.css` spacing to `--space-*` (values already coincide: 4/6/8/12/16/24), z-index to `--z-*`, radii to `--radius-*`. Mechanical, one PR per section header |
| S3 | Retire `--forge-*` readers section by section in favour of the semantic layer (`tokens.ts:266-310`); delete the alias table when the last reader is gone |
| S4 | Add the missing rungs: `--radius-2xs: 2px` for hairline chips; `--motion-enter`/`--motion-exit` easing pair; `--space-0.5: 2px` is `--space-1` already |

### 5.2 Split the stylesheet

`styles.css` mixes reset, font-face, focus rules and every screen. Target:

```
src/styles/
  base.css        reset, @font-face, focus ring, scrollbars, reduced-motion
  shell.css       title bar, rail, tabs, status bar, attention bar
  panels.css      inspector panels, tree, cards, filters
  workbench.css   editor, diff, PR, feature, job stream
  settings.css
  harness.css
```

Each imported from the component that owns it (Vite dedupes), so a route chunk
carries only its CSS. Section headers in the current file already map 1:1.

### 5.3 Missing primitives (all Kobalte-backed, all in `src/ui/`)

| Primitive | Why | Replaces |
| --- | --- | --- |
| `Switch` | One answer for a boolean setting | pill buttons / checkboxes in `settings/` |
| `Combobox` | Palette, branch picker, file palette | two hand-rolled comboboxes |
| `Toast` | Background outcomes (U16) | nothing today |
| `Progress` + `Skeleton` | Long jobs, tree load, diff load | `Reading…` copy |
| `Kbd` | Chord display in tooltips, menus, palette, settings | ad-hoc `<span>` |
| `Tree` | Shared roving-tabindex/ARIA tree for sidebar and file tree | two divergent trees |
| `ListCard` / `FilterHeader` | U12, U13 | three copies each |
| `Breadcrumbs` | Editor path header (A7) | `tree-label` span |
| `Popover` | Session menu, project icon picker | `Menu` misuse |

### 5.4 Theme

| # | Deliverable |
| --- | --- |
| T1 | Light base: `gruvbox-light` and `neutral-light`, with the same `derivedTokens()` formulas and Legibility floors; `applyThemeBase` honours `prefers-color-scheme` when the preference is `system` |
| T2 | Editor theme per base (A2) and terminal ANSI per base already exist; make the three agree in one test |
| T3 | Density setting (`compact 22 px` / `default 26 px` / `comfortable 30 px`) as one multiplier on `rowH` and the control ladder; persisted in `app_state` |
| T4 | Bundle `JetBrainsMono-BoldItalic` (or the variable font); preload Regular |
| T5 | `theme/Gallery.tsx` becomes the review surface for every primitive above, with a light/dark and density toggle in its header; a visual-regression screenshot per section via the `run` skill is the gate for S2 |

---

## 6. Roadmap

Phases are sized so each ends green under `make check` and ships alone.

| Phase | Weeks | Contents | Gate |
| --- | --- | --- | --- |
| **1 · Foundations** | 1–2 | S1 lint ratchet, S2 spacing/z/radius migration, U11 dialogs, U15, U17 tooltips, P1–P3, P7 | Lint baseline at zero new; job stream budget met |
| **2 · Editor core** | 3–5 | A1–A4, A6, P8, P11 | Editor budgets (§3.3); conflict E2E still passes |
| **3 · Diff + tree** | 6–7 | D1–D4, A5, U5, U8, P9, P10 | Diff and tree budgets; ARIA audit of both trees clean |
| **4 · Keyboard** | 8–9 | U1–U4, U6, U7, U3 chords, `Combobox`, `Kbd` | Every action in the palette with its chord; keymap file round-trips |
| **5 · Components** | 10–11 | U9, U10, U12–U14, U16, `Switch`/`Toast`/`Progress`/`Skeleton`/`Tree`/`Breadcrumbs`, S3, 5.2 split | Gallery covers every primitive; no `title=` on chrome |
| **6 · Theme** | 12 | T1–T5, P4–P6, P12–P13 | Light base passes Legibility; density switch has no layout jump |
| **7 · Editing model** | 13–15 | A7–A11 | Vim/Helix opt-in; tree mutations E2E |

Phases 1 and 5 are independent of 2–4 and can run in parallel with them.

---

## 7. Acceptance metrics

| Area | Metric | Today | Target |
| --- | --- | --- | --- |
| Editor | max lines with colour | 6 000 | unbounded (viewport) |
| Editor | keystroke → paint p95, 20k-line file | not measurable (whole-file settle) | ≤ 16 ms |
| Diff | expand 2 000-line patch | unmeasured | ≤ 50 ms |
| Job stream | main-thread idle at 1 000 lines/s | unmeasured | ≥ 70 % |
| Tree | 50k paths scroll | 60 fps (virtualised) | keep; filter ≤ 8 ms |
| Bundle | initial JS gz | unmeasured | ≤ 350 kB |
| CSS | raw px/z/hex in `styles.css` | 221 / 7 / 10 | 0 / 0 / allowlist |
| A11y | trees with compliant keyboard + ARIA | 0 of 2 | 2 of 2 |
| Keys | actions with a visible chord in the palette | partial | 100 % |
| Theme | bases | 3 dark | 3 dark + 2 light |

---

## 8. Risks and open decisions

1. **CM6 in a WKWebView.** CodeMirror is well exercised in Safari; the risk is
   IME and `⌘`-chord capture fighting `actions/dispatch.ts`'s capture-phase
   listener. Decision: the `EDITOR` context claims the keydown first and CM6's
   `keymap` runs inside it; app chords that must win (`⌘K`, `⌘P`, `⌘W`) stay
   in `App` and are excluded from CM6's default keymap.
2. **Grammar coverage.** Lezer lacks a first-party TOML grammar; `legacy-modes`
   covers it with StreamLanguage (no folding). Acceptable.
3. **Shiki's fate.** Keep only if the PR/feature views want read-only coloured
   snippets; otherwise drop the wasm chunk. Decide in phase 2.
4. **Helix vs Vim first.** `plan-helix-editor.md` §6 asked; this plan orders
   Vim (a dependency) before Helix (in-repo). Reverse if the team is Helix-only.
5. **Keymap file ownership.** Read by the daemon (parity with `theme.json`) or
   by the WebView (`app_state`)? Daemon keeps one config directory; recommended.
6. **Light theme demand.** T1 is the largest theme item; it is table stakes for
   parity but the least requested. Keep it last in phase 6.

---

## 9. Out of scope

LSP, diagnostics, completions (unchanged from `plan-editor.md` §4). Split
editor panes. Embedded browser preview. Minimap. Collaborative editing.
Any change to the terminal renderer beyond P4–P6: it is under budget and
measured, and this plan does not reopen `docs/terminal.md`.


---

## 10. What was executed, and what was not

The plan was worked end to end. Phases 1–7 landed; the gate below was green at
the end of each of them.

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
apps/tauri: pnpm lint && pnpm test && pnpm exec tsc --noEmit && pnpm build
```

### Not delivered

| Item | Why |
| --- | --- |
| **U3 `⌘F` find-in-terminal** | It needs a search over the scrollback and a highlight in the cell renderer — a feature, not a chord. The action and its binding were *removed* rather than shipped pointing at nothing: a chord bound to a no-op is exactly what U3 exists to eliminate. |
| **U3 `⌘\` preview terminal** | Same reasoning. The existing `PreviewTerminal` is the harness's session preview, not a terminal under the open file; the pane it would need is layout work out of proportion to the item. |
| **T4 `JetBrainsMono-BoldItalic`** | Adding a font binary is not something this change could do. Nothing in the app currently sets bold *and* italic on the same run — the editor's comments are italic and its headings are 600 — so the gap is real but unreached. |
| **A7 preview/pinned tabs** | The bulk closes, middle-click close and the tab context menu landed (U10). Preview-on-single-click needs a `preview` slot in `ParkedViews` and a promotion rule on edit; it is the one part of A7 with a model change behind it, and the rest of A7 (the breadcrumb, `⌘⇧T`) does not depend on it. |
| **U10 drag a Code tab into its own session tab** | The plan already marks this "(later)". |

### Where the implementation departed from the plan

- **§2.3 D1 does not use `unifiedMergeView`.** `@codemirror/merge` wants two
  whole documents; `DiffFile.patch` is `git diff` output, and reconstructing
  "before" and "after" from it yields two files with holes where the unchanged
  regions were. The patch itself became the CodeMirror document instead
  (`workbench/diff/`), which gets D1's actual goal — virtualisation — and keeps
  both line-number columns, which reconstruction would have lost.
- **§4.1 U1 stores the keymap in `app_state`, not `~/…/Forge/keymap.json`.**
  The plan cited "parity with `theme.json`", but there is no such file: it is a
  protocol fixture, and the daemon has no config-directory reader to add a
  second file to. `app_state` is the daemon's config store and already holds
  every other `ui.*` preference. The stored shape is the JSON a file would have
  held, so a file reader can be added later without changing the merge.
- **§4.1 U6 does not use Kobalte's `Combobox`.** Kobalte's is a closed,
  select-like control that owns a value, filters its own options and positions a
  popper in a portal. The palette is the opposite shape — always open, full
  bleed inside a modal, showing every result the caller decided on, with
  headings, yielding an action rather than a value it keeps. `ui/Combobox.tsx`
  shares the semantics (the combobox ARIA pattern, the cursor, the keyboard)
  between the two call sites, which is what the deliverable was for.
- **§5.1 S3 retires the readers, not the layer.** Every stylesheet rule now
  names the semantic layer wherever one exists, and
  `theme/stylesheets.test.ts` fails on a new `var(--forge-*)` that has a
  semantic spelling. The `--forge-*` names that remain are not aliases: they are
  the generated layer that carries the palette and the metrics, `theme/mix.ts`
  is a documented port of `tokens.ts`, and several — `--forge-git-added`,
  `--forge-term-bg`, `--forge-title-h` — are the only name their value has.
- **§5.2 splits the stylesheet but route-scopes only `settings.css`.** The six
  files exist. Only `SettingsRoute` is lazily loaded *and* owns its CSS
  exclusively; `workbench.css` is painted partly by `CenterStack`, which is in
  the entry chunk. The split was checked for cascade safety first: no two of the
  seven files declare the same property on the same selector, apart from
  `.connection-pill { min-width }`, which is why `late.css` loads last.
- **§5.4 T1 adds `system` as a preference value.** "No stored value" and
  "follow the OS" are different states — the first is a fresh install, the
  second is an answer — and only the second should move when the machine
  switches at dusk.
- **Two defects the plan did not name were fixed on the way.** The `FILES`
  context was entered on *mount*, so the file tree took `j`/`k`/`h`/`l` and
  `Enter` away from the editor and every focused control for as long as the
  inspector was on the Files tab; both trees now claim their context on focus,
  and `resolve` ignores bare-letter chords whose target takes text.
  `search_by_name` kept the first `limit` subsequence hits in directory-walk
  order, so the best answer to a query could be the one dropped — it is scored
  now, with the same formula the palette uses.
