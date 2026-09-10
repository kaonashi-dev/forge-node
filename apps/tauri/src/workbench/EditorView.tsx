import {
  For,
  Show,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  untrack,
} from "solid-js";
import { EDITOR } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import { setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import {
  clearEditorReveal,
  editorReveal,
  openDiff,
  openEditorAt,
  revealInTree,
} from "../store/viewsStore";
import { themeBase } from "../theme/ThemeProvider";
import {
  beginWorkbenchRequest,
  failWorkbenchRequest,
  openFile,
  saveFile,
  searchFiles,
} from "./api";
import { candidateLabel, resolveDefinition } from "./definition";
import type { SearchMatch } from "./types";
import { grammarFor } from "./language";
import { actionForDiskRead } from "./editor/conflict";
import { createEditor, type EditorHandle } from "./editor/createEditor";
import { gitMarksFor, patchFor } from "./editor/gitMarks";
import { languageFor } from "./editor/language";
import { CompareView } from "./editor/CompareView";
import { AUTOSAVE_KEY, readFlag } from "../shell/layout";
import { Button, Tooltip } from "../ui";

/**
 * The in-app editor (ADR-012).
 *
 * The GUI never touches a workspace with `std::fs`: the text came from a
 * `ReadFile` and goes back through a `WriteFile` conditioned on the revision
 * that read carried. A stale revision means an agent wrote the same path while
 * this was open — the daemon refuses, and the choice of what to keep is put to
 * the person rather than resolved here.
 *
 * The text itself is CodeMirror's. What used to be here — a transparent
 * `<textarea>` over a `<pre>` shiki had coloured — painted the whole file on
 * every settle, capped colour at 6 000 lines, and kept two layers on the same
 * pixel by duplicating every metric that affects layout. This component now
 * owns the daemon conversation and nothing else; `editor/createEditor.ts` owns
 * what is on screen (`plan-ui-ux.md` §2.1).
 */
/**
 * How long the draft has to stand still before an autosave fires.
 *
 * A second, which is a pause rather than a gap between words. Shorter and a
 * burst of typing becomes a burst of `WriteFile`s; longer and "it saves
 * itself" stops being true in the moment someone closes the window.
 */
const AUTOSAVE_IDLE_MS = 1_000;

export function EditorView(props: { path: string }) {
  let host!: HTMLDivElement;
  let handle: EditorHandle | undefined;
  let lastRevision: string | undefined;

  const [dirty, setDirty] = createSignal(false);
  /**
   * A8: autosave, when it is switched on.
   *
   * Off by default, and that default is the whole design. Agents write these
   * same files: a save that fires on a pause in typing races whatever an agent
   * is doing in the same checkout, and the `PreconditionFailed` it earns turns
   * into a conflict banner that nobody asked for. Someone who is editing alone
   * can turn it on and never think about it again.
   */
  const autosave = () => readFlag(AUTOSAVE_KEY, false);
  let idleTimer: number | undefined;

  function scheduleAutosave(): void {
    clearTimeout(idleTimer);
    if (!autosave()) return;
    idleTimer = window.setTimeout(() => {
      // Not while a conflict is up: the whole point of the banner is that the
      // choice has not been made yet, and saving would make it.
      if (dirty() && !conflict()) save();
    }, AUTOSAVE_IDLE_MS);
  }

  onCleanup(() => clearTimeout(idleTimer));
  const [conflict, setConflict] = createSignal(false);
  const [comparing, setComparing] = createSignal(false);

  const file = () => (workbenchStore.file?.path === props.path ? workbenchStore.file : null);
  const grammar = createMemo(() => {
    const contents = file();
    return contents ? grammarFor(contents.language, contents.path) : null;
  });

  /** The patch for this path, if the Diff tab's answer covers it (A5). */
  const marks = createMemo(() => {
    const patch = patchFor(workbenchStore.diff?.files ?? [], props.path);
    return patch === null ? new Map() : gitMarksFor(patch);
  });

  function requestFile(workspace: string, path: string): void {
    beginWorkbenchRequest("file");
    void openFile(workspace, path).catch((error) => failWorkbenchRequest("file", error));
  }

  function save(): void {
    const contents = file();
    const workspace = workbenchStore.workspace;
    if (!contents || !workspace || !handle) return;
    beginWorkbenchRequest("file");
    // Dirty and conflict wait for the re-read: sending the command is not a
    // save. A `PreconditionFailed` must leave the draft marked unsaved.
    void saveFile(workspace, props.path, handle.text(), contents.revision).catch((error) =>
      failWorkbenchRequest("file", error),
    );
  }

  /** A6 "Take disk": throw the draft away and re-read. */
  function takeDisk(): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    setDirty(false);
    setConflict(false);
    setComparing(false);
    // Forget the snapshot we conflicted with so the re-read is applied even
    // when the bytes on disk have not moved again.
    lastRevision = undefined;
    requestFile(workspace, props.path);
  }

  /**
   * A6 "Keep mine": write the draft over what landed.
   *
   * `save()` is the whole of it, and that is not a shortcut. The conflict was
   * raised *because* a fresh `ReadFile` answer replaced the one in the store,
   * so the revision the write is conditioned on is the newest this side has
   * seen — the precondition ADR-012 asks for, honoured rather than bypassed.
   * If something wrote again in the meantime the daemon refuses a second time
   * and the banner comes back, which is the correct answer and not a bug.
   */
  function keepMine(): void {
    setComparing(false);
    save();
  }

  /* ------------------------------------------------- go to definition --- */

  /**
   * The lookup in flight, if any.
   *
   * The line travels with it because the answer has to be read against where
   * the question was asked: the declaration the caret is already on is not
   * somewhere to go, and only the asker knows which line that was.
   */
  const [lookup, setLookup] = createSignal<{ symbol: string; line: number } | null>(null);
  const [candidates, setCandidates] = createSignal<SearchMatch[]>([]);
  const [candidatesOf, setCandidatesOf] = createSignal<string | null>(null);
  const [moreCandidates, setMoreCandidates] = createSignal(false);
  const [missing, setMissing] = createSignal<string | null>(null);

  function dismissDefinition(): void {
    setCandidates([]);
    setCandidatesOf(null);
    setMoreCandidates(false);
    setMissing(null);
  }

  /**
   * Ask the daemon where `symbol` is declared.
   *
   * A `SearchFiles` and not a language server: there is none in this shell,
   * and adding one is a subsystem rather than a shortcut (`plan-editor.md`
   * §4). The daemon greps the checkout and keeps the declaration-shaped
   * lines, so what comes back is a good guess — which is why several answers
   * are offered rather than one of them being picked here.
   */
  function goToDefinition(symbol: string, line: number): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    dismissDefinition();
    setLookup({ symbol, line });
    void searchFiles(workspace, symbol, "definition").catch((error) => {
      setLookup(null);
      failWorkbenchRequest("file", error);
    });
  }

  function askForDefinitionAtCursor(): void {
    const at = handle?.symbolAtCursor();
    if (at) goToDefinition(at.symbol, at.line);
  }

  onMount(() => {
    handle = createEditor(host, {
      doc: file()?.text ?? "",
      base: themeBase(),
      onChange: () => {
        setDirty(true);
        scheduleAutosave();
      },
      onBlur: () => {
        // Leaving the editor is a stronger signal than a pause: no timer.
        clearTimeout(idleTimer);
        if (autosave() && dirty() && !conflict()) save();
      },
      // A5: a click on a gutter stripe is a request to see the hunk, not to
      // put the caret on the line — the line is already right there.
      onRevealDiff: () => openDiff(),
      onOpenDefinition: goToDefinition,
    });
    const editor = handle;
    onCleanup(() => editor.destroy());
    onCleanup(enterContext(EDITOR));
    onCleanup(registerAction("save_file", save));
    onCleanup(registerAction("go_to_definition", askForDefinitionAtCursor));
  });

  // A new path is a new document. The draft is dropped rather than carried:
  // it belongs to the file that was open.
  let previousPath: string | undefined;
  createEffect(() => {
    const path = props.path;
    if (path === previousPath) return;
    previousPath = path;
    lastRevision = undefined;
    setDirty(false);
    setConflict(false);
    setComparing(false);
    setLookup(null);
    dismissDefinition();
    handle?.setDoc("");
    setWorkbenchStore("fileError", null);
  });

  // Parked editor views are mounted without a content cache. Ask for the
  // active path whenever the shared answer does not match it.
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    const path = props.path;
    if (!workspace || file() || workbenchStore.loading.file || workbenchStore.fileError) return;
    requestFile(workspace, path);
  });

  // A fresh read replaces the document only when nothing is unsaved: landing
  // an agent's version on top of edits is the one thing this must not do
  // silently. `dirty` is untracked — a keystroke is not a disk change, and
  // tracking it raised the conflict banner on every edit.
  createEffect(() => {
    const contents = file();
    const editor = handle;
    if (!contents || !editor) return;
    const revision = contents.revision;
    const disk = contents.text;
    untrack(() => {
      const action = actionForDiskRead({
        seenRevision: lastRevision,
        revision,
        dirty: dirty(),
        disk,
        mine: editor.text(),
      });
      if (action === "ignore") return;
      lastRevision = revision;
      if (action === "apply") {
        editor.setDoc(disk);
        setConflict(false);
        return;
      }
      if (action === "conflict") {
        setConflict(true);
        return;
      }
      setDirty(false);
      setConflict(false);
    });
  });

  /*
   * The answer to a lookup, when one is out.
   *
   * `SearchFiles` acks and reports through an event, and that event carries
   * the query and no request id — so the query is what tells a second `cmd-b`
   * from the first one's answer, and an answer to a different question is
   * simply not this one yet.
   */
  createEffect(() => {
    const asked = lookup();
    if (!asked) return;
    const results = workbenchStore.search;
    // A failed search reports on the file surface; drop the lookup rather than
    // leave "Looking for…" up forever.
    if (workbenchStore.fileError) {
      setLookup(null);
      return;
    }
    if (!results || results.query !== asked.symbol) return;
    setLookup(null);
    const answer = resolveDefinition(results, asked.symbol, {
      path: props.path,
      line: asked.line,
    });
    if (answer.kind === "jump") {
      openEditorAt(answer.target.path, answer.target.line);
      return;
    }
    if (answer.kind === "choose") {
      setCandidates(answer.targets);
      setCandidatesOf(asked.symbol);
      setMoreCandidates(answer.truncated);
      return;
    }
    setMissing(asked.symbol);
  });

  /*
   * The line someone asked to land on — a diff hunk, a review note, a
   * definition in another file.
   *
   * It waits for the read: opening the view is what mounts this component,
   * and the document is empty until `ReadFile` answers. Declared after the
   * effect that applies that answer so the text is in the buffer by the time
   * the caret is moved into it.
   */
  createEffect(() => {
    const reveal = editorReveal();
    const contents = file();
    if (!reveal || reveal.path !== props.path || !contents) return;
    handle?.revealLine(reveal.line);
    clearEditorReveal();
  });

  createEffect(() => {
    const base = themeBase();
    handle?.setBase(base);
  });

  createEffect(() => {
    const support = languageFor(grammar());
    if (!support) {
      handle?.setLanguage(null);
      return;
    }
    const path = props.path;
    void support.then((loaded) => {
      // The chunk may land after the tab moved on; installing a parser for a
      // file that is no longer open would colour the next one wrong.
      if (path === props.path) handle?.setLanguage(loaded);
    });
  });

  createEffect(() => {
    const next = marks();
    handle?.setGitMarks(next);
  });

  const unopenable = () => {
    const contents = file();
    if (!contents) return null;
    if (contents.binary) return "Binary file.";
    if (contents.too_large) return "Too large to open.";
    return null;
  };

  return (
    <div class="editor-view">
      <header class="editor-head">
        <Breadcrumb path={props.path} />
        <Show when={dirty()}>
          <span class="editor-dirty">unsaved</span>
        </Show>
        <Button variant="secondary" size="xs" onClick={save} disabled={!dirty()}>
          Save
        </Button>
      </header>

      {/* A6. A later ReadFile landed while the buffer was dirty. Both versions
          still exist, so all three answers are offered rather than the one
          that throws work away. */}
      <Show when={conflict()}>
        <div class="editor-conflict">
          <span>This file changed on disk while you were editing it.</span>
          <Button variant="secondary" size="xs" onClick={keepMine}>
            Keep mine
          </Button>
          <Button variant="secondary" size="xs" onClick={takeDisk}>
            Take disk
          </Button>
          <Button
            variant="secondary"
            size="xs"
            selected={comparing()}
            onClick={() => setComparing((open) => !open)}
          >
            Compare
          </Button>
        </div>
      </Show>
      <Show when={workbenchStore.fileError}>{(error) => <p class="panel-error">{error()}</p>}</Show>

      {/* The search is a heuristic, so what it found is shown as what it is:
          one answer is jumped to, several are offered, and none says so. */}
      <Show when={lookup()}>
        {(asked) => <p class="editor-note">Looking for {asked().symbol}…</p>}
      </Show>
      <Show when={missing()}>
        {(symbol) => (
          <p class="editor-note">
            No definition found for <code>{symbol()}</code>.
          </p>
        )}
      </Show>
      <Show when={candidatesOf()}>
        {(symbol) => (
          <div class="editor-definitions">
            <header>
              <span>
                {candidates().length}
                {moreCandidates() ? "+" : ""} definitions of <code>{symbol()}</code>
              </span>
              <Button variant="secondary" size="xs" onClick={dismissDefinition}>
                Dismiss
              </Button>
            </header>
            <ul>
              <For each={candidates()}>
                {(match) => (
                  <li>
                    <button
                      type="button"
                      onClick={() => {
                        dismissDefinition();
                        openEditorAt(match.path, match.line);
                      }}
                    >
                      <span class="editor-definition-where">{candidateLabel(match)}</span>
                      <span class="editor-definition-text">{match.text.trim()}</span>
                    </button>
                  </li>
                )}
              </For>
            </ul>
          </div>
        )}
      </Show>

      <Show when={comparing() && file()}>
        {(contents) => (
          <CompareView disk={contents().text} mine={handle?.text() ?? ""} base={themeBase()} />
        )}
      </Show>

      <Show when={unopenable()}>{(reason) => <p class="empty-copy">{reason()}</p>}</Show>
      <Show when={!file() && !workbenchStore.fileError}>
        <p class="empty-copy">{workbenchStore.loading.file ? "Reading…" : "Nothing open."}</p>
      </Show>

      {/* Never unmounted, and never inside the `Show` above: tearing the view
          down and building it again on every read would throw away the undo
          history and the scroll position with it. It is hidden instead. */}
      <div
        ref={host}
        class="editor-code"
        classList={{ hidden: comparing() || unopenable() !== null || !file() }}
      />
    </div>
  );
}

/**
 * A7: the path, as segments that can be clicked.
 *
 * Clicking a segment reveals that directory in the file tree rather than
 * navigating anywhere — the editor has one file in it, and the useful question
 * a path answers is "where is this?".
 */
function Breadcrumb(props: { path: string }) {
  const segments = createMemo(() => {
    const parts = props.path.split("/").filter(Boolean);
    return parts.map((label, index) => ({
      label,
      path: parts.slice(0, index + 1).join("/"),
      last: index === parts.length - 1,
    }));
  });

  return (
    <nav class="editor-breadcrumb" aria-label="File path">
      <For each={segments()}>
        {(segment) => (
          <>
            <Tooltip label={`Reveal ${segment.path} in the file tree`} contents>
              <Button
                size="xs"
                class={`editor-crumb${segment.last ? " current" : ""}`}
                onClick={() => revealInTree(segment.path)}
              >
                {segment.label}
              </Button>
            </Tooltip>
            <Show when={!segment.last}>
              <span class="editor-crumb-sep" aria-hidden="true">
                /
              </span>
            </Show>
          </>
        )}
      </For>
    </nav>
  );
}
