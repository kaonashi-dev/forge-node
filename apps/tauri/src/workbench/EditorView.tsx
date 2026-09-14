import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  onCleanup,
  onMount,
  untrack,
} from "solid-js";
import { EDITOR } from "../actions/actions";
import { enterContext, registerAction } from "../actions/dispatch";
import { CLIPBOARD_MOD, MOD, describeChord, parseChord } from "../actions/keys";
import { setWorkbenchStore, workbenchStore } from "../store/workbenchStore";
import {
  clearEditorReveal,
  editorReveal,
  openDiff,
  openEditorAt,
  requestFindInFiles,
  revealInTree,
} from "../store/viewsStore";
import { showView } from "../store/sidebarStore";
import { runtimeStore } from "../store/runtimeStore";
import { themeBase } from "../theme/ThemeProvider";
import {
  beginWorkbenchRequest,
  failWorkbenchRequest,
  openFile,
  saveFile,
  searchFiles,
} from "./api";
import { fileAffected } from "./fileInvalidation";
import { watchFiles, fileWatchError } from "./fileWatch";
import { candidateLabel, stepLookup, type Lookup } from "./definition";
import type { SearchMatch } from "./types";
import { grammarFor } from "./language";
import { actionForDiskRead } from "./editor/conflict";
import { createEditor, type EditorHandle, type EditorMetrics } from "./editor/createEditor";
import { gitChangesFor, marksForChanges, patchFor } from "./editor/gitMarks";
import { lineAt, rulerTicks } from "./editor/overviewRuler";
import { ensureDiff, refreshDiff } from "./decorations";
import { CompareView } from "./editor/CompareView";
import {
  AUTOSAVE_KEY,
  EDITOR_FONT_SIZE_KEY,
  EDITOR_FONT_SIZE_RANGE,
  EDITOR_LINE_HEIGHT_KEY,
  EDITOR_LINE_HEIGHT_RANGE,
  readFlag,
  readScale,
} from "../shell/layout";
import { Button, ContextMenu, Tooltip, type MenuItem } from "../ui";
import { Markdown, type MdImageLoader } from "./Markdown";
import { imageMime, imageSource, svgDataUrl } from "./previewImages";
import { previewImageReader } from "./api";

/**
 * The in-app editor (ADR-012).
 *
 * The GUI never touches a workspace with `std::fs`: the text came from a
 * `ReadFile` and goes back through a `WriteFile` conditioned on the revision
 * that read carried. A stale revision means an agent wrote the same path while
 * this was open — the daemon refuses, and the choice of what to keep is put to
 * the person rather than resolved here.
 *
 * The text itself is the portable file-workbench editor. This component owns
 * the daemon conversation and nothing else; `editor/createEditor.ts` owns what
 * is on screen.
 */
/**
 * How long the draft has to stand still before an autosave fires.
 *
 * A second, which is a pause rather than a gap between words. Shorter and a
 * burst of typing becomes a burst of `WriteFile`s; longer and "it saves
 * itself" stops being true in the moment someone closes the window.
 */
const AUTOSAVE_IDLE_MS = 1_000;

/** How a file with a rendered form is shown: its source, or what it renders to. */
type SourceMode = "code" | "preview";

/**
 * A chord spec as the platform spells it, for a menu's right-hand column.
 *
 * Through `parseChord`/`describeChord` rather than a literal, so the menu and
 * the keymap cannot drift, and so Linux gets `Ctrl+Shift+C` for a copy that is
 * `⌘C` on a Mac instead of a Mac label on every platform.
 */
function chordLabel(spec: string): string {
  return describeChord(parseChord(spec));
}

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

  /**
   * The reader's type metrics, read reactively.
   *
   * A plain accessor rather than a signal seeded once: `forgeStore.app_state`
   * arrives after the window paints, and a value snapshotted at creation would
   * keep the fallback forever (`seedFromAppState` documents the same trap).
   */
  const metrics = (): EditorMetrics => ({
    fontSize: readScale(
      EDITOR_FONT_SIZE_KEY,
      EDITOR_FONT_SIZE_RANGE.min,
      EDITOR_FONT_SIZE_RANGE.max,
      EDITOR_FONT_SIZE_RANGE.fallback,
    ),
    lineHeight: readScale(
      EDITOR_LINE_HEIGHT_KEY,
      EDITOR_LINE_HEIGHT_RANGE.min,
      EDITOR_LINE_HEIGHT_RANGE.max,
      EDITOR_LINE_HEIGHT_RANGE.fallback,
    ),
  });

  /** Where a right-click landed, and so where the menu hangs. `null` is closed. */
  const [menuAt, setMenuAt] = createSignal<{ x: number; y: number } | null>(null);

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
  /** A Markdown file opens as the document it renders to (see `previewing`). */
  const [sourceMode, setSourceMode] = createSignal<SourceMode>("preview");
  /**
   * Bumped whenever the buffer's text changes, so the preview can re-read it.
   * The editor's text is not a signal, and a keystroke with no preview on
   * screen has nothing subscribed to this.
   */
  const [docVersion, setDocVersion] = createSignal(0);
  const [invalidated, setInvalidated] = createSignal(false);
  const [position, setPosition] = createSignal({ line: 1, column: 1, lines: 1 });
  const touchDoc = (): void => {
    setDocVersion((version) => version + 1);
  };
  /**
   * The preview's in-progress block. Cmd+S is captured before the textarea
   * blurs, so a save has to pick this up itself rather than waiting for a
   * commit.
   */
  let previewDraft: { start: number; end: number; text: string } | null = null;

  function applyPreviewDraft(): void {
    const draft = previewDraft;
    const editor = handle;
    if (!draft || !editor) return;
    previewDraft = null;
    const current = editor.text();
    const next = current.slice(0, draft.start) + draft.text + current.slice(draft.end);
    if (next === current) return;
    editor.setDoc(next);
    setDirty(true);
    touchDoc();
    scheduleAutosave();
  }

  function onPreviewDraft(draft: { start: number; end: number; text: string } | null): void {
    previewDraft = draft;
    if (!draft || !handle) return;
    if (draft.text !== handle.text().slice(draft.start, draft.end)) {
      setDirty(true);
      scheduleAutosave();
    }
  }

  function onPreviewEdit(start: number, end: number, next: string): void {
    previewDraft = null;
    const editor = handle;
    if (!editor) return;
    const current = editor.text();
    const spliced = current.slice(0, start) + next + current.slice(end);
    if (spliced === current) return;
    editor.setDoc(spliced);
    setDirty(true);
    touchDoc();
    scheduleAutosave();
  }

  const file = () => (workbenchStore.file?.path === props.path ? workbenchStore.file : null);
  const grammar = createMemo(() => {
    const contents = file();
    return contents ? grammarFor(contents.language, contents.path) : null;
  });

  /*
   * A5: the stripes and the ruler need a diff, and opening a file is not
   * opening the Git tab. Guarded and idempotent, so a file opened from the
   * tree — which asks for the same thing — makes one read between them.
   */
  createEffect(() => {
    if (workbenchStore.workspace) ensureDiff();
  });

  const changes = createMemo(() => {
    const contents = file();
    if (!contents || dirty() || conflict()) return [];
    // A re-read invalidates the details even when the saved line count is unchanged.
    contents.revision;
    const patch = patchFor(workbenchStore.diff?.files ?? [], props.path);
    return patch === null ? [] : gitChangesFor(patch, contents.text.split("\n").length);
  });
  const marks = createMemo(() => marksForChanges(changes()));

  /*
   * The ruler's geometry, from the file the daemon read rather than from the
   * live buffer: the marks are the patch's, so measuring against a dirty
   * document would put a tick at a line the diff never named.
   */
  const totalLines = createMemo(() => {
    const contents = file();
    return contents ? contents.text.split("\n").length : 0;
  });
  const ticks = createMemo(() => rulerTicks(marks(), totalLines()));

  function requestFile(workspace: string, path: string): void {
    beginWorkbenchRequest("file");
    void openFile(workspace, path).catch((error) => failWorkbenchRequest("file", error));
  }

  function save(): void {
    applyPreviewDraft();
    const contents = file();
    const workspace = workbenchStore.workspace;
    if (!contents || !workspace || !handle) return;
    /*
     * Never write the buffer back over a file the buffer never came from.
     *
     * A binary or over-budget read hands back no text, so the editor holds an
     * empty document — and `⌘S` reaches `save` directly, past the disabled
     * button, whether or not anything is on screen to type into. Saving there
     * truncates the file: the PNG this is now happy to draw, or the large one
     * somebody opened to look at and left.
     */
    if (contents.binary || contents.too_large) return;
    beginWorkbenchRequest("file");
    // Dirty and conflict wait for the re-read: sending the command is not a
    // save. A `PreconditionFailed` must leave the draft marked unsaved.
    void saveFile(workspace, props.path, handle.text(), contents.revision)
      // A write moves the decorations: the gutter, the ruler and the tree all
      // read the same diff, and nothing else re-reads it after a save.
      .then(() => refreshDiff())
      .catch((error: unknown) => failWorkbenchRequest("file", error));
  }

  /** A6 "Take disk": throw the draft away and re-read. */
  function takeDisk(): void {
    const workspace = workbenchStore.workspace;
    if (!workspace) return;
    previewDraft = null;
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
  const [lookup, setLookup] = createSignal<Lookup | null>(null);
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
    // One grep at a time. The workbench worker is a single thread on one
    // queue, so a held-down `cmd-b` would put a search per repeat — each with
    // its own 30 s timeout — in front of the tree, the open file and the diff.
    if (lookup()) return;
    dismissDefinition();
    // The answer is correlated by its query and nothing else, so last time's
    // answer to the same symbol is indistinguishable from this one's and would
    // resolve it synchronously against a line the edit has moved.
    setWorkbenchStore({ search: null, searchError: null });
    setLookup({ symbol, line });
    void searchFiles(workspace, symbol, "definition").catch((error: unknown) => {
      setLookup(null);
      setWorkbenchStore("searchError", error instanceof Error ? error.message : String(error));
    });
  }

  function askForDefinitionAtCursor(): void {
    const at = handle?.symbolAtCursor();
    if (at) goToDefinition(at.symbol, at.line);
  }

  /**
   * A references search is the content grep, with the symbol filled in.
   *
   * Not a semantic one: there is no language server behind any of this, so
   * this finds the word and the person reads the hits. That is the same
   * bargain "Go to Definition" already makes (`fs-service::definition_rank`
   * ranks grep hits), and calling it references rather than "find this word"
   * is the honest name for what a reader wants from it.
   */
  function findReferencesAtCursor(): void {
    const at = handle?.symbolAtCursor();
    if (!at) return;
    requestFindInFiles(at.symbol);
    showView("Files");
  }

  /**
   * Open the menu on what was right-clicked.
   *
   * The caret is moved first: every item below reads the caret or the
   * selection, and a menu that acted on wherever the caret last sat would be
   * a menu about the wrong word. A click inside a selection leaves it, so
   * Copy still has its range.
   */
  function openMenu(event: MouseEvent): void {
    event.preventDefault();
    handle?.caretAtPoint(event.clientX, event.clientY);
    setMenuAt({ x: event.clientX, y: event.clientY });
  }

  /**
   * What the right-click menu offers, given where the caret ended up.
   *
   * Everything here is something this editor can actually do. The rest of what
   * a language-server editor puts in this menu — declaration, type definition,
   * implementation, call hierarchy, rename, code actions — is absent rather
   * than disabled, because there is no language server to turn on later and no
   * honest way to grey out an item that is not coming.
   */
  function menuItems(): MenuItem[] {
    const symbol = handle?.symbolAtCursor()?.symbol ?? null;
    const selected = (handle?.selectedText() ?? "").length > 0;
    const editable = !previewing();
    return [
      {
        kind: "item",
        label: "Go to Definition",
        detail: chordLabel(`${MOD}-b`),
        disabled: !symbol,
        run: askForDefinitionAtCursor,
      },
      {
        kind: "item",
        label: "Find All References",
        disabled: !symbol,
        run: findReferencesAtCursor,
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "Cut",
        detail: chordLabel(`${CLIPBOARD_MOD}-x`),
        disabled: !selected || !editable,
        run: () => void cutSelection(),
      },
      {
        kind: "item",
        label: "Copy",
        detail: chordLabel(`${CLIPBOARD_MOD}-c`),
        disabled: !selected,
        run: () => void copySelection(),
      },
      {
        kind: "item",
        label: "Paste",
        detail: chordLabel(`${CLIPBOARD_MOD}-v`),
        disabled: !editable,
        run: () => void pasteIntoSelection(),
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "Reveal in Files",
        run: () => {
          revealInTree(props.path);
          showView("Files");
        },
      },
    ];
  }

  async function copySelection(): Promise<void> {
    const text = handle?.selectedText();
    if (text) await navigator.clipboard.writeText(text).catch(() => undefined);
  }

  async function cutSelection(): Promise<void> {
    const text = handle?.selectedText();
    if (!text) return;
    await navigator.clipboard.writeText(text).catch(() => undefined);
    handle?.replaceSelection("");
  }

  /* Reads the clipboard before touching the document: a denied or empty read
     must leave the selection standing rather than delete it for nothing. */
  async function pasteIntoSelection(): Promise<void> {
    const text = await navigator.clipboard.readText().catch(() => "");
    if (text) handle?.replaceSelection(text);
  }

  onMount(() => {
    handle = createEditor(host, {
      doc: file()?.text ?? "",
      base: themeBase(),
      metrics: metrics(),
      onCursor: setPosition,
      onChange: () => {
        setDirty(true);
        touchDoc();
        scheduleAutosave();
      },
      onBlur: () => {
        // Leaving the editor is a stronger signal than a pause: no timer.
        clearTimeout(idleTimer);
        if (autosave() && dirty() && !conflict()) save();
      },
      onRevealDiff: () => openDiff(),
      onOpenDefinition: goToDefinition,
    });
    const editor = handle;
    onCleanup(() => {
      applyPreviewDraft();
      editor.destroy();
    });
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
    previewDraft = null;
    setDirty(false);
    setConflict(false);
    setComparing(false);
    setLookup(null);
    dismissDefinition();
    setSourceMode("preview");
    handle?.setDoc("", false);
    touchDoc();
    setWorkbenchStore({ fileError: null, searchError: null });
  });

  // Parked editor views are mounted without a content cache. Ask for the
  // active path whenever the shared answer does not match it.
  createEffect(() => {
    const workspace = workbenchStore.workspace;
    const path = props.path;
    if (!workspace || file() || workbenchStore.loading.file || workbenchStore.fileError) return;
    requestFile(workspace, path);
  });

  createEffect(() => {
    const workspace = workbenchStore.workspace;
    const path = props.path;
    if (!workspace) return;
    const parent = path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
    onCleanup(
      watchFiles(workspace, [parent], (changed) => {
        if (fileAffected(path, changed)) setInvalidated(true);
      }),
    );
  });
  createEffect(() => {
    if (!invalidated() || workbenchStore.loading.file) return;
    const workspace = workbenchStore.workspace;
    const path = props.path;
    if (!workspace) return;
    setInvalidated(false);
    untrack(() => requestFile(workspace, path));
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
        touchDoc();
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
    const step = stepLookup(asked, workbenchStore.search, workbenchStore.searchError, props.path);
    if (step.kind === "wait" || !asked) return;
    setLookup(null);
    // The error is already on screen; dropping the lookup is what stops
    // "Looking for…" standing there for an answer that is not coming.
    if (step.kind === "failed") return;
    const answer = step.answer;
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
   * A drop is the one way a lookup ends without an answer, and the in-flight
   * guard in `goToDefinition` would otherwise leave `cmd-b` dead for the life
   * of the tab. Nothing is said about it: the reconnect banner already is.
   */
  createEffect(() => {
    if (runtimeStore.connection.kind !== "connected") setLookup(null);
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
    // A line is a place in the source; the rendered document has no lines.
    setSourceMode("code");
    handle?.revealLine(reveal.line);
    clearEditorReveal();
  });

  createEffect(() => {
    const base = themeBase();
    handle?.setBase(base);
  });

  /* Tracked rather than seeded: `app_state` lands after the window paints, so
     the first read here is the fallback and this is what corrects it. */
  createEffect(() => {
    handle?.setMetrics(metrics());
  });

  createEffect(() => {
    handle?.setGrammar(grammar());
  });

  createEffect(() => {
    const next = changes();
    handle?.setGitChanges(next);
  });

  const unopenable = () => {
    const contents = file();
    if (!contents) return null;
    // A raster image is binary, and that is not a problem here: `ReadFile`
    // is right to refuse it and the picture comes from `ReadImage` instead.
    // Saying "Binary file." over a PNG we can draw is the bug this skips.
    if (raster()) return null;
    if (contents.binary) return "Binary file.";
    if (contents.too_large) return "Too large to open.";
    return null;
  };

  const markdown = () => grammar() === "markdown";
  /** An SVG is both: text to edit, and a picture to look at. */
  const svg = () => imageMime(props.path) === "image/svg+xml";
  /**
   * An image with no source worth showing — PNG, JPEG, GIF, WebP and the rest.
   *
   * Keyed off the path rather than off `contents.binary`, because the question
   * is not "can this be parsed as text" but "will the daemon hand over the
   * bytes": `read_image` gates on the extension, and this list is the mirror
   * of that one.
   */
  const raster = () => {
    const mime = imageMime(props.path);
    return mime !== null && mime !== "image/svg+xml";
  };
  /** The kinds that have a rendered form *and* a source, so a toggle applies. */
  const renderable = () => markdown() || svg();
  const previewing = () =>
    renderable() && sourceMode() === "preview" && !comparing() && unopenable() === null;
  /** Whether a picture is what is on screen right now. */
  const showingImage = () => raster() || (svg() && previewing());

  /*
   * A raster image's bytes, read once per path.
   *
   * `ReadImage` and not the text read: it is a separate command, gated in the
   * daemon on the extension, which is what stops a preview from naming `.env`
   * and being handed raw bytes for it.
   */
  const [rasterImage] = createResource(
    () => (raster() ? { workspace: workbenchStore.workspace, path: props.path } : null),
    async (key: { workspace: string | null; path: string }) =>
      key.workspace === null ? null : await previewImageReader.read(key.workspace, key.path),
  );

  /*
   * An SVG's picture, drawn from the buffer rather than from disk.
   *
   * The bytes are already here, so a `ReadImage` would be a round trip that
   * also showed the version before the edit just made. Tracks `docVersion`
   * only while the preview is up, so typing in the source pays for nothing.
   */
  const svgImage = createMemo(() => {
    if (!(svg() && previewing())) return null;
    docVersion();
    return svgDataUrl(handle?.text() ?? "");
  });

  /** What went wrong reading the picture, in the daemon's own words. */
  const imageError = () => {
    const failure: unknown = rasterImage.error;
    if (!failure) return null;
    return failure instanceof Error ? failure.message : String(failure);
  };

  createEffect(() => {
    if (!previewing()) applyPreviewDraft();
  });

  /*
   * The preview draws the buffer, not the last read: what is being looked at
   * is the draft, unsaved edits included. It tracks `docVersion` only while it
   * is on screen, so typing in the source pays for no parse.
   */
  const previewText = createMemo(() => {
    if (!previewing()) return "";
    docVersion();
    return handle?.text() ?? "";
  });

  /*
   * One image loader per rendered text. A checkout image is read once for the
   * document it appears in, and read again when the document changes or the
   * preview is opened again — there is no watcher to say it moved on disk.
   */
  const previewImages = createMemo((): MdImageLoader => {
    previewText();
    const reads = new Map<string, Promise<string>>();
    return (src) => {
      const workspace = workbenchStore.workspace;
      const source = imageSource(src, props.path);
      if (!workspace || source === null) return null;
      if (source.kind === "url") return Promise.resolve(source.url);
      let read = reads.get(source.path);
      if (!read) {
        read = previewImageReader.read(workspace, source.path);
        reads.set(source.path, read);
      }
      return read;
    };
  });

  return (
    <div class="editor-view fw-document">
      <header class="fw-document-head">
        <Breadcrumb path={props.path} />
        <Show when={dirty()}>
          <span class="editor-dirty">unsaved</span>
        </Show>
        <Show when={renderable() && file() && unopenable() === null}>
          <div class="editor-mode" role="group" aria-label="Show as">
            <Button
              variant="ghost"
              size="xs"
              selected={sourceMode() === "code"}
              onClick={() => {
                applyPreviewDraft();
                setSourceMode("code");
              }}
            >
              Code
            </Button>
            <Button
              variant="ghost"
              size="xs"
              selected={sourceMode() === "preview"}
              onClick={() => setSourceMode("preview")}
            >
              Preview
            </Button>
          </div>
        </Show>
        <Button
          variant="ghost"
          size="xs"
          onClick={() => {
            const workspace = workbenchStore.workspace;
            if (workspace) requestFile(workspace, props.path);
          }}
          disabled={workbenchStore.loading.file}
        >
          Reload
        </Button>
        <Button
          variant="secondary"
          size="xs"
          onClick={save}
          disabled={!dirty() || conflict() || workbenchStore.loading.file}
        >
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
      <Show when={workbenchStore.searchError}>
        {(error) => <p class="panel-error">Could not search for a definition: {error()}</p>}
      </Show>

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
        {(contents) => <CompareView disk={contents().text} mine={handle?.text() ?? ""} />}
      </Show>

      <Show when={unopenable()}>{(reason) => <p class="empty-copy">{reason()}</p>}</Show>

      {/* Drawn through `<img>`, never inlined into the document. An SVG in an
          `<img>` cannot run script or fetch anything external; the same markup
          inlined very much can, and these files come out of a checkout that
          agents write to. */}
      <Show when={showingImage()}>
        <div class="editor-image">
          <Show
            when={svg() ? svgImage() : rasterImage()}
            fallback={
              <p class="empty-copy">
                {imageError() ?? (rasterImage.loading ? "Reading the image\u2026" : "")}
              </p>
            }
          >
            {(url) => <img class="editor-image-canvas" src={url()} alt={props.path} />}
          </Show>
        </div>
      </Show>
      <Show when={!file() && !workbenchStore.fileError}>
        <p class="empty-copy">{workbenchStore.loading.file ? "Reading…" : "Nothing open."}</p>
      </Show>

      <Show when={markdown() && previewing() && file()}>
        <div class="editor-preview">
          <Markdown
            text={previewText()}
            class="forge-md-doc"
            images={previewImages()}
            onEdit={onPreviewEdit}
            onDraft={onPreviewDraft}
          />
        </div>
      </Show>

      {/* Never unmounted, and never inside the `Show` above: tearing the view
          down and building it again on every read would throw away the undo
          history and the scroll position with it. It is hidden instead. */}
      <div
        class="editor-code-row"
        classList={{
          hidden: comparing() || previewing() || showingImage() || unopenable() !== null || !file(),
        }}
      >
        <div ref={host} class="editor-code" onContextMenu={openMenu} />
        <Show when={menuAt()}>
          {(at) => (
            <ContextMenu
              x={at().x}
              y={at().y}
              items={menuItems()}
              onDismiss={() => setMenuAt(null)}
            />
          )}
        </Show>
        {/* A5: the gutter answers "did this line change" for the lines on
            screen; the ruler answers "where else" for the ones that are not. */}
        <Show when={ticks().length > 0}>
          <div
            class="editor-ruler"
            aria-hidden="true"
            onClick={(event) => {
              const box = event.currentTarget.getBoundingClientRect();
              const line = lineAt(ticks(), (event.clientY - box.top) / box.height, totalLines());
              if (line !== null) handle?.revealLine(line);
            }}
          >
            <For each={ticks()}>
              {(tick) => (
                <span
                  class={`editor-ruler-tick ${tick.mark}`}
                  style={{ top: `${tick.top}%`, height: `${tick.height}%` }}
                />
              )}
            </For>
          </div>
        </Show>
      </div>
      <footer class="fw-document-foot">
        <span data-dirty={dirty()}>
          {conflict()
            ? "Changed on disk"
            : dirty()
              ? "Unsaved"
              : workbenchStore.loading.file
                ? "Reading…"
                : "Saved"}
        </span>
        <span>{grammar() ?? "text"}</span>
        <Show when={fileWatchError()}>
          {(error) => <span title={error()}>Manual refresh</span>}
        </Show>
        <Show
          when={previewing()}
          fallback={
            <span class="fw-position">
              {position().line}:{position().column}
            </span>
          }
        >
          <span>Click a block to edit</span>
        </Show>
      </footer>
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
                onClick={() => {
                  // The breadcrumb is a request to see this file in the tree,
                  // so it switches the sidebar; `revealInTree` alone is a
                  // passive sync (a definition jump) and must not move the view.
                  revealInTree(segment.path);
                  showView("Files");
                }}
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
