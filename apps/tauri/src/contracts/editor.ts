// The DOM editor surface's input wire, as `domain::editor_input` serializes it.

export type EditorKey =
  | { Char: string }
  | "Enter"
  | "Tab"
  | "Backspace"
  | "Delete"
  | "Escape"
  | "Left"
  | "Right"
  | "Up"
  | "Down"
  | "Home"
  | "End"
  | "PageUp"
  | "PageDown"
  | "Insert"
  | { Function: number };

export type EditorInputEvent =
  | { Key: { key: EditorKey; modifiers: number } }
  | { Text: string }
  | { Pointer: { kind: "Down" | "Drag" | "Up"; line: number; column: number; modifiers: number } }
  | { Wheel: { lines: number } }
  | { Focus: boolean };
