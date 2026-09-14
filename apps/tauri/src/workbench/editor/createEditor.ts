// The shell's binding for the portable editor: accelerator, theme scopes.

import {
  createFileEditor,
  type FileEditorHandle,
  type FileEditorOptions,
} from "@forge-node/file-workbench/editor";
import { isMac } from "../../actions/keys";
import { editorPalette } from "../../theme/editorTheme";
import type { ThemeBaseId } from "../../theme/tokens";

export type { GitMark, GitMarks } from "@forge-node/file-workbench/editor";

/** The two type metrics the editor takes apart from the theme. */
export type EditorMetrics = {
  /** Pixels. Plain inheritance: `.fw-editor` is `font: inherit`. */
  fontSize: number;
  /** Unitless multiplier, shared by the gutter, the overlay and the textarea. */
  lineHeight: number;
};

export type EditorOptions = FileEditorOptions & { base: ThemeBaseId; metrics: EditorMetrics };
export type EditorHandle = FileEditorHandle & {
  setBase: (base: ThemeBaseId) => void;
  setMetrics: (metrics: EditorMetrics) => void;
};

function applyScopes(host: HTMLElement, base: ThemeBaseId): void {
  const palette = editorPalette(base);
  const style = host.style;
  style.setProperty("--fw-editor-ground", palette.background);
  style.setProperty("--fw-foreground", palette.foreground);
  style.setProperty("--fw-active-line", palette.activeLine);
  style.setProperty("--fw-selection", palette.selection);
  style.setProperty("--fw-accent", palette.caret);
  style.setProperty("--fw-scope-comment", palette.scopes.comment);
  style.setProperty("--fw-scope-keyword", palette.scopes.keyword);
  style.setProperty("--fw-scope-controlKeyword", palette.scopes.controlKeyword);
  style.setProperty("--fw-scope-string", palette.scopes.string);
  style.setProperty("--fw-scope-number", palette.scopes.number);
  style.setProperty("--fw-scope-type", palette.scopes.type);
  style.setProperty("--fw-scope-function", palette.scopes.function);
  style.setProperty("--fw-scope-property", palette.scopes.property);
  style.setProperty("--fw-scope-operator", palette.scopes.operator);
  style.setProperty("--fw-scope-punctuation", palette.scopes.punctuation);
  style.setProperty("--fw-scope-meta", palette.scopes.meta);
  style.setProperty("--fw-scope-tag", palette.scopes.tag);
}

/**
 * Push the reader's type metrics onto the host.
 *
 * Font size rides ordinary inheritance and line height rides a custom
 * property, which looks inconsistent and is not: three elements — the gutter,
 * the highlight overlay and the textarea — have to share one line box to the
 * pixel, and a single `--fw-editor-line-height` is what keeps them from
 * drifting. Font size has no such problem, because all three inherit it.
 */
function applyMetrics(host: HTMLElement, metrics: EditorMetrics): void {
  host.style.fontSize = `${metrics.fontSize}px`;
  host.style.setProperty("--fw-editor-line-height", String(metrics.lineHeight));
}

export function createEditor(host: HTMLElement, options: EditorOptions): EditorHandle {
  const { base, metrics, ...rest } = options;
  applyScopes(host, base);
  applyMetrics(host, metrics);
  const editor = createFileEditor(host, {
    ...rest,
    // The shell already decided this once; the editor must not disagree with
    // the rest of the keymap about which key is the accelerator.
    accelerator: isMac() ? "meta" : "ctrl",
  });
  return {
    ...editor,
    setBase: (next) => applyScopes(host, next),
    setMetrics: (next) => applyMetrics(host, next),
  };
}
