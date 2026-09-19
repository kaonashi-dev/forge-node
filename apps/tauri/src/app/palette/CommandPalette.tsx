import { Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { COMMAND_PALETTE } from "../../actions/actions";
import { enterContext } from "../../actions/dispatch";
import { forgeStore } from "../../state/forgeStore";
import { connectionStore } from "../../state/connection";
import { Button, Combobox, Dialog, type ComboboxOption } from "../../ui/index";
import { SessionGlyph } from "../../features/sessions/SessionGlyph";
import { Icon, LangIcon } from "../../theme/icons/index";
import {
  BRANCHES,
  CREATE,
  FILES,
  SESSIONS,
  admits,
  fileEntries,
  paletteEntries,
  rank,
  scopeLabel,
  scopePlaceholder,
  type PaletteEntry,
  type PaletteScope,
} from "./entries";
import { initialFiles, recentPaths } from "../../features/files/index/recentFiles";
import { navigationFileIndex, navigationFilePaths } from "../../features/files/index/fileIndex";
import { nameResults, scheduleNameSearch } from "../../features/files/search/paletteSearch";
import { warmFileTree } from "../../features/files/commands";
import { filesStore, setFilesStore } from "../../features/files/state";
import { gitStore } from "../../features/git/state";
import { activeWorkspace } from "../../state/workspace";

/** Rows shown before the list scrolls. */
const VISIBLE_ROWS = 12;

export type CommandPaletteProps = {
  scope: PaletteScope;
  onChoose: (entry: PaletteEntry) => void;
  onDismiss: () => void;
};

export function CommandPalette(props: CommandPaletteProps) {
  const [query, setQuery] = createSignal("");

  /**
   * Whether the repository listing has been needed yet.
   *
   * The palette opens on the recent files. Typed queries go to
   * `SearchFiles { kind: Name }` so ranking stays in the daemon.
   */
  const [searching, setSearching] = createSignal(false);

  createEffect(() => {
    const workspace = activeWorkspace();
    if (workspace && admits(props.scope, FILES)) warmFileTree(workspace);
  });

  const index = createMemo(navigationFileIndex);
  const treePaths = createMemo(() => navigationFilePaths());

  createEffect(() => {
    if (searching() && admits(props.scope, FILES)) {
      scheduleNameSearch(activeWorkspace(), query());
    }
  });

  const allFiles = createMemo(() => {
    if (!(searching() && admits(props.scope, FILES))) return [];
    const hits = nameResults();
    if (!hits) return [];
    return fileEntries(
      activeWorkspace(),
      hits.matches.map((match) => match.path),
    );
  });

  /**
   * What the file list opens on: recently opened here, then whatever has
   * uncommitted changes.
   *
   * The tree is `git ls-files` and carries no mtime, so this is as close to
   * "recently edited" as the shell can get without asking the daemon a new
   * question — and in a checkout an agent has been writing to, the changed
   * files are the better half of the answer anyway.
   */
  const recentFiles = createMemo(() => {
    if (!admits(props.scope, FILES)) return [];
    const paths = treePaths();
    if (!paths) return [];
    const workspace = activeWorkspace();
    return fileEntries(
      workspace,
      initialFiles(
        recentPaths(workspace),
        gitStore.diff?.files.map((file) => file.path) ?? [],
        new Set(paths),
      ),
    );
  });

  const files = () => (query().trim() === "" ? recentFiles() : allFiles());

  const entries = createMemo(() => {
    const base =
      props.scope === "files"
        ? []
        : paletteEntries(forgeStore, connectionStore.activeSession).filter((entry) =>
            admits(props.scope, entry.group),
          );
    const ranked = rank(base, query());
    const listed = files();
    if (listed.length === 0) return ranked;
    const head = new Set<string>([CREATE, SESSIONS, BRANCHES]);
    return [
      ...ranked.filter((entry) => head.has(entry.group)),
      ...listed,
      ...ranked.filter((entry) => !head.has(entry.group)),
    ];
  });

  const options = createMemo<ComboboxOption<PaletteEntry>[]>(() =>
    entries().map((entry) => ({
      value: entry,
      group: entry.group,
      disabled: !entry.enabled,
      render: () => (
        <>
          <PaletteGlyph entry={entry} />
          <span class="palette-label">{entry.label}</span>
          <Show when={entry.note}>{(note) => <span class="palette-note">{note()}</span>}</Show>
          <Show when={entry.hint}>
            {(hint) => <kbd class="forge-kbd palette-hint">{hint()}</kbd>}
          </Show>
        </>
      ),
    })),
  );

  onMount(() => {
    // While the palette is up it owns the keyboard: without this the terminal
    // context is still active and ⌘C would copy the grid behind the card.
    onCleanup(enterContext(COMMAND_PALETTE));
  });

  return (
    <Dialog title="Command palette" hideTitle flush size="lg" onDismiss={props.onDismiss}>
      <Combobox
        options={options()}
        query={query()}
        onQuery={(next) => {
          if (next.trim() !== "") setSearching(true);
          setQuery(next);
        }}
        onChoose={props.onChoose}
        label={scopePlaceholder(props.scope)}
        placeholder={scopePlaceholder(props.scope)}
        visibleRows={VISIBLE_ROWS}
        empty={<p class="empty-copy">Nothing matches.</p>}
        leading={
          <Show when={scopeLabel(props.scope)}>
            {(label) => <span class="palette-scope">{label()}</span>}
          </Show>
        }
      />
      <Show when={admits(props.scope, FILES) && index()?.truncated}>
        <p class="panel-note">Partial file index — some paths may not be listed.</p>
      </Show>
      <Show when={admits(props.scope, FILES) && filesStore.nameSearchError}>
        <p class="panel-error">{filesStore.nameSearchError}</p>
      </Show>
      <Show when={admits(props.scope, FILES) && filesStore.treeError}>
        <p class="panel-error">Could not refresh the file index: {filesStore.treeError}</p>
        <Button
          onClick={() => {
            setFilesStore("treeError", null);
            const workspace = activeWorkspace();
            if (workspace) warmFileTree(workspace);
          }}
        >
          Retry file index
        </Button>
      </Show>
    </Dialog>
  );
}

/**
 * The mark on a palette row.
 *
 * Read off the choice rather than carried on the entry: every kind of row
 * already says what it does, and a second field to keep in step would be a
 * second place for a new kind of entry to be forgotten. A command has no mark
 * — nothing distinguishes one command from another at 14px — but it keeps the
 * column, so every label in the list starts on the same pixel.
 */
function PaletteGlyph(props: { entry: PaletteEntry }) {
  const glyph = () => {
    const choice = props.entry.choice;
    switch (choice.kind) {
      case "new_shell":
      case "focus_session":
        return <Icon name="square-terminal" class="forge-icon-faint palette-glyph" size={14} />;
      case "new_agent":
        return (
          <span class="palette-glyph">
            <SessionGlyph providerId={choice.provider} emphasis="dim" size={14} />
          </span>
        );
      case "focus_workspace":
        return <Icon name="git-branch" class="forge-icon-faint palette-glyph" size={14} />;
      case "open_file":
        // The same mark the file tree draws (`LangIcon`), read off the same
        // name: two lists of the same paths that disagree about what a
        // `.json` looks like read as two different inventories.
        return <LangIcon path={choice.path} class="palette-glyph" size={14} />;
      default:
        return <span class="palette-glyph" aria-hidden="true" />;
    }
  };

  return <>{glyph()}</>;
}
