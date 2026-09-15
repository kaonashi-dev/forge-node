import { Show, createMemo, onCleanup, onMount } from "solid-js";
import { editorChrome } from "./editorChrome";
import { resizeEditor, sendEditorKey, sendEditorPaste, sendEditorText } from "../runtime/api";
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
      <div ref={host} class="editor-terminal-body">
        <canvas ref={canvas} />
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
