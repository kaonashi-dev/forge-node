# File workbench

A non-modal, terminal-styled file explorer and CodeMirror editor. The DOM API
works with plain TypeScript or a framework: mount into an element, update through
the handle, and call `destroy()` on unmount. There are no Solid, React, Tauri,
filesystem, network, global keyboard or application-store dependencies.

## Build and consume

In Forge, run `bun install` in `apps/tauri`, then `bun run build:file-workbench`.
The app uses this package through its workspace dependency. To use it elsewhere,
copy this directory, run `npm install`, then `npm pack`. The tarball includes ESM,
TypeScript declarations and the stylesheet. Install the tarball in the consumer.
CodeMirror packages are peer dependencies so the host can share one instance.

```ts
import { createFileExplorer } from "@forge-node/file-workbench";
import { createFileEditor } from "@forge-node/file-workbench/editor";
import "@forge-node/file-workbench/style.css";

const editor = createFileEditor(editorElement, {
  doc: initialText,
  onChange: () => markUnsaved(),
  onCursor: ({ line, column }) => showPosition(line, column),
});

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

// A host-approved disk update retains the selection and scroll position.
editor.setDoc(newText);
// Switching documents resets selection and undo history.
editor.setDoc(otherDocumentText, false);

// On unmount:
explorer.destroy();
editor.destroy();
```

The root entry only loads the explorer. Import `/editor` lazily to keep CodeMirror
out of the initial bundle. `/tree` exposes pure listing/filter/navigation helpers.
`/symbol` exposes identifier extraction without importing an editor.

## Ownership and extension points

- The host owns file identities, active documents, drafts, revision checks, saves,
  tabs and persistence. `onChange` is an invalidation, not a full-document copy;
  call `editor.text()` when saving or otherwise needing the complete text.
- `setDoc()` is silent: it never calls `onChange`. It does report cursor changes.
  The host must resolve conflicts before applying external content.
- Supply CodeMirror `theme` and `findExtension` extensions at construction;
  `setTheme`, `setLanguage` and `setGitMarks` update the editor in place.
  `onOpenDefinition` and `onRevealDiff` bridge navigation to the host.
- `readOnly` disables both editing and editing commands. It is fixed for that
  editor instance. Normal editors use ordinary text editing, undo, selection,
  mouse input and find; there are no Vim modes or exit gestures.
- A `FileTree` contains relative paths. `Directory` entries can represent empty
  folders; ignored directory entries are opaque until the host peels them
  (`onExpandOpaque` → one-level listing merged in). Directories implied by file
  paths are synthesized. The package does not crawl them.
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
  - The tree hides itself when there are no rows. Collapse the mount element
    too (`.fw-mount[hidden]`), or the host's message renders beside an empty
    box that is still claiming the space.
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
    corrected. `Escape` cancels. Losing focus cancels — but only to somewhere:
    a blur with no `relatedTarget` (a menu unmounting, the window going away)
    is not the person leaving the field.
  - A name that is blank or unchanged is a cancel, not a write. `editedName`
    and `nameSelection` are exported as the pure rules behind that and behind
    the stem-not-extension preselection, so a host can test its own copy.
  - While the field has focus the tree's own keymap stands down, so arrows,
    Enter and the filter chord belong to the input.
- `onDirectoriesChange` reports root (`""`) and expanded directories, not an
  instruction to recursively scan the workspace. Filtering does not create new
  watch interests. Native watchers belong to the host and must have budgets.
- The explorer uses delegated events, a ResizeObserver and windowed rows. Only
  the visible range plus overscan has DOM nodes. Its animation frame is scheduled
  by changes/scrolling, with no continuous render loop.
- Row nodes are reused: a paint rewrites only the rows whose content moved, and
  the listing is re-derived only when the tree, the folds or the filter change.
  A `setState` that carries the same listing back — a save that renamed nothing,
  a loading flag going up and down — costs no DOM and no re-derivation.

## Styling

Use `.fw-document` around an editor, `.fw-document-head` for its toolbar,
`.fw-document-content` for the editor host, and `.fw-document-foot` for status.
The explorer supplies its own frame. Both use the following optional variables;
defaults use CSS system colors and monospace, with no imported app theme:

| Variable                                      | Purpose                                    |
| --------------------------------------------- | ------------------------------------------ |
| `--fw-background`, `--fw-foreground`          | Surface and text                           |
| `--fw-muted`, `--fw-border`, `--fw-accent`    | Secondary text, separators, focus          |
| `--fw-hover`, `--fw-selection`                | Row interaction                            |
| `--fw-added`, `--fw-modified`, `--fw-deleted` | Optional decorations                       |
| `--fw-font`, `--fw-font-size`                 | Monospaced typography                      |
| `--fw-row-height`                             | Unitless row height in pixels, at least 16 |

## Standalone example

With Forge's Vite dev server running, open
`/packages/file-workbench/demo/`. It imports only the public package entries and
mounts both components without Solid or Tauri. The in-memory fixture includes
external updates, unsaved-draft protection and a 50,000-file listing.

In Forge, the adapter maps theme tokens, dialogs, shortcuts and revisioned IO.
The daemon watches visible directories and parents of open documents, coalesces
native events, and releases watches with the connection. This is not a recursive
workspace index; closed subtrees still refresh when the full listing is re-read.
