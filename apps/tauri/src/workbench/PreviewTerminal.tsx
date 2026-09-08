import { createEffect, onCleanup, onMount } from "solid-js";
import * as harness from "../harness/api";
import { Button } from "../ui";
import { previewCellsChannel } from "../runtime/bus";
import { harnessStore, setHarnessStore } from "../store/harnessStore";
import { metrics as tokens } from "../theme/tokens";
import { measureCell, gridSize, type CellMetrics } from "../terminal/metrics";
import { readPalette } from "../terminal/palette";
import { TerminalRenderer } from "../terminal/renderer";
import { Viewport } from "../terminal/viewport";

/** Rows the preview shows. Enough to see what a step is doing, not to work in. */
const PREVIEW_ROWS = 12;

/** Rows once it is expanded: a step being read rather than glanced at. */
const PREVIEW_ROWS_EXPANDED = 32;

/** Breathing room between the grid and the pane edges, as in the main pane. */
const PAD = 8;

/**
 * A second terminal, watching a harness session from inside the feature tab.
 *
 * The same passive renderer as the main pane and the same wire, on its own
 * event: the point is to read a running step without leaving the tab, and
 * without moving the terminal pane off whatever the user was working in.
 *
 * Deliberately thin next to `TerminalPane`. No selection, no scrollback, no
 * latency probe, no resize negotiation beyond the column count — those all
 * exist for a terminal you *work in*, and this is one you look at. Keys still
 * go through, because a step that stopped on a prompt has to be answerable.
 */
export function PreviewTerminal() {
  let host!: HTMLDivElement;
  let canvas!: HTMLCanvasElement;

  const rows = () => (harnessStore.previewFull ? PREVIEW_ROWS_EXPANDED : PREVIEW_ROWS);

  const viewport = new Viewport();
  let renderer: TerminalRenderer | null = null;
  let cell: CellMetrics = measureCell(tokens.monoSize, tokens.mono, tokens.monoLineHeight);
  let frame = 0;
  const dirty = new Set<number>();
  let repaintAll = true;

  function schedule(): void {
    if (frame !== 0) return;
    frame = requestAnimationFrame(() => {
      frame = 0;
      if (!renderer) return;
      if (repaintAll) {
        repaintAll = false;
        dirty.clear();
        renderer.paintAll(viewport);
        return;
      }
      if (dirty.size === 0) return;
      const rows = [...dirty];
      dirty.clear();
      renderer.paintRows(viewport, rows);
    });
  }

  onMount(() => {
    renderer = new TerminalRenderer(canvas, cell, readPalette());
    fit();

    const unsubscribe = previewCellsChannel.subscribe((payload) => {
      const changed = viewport.apply(payload);
      if (payload.full) repaintAll = true;
      else for (const row of changed) dirty.add(row);
      schedule();
    });

    // Height follows the expand toggle and nothing else; the width is
    // negotiated whenever the tab is resized.
    const observer = new ResizeObserver(() => fit());
    observer.observe(host);
    createEffect(() => {
      rows();
      fit();
    });

    onCleanup(() => {
      unsubscribe();
      observer.disconnect();
      if (frame !== 0) cancelAnimationFrame(frame);
      // Letting go here and not only on the tab's own close: the card is
      // hidden the moment the store forgets the session, and an attachment
      // nobody is painting is a stream the daemon keeps sending.
      setHarnessStore({ previewSession: null, previewFull: false });
      void harness.detachPreview().catch(() => undefined);
    });
  });

  function fit(): void {
    if (!renderer || !host) return;
    const width = Math.max(1, host.clientWidth - PAD * 2);
    const height = rows() * cell.height;
    if (renderer.resize(width, height)) {
      repaintAll = true;
      schedule();
    }
    const { cols } = gridSize(width, height, cell);
    void harness
      .resizePreview(cols, rows(), Math.round(width), Math.round(height))
      .catch(() => undefined);
  }

  function onKeyDown(event: KeyboardEvent): void {
    // Everything the browser would otherwise steal from a focused element:
    // the point of typing here is that the step sees it, not the page.
    event.preventDefault();
    void harness
      .sendPreviewKey({
        key: event.key,
        ctrl: event.ctrlKey,
        alt: event.altKey,
        shift: event.shiftKey,
      })
      .catch(() => undefined);
  }

  return (
    <section class="feature-preview" aria-label="Session preview">
      <header class="feature-preview-head">
        <span>Preview</span>
        <span class="history-spacer" />
        <Button
          variant="secondary"
          size="xs"
          onClick={() => setHarnessStore("previewFull", !harnessStore.previewFull)}
        >
          {harnessStore.previewFull ? "Collapse" : "Expand"}
        </Button>
        <Button
          variant="secondary"
          size="xs"
          onClick={() => {
            setHarnessStore({ previewSession: null, previewFull: false });
            void harness.detachPreview().catch(() => undefined);
          }}
        >
          Hide preview
        </Button>
      </header>
      <div
        ref={host}
        class="feature-preview-body"
        tabIndex={0}
        role="textbox"
        aria-label="Preview terminal"
        onKeyDown={onKeyDown}
      >
        <canvas ref={canvas} />
      </div>
    </section>
  );
}
