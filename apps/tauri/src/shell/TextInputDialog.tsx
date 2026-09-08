import { createSignal } from "solid-js";
import { Button, Dialog, TextField } from "../ui";
import type { TextInputRequest } from "../store/runtimeStore";

/* The request shape lives in `runtimeStore` so any panel can raise one. */
export type { TextInputRequest } from "../store/runtimeStore";

export type TextInputDialogProps = {
  request: TextInputRequest;
  onDismiss: () => void;
};

/** In-app replacement for `window.prompt` — Tauri WKWebView does not reliably surface native prompts. */
export function TextInputDialog(props: TextInputDialogProps) {
  const [value, setValue] = createSignal(props.request.value);
  const empty = () => !props.request.allowEmpty && value().trim() === "";

  function submit(): void {
    if (empty()) return;
    props.request.onSubmit(value().trim());
    props.onDismiss();
  }

  return (
    <Dialog
      title={props.request.title}
      size="sm"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={empty()} onClick={submit}>
            {props.request.confirmLabel}
          </Button>
        </>
      }
    >
      <TextField
        label={props.request.label}
        value={value()}
        placeholder={props.request.placeholder}
        onChange={setValue}
        ref={(element) => {
          // Kobalte moves focus into the card; selecting the text is the part
          // that makes a pre-filled rename immediately replaceable.
          queueMicrotask(() => element.select());
        }}
        onKeyDown={(event) => {
          if (event.key !== "Enter") return;
          event.preventDefault();
          submit();
        }}
      />
    </Dialog>
  );
}
