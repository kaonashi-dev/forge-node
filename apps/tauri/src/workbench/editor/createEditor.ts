// The shell's binding for the portable editor: this file supplies the theme,
// the find bar and the accelerator; the package owns the document and its DOM.

import {
  createFileEditor,
  type FileEditorHandle,
  type FileEditorOptions,
} from "@forge-node/file-workbench/editor";
import { isMac } from "../../actions/keys";
import type { ThemeBaseId } from "../../theme/tokens";
import { findBar } from "./searchPanel";
import { editorTheme } from "./theme";
export type { GitMark, GitMarks } from "@forge-node/file-workbench/editor";
export type EditorOptions = Omit<FileEditorOptions, "theme" | "findExtension" | "accelerator"> & {
  base: ThemeBaseId;
};
export type EditorHandle = FileEditorHandle & { setBase: (base: ThemeBaseId) => void };
export function createEditor(host: HTMLElement, options: EditorOptions): EditorHandle {
  const editor = createFileEditor(host, {
    ...options,
    theme: editorTheme(options.base),
    findExtension: findBar(),
    // The shell already decided this once; the editor must not disagree with
    // the rest of the keymap about which key is the accelerator.
    accelerator: isMac() ? "meta" : "ctrl",
  });
  return { ...editor, setBase: (base) => editor.setTheme(editorTheme(base)) };
}
