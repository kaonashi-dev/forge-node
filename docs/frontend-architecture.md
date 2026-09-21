# Frontend architecture (`apps/tauri/src`)

The WebView is Solid, not React. It renders chrome and a passive cell grid from
a replica `client` keeps; the daemon owns PTYs, Git, files and SQLite. The Rust
host is a thin bridge ([architecture.md](./architecture.md)); this page is the
ownership map for the TypeScript.

Frontend paths on this page are relative to `apps/tauri/src/`. The Rust host
lives in `apps/tauri/src-tauri/src/`.

## Folders

```text
src/
  main.tsx                 entry: theme, platform flag, render
  app/                     composition; may import everything below
    App.tsx
    lifecycle/             listener binding, effects startup, connection
      events/              Tauri listener groups and event application
    integrations/          checkout/workspace coordination and path retargeting
    shell/                 window composition: AppShell, CenterStack, Sidebar, TitleBar
      dialogs/             application-level confirm/text-input
      tabs/                tab strip interaction (order and MRU live in navigation/)
    palette/               command/file palette composition
  contracts/               hand-written wire mirrors of `domain`/`protocol`
    generated/             generated fixture constants (not hand-edited)
  runtime/                 Tauri invoke transport, frame channels and shared clock
  state/                   cross-feature application state
  navigation/              passive view descriptors, parked tabs, reveals, tab order
  actions/                 action ids, key matching, dispatch registry
  features/<name>/         one capability: UI, commands, state, tests
  shared/                  reusable and feature-free (cell-grid, input, markdown, paths)
  ui/                      control kit (visual primitives only)
  theme/                   tokens, theme primitives, icons
  styles/                  application stylesheets
```

`features/` today: `terminal`, `editor` (`cells/`, `dom/`, `conflict/`),
`files` (`explorer/`, `directories/`, `index/`, `watches/`, `operations/`,
`search/`, `preview/`, `references/`), `git`, `pull-requests`, `sessions`,
`projects`, `settings`. Commands and answer state belong with their
capability; names include `commands.ts`, `state.ts` and feature-specific stores.
There is no required `components/` + `services/` split inside a feature.

## Dependency rules

The design contract is one-way foundation dependencies and narrow, named APIs
between features. Cross-feature orchestration belongs in `app/integrations/`;
shared rendering receives effects from callers. For example,
`shared/markdown/Markdown.tsx` takes URL activation and path-rendering callbacks.

The table describes the current checker's allowances for direct imports within
`src/`. External packages are not covered by this ownership table.

| Owner | May import | May not import |
|-------|------------|----------------|
| `contracts/` | contracts | everything else |
| `runtime/` | contracts, `runtime/` | state, features, app |
| `state/` | contracts, state, `runtime/host.ts` | features, app |
| `shared/` | contracts, shared, theme, ui | state, features, app |
| `theme/` | contracts, theme | ui, state, features, app |
| `ui/` | actions, contracts, theme, ui | state, features, app |
| `actions/` | actions, contracts, `runtime/host.ts`, state | features, app |
| `navigation/` | contracts, navigation, shared, state, theme | features, app |
| `features/<name>/` | the foundations above, its own files, and the peer features listed below | `app/`, other features not listed |
| `app/` | anything | — |

Peer feature allowances are enumerated in
[`scripts/boundaries.ts`](../apps/tauri/scripts/boundaries.ts) (`PEER_EDGES`): `editor → files`;
`files → editor, git, settings, terminal`; `git → editor, files,
pull-requests, sessions, settings`;
`projects → git, pull-requests, sessions, settings`; `pull-requests → editor,
files, git, sessions, settings`; `sessions → editor, git, settings,
terminal`; `settings → projects, sessions, terminal`; `terminal →
files`. Several allowances are reciprocal. They currently permit every module
in the target feature, including its internal stores; the checker does not
enforce public entry points.
These broad allowances are remaining coupling to narrow, not permission to add
arbitrary peer imports. Review each new dependency against a named public API.

### What the boundary check verifies

`bun run boundaries`, also part of `bun run check`, builds a graph from production
`.ts` and `.tsx` files. It excludes `.test.ts`/`.test.tsx` files and
`contracts/generated/`, checks detected imports against the allowances above,
and rejects multi-module cycles among edges classified as runtime imports.

Its current parser uses regular expressions and resolves relative paths only:

- It recognizes common static, side-effect, re-export and literal dynamic imports.
- Declaration-level `import type` and `export type` edges are excluded from cycle
  checking; inline `import { type X }` is classified as runtime and can report a
  false cycle.
- Aliases and unresolved specifiers are omitted. Multiple statements on one line
  can hide an import from the parser.

A passing check covers the detected graph, not complete TypeScript resolution
or feature API encapsulation. TypeScript parser/resolver coverage and module-level
peer rules remain implementation work; keep typechecks and dependency review in
the gate alongside this check.

## State owners

| Store | Owns |
|-------|------|
| `state/forgeStore.ts` | the snapshot replica; `applyShellSnapshot` reconciles by id |
| `state/connection.ts` | connection kind, generation, active session/terminal, notice, the pending session selection the focus ring waits on |
| `state/loading.ts` | per-surface in-flight flags |
| `state/workspace.ts` | active checkout identity and `focusWorkspace`; feature resets register through `onWorkspaceChange` |
| `state/preferences.ts` | `ui.*` keys, ranges, read/write helpers, one-shot seeding |
| `features/files/state.ts` | tree, file, search, name-search answers and staleness |
| `features/git/state.ts` | diff, review, rebase, branches, Juva draft |
| `features/settings/state.ts` | usage analytics |
| `features/projects/dialogs.ts`, `features/sessions/dialogs.ts`, `state/dialogs.ts` | dialog request slots by owner |
| `navigation/viewsStore.ts` | parked centre views, centre mode, reveals; passive only |
| `navigation/sidebarStore.ts`, `navigation/tabOrder.ts`, `navigation/tabMru.ts`, `navigation/tabSwitcher.ts` | sidebar view choice, strip order, focus ring over every pane (Code views and sessions), switcher gesture |
| `navigation/switcherRing.ts` | reads the three stores above the ring and installs the focus-tracking effect; `navigation/tabTargets.ts` beside it is pure and owns nothing |
| `features/terminal/terminalStore.ts` | terminal pane state |
| `features/editor/conflict/editorConflictStore.ts`, `features/git/sessionChangesStore.ts`, `features/pull-requests/{prComposeStore,prReviewStore}.ts` | feature answers |

`app/integrations/workspaceFocus.ts` coordinates directory focus, pending path
operation reconciliation, and file/Git/loading resets. Currently `focusWorkspace`
publishes the new identity before invoking those listeners. The resets are batched,
but the whole transition is not: a reactive observer can see the new workspace
with the previous workspace's answers. Making that transition atomic remains open.

Terminal frames never enter a store: `runtime/bus.ts` has dedicated
`cellsChannel`, `editorCellsChannel` and `editorFrameChannel`
channels, and panes subscribe to the one they paint. Clipboard and file-change
notifications use separate channels in the same module.

## Lifecycle

`app/lifecycle/index.ts` (`startAppRuntime`) binds runtime and capability listener
groups through `bindAll`, starts document watches, workspace-focus coordination,
path-mutation effects and Git decoration effects, then connects and applies the
first snapshot. It returns a disposer consumed by `AppShell`'s cleanup.

`app/shell/AppShell.tsx` also starts Git sync, checkout watching, keymap/tab-switch
handlers, native menu listening and update listening. It remains an additional
lifecycle owner; `startAppRuntime` does not start every application effect.

`app/lifecycle/events/` groups Tauri listeners and applies events through feature
stores and handlers. The grouping is not yet strictly by capability: `events/git.ts`
also handles editor conflicts, transcripts and usage analytics.
`app/integrations/editorTabs.ts` synchronizes authoritative session paths into
views and conflict state; tab-close workflows live in `features/editor/tabs.ts`.
Explorer/terminal drag coordination currently lives in
`features/files/explorer/fileDrag.ts`, with live targets registered by the terminal.

### Lifecycle gaps to preserve as follow-up work

New subscriptions and timers should start explicitly and return cleanup functions.
The current implementation still has these gaps:

| Owner | Current limitation |
|-------|--------------------|
| `app/lifecycle/bind.ts` and `events/*` | `bindAll` cleans successful groups on failure, but a group's internal `Promise.all` loses successful listener disposers when another registration rejects. |
| `features/git/decorations.ts` | The disposer removes the subscription and reactive root, but leaves its pending refresh timer and queued work alive. |
| `app/shell/AppShell.tsx` | Runtime/menu/update registration resolves asynchronously; cleanup before resolution does not dispose the subsequently returned handles. |
| `runtime/clock.ts` | Importing the module in a WebView starts a 15-second interval with no disposer. |
| `features/files/directories/directoryState.ts` | `freshDirectoryTree` is a module-level reactive root whose disposer is not retained. |

These limitations require code and lifecycle tests; moving files or updating this
map does not resolve them.

## Rust host ownership

Within `apps/tauri/src-tauri/src/`, `lib.rs` sets up Tauri and registers thin entry
points from `commands.rs`; `menus.rs` builds the native menus. `runtime/bridge.rs`
owns the runtime loop and frame/state routing. `runtime/workbench/mod.rs` owns the
separate bounded worker, enqueue handling and dispatcher, with command definitions
in `runtime/workbench/commands.rs` and capability handlers in sibling modules.

The workbench split is within one worker sharing the existing `client::Client`.
It does not create a thread per feature or move file/Git reads onto the terminal
input queue. Workspace filesystem and Git effects remain daemon-owned.

## Where to change what

| Change | Look in |
|--------|---------|
| terminal rendering and selection | `shared/cell-grid/`, `features/terminal/TerminalPane.tsx` |
| clipboard and terminal input | `shared/input/clipboard.ts`, `features/terminal/commands.ts`; host `runtime/input.rs` and `crates/terminal-input` encode PTY input |
| editor pane (cells surface), chords, chrome, autosave | `features/editor/cells/`, `features/editor/editorChrome.ts`, `features/editor/editorAutosave.ts` |
| DOM editor surface, key translation, windowing | `features/editor/dom/` |
| editor/disk conflict UI | `features/editor/conflict/` |
| file tree, expansion, drag | `features/files/explorer/`, `shared/paths.ts` |
| directory reads, request identity | `features/files/directories/` |
| file index, recents, invalidation | `features/files/index/` |
| file watches and document reloads | `features/files/watches/` |
| create/rename/delete and reconciliation | `features/files/operations/` |
| project search and palette file search | `features/files/search/` |
| previews, images, markdown routing | `features/files/preview/`, `shared/markdown/` |
| path links in terminal/transcript/markdown | `features/files/references/` |
| diff, review, rebase, branches, decorations | `features/git/` |
| pull request list/detail/compose/review | `features/pull-requests/` |
| session launch, menus, context, history, attention | `features/sessions/` |
| projects, worktrees, ignores, share rules | `features/projects/` |
| settings sections, profiles, usage | `features/settings/` |
| window composition, tabs, palette, dialogs | `app/shell/`, `app/palette/` |
| workspace transitions and confirmed path retargeting | `state/workspace.ts`, `app/integrations/workspaceFocus.ts`, `app/integrations/retarget.ts` |
| listener startup and event application | `app/lifecycle/`, `app/shell/AppShell.tsx` |
| keybindings and actions | `actions/` |
| theme tokens and primitives | `theme/` (see [theming.md](./theming.md)) |

## Gates

From `apps/tauri`:

```sh
bun run check          # lint, format, vitest, bun tests, boundaries, build, bundle budgets
bun run boundaries     # detected ownership edges and multi-module cycles; limits above
```

From the repository root, for the Rust host: `cargo test -p forge-tauri`, or
the full `scripts/dev check` gate. Bundle budgets and the terminal render
procedures live in [performance.md](./performance.md) and
the [app README](../apps/tauri/README.md). A green build or boundary check does
not establish visual parity, lifecycle cleanup or terminal/editor paint latency;
those need the corresponding behavior checks and measurements.
