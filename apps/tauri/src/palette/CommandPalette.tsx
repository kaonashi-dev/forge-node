import { Show, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { COMMAND_PALETTE } from "../actions/actions";
import { enterContext } from "../actions/dispatch";
import { forgeStore } from "../store/forgeStore";
import { runtimeStore } from "../store/runtimeStore";
import { Combobox, Dialog, type ComboboxOption } from "../ui";
import { Icon, LangIcon, SessionGlyph } from "../theme/icons";
import {
  FILES,
  admits,
  fileEntries,
  paletteEntries,
  rank,
  scopeLabel,
  scopePlaceholder,
  type PaletteEntry,
  type PaletteScope,
} from "./entries";
import { workbenchStore } from "../store/workbenchStore";

/** Rows shown before the list scrolls. */
const VISIBLE_ROWS = 9;

export type CommandPaletteProps = {
  scope: PaletteScope;
  onChoose: (entry: PaletteEntry) => void;
  onDismiss: () => void;
};

/**
 * The `⌘K` palette (§16.4).
 *
 * Owns only its query; everything it lists is derived from the store, which
 * keeps it a pure function of `ShellSnapshot` plus a string. The cursor, the
 * keyboard and the combobox ARIA all moved into `ui/Combobox`, which the
 * branch picker shares (§4.1 U6) — the two used to carry a copy each.
 *
 * The list is a listbox rather than a menu: `Menu` closes on select and moves
 * focus per item, whereas here focus stays in the query field while the cursor
 * moves under it. Inside `Dialog`, which is where the scrim, the focus trap
 * and Escape come from.
 */
export function CommandPalette(props: CommandPaletteProps) {
  const [query, setQuery] = createSignal("");

  /**
   * Files are built separately from everything else because they come from a
   * different store and only exist once a workspace's tree has been read —
   * but they are the same kind of row, so once built they go through the same
   * ranking, and a scope that admits both lists both.
   */
  const files = createMemo(() =>
    admits(props.scope, FILES)
      ? fileEntries(
          workbenchStore.workspace,
          workbenchStore.tree?.entries.map((entry) => entry.path) ?? null,
        )
      : [],
  );

  const entries = createMemo(() => {
    const base =
      props.scope === "files"
        ? []
        : paletteEntries(forgeStore, runtimeStore.activeSession).filter((entry) =>
            admits(props.scope, entry.group),
          );
    return rank([...base, ...files()], query());
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
    <Dialog title="Command palette" hideTitle flush onDismiss={props.onDismiss}>
      <Combobox
        options={options()}
        query={query()}
        onQuery={setQuery}
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
