import { createStore } from "solid-js/store";

export type TextInputRequest = {
  title: string;
  label: string;
  value: string;
  placeholder?: string;
  confirmLabel: string;
  allowEmpty: boolean;
  onSubmit: (value: string) => void;
};

export type ConfirmRequest = {
  title: string;
  /** The consequence, in a sentence. Skipped when the title says it all. */
  description?: string;
  /** Label on the button that goes ahead. Names the act: "Close", "Abort". */
  confirmLabel: string;
  /** Draws the confirm button in danger colours; true for anything that loses work. */
  destructive?: boolean;
  onConfirm: () => void;
};

export const [dialogsStore, setDialogsStore] = createStore({
  confirm: null as ConfirmRequest | null,
  textInput: null as TextInputRequest | null,
});

/** Ask for a name. Replaces `window.prompt`, which WKWebView does not show. */
export function requestTextInput(request: TextInputRequest): void {
  setDialogsStore("textInput", request);
}

/** Put a destructive choice to the person. Replaces `window.confirm`. */
export function requestConfirm(request: ConfirmRequest): void {
  setDialogsStore("confirm", request);
}
