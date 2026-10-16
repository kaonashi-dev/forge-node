import { createSignal } from "solid-js";

/** What the editor on screen reports for the card footer: position, facts, mark. */
export type EditorFooter = { position: string | null; details: string[]; mark: string };

const [footer, setFooter] = createSignal<EditorFooter | null>(null);

/** The facts of the editor the card is showing, or `null` with none mounted. */
export const editorFooter = footer;
export const setEditorFooter = setFooter;
