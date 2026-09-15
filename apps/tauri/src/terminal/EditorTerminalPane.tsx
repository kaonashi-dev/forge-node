import { Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { editorChrome } from "./editorChrome";
import { resizeEditor, sendEditorKey, sendEditorPaste, sendEditorText } from "../runtime/api";
import { loadEditorConflict, overwriteEditorBuffer, reloadEditorBuffer } from "../workbench/api";
import {
  clearEditorConflict,
  editorConflictFor,
  startEditorConflictLoad,
} from "../store/editorConflictStore";
import { CompareView } from "../workbench/CompareView";
import { Button, ContextMenu, type MenuItem } from "../ui";
import { revealInTree } from "../store/viewsStore";
import { showView } from "../store/sidebarStore";
import { openDiff } from "../store/viewsStore";
import { editorCellsChannel } from "../runtime/bus";
import { forgeStore } from "../store/forgeStore";
import { metrics as tokens } from "../theme/tokens";
import { measureCell, gridSize, type CellMetrics } from "./metrics";
import { readPalette } from "./palette";
import { TerminalRenderer } from "./renderer";
import { Viewport } from "./viewport";
import { clipboardPaste } from "./clipboard";
import type { CellsPayload } from "./types";
import { LatencyProbe } from "./latency";

const PAD = 8;
const RESIZE_DEBOUNCE_MS = 80;

type EditorTerminalPaneProps = {
  session: string;
  path: string;
};

/**
 * A Code-region pane for a daemon-supervised `forge-editor`.
 *
 * Same renderer, palette and textarea input path as `TerminalPane`; its own
 * frame channel so the hidden main pane is undisturbed. Chrome reads
 * `Session.editor`, never ANSI.
 */
export function EditorTerminalPane(props: EditorTerminalPaneProps) {
  let host!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;
  let keys!: HTMLTextAreaElement;

  const viewport = new Viewport();
  const probe = new LatencyProbe();
  let renderer: TerminalRenderer | null = null;
  let cell: CellMetrics = measureCell(tokens.monoSize, tokens.mono, tokens.monoLineHeight);
  const dirty = new Set<number>();
  let repaintAll = true;
  let frame = 0;
  let resizeTimer: number | undefined;
  let lastSize = { cols: 0, rows: 0 };
  let terminal: string | null = null;

  const session = createMemo(() => forgeStore.sessions.find((item) => item.id === props.session));
  const chrome = createMemo(() => editorChrome(session()?.editor, props.path));
  /** The daemon's word that a save was refused, not something read off ANSI. */
  const conflict = createMemo(() => session()?.editor?.conflict === true);
  const [comparing, setComparing] = createSignal(false);
  const [menuAt, setMenuAt] = createSignal<{ x: number; y: number } | null>(null);
  const sides = createMemo(() => editorConflictFor(props.session));

  /* A conflict that resolves takes its comparison with it: leaving the panel
     up would show two sides of a disagreement that no longer exists. */
  createEffect(() => {
    if (!conflict()) {
      setComparing(false);
      clearEditorConflict(props.session);
    }
  });

  /*
   * The pane's own menu. Copy and paste go through the *terminal's* selection
   * and the system clipboard, not the editor's internal register: what the
   * person sees highlighted is what a copy should take, and the register is
   * the editor's own scratch space.
   */
  function menuItems(): MenuItem[] {
    return [
      {
        kind: "item",
        label: "Paste",
        run: () => {
          void navigator.clipboard
            .readText()
            .then((text) => {
              if (text) void sendEditorPaste(props.session, text, probe.send());
            })
            .catch(() => undefined);
        },
      },
      { kind: "rule" },
      {
        kind: "item",
        label: "Reveal in Files",
        run: () => {
          revealInTree(chrome().path);
          showView("Files");
        },
      },
      {
        kind: "item",
        label: "Open Diff",
        run: () => openDiff(),
      },
    ];
  }

  function toggleCompare(): void {
    if (comparing()) {
      setComparing(false);
      return;
    }
    setComparing(true);
    // Fetched on demand: the texts are never on `SessionUpdated`.
    startEditorConflictLoad(props.session);
    void loadEditorConflict(props.session).catch(() => undefined);
  }
  const failed = createMemo(() => {
    const state = session()?.state;
    if (state && typeof state === "object" && "Failed" in state) {
      return state.Failed?.reason ?? "failed";
    }
    return null;
  });

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (!renderer) return;
      if (repaintAll) {
        renderer.paintAll(viewport);
        repaintAll = false;
        dirty.clear();
        return;
      }
      if (dirty.size === 0) return;
      renderer.paintRows(viewport, [...dirty]);
      dirty.clear();
    });
  }

  function onFrame(payload: CellsPayload): void {
    if (terminal && payload.terminal !== terminal) return;
    terminal = payload.terminal;
    const rows = viewport.apply(payload);
    if (payload.full) repaintAll = true;
    else for (const row of rows) dirty.add(row);
    schedule();
  }

  function measurePane(): void {
    if (!renderer || !host) return;
    const rect = host.getBoundingClientRect();
    const width = Math.max(0, rect.width - PAD * 2);
    const height = Math.max(0, rect.height - PAD * 2);
    if (renderer.resize(width, height)) repaintAll = true;
    const size = gridSize(width, height, cell);
    if (size.cols !== lastSize.cols || size.rows !== lastSize.rows) {
      lastSize = size;
      window.clearTimeout(resizeTimer);
      resizeTimer = window.setTimeout(() => {
        void resizeEditor(
          props.session,
          size.cols,
          size.rows,
          Math.round(width),
          Math.round(height),
        ).catch(() => undefined);
      }, RESIZE_DEBOUNCE_MS);
    }
    schedule();
  }

  onMount(() => {
    renderer = new TerminalRenderer(canvas, cell, readPalette());
    measurePane();
    const unsubscribe = editorCellsChannel.subscribe(onFrame);
    const observer = new ResizeObserver(() => measurePane());
    observer.observe(host);
    onCleanup(() => {
      unsubscribe();
      observer.disconnect();
      if (frame !== 0) cancelAnimationFrame(frame);
      window.clearTimeout(resizeTimer);
    });
  });

  function onKeyDown(event: KeyboardEvent): void {
    if (event.isComposing || event.keyCode === 229) return;
    if (event.metaKey) return;
    event.preventDefault();
    void sendEditorKey(
      props.session,
      {
        key: event.key,
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
      },
      probe.send(),
    ).catch(() => undefined);
  }

  function onPaste(event: ClipboardEvent): void {
    event.preventDefault();
    const paste = clipboardPaste(event.clipboardData);
    if (paste.kind === "text") {
      void sendEditorPaste(props.session, paste.text, probe.send()).catch(() => undefined);
    }
  }

  function onCompositionEnd(event: CompositionEvent): void {
    const text = event.data;
    keys.value = "";
    if (text) void sendEditorText(props.session, text, probe.send()).catch(() => undefined);
  }

  return (
    <div class="editor-terminal-pane">
      <header class="editor-terminal-chrome">
        <span class="editor-terminal-path">{chrome().path}</span>
        <Show when={chrome().position}>
          {(position) => <span class="panel-note">{position()}</span>}
        </Show>
        <span class="panel-note">{chrome().mark}</span>
        <span class="history-spacer" />
        <Show when={failed()}>{(reason) => <span class="panel-note">{reason()}</span>}</Show>
      </header>

      {/* The same three answers the DOM editor offers, for the same reason:
          the draft and the other write both still exist, so throwing one away
          is a choice rather than the only way forward. */}
      <Show when={conflict()}>
        <div class="editor-conflict">
          <span>This file changed on disk while you were editing it.</span>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => void overwriteEditorBuffer(props.session).catch(() => undefined)}
          >
            Keep mine
          </Button>
          <Button
            variant="secondary"
            size="xs"
            onClick={() => void reloadEditorBuffer(props.session).catch(() => undefined)}
          >
            Take disk
          </Button>
          <Button variant="secondary" size="xs" selected={comparing()} onClick={toggleCompare}>
            Compare
          </Button>
        </div>
      </Show>
      <Show when={comparing()}>
        <Show when={sides().error}>
          {(error) => <p class="panel-error">Could not read the conflict: {error()}</p>}
        </Show>
        <Show when={sides().conflict}>
          {(both) => <CompareView disk={both().disk} mine={both().mine} />}
        </Show>
      </Show>
      <div
        ref={host}
        class="editor-terminal-body"
        onContextMenu={(event) => {
          event.preventDefault();
          setMenuAt({ x: event.clientX, y: event.clientY });
        }}
      >
        <canvas ref={canvas} />
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
        <textarea
          ref={keys}
          class="terminal-keys"
          autocomplete="off"
          autocapitalize="off"
          spellcheck={false}
          onKeyDown={onKeyDown}
          onPaste={onPaste}
          onCompositionEnd={onCompositionEnd}
          onInput={() => {
            keys.value = "";
          }}
        />
      </div>
    </div>
  );
}
