import { TextField as Kobalte } from "@kobalte/core/text-field";
import { Show, splitProps, type JSX } from "solid-js";
import { Icon } from "../theme/icons";

/** Fields ride the same two rungs as the rest of the controls. */
export type FieldSize = "sm" | "md";
/** `plain` drops the border and fill — a field that *is* its surface. */
export type FieldVariant = "default" | "plain";

type BaseProps = {
  value: string;
  onChange?: (value: string) => void;
  /** Visible label. Omit for a bare field and pass `aria-label` instead. */
  label?: string;
  description?: string;
  placeholder?: string;
  disabled?: boolean;
  class?: string;
  "aria-label"?: string;
};

export type TextFieldProps = BaseProps & {
  type?: "text" | "password" | "email" | "url" | "search";
  size?: FieldSize;
  variant?: FieldVariant;
  /** Marks the field invalid and shows `errorMessage` beneath it. */
  invalid?: boolean;
  errorMessage?: string;
  /** A glyph or button before the input — a search mark, a chord. */
  leading?: JSX.Element;
  /** A glyph or button after the input — a spinner, a clear button. */
  trailing?: JSX.Element;
  /** Opt back into the spell checker for the rare single-line prose field. */
  spellcheck?: boolean;
  ref?: (element: HTMLInputElement) => void;
  onKeyDown?: JSX.EventHandlerUnion<HTMLInputElement, KeyboardEvent>;
};

/**
 * A single-line text field.
 *
 * Kobalte wires the label, description, error and `aria-*` to the input, which
 * is the part every hand-rolled field in the shell used to get wrong. The
 * bordered box is `.forge-field`; the input inside it is `.forge-field-input`,
 * with the leading/trailing slots on either side.
 */
export function TextField(props: TextFieldProps) {
  const [own, rest] = splitProps(props, [
    "label",
    "description",
    "placeholder",
    "class",
    "type",
    "size",
    "variant",
    "invalid",
    "errorMessage",
    "leading",
    "trailing",
    "spellcheck",
    "ref",
    "onKeyDown",
    "aria-label",
  ]);
  const size = () => own.size ?? "md";
  return (
    <Kobalte
      class={`forge-textfield ${own.class ?? ""}`}
      validationState={own.invalid ? "invalid" : "valid"}
      {...rest}
    >
      <Show when={own.label}>
        {(label) => <Kobalte.Label class="forge-label">{label()}</Kobalte.Label>}
      </Show>
      <div
        class={`forge-control forge-control-${size()} forge-field ${
          own.variant === "plain" ? "forge-field-plain" : ""
        }`}
        data-invalid={own.invalid ? "true" : undefined}
      >
        <Show when={own.leading}>
          {(glyph) => <span class="forge-field-affix">{glyph()}</span>}
        </Show>
        {/* A single-line field here is an identifier, not prose: a folder
            name, an executable path, a branch. macOS substitution rewriting
            `test-1` while it is typed corrupts the value silently, so the
            correction machinery is off unless a caller asks for it back.
            Prose belongs in `TextArea`, which keeps the spell checker. */}
        <Kobalte.Input
          ref={own.ref}
          class="forge-field-input"
          type={own.type ?? "text"}
          placeholder={own.placeholder}
          aria-label={own["aria-label"]}
          spellcheck={own.spellcheck ?? false}
          autocorrect="off"
          autocapitalize="off"
          autocomplete="off"
          onKeyDown={own.onKeyDown}
        />
        <Show when={own.trailing}>
          {(glyph) => <span class="forge-field-affix">{glyph()}</span>}
        </Show>
      </div>
      <Show when={own.description}>
        {(text) => <Kobalte.Description class="forge-hint">{text()}</Kobalte.Description>}
      </Show>
      <Show when={own.errorMessage}>
        {(text) => <Kobalte.ErrorMessage class="forge-error">{text()}</Kobalte.ErrorMessage>}
      </Show>
    </Kobalte>
  );
}

export type SearchFieldProps = {
  value: string;
  onChange?: (value: string) => void;
  /** Shown as a clear button once there is a query. */
  onClear?: () => void;
  placeholder?: string;
  size?: FieldSize;
  variant?: FieldVariant;
  disabled?: boolean;
  class?: string;
  "aria-label"?: string;
  ref?: (element: HTMLInputElement) => void;
  onKeyDown?: JSX.EventHandlerUnion<HTMLInputElement, KeyboardEvent>;
};

/**
 * A `TextField` preset for filtering: a search glyph up front, and a clear
 * button that appears once there is something to clear. This is what the
 * palette, the branch picker and every panel search reach for instead of a
 * raw `<input>`.
 */
export function SearchField(props: SearchFieldProps) {
  const [own, rest] = splitProps(props, ["onClear", "value"]);
  return (
    <TextField
      {...rest}
      value={own.value}
      type="search"
      leading={<Icon name="search" size={14} />}
      trailing={
        <Show when={own.value && own.onClear}>
          <button
            type="button"
            class="forge-field-clear"
            aria-label="Clear search"
            onClick={() => own.onClear?.()}
          >
            <Icon name="close" size={12} />
          </button>
        </Show>
      }
    />
  );
}

export type TextAreaProps = BaseProps & {
  maxLength?: number;
  spellcheck?: boolean;
  rows?: number;
  autoResize?: boolean;
  ref?: (element: HTMLTextAreaElement) => void;
  onKeyDown?: JSX.EventHandlerUnion<HTMLTextAreaElement, KeyboardEvent>;
};

/** The multi-line counterpart, used by the Juva draft and the PR composer. */
export function TextArea(props: TextAreaProps) {
  const [own, rest] = splitProps(props, [
    "maxLength",
    "spellcheck",
    "label",
    "description",
    "placeholder",
    "class",
    "rows",
    "autoResize",
    "ref",
    "onKeyDown",
    "aria-label",
  ]);
  return (
    <Kobalte class={`forge-textfield ${own.class ?? ""}`} {...rest}>
      <Show when={own.label}>
        {(label) => <Kobalte.Label class="forge-label">{label()}</Kobalte.Label>}
      </Show>
      <Kobalte.TextArea
        ref={own.ref}
        class="forge-textarea"
        maxLength={own.maxLength}
        spellcheck={own.spellcheck}
        rows={own.rows}
        autoResize={own.autoResize}
        placeholder={own.placeholder}
        aria-label={own["aria-label"]}
        onKeyDown={own.onKeyDown}
      />
      <Show when={own.description}>
        {(text) => <Kobalte.Description class="forge-hint">{text()}</Kobalte.Description>}
      </Show>
    </Kobalte>
  );
}
