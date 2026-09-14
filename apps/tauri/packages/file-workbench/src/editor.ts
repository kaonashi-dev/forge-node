// Framework-free editor and contextual change details. The host owns IO;
// text arrives through setDoc and edits are reported through onChange.

import { symbolAt } from "./symbol.js";
import { highlight, tokensToHtml } from "./highlight.js";
import { createChangeDetails, type GitChange } from "./changeDetails.js";

export type { GitChange } from "./changeDetails.js";

function defaultAccelerator(): "meta" | "ctrl" {
  if (typeof navigator === "undefined") return "ctrl";
  return /mac|iphone|ipad/i.test(navigator.platform || navigator.userAgent) ? "meta" : "ctrl";
}

export type GitMark = "added" | "modified" | "deleted";

/** Line number (1-based, as git counts) to the mark that line carries. */
export type GitMarks = ReadonlyMap<number, GitMark>;

export type FileEditorOptions = {
  doc: string;
  accelerator?: "meta" | "ctrl";
  /** Line and column are 1-based, like the numbers in the gutter. */
  onCursor?: (position: { line: number; column: number; lines: number }) => void;
  /** Fired for edits a person made, never for a document set through `setDoc`. */
  onChange: () => void;
  /** Opens the host's full diff at a 1-based line; marks without details call this directly. */
  onRevealDiff?: (line: number) => void;
  /**
   * The accelerator plus a click on a name, with the 1-based line it sat on.
   *
   * The keyboard half of the gesture is not here: a host binds its own chord
   * and asks [`FileEditorHandle.symbolAtCursor`].
   */
  onOpenDefinition?: (symbol: string, line: number) => void;
  onBlur?: () => void;
  /** Fixed at construction: there is no setter for it. */
  readOnly?: boolean;
  /** Grammar id for the highlight overlay; unknown stays plain. */
  grammar?: string | null;
};

export type FileEditorHandle = {
  text: () => string;
  /**
   * Replace the document without reporting a change.
   *
   * `preservePosition` is for a re-read of the same file. Pass `false` when
   * the text belongs to a different file — that also drops the undo history.
   */
  setDoc: (text: string, preservePosition?: boolean) => void;
  /** Replaces the whole set; an empty map clears the gutter. */
  setGitMarks: (marks: GitMarks) => void;
  /** Details and marks must describe the same saved document. Edits invalidate both. */
  setGitChanges: (changes: readonly GitChange[]) => void;
  /** Switch the highlight grammar; `null` is plain text. */
  setGrammar: (grammar: string | null) => void;
  /** Put the caret on a 1-based line and scroll it into view; past the end is ignored. */
  revealLine: (line: number) => void;
  /** The name under the caret and the 1-based line it is on, or `null`. */
  symbolAtCursor: () => { symbol: string; line: number } | null;
  focus: () => void;
  destroy: () => void;
};

const MATCH_CAP = 5_000;

type Match = { from: number; to: number };

function lineStarts(text: string): number[] {
  const starts = [0];
  for (let i = 0; i < text.length; i += 1) {
    if (text[i] === "\n") starts.push(i + 1);
  }
  return starts;
}

function lineColumnAt(
  text: string,
  offset: number,
): { line: number; column: number; lines: number } {
  const starts = lineStarts(text);
  let line = 1;
  while (line < starts.length && starts[line]! <= offset) line += 1;
  const start = starts[line - 1]!;
  return { line, column: offset - start + 1, lines: starts.length };
}

function offsetAt(text: string, line: number, column: number): number {
  const starts = lineStarts(text);
  const index = Math.min(Math.max(line, 1), starts.length) - 1;
  const start = starts[index]!;
  const end = index + 1 < starts.length ? starts[index + 1]! - 1 : text.length;
  return Math.min(start + Math.max(column - 1, 0), end);
}

function isWholeWord(text: string, from: number, to: number): boolean {
  const before = from === 0 ? "" : text[from - 1]!;
  const after = to >= text.length ? "" : text[to]!;
  const edge = (ch: string) => ch === "" || !/[\w$]/.test(ch);
  return edge(before) && edge(after);
}

function collectMatches(
  text: string,
  query: string,
  caseSensitive: boolean,
  wholeWord: boolean,
  regex: boolean,
): Match[] | "invalid" {
  if (!query) return [];
  const matches: Match[] = [];
  try {
    if (regex) {
      const pattern = new RegExp(query, caseSensitive ? "g" : "gi");
      let hit: RegExpExecArray | null;
      while ((hit = pattern.exec(text)) !== null) {
        if (hit[0].length === 0) {
          pattern.lastIndex += 1;
          continue;
        }
        if (wholeWord && !isWholeWord(text, hit.index, hit.index + hit[0].length)) continue;
        matches.push({ from: hit.index, to: hit.index + hit[0].length });
        if (matches.length >= MATCH_CAP) break;
      }
      return matches;
    }
    const needle = caseSensitive ? query : query.toLowerCase();
    const hay = caseSensitive ? text : text.toLowerCase();
    let from = 0;
    while (from <= hay.length) {
      const at = hay.indexOf(needle, from);
      if (at < 0) break;
      const to = at + needle.length;
      if (!wholeWord || isWholeWord(text, at, to)) {
        matches.push({ from: at, to });
        if (matches.length >= MATCH_CAP) break;
      }
      from = at + Math.max(needle.length, 1);
    }
    return matches;
  } catch {
    return "invalid";
  }
}

function button(className: string, label: string, title: string): HTMLButtonElement {
  const node = document.createElement("button");
  node.type = "button";
  node.className = className;
  node.textContent = label;
  node.title = title;
  node.setAttribute("aria-label", title);
  return node;
}

export function createFileEditor(host: HTMLElement, options: FileEditorOptions): FileEditorHandle {
  let quiet = false;
  let marks: GitMarks = new Map();
  let changes: readonly GitChange[] = [];
  let changeIndex = -1;
  let changeAnchor = 1;
  let details: ReturnType<typeof createChangeDetails> | undefined;
  let findOpen = false;
  let gotoOpen = false;
  let replaceShown = false;
  let matchIndex = -1;
  let cachedMatches: Match[] = [];
  let cursorLine = 1;
  let input: HTMLTextAreaElement;

  const root = document.createElement("div");
  root.className = "fw-editor";

  const find = document.createElement("div");
  find.className = "fw-find";
  find.hidden = true;

  const findRow = document.createElement("div");
  findRow.className = "fw-find-row";
  const findPrompt = document.createElement("span");
  findPrompt.className = "fw-find-prompt";
  findPrompt.textContent = "/";
  const findInput = document.createElement("input");
  findInput.className = "fw-find-query";
  findInput.type = "text";
  findInput.placeholder = "search";
  findInput.setAttribute("aria-label", "Find");
  const count = document.createElement("span");
  count.className = "fw-find-count";
  const caseBtn = button("fw-find-toggle", "Aa", "Match case");
  caseBtn.setAttribute("aria-pressed", "false");
  const wordBtn = button("fw-find-toggle", "W", "Whole word");
  wordBtn.setAttribute("aria-pressed", "false");
  const regexBtn = button("fw-find-toggle", ".*", "Regular expression");
  regexBtn.setAttribute("aria-pressed", "false");
  const prevBtn = button("fw-find-action", "N", "Previous match");
  const nextBtn = button("fw-find-action", "n", "Next match");
  const toggleReplaceBtn = button("fw-find-action", "s", "Toggle replace");
  findRow.append(
    findPrompt,
    findInput,
    count,
    caseBtn,
    wordBtn,
    regexBtn,
    prevBtn,
    nextBtn,
    toggleReplaceBtn,
  );

  const replaceRow = document.createElement("div");
  replaceRow.className = "fw-find-row";
  replaceRow.hidden = true;
  const replacePrompt = document.createElement("span");
  replacePrompt.className = "fw-find-prompt";
  replacePrompt.textContent = "→";
  const replaceInput = document.createElement("input");
  replaceInput.className = "fw-find-replace";
  replaceInput.type = "text";
  replaceInput.placeholder = "replace";
  replaceInput.setAttribute("aria-label", "Replace");
  const replaceBtn = button("fw-find-action", "c", "Replace");
  const replaceAllBtn = button("fw-find-action", "a", "Replace all");
  replaceRow.append(replacePrompt, replaceInput, replaceBtn, replaceAllBtn);

  const gotoRow = document.createElement("div");
  gotoRow.className = "fw-find-row";
  gotoRow.hidden = true;
  const gotoPrompt = document.createElement("span");
  gotoPrompt.className = "fw-find-prompt";
  gotoPrompt.textContent = ":";
  const gotoInput = document.createElement("input");
  gotoInput.className = "fw-find-goto";
  gotoInput.type = "text";
  gotoInput.inputMode = "numeric";
  gotoInput.placeholder = "line";
  gotoInput.setAttribute("aria-label", "Go to line");
  gotoRow.append(gotoPrompt, gotoInput);

  find.append(findRow, replaceRow, gotoRow);

  const body = document.createElement("div");
  body.className = "fw-editor-body";

  const gutter = document.createElement("div");
  gutter.className = "fw-gutter";
  gutter.setAttribute("aria-label", "Line numbers and changes");

  const code = document.createElement("div");
  code.className = "fw-code";

  const highlightLayer = document.createElement("pre");
  highlightLayer.className = "fw-highlight";
  highlightLayer.setAttribute("aria-hidden", "true");

  input = document.createElement("textarea");
  input.className = "fw-input";
  input.spellcheck = false;
  input.wrap = "off";
  input.value = options.doc;
  input.readOnly = options.readOnly ?? false;

  code.append(highlightLayer, input);
  body.append(gutter, code);
  root.append(find, body);
  host.append(root);

  let grammar: string | null = options.grammar ?? null;
  let highlightTimer = 0;

  function closeDetails(focus = false): void {
    if (!details) return;
    details.destroy();
    details = undefined;
    changeIndex = -1;
    document.removeEventListener("pointerdown", dismissDetails, true);
    document.removeEventListener("focusin", dismissDetails);
    if (focus) input.focus({ preventScroll: true });
    paintGutter();
  }

  function dismissDetails(event: Event): void {
    const target = event.target;
    if (!(target instanceof Node) || details?.element.contains(target) || gutter.contains(target))
      return;
    closeDetails();
  }

  function positionDetails(): void {
    if (!details) return;
    const style = getComputedStyle(input);
    const height = Number.parseFloat(style.lineHeight) || 20;
    const lineTop = (changeAnchor - 1) * height - input.scrollTop;
    const available = body.clientHeight;
    const panelHeight = details.element.offsetHeight;
    const below = lineTop + height;
    const top = below + panelHeight <= available ? below : lineTop - panelHeight;
    details.element.style.top = `${Math.max(0, Math.min(top, available - panelHeight))}px`;
  }

  function showChange(index: number, anchor?: number): void {
    const change = changes[index];
    if (!change) return;
    const first = !details;
    changeIndex = index;
    changeAnchor = anchor ?? change.from;
    details ??= createChangeDetails(body, {
      onClose: () => closeDetails(true),
      onStep: (step) => {
        const next = changeIndex + step;
        if (!changes[next]) return;
        revealLine(changes[next].from);
        showChange(next);
      },
      onOpenDiff: options.onRevealDiff
        ? () => {
            const line = changes[changeIndex].from;
            closeDetails();
            options.onRevealDiff?.(line);
          }
        : undefined,
    });
    details.show(change, index, changes.length);
    positionDetails();
    details.element.focus({ preventScroll: true });
    if (first) {
      document.addEventListener("pointerdown", dismissDetails, true);
      document.addEventListener("focusin", dismissDetails);
    }
    paintGutter();
  }

  function invalidateChanges(): void {
    closeDetails();
    changes = [];
    marks = new Map();
  }

  const resize = new ResizeObserver(positionDetails);
  resize.observe(body);

  function pressed(btn: HTMLButtonElement): boolean {
    return btn.getAttribute("aria-pressed") === "true";
  }

  function paintHighlight(): void {
    highlightLayer.innerHTML = tokensToHtml(highlight(input.value, grammar));
  }

  function scheduleHighlight(): void {
    window.clearTimeout(highlightTimer);
    highlightTimer = window.setTimeout(paintHighlight, 32);
  }

  function paintGutter(): void {
    const text = input.value;
    const lines = lineStarts(text).length;
    const width = String(lines).length;
    const parts: string[] = [];
    for (let line = 1; line <= lines; line += 1) {
      const mark = marks.get(line);
      const classes = [
        "fw-gutter-line",
        mark ? `fw-git-${mark}` : "",
        line === cursorLine ? "fw-gutter-active" : "",
      ]
        .filter(Boolean)
        .join(" ");
      const accessible = mark
        ? ` role="button" tabindex="0" aria-label="Show ${mark} change at line ${line}" aria-expanded="${changeIndex >= 0 && line >= changes[changeIndex].from && line <= changes[changeIndex].to}"`
        : ' aria-hidden="true"';
      parts.push(
        `<div class="${classes}" data-line="${line}"${accessible}>${String(line).padStart(width, " ")}</div>`,
      );
    }
    gutter.innerHTML = parts.join("");
    gutter.style.width = `${Math.max(width + 2, 3)}ch`;
  }

  function reportCursor(): void {
    const at = lineColumnAt(input.value, input.selectionStart);
    if (at.line !== cursorLine) {
      cursorLine = at.line;
      paintGutter();
    }
    options.onCursor?.(at);
  }

  function syncScroll(): void {
    gutter.scrollTop = input.scrollTop;
    highlightLayer.scrollTop = input.scrollTop;
    highlightLayer.scrollLeft = input.scrollLeft;
    positionDetails();
  }

  function closeOverlays(): void {
    findOpen = false;
    gotoOpen = false;
    find.hidden = true;
    findRow.hidden = true;
    replaceRow.hidden = true;
    gotoRow.hidden = true;
    input.focus();
  }

  function setFindOpen(open: boolean): void {
    closeDetails();
    if (!open) {
      closeOverlays();
      return;
    }
    findOpen = true;
    gotoOpen = false;
    find.hidden = false;
    findRow.hidden = false;
    // Replace stays folded until `s`; go-to-line is a separate Mod-g surface.
    replaceRow.hidden = !replaceShown;
    gotoRow.hidden = true;
    findInput.focus();
    findInput.select();
    refreshMatches(true);
  }

  function setGotoOpen(open: boolean): void {
    closeDetails();
    if (!open) {
      closeOverlays();
      return;
    }
    gotoOpen = true;
    findOpen = false;
    find.hidden = false;
    findRow.hidden = true;
    replaceRow.hidden = true;
    gotoRow.hidden = false;
    gotoInput.value = String(cursorLine);
    gotoInput.focus();
    gotoInput.select();
  }

  function refreshMatches(selectFirst: boolean): void {
    const result = collectMatches(
      input.value,
      findInput.value,
      pressed(caseBtn),
      pressed(wordBtn),
      pressed(regexBtn),
    );
    if (result === "invalid") {
      cachedMatches = [];
      matchIndex = -1;
      count.textContent = "bad";
      return;
    }
    cachedMatches = result;
    if (cachedMatches.length === 0) {
      matchIndex = -1;
      count.textContent = findInput.value ? "0" : "";
      return;
    }
    if (selectFirst || matchIndex < 0 || matchIndex >= cachedMatches.length) {
      matchIndex = 0;
      selectMatch(cachedMatches[0]!);
    }
    const total =
      cachedMatches.length >= MATCH_CAP ? `${MATCH_CAP}+` : String(cachedMatches.length);
    count.textContent = `${matchIndex + 1}/${total}`;
  }

  function selectMatch(match: Match): void {
    input.setSelectionRange(match.from, match.to);
    revealLine(lineColumnAt(input.value, match.from).line);
    reportCursor();
  }

  function stepMatch(delta: number): void {
    if (cachedMatches.length === 0) {
      refreshMatches(true);
      return;
    }
    matchIndex = (matchIndex + delta + cachedMatches.length) % cachedMatches.length;
    selectMatch(cachedMatches[matchIndex]!);
    const total =
      cachedMatches.length >= MATCH_CAP ? `${MATCH_CAP}+` : String(cachedMatches.length);
    count.textContent = `${matchIndex + 1}/${total}`;
  }

  function applyUserText(text: string, caret: number): void {
    invalidateChanges();
    input.value = text;
    const at = Math.min(Math.max(caret, 0), text.length);
    input.setSelectionRange(at, at);
    paintGutter();
    scheduleHighlight();
    reportCursor();
    options.onChange();
  }

  function replaceOne(): void {
    if (options.readOnly) return;
    if (matchIndex < 0 || matchIndex >= cachedMatches.length) {
      refreshMatches(true);
      return;
    }
    const match = cachedMatches[matchIndex]!;
    applyUserText(
      input.value.slice(0, match.from) + replaceInput.value + input.value.slice(match.to),
      match.from + replaceInput.value.length,
    );
    refreshMatches(true);
  }

  function replaceEvery(): void {
    if (options.readOnly) return;
    refreshMatches(false);
    if (cachedMatches.length === 0) return;
    let text = input.value;
    let shift = 0;
    const replacement = replaceInput.value;
    for (const match of cachedMatches) {
      const from = match.from + shift;
      const to = match.to + shift;
      text = text.slice(0, from) + replacement + text.slice(to);
      shift += replacement.length - (match.to - match.from);
    }
    applyUserText(text, input.selectionStart);
    refreshMatches(false);
  }

  function commitGoto(): void {
    const line = Number.parseInt(gotoInput.value.trim(), 10);
    closeOverlays();
    if (Number.isFinite(line)) revealLine(line);
  }

  function onInput(): void {
    if (quiet) return;
    invalidateChanges();
    paintGutter();
    scheduleHighlight();
    reportCursor();
    options.onChange();
    if (findOpen) refreshMatches(false);
  }

  function onKeyDown(event: KeyboardEvent): void {
    const accelerator = options.accelerator ?? defaultAccelerator();
    const mod = accelerator === "meta" ? event.metaKey : event.ctrlKey;
    if (mod && event.key.toLowerCase() === "f") {
      event.preventDefault();
      setFindOpen(true);
      return;
    }
    if (mod && event.key.toLowerCase() === "g") {
      event.preventDefault();
      setGotoOpen(true);
      return;
    }
    if (event.key === "Escape" && (findOpen || gotoOpen)) {
      event.preventDefault();
      closeOverlays();
      return;
    }
    if (gotoOpen && event.key === "Enter" && event.target === gotoInput) {
      event.preventDefault();
      commitGoto();
      return;
    }
    if (findOpen && event.key === "Enter" && event.target === findInput) {
      event.preventDefault();
      stepMatch(event.shiftKey ? -1 : 1);
      return;
    }
    if (findOpen && event.key === "Enter" && event.target === replaceInput) {
      event.preventDefault();
      replaceOne();
    }
  }

  function wireInput(node: HTMLTextAreaElement): void {
    node.addEventListener("input", onInput);
    node.addEventListener("keydown", onKeyDown);
    node.addEventListener("keyup", reportCursor);
    node.addEventListener("click", reportCursor);
    node.addEventListener("scroll", syncScroll);
    node.addEventListener("blur", () => options.onBlur?.());
    node.addEventListener("mousedown", onMouseDown);
  }

  function onMouseDown(event: MouseEvent): void {
    if (event.button !== 0 || !options.onOpenDefinition) return;
    const accelerator = options.accelerator ?? defaultAccelerator();
    if (!(accelerator === "meta" ? event.metaKey : event.ctrlKey)) return;
    requestAnimationFrame(() => {
      const hit = symbolAtCursor();
      if (hit) options.onOpenDefinition?.(hit.symbol, hit.line);
    });
  }

  function revealLine(line: number): void {
    const text = input.value;
    const starts = lineStarts(text);
    if (line < 1 || line > starts.length) return;
    const from = starts[line - 1]!;
    input.setSelectionRange(from, from);
    const style = getComputedStyle(input);
    const lineHeight = Number.parseFloat(style.lineHeight) || 20;
    input.scrollTop = Math.max(0, (line - 1) * lineHeight - input.clientHeight / 2);
    syncScroll();
    cursorLine = line;
    paintGutter();
    input.focus({ preventScroll: true });
  }

  function symbolAtCursor(): { symbol: string; line: number } | null {
    const text = input.value;
    const at = input.selectionStart;
    const pos = lineColumnAt(text, at);
    const starts = lineStarts(text);
    const start = starts[pos.line - 1]!;
    const end = pos.line < starts.length ? starts[pos.line]! - 1 : text.length;
    const symbol = symbolAt(text.slice(start, end), at - start);
    return symbol ? { symbol, line: pos.line } : null;
  }

  caseBtn.addEventListener("click", () => {
    caseBtn.setAttribute("aria-pressed", pressed(caseBtn) ? "false" : "true");
    refreshMatches(true);
  });
  wordBtn.addEventListener("click", () => {
    wordBtn.setAttribute("aria-pressed", pressed(wordBtn) ? "false" : "true");
    refreshMatches(true);
  });
  regexBtn.addEventListener("click", () => {
    regexBtn.setAttribute("aria-pressed", pressed(regexBtn) ? "false" : "true");
    refreshMatches(true);
  });
  prevBtn.addEventListener("click", () => stepMatch(-1));
  nextBtn.addEventListener("click", () => stepMatch(1));
  replaceBtn.addEventListener("click", replaceOne);
  replaceAllBtn.addEventListener("click", replaceEvery);
  toggleReplaceBtn.addEventListener("click", () => {
    replaceShown = !replaceShown;
    replaceRow.hidden = !replaceShown;
    if (replaceShown) replaceInput.focus();
  });
  findInput.addEventListener("input", () => refreshMatches(true));
  findInput.addEventListener("keydown", onKeyDown);
  replaceInput.addEventListener("keydown", onKeyDown);
  gotoInput.addEventListener("keydown", onKeyDown);
  function activateGutter(event: Event): void {
    const target = (event.target as HTMLElement | null)?.closest(".fw-gutter-line");
    if (!target) return;
    const line = Number(target.getAttribute("data-line"));
    if (!Number.isFinite(line)) return;
    const index = changes.findIndex((change) => line >= change.from && line <= change.to);
    if (index >= 0) {
      if (index === changeIndex) closeDetails(true);
      else showChange(index, line);
      return;
    }
    if (options.onRevealDiff && marks.has(line)) {
      options.onRevealDiff(line);
      return;
    }
    revealLine(line);
  }
  gutter.addEventListener("click", activateGutter);
  gutter.addEventListener("keydown", (event) => {
    if (event.key !== "Enter" && event.key !== " ") return;
    event.preventDefault();
    activateGutter(event);
  });
  gutter.addEventListener("pointerdown", (event) => {
    if (event.button === 0 && (event.target as HTMLElement).closest('[role="button"]'))
      event.preventDefault();
  });

  // Find / replace / go-to-line stay closed until Mod-f / Mod-g.
  find.hidden = true;
  findRow.hidden = true;
  replaceRow.hidden = true;
  gotoRow.hidden = true;

  wireInput(input);
  paintGutter();
  paintHighlight();
  reportCursor();

  return {
    text: () => input.value,
    setDoc: (text, preservePosition = true) => {
      if (preservePosition && text === input.value) return;
      invalidateChanges();
      const oldLine = lineColumnAt(input.value, input.selectionStart);
      const oldAnchor = lineColumnAt(input.value, input.selectionEnd);
      const scroll = input.scrollTop;
      quiet = true;
      try {
        if (!preservePosition) {
          const next = document.createElement("textarea");
          next.className = "fw-input";
          next.spellcheck = false;
          next.wrap = "off";
          next.value = text;
          next.readOnly = options.readOnly ?? false;
          input.replaceWith(next);
          input = next;
          wireInput(input);
        } else {
          input.value = text;
          const head = offsetAt(text, oldLine.line, oldLine.column);
          const anchor = offsetAt(text, oldAnchor.line, oldAnchor.column);
          input.setSelectionRange(Math.min(anchor, head), Math.max(anchor, head));
          input.scrollTop = scroll;
        }
      } finally {
        quiet = false;
      }
      paintGutter();
      paintHighlight();
      syncScroll();
      reportCursor();
    },
    setGitMarks: (next) => {
      closeDetails();
      changes = [];
      marks = next;
      paintGutter();
    },
    setGitChanges: (next) => {
      closeDetails();
      changes = next;
      const nextMarks = new Map<number, GitMark>();
      for (const change of changes) {
        for (let line = change.from; line <= change.to; line += 1) {
          if (change.kind !== "deleted" || !nextMarks.has(line)) nextMarks.set(line, change.kind);
        }
      }
      marks = nextMarks;
      paintGutter();
    },
    setGrammar: (next) => {
      grammar = next;
      paintHighlight();
    },
    revealLine,
    symbolAtCursor,
    focus: () => input.focus(),
    destroy: () => {
      window.clearTimeout(highlightTimer);
      closeDetails();
      resize.disconnect();
      root.remove();
    },
  };
}
