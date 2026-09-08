// The find bar (A4's `Mod-f`), in place of CodeMirror's own search panel.
//
// Framework-free like `createEditor.ts`: CM6 owns this DOM and only hands the
// panel an `update`, so Solid must not be rendering into it. What it adds over
// the stock panel is a match count, toggles that read as toggles rather than
// as browser checkboxes, and a replace row that stays folded until asked for.

import {
  SearchQuery,
  closeSearchPanel,
  findNext,
  findPrevious,
  getSearchQuery,
  replaceAll,
  replaceNext,
  search,
  selectMatches,
  setSearchQuery,
} from "@codemirror/search";
import { EditorView, runScopeHandlers, type Panel } from "@codemirror/view";
import type { EditorState, Extension } from "@codemirror/state";
import { isMac } from "../../actions/keys";
import { matchLabel, tallyMatches, type MatchStatus } from "./searchMatches";

/**
 * Whether the replace row is folded out, remembered across opens.
 *
 * Module state rather than a preference: someone who is replacing does it a
 * few times in a row, and re-opening the bar to a row they just closed is the
 * annoyance this remembers away. It is deliberately not persisted — a new
 * window opens on find, not on find-and-replace.
 */
let replaceShown = false;

/** Modifier glyphs for the button titles, in the platform's own spelling. */
const KEY = isMac()
  ? { mod: "⌘", shift: "⇧", alt: "⌥", enter: "↵" }
  : { mod: "Ctrl+", shift: "Shift+", alt: "Alt+", enter: "Enter" };

const SVG_NS = "http://www.w3.org/2000/svg";

const ICON = {
  search: ["M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16", "m21 21-4.35-4.35"],
  replace: ["m15 10 5 5-5 5", "M4 4v7a4 4 0 0 0 4 4h12"],
  up: ["m5 12 7-7 7 7", "M12 19V5"],
  down: ["M12 5v14", "m19 12-7 7-7-7"],
  all: ["M4 7h16", "M4 12h16", "M4 17h10"],
  chevron: ["m9 18 6-6-6-6"],
  close: ["M18 6 6 18", "m6 6 12 12"],
} as const;

function icon(paths: readonly string[]): SVGSVGElement {
  const svg = document.createElementNS(SVG_NS, "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  svg.setAttribute("fill", "none");
  svg.setAttribute("stroke", "currentColor");
  svg.setAttribute("stroke-width", "2");
  svg.setAttribute("stroke-linecap", "round");
  svg.setAttribute("stroke-linejoin", "round");
  svg.setAttribute("aria-hidden", "true");
  for (const d of paths) {
    const path = document.createElementNS(SVG_NS, "path");
    path.setAttribute("d", d);
    svg.append(path);
  }
  return svg;
}

function div(className: string): HTMLDivElement {
  const node = document.createElement("div");
  node.className = className;
  return node;
}

function button(className: string, title: string, onClick: () => void): HTMLButtonElement {
  const node = document.createElement("button");
  node.className = className;
  node.type = "button";
  node.title = title;
  node.setAttribute("aria-label", title);
  node.addEventListener("click", onClick);
  return node;
}

function iconButton(
  className: string,
  title: string,
  paths: readonly string[],
  onClick: () => void,
): HTMLButtonElement {
  const node = button(className, title, onClick);
  node.append(icon(paths));
  return node;
}

function textField(className: string, placeholder: string): HTMLInputElement {
  const input = document.createElement("input");
  input.className = className;
  input.type = "text";
  input.placeholder = placeholder;
  input.setAttribute("aria-label", placeholder);
  input.spellcheck = false;
  input.autocomplete = "off";
  // The panel is not in a form, and CM's own markup opts out the same way: a
  // stray Enter must reach the keymap below, never submit anything.
  input.setAttribute("form", "");
  return input;
}

/**
 * Whether replacing is on offer at all.
 *
 * `EditorView.editable` and not only `EditorState.readOnly`, because a
 * read-only editor here is built from the first (`createEditor`) — and a
 * `replaceAll` dispatched from a panel is not user input, so nothing else
 * would stop it from writing to a diff's read-only side.
 */
function canReplace(state: EditorState): boolean {
  return !state.readOnly && state.facet(EditorView.editable);
}

function findPanel(view: EditorView): Panel {
  const initial = getSearchQuery(view.state);
  const flags = {
    caseSensitive: initial.caseSensitive,
    regexp: initial.regexp,
    wholeWord: initial.wholeWord,
  };

  const searchInput = textField("forge-find-input", "Find");
  searchInput.value = initial.search;
  searchInput.setAttribute("main-field", "true");
  const replaceInput = textField("forge-find-input", "Replace");
  replaceInput.value = initial.replace;

  function query(): SearchQuery {
    return new SearchQuery({
      search: searchInput.value,
      replace: replaceInput.value,
      caseSensitive: flags.caseSensitive,
      regexp: flags.regexp,
      wholeWord: flags.wholeWord,
    });
  }

  function commit(): void {
    const next = query();
    if (next.eq(getSearchQuery(view.state))) return;
    view.dispatch({ effects: setSearchQuery.of(next) });
  }

  searchInput.addEventListener("input", commit);
  replaceInput.addEventListener("input", commit);

  function flag(
    label: string,
    title: string,
    read: () => boolean,
    write: (on: boolean) => void,
  ): HTMLButtonElement {
    const node = button("forge-find-flag", title, () => {
      write(!read());
      node.setAttribute("aria-pressed", String(read()));
      commit();
      // Focus stays in the field: a toggle is a refinement of the query being
      // typed, not a departure from it.
      searchInput.focus();
    });
    node.textContent = label;
    node.setAttribute("aria-pressed", String(read()));
    return node;
  }

  const flagButtons = [
    flag(
      "Aa",
      "Match case",
      () => flags.caseSensitive,
      (on) => (flags.caseSensitive = on),
    ),
    flag(
      "ab",
      "Whole word",
      () => flags.wholeWord,
      (on) => (flags.wholeWord = on),
    ),
    flag(
      ".*",
      "Regular expression",
      () => flags.regexp,
      (on) => (flags.regexp = on),
    ),
  ];
  flagButtons[1]!.classList.add("forge-find-flag-word");

  const count = document.createElement("span");
  count.className = "forge-find-count";
  count.setAttribute("aria-live", "polite");

  const searchField = div("forge-find-field");
  const flagGroup = div("forge-find-flags");
  flagGroup.append(...flagButtons);
  searchField.append(icon(ICON.search), searchInput, count, flagGroup);

  const previous = iconButton(
    "forge-find-action",
    `Previous match (${KEY.shift}${KEY.enter})`,
    ICON.up,
    () => findPrevious(view),
  );
  const next = iconButton("forge-find-action", `Next match (${KEY.enter})`, ICON.down, () =>
    findNext(view),
  );
  const all = iconButton(
    "forge-find-action",
    `Select all matches (${KEY.alt}${KEY.enter})`,
    ICON.all,
    () => selectMatches(view),
  );

  const searchRow = div("forge-find-row");
  const actions = div("forge-find-actions");
  actions.append(previous, next, all);
  searchRow.append(searchField, actions);

  const replaceOne = button("forge-find-button", `Replace (${KEY.enter})`, () => replaceNext(view));
  replaceOne.textContent = "Replace";
  const replaceEvery = button("forge-find-button", `Replace all (${KEY.mod}${KEY.enter})`, () =>
    replaceAll(view),
  );
  replaceEvery.textContent = "All";

  const replaceField = div("forge-find-field");
  replaceField.append(icon(ICON.replace), replaceInput);
  const replaceRow = div("forge-find-row forge-find-row-replace");
  const replaceActions = div("forge-find-actions");
  replaceActions.append(replaceOne, replaceEvery);
  replaceRow.append(replaceField, replaceActions);

  const disclosure = button("forge-find-disclosure", "Toggle replace", () => {
    setReplaceShown(!replaceShown);
    (replaceShown ? replaceInput : searchInput).focus();
  });
  disclosure.append(icon(ICON.chevron));

  function setReplaceShown(shown: boolean): void {
    replaceShown = shown;
    disclosure.setAttribute("aria-expanded", String(shown));
    replaceRow.hidden = !shown;
  }

  const rows = div("forge-find-rows");
  rows.append(searchRow, replaceRow);
  // Deliberately not CM's own `cm-search` class: that pulls the stock panel's
  // base-theme margins onto these controls, which outrank the rules below.
  const dom = div("forge-find");
  dom.setAttribute("role", "search");
  dom.append(
    disclosure,
    rows,
    iconButton("forge-find-close", "Close (Esc)", ICON.close, () => closeSearchPanel(view)),
  );

  /** The count, and everything that is disabled when there is nothing to step through. */
  function refresh(): void {
    const status = describe();
    count.textContent = matchLabel(status);
    const empty = status === "invalid" || (status !== "empty" && status.total === 0);
    searchField.classList.toggle("forge-find-empty", empty);
    const stepping = status !== "empty" && status !== "invalid" && status.total > 0;
    for (const control of [previous, next, all, replaceOne, replaceEvery]) {
      control.disabled = !stepping;
    }
  }

  function describe(): MatchStatus {
    const current = getSearchQuery(view.state);
    if (!current.search) return "empty";
    if (!current.valid) return "invalid";
    const main = view.state.selection.main;
    return tallyMatches(current.getCursor(view.state), { from: main.from, to: main.to });
  }

  function sync(next: SearchQuery): void {
    searchInput.value = next.search;
    replaceInput.value = next.replace;
    flags.caseSensitive = next.caseSensitive;
    flags.regexp = next.regexp;
    flags.wholeWord = next.wholeWord;
    for (const [index, on] of [next.caseSensitive, next.wholeWord, next.regexp].entries()) {
      flagButtons[index]!.setAttribute("aria-pressed", String(on));
    }
  }

  function syncReplaceable(): void {
    const allowed = canReplace(view.state);
    disclosure.hidden = !allowed;
    replaceRow.hidden = !allowed || !replaceShown;
  }

  dom.addEventListener("keydown", (event) => {
    // The search keymap's own bindings are scoped to this panel — `Escape` to
    // close, `F3`/`Mod-g` to step — so they are given first refusal before
    // Enter is interpreted.
    if (runScopeHandlers(view, event, "search-panel")) {
      event.preventDefault();
      return;
    }
    if (event.key !== "Enter") return;
    event.preventDefault();
    if (event.target === replaceInput) {
      ((isMac() ? event.metaKey : event.ctrlKey) ? replaceAll : replaceNext)(view);
      return;
    }
    if (event.altKey) selectMatches(view);
    else (event.shiftKey ? findPrevious : findNext)(view);
  });

  return {
    dom,
    top: true,
    mount() {
      setReplaceShown(replaceShown);
      syncReplaceable();
      refresh();
      searchInput.select();
    },
    update(update) {
      let queried = false;
      for (const transaction of update.transactions) {
        for (const effect of transaction.effects) {
          if (!effect.is(setSearchQuery)) continue;
          sync(effect.value);
          queried = true;
        }
      }
      // A recount is a scan of the document, so it is tied to the three things
      // that can change the answer rather than run on every view update.
      if (queried || update.docChanged || update.selectionSet) refresh();
      syncReplaceable();
    },
  };
}

/**
 * Structure and chrome for the bar.
 *
 * A `baseTheme` and not the per-base `editorTheme`: none of this varies with
 * the palette, because the colours are the shell's semantic custom properties,
 * which the theme provider has already swapped by the time a rule reads one.
 * Every selector carries two of the panel's own classes so it outranks the
 * generic `.cm-panels input, .cm-panels button` rules in `theme.ts`.
 */
const styles: Extension = EditorView.baseTheme({
  ".forge-find": {
    display: "flex",
    alignItems: "flex-start",
    gap: "var(--space-4)",
    padding: "var(--space-6) var(--space-8)",
    font: "var(--text-sm) / 1.2 var(--font-sans)",
  },
  ".forge-find .forge-find-rows": {
    display: "flex",
    flexDirection: "column",
    gap: "var(--space-4)",
    flex: "1",
    minWidth: "0",
  },
  ".forge-find .forge-find-row": {
    display: "flex",
    alignItems: "center",
    gap: "var(--space-6)",
  },
  // `display: flex` above outranks the user agent's rule for `[hidden]`, which
  // is how a folded replace row stayed on screen.
  ".forge-find .forge-find-row[hidden], .forge-find .forge-find-disclosure[hidden]": {
    display: "none",
  },
  ".forge-find .forge-find-field": {
    display: "flex",
    alignItems: "center",
    gap: "var(--space-4)",
    flex: "1 1 auto",
    // Wide enough for a long identifier, capped so the bar does not turn into
    // one field stretched across a maximised window.
    maxWidth: "420px",
    minWidth: "0",
    height: "var(--control-sm)",
    padding: "0 var(--space-4) 0 var(--space-6)",
    backgroundColor: "var(--bg-base)",
    border: "1px solid var(--border-subtle)",
    borderRadius: "var(--radius-sm)",
    color: "var(--fg-muted)",
  },
  ".forge-find .forge-find-field > svg": {
    width: "13px",
    height: "13px",
    flex: "none",
    color: "var(--fg-subtle)",
  },
  ".forge-find .forge-find-field:focus-within": {
    borderColor: "var(--accent-solid)",
    boxShadow: "0 0 0 2px var(--ring)",
  },
  ".forge-find .forge-find-field.forge-find-empty": { borderColor: "var(--danger-solid)" },
  ".forge-find .forge-find-field.forge-find-empty .forge-find-count": {
    color: "var(--danger-solid)",
  },
  ".forge-find .forge-find-input": {
    flex: "1 1 auto",
    minWidth: "0",
    height: "100%",
    margin: "0",
    padding: "0",
    border: "none",
    outline: "none",
    background: "transparent",
    color: "var(--fg-default)",
    font: "var(--forge-mono-size) / 1.2 var(--font-mono)",
  },
  ".forge-find .forge-find-input::placeholder": { color: "var(--fg-subtle)" },
  ".forge-find .forge-find-count": {
    flex: "none",
    color: "var(--fg-subtle)",
    fontSize: "var(--text-xs)",
    fontVariantNumeric: "tabular-nums",
    whiteSpace: "nowrap",
  },
  ".forge-find .forge-find-flags": {
    display: "flex",
    gap: "var(--space-2)",
    flex: "none",
    marginLeft: "var(--space-4)",
  },
  ".forge-find .forge-find-flag": {
    width: "var(--control-xs)",
    height: "var(--control-xs)",
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    margin: "0",
    padding: "0",
    border: "1px solid transparent",
    borderRadius: "var(--radius-2xs)",
    background: "transparent",
    color: "var(--fg-subtle)",
    font: "var(--text-xs) / 1 var(--font-mono)",
    cursor: "pointer",
  },
  ".forge-find .forge-find-flag-word": { textDecoration: "underline" },
  ".forge-find .forge-find-flag:hover": {
    backgroundColor: "var(--bg-hover)",
    color: "var(--fg-default)",
  },
  ".forge-find .forge-find-flag[aria-pressed=true]": {
    backgroundColor: "var(--accent-soft)",
    borderColor: "var(--accent-solid)",
    color: "var(--fg-default)",
  },
  ".forge-find .forge-find-actions": {
    display: "flex",
    alignItems: "center",
    gap: "var(--space-2)",
    flex: "none",
  },
  ".forge-find .forge-find-action, .forge-find .forge-find-disclosure, .forge-find .forge-find-close":
    {
      width: "var(--control-sm)",
      height: "var(--control-sm)",
      display: "flex",
      alignItems: "center",
      justifyContent: "center",
      margin: "0",
      padding: "0",
      border: "none",
      borderRadius: "var(--radius-xs)",
      background: "transparent",
      color: "var(--fg-muted)",
      cursor: "pointer",
    },
  ".forge-find .forge-find-action > svg, .forge-find .forge-find-disclosure > svg, .forge-find .forge-find-close > svg":
    { width: "14px", height: "14px" },
  ".forge-find .forge-find-action:hover:not(:disabled), .forge-find .forge-find-disclosure:hover, .forge-find .forge-find-close:hover":
    { backgroundColor: "var(--bg-hover)", color: "var(--fg-default)" },
  ".forge-find .forge-find-action:disabled, .forge-find .forge-find-button:disabled": {
    opacity: "0.4",
    cursor: "default",
  },
  ".forge-find .forge-find-disclosure": { marginTop: "var(--space-1)" },
  ".forge-find .forge-find-disclosure[aria-expanded=true] > svg": { transform: "rotate(90deg)" },
  ".forge-find .forge-find-button": {
    height: "var(--control-sm)",
    margin: "0",
    padding: "0 var(--space-8)",
    backgroundColor: "var(--bg-base)",
    border: "1px solid var(--border-subtle)",
    borderRadius: "var(--radius-xs)",
    color: "var(--fg-default)",
    font: "var(--text-sm) / 1 var(--font-sans)",
    cursor: "pointer",
  },
  ".forge-find .forge-find-button:hover:not(:disabled)": {
    backgroundColor: "var(--bg-hover)",
    borderColor: "var(--border-default)",
  },
  ".forge-find .forge-find-action:focus-visible, .forge-find .forge-find-flag:focus-visible, .forge-find .forge-find-button:focus-visible, .forge-find .forge-find-disclosure:focus-visible, .forge-find .forge-find-close:focus-visible":
    { outline: "2px solid var(--accent-solid)", outlineOffset: "1px" },
});

/** The find bar, replacing CM6's stock search panel. */
export function findBar(): Extension {
  return [search({ top: true, createPanel: findPanel }), styles];
}
