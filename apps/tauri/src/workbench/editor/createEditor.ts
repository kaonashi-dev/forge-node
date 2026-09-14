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
export type EditorOptions = FileEditorOptions & { base: ThemeBaseId };
export type EditorHandle = FileEditorHandle & { setBase: (base: ThemeBaseId) => void };

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

export function createEditor(host: HTMLElement, options: EditorOptions): EditorHandle {
  const { base, ...rest } = options;
  applyScopes(host, base);
  const editor = createFileEditor(host, {
    ...rest,
    // The shell already decided this once; the editor must not disagree with
    // the rest of the keymap about which key is the accelerator.
    accelerator: isMac() ? "meta" : "ctrl",
  });
  return {
    ...editor,
    setBase: (next) => applyScopes(host, next),
  };
}
