# File workbench

A non-modal, terminal-styled file explorer. The DOM API
works with plain TypeScript or a framework: mount into an element, update through
the handle, and call `destroy()` on unmount. There are no Solid, React, Tauri,
filesystem, network, global keyboard or application-store dependencies.

## Build and consume

In Forge, run `bun install` in `apps/tauri`, then `bun run build:file-workbench`.
The app uses this package through its workspace dependency. To use it elsewhere,
copy this directory, run `npm install`, then `npm pack`. The tarball includes ESM,
TypeScript declarations and the stylesheet. Install the tarball in the consumer.

```ts
import { createFileExplorer } from "@forge-node/file-workbench";
import "@forge-node/file-workbench/style.css";

const explorer = createFileExplorer(treeElement, {
  icon: (row) =>
    row.isFile
      ? { url: `/icons/${extensionOf(row.path)}.svg` }
      : { url: "/icons/folder.svg", tint: true },
  onOpen: (path) => openDocument(path),
  onRefresh: () => reloadListing(),
  onContextMenu: (row, point) => showMenu(row.path, point),
  onDirectoriesChange: (directories) => updateWatchInterest(directories),
});
explorer.setState({
  tree: {
    entries: [{ path: "src/main.ts", kind: "File", ignored: false }],
    truncated: false,
  },
});

// On unmount:
explorer.destroy();
```

The root entry exports the explorer. `/tree` exposes pure listing/filter/navigation
helpers. Document editing belongs to the host; Forge uses its daemon-supervised editor.

## Ownership and extension points

- The host owns file identities, active documents, filesystem mutations, tabs
  and persistence. The package reports gestures and renders supplied observations.
- A `FileTree` contains relative paths. `Directory` entries represent real empty
  folders too. Lazy hosts supply `loadedDirectories` and answer
  `onExpandDirectory(path)` with immediate children for every folder, ignored or
  not. Legacy `onExpandOpaque` remains available. The package never crawls disk;
  the host owns request IDs, generations, retries and per-directory errors.
- `setState` updates the listing and optional decorations without discarding
  expanded directories. `reset` clears workspace-local navigation.
- `reveal(path)` is the explicit _show this path in the tree_ gesture: it opens
  the path's ancestors, selects it, scrolls it into view and drops the filter,
  so a host-drawn filter box has to be cleared with it. `follow(path)` is the
  passive counterpart, for when the host's active document changed — a tab
  click, the palette, a path link. It selects without dropping the filter,
  opens ancestors but never folds one back, and holds the selection still when
  the listing does not name the path (a truncated scan, a file deleted under
  its open tab, a path outside the checkout). A row already selected and on
  screen is not scrolled. Which one a host calls _is_ the setting: there is no
  toggle inside the package.
- `chrome` decides how much of the surface the package draws. `"full"` (the
  default, and what the demo runs) is the standalone frame: header, refresh
  button, filter box, message and footer. `"list"` is for a host with a
  component kit of its own — the package keeps only the virtualised tree, and
  the frame becomes the host's:
  - `setFilter(query)` narrows the listing from the host's own input, and
    `onFilterFocus` is where `action("filter")` sends focus instead of the
    suppressed one. `reset()` and `reveal()` clear the query.
  - `onDerived` reports `{ rows, files, filtered, truncated, loading }` on
    every change and only on a change, so the host renders a count, a loading
    placeholder and the two different empty states — nothing here, versus
    nothing matching the filter — without walking the rows a second time.
  - `state.error` is not drawn in `"list"` chrome; the host renders failures.
  - Keep the stable mount available for root/background interactions and root
    creation even when no real files exist. Draft rows count as visible rows.
- `icon` resolves a row's mark to a URL the host owns, called per painted row
  and never per listing. The package places the artwork and keeps its colours;
  it ships none of its own and reads nothing but the row. When the resolver's
  answer depends on host state the listing does not carry — an icon set that
  follows the theme — pass a new `state.revision` to repaint. `tint: true`
  draws the same URL as a `mask-image` over `currentColor` instead: use it for
  a mark that says what the row _is_ — a folder — and style
  `.fw-tree-icon[data-tint="true"]` to choose the colour. Artwork whose hue is
  the information — a language mark — stays untinted.
- `edit(request)` renames a row in place or opens a blank row under a parent
  for a path that does not exist yet; `edit(null)` closes the field. The
  package owns the input — option (a) of the two shapes, because the row it
  edits is virtualised and an overlay the host positions would drift the
  moment the listing repaints or the list scrolls. It is one element, moved by
  `top` and never re-parented, so a re-listing arriving mid-type moves the
  field and keeps the half-typed name and the caret. A create inserts one
  draft row so the rows below make room; it is not in the tree, is not counted
  as a file, and is not reported as a directory to watch.
  - `Enter` commits through `onEditCommit(request, name)` and the field stays
    open: the host writes, then calls `edit(null)` on success or
    `editFailed(message)` to show the refusal on the row and let the name be
    corrected. `Escape` cancels an unsubmitted edit. Losing focus leaves the
    field intact so menu dismissal and workspace refreshes do not discard a draft.
  - Rename edits the target basename, never its compact visual label. The host
    validates the proposed name/path before writing. A submitted field cannot
    send Enter twice; `editFailed` restores editing without losing its value.
  - While the field has focus the tree's own keymap stands down, so arrows,
    Enter and the filter chord belong to the input.
- `onDirectoriesChange` reports root (`""`) and expanded directories, not an
  instruction to recursively scan the workspace. Filtering does not create new
  watch interests. Native watchers belong to the host and must have budgets.
- `collapseAll()` folds the current directories. `reveal(path)` retains its
  ancestor intent as lazy responses arrive. `onPointerDown(event, row)` is an
  optional host gesture hook on the stable tree element; row nodes retain
  `data-path` and `data-file-directory` and the tree has `data-file-tree-root`. A host drag controller owns
  capture, thresholds, cancellation and click suppression, not recycled rows.
  `expand(path)` opens a folded folder without changing selection or scroll and
  ignores an active inline edit. Forge's controller uses it after hover dwell;
  `reveal(path)` remains the confirmed-move selection action.
- `retarget(from, to)` maps retained selection after a confirmed move, even if
  the old row has already left the listing. Link target metadata is preserved
  in rows and explained in tooltips; directory links remain addressable rather
  than compacting through their target.
- The explorer uses delegated events, a ResizeObserver and windowed rows. Only
  the visible range plus overscan has DOM nodes. Its animation frame is scheduled
  by changes/scrolling, with no continuous render loop.
- Row nodes are reused: a paint rewrites only the rows whose content moved, and
  the listing is re-derived only when the tree, the folds or the filter change.
  A `setState` that carries the same listing back — a save that renamed nothing,
  a loading flag going up and down — costs no DOM and no re-derivation.

## Styling

The explorer supplies its own frame and uses the following optional variables;
defaults use CSS system colors and monospace, with no imported app theme:

| Variable                                      | Purpose                                                                           |
| --------------------------------------------- | --------------------------------------------------------------------------------- |
| `--fw-background`, `--fw-foreground`          | Surface and text                                                                  |
| `--fw-muted`, `--fw-border`, `--fw-accent`    | Secondary text, separators, focus                                                 |
| `--fw-hover`, `--fw-selection`                | Row interaction                                                                   |
| `--fw-added`, `--fw-modified`, `--fw-deleted` | Optional decorations                                                              |
| `--fw-untracked`, `--fw-conflict`             | Optional; fall back to added and deleted                                          |
| `--fw-font`, `--fw-font-size`                 | Monospaced typography                                                             |
| `--fw-row-height`                             | Unitless row height in pixels, at least 16; a host may size `.fw-tree-row` itself |

## Standalone example

With Forge's Vite dev server running, open
`/packages/file-workbench/demo/`. It imports only the public package entries and
mounts an explorer without Solid or Tauri. The in-memory fixture shows a small
listing and reports the selected file in a status element.

In Forge, the adapter maps theme tokens, dialogs, shortcuts and revisioned IO.
The daemon watches visible directories and parents of open documents, coalesces
native events, and releases watches with the connection. This is not a recursive
workspace index; closed directories reconcile when expanded. The global file
palette uses a separate bounded index, not only the currently expanded tree.
