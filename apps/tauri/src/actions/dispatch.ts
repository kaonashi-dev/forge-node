// Action dispatch.
//
// One listener, installed on `window` in the **capture** phase. That phase is
// the whole point: the terminal's own `keydown` handler runs on the way back
// up, so a chord that matches here is consumed before the PTY can see it. A
// bubble-phase listener would let `cmd-t` open a tab *and* type `t`.

import { createSignal } from "solid-js";
import { CONTEXT_ORDER, type ActionId, type Binding, type ContextId } from "./actions";
import { bindings as mergedBindings } from "./bindings";
import { isMac, isSelectAll, matches, type Chord } from "./keys";

export type ActionHandler = (argument?: number) => void;

const handlers = new Map<ActionId, ActionHandler[]>();
const counts = new Map<ContextId, number>();
const [contexts, setContexts] = createSignal<ContextId[]>([]);

/** The contexts currently active, innermost first. */
export const activeContexts = contexts;

/**
 * Enter a context for as long as the returned function is uncalled.
 *
 * Counted rather than a flag: two panes can hold `Files` at once during a
 * transition, and the first to leave must not close it for the other.
 */
export function enterContext(context: ContextId): () => void {
  counts.set(context, (counts.get(context) ?? 0) + 1);
  publish();
  let left = false;
  return () => {
    if (left) return;
    left = true;
    const next = (counts.get(context) ?? 1) - 1;
    if (next <= 0) counts.delete(context);
    else counts.set(context, next);
    publish();
  };
}

function publish(): void {
  setContexts(CONTEXT_ORDER.filter((context) => (counts.get(context) ?? 0) > 0));
}

/**
 * Register what an action does.
 *
 * The most recent registration wins, and unregistering restores the one
 * before it, so a panel can take an action over while it is on screen and hand
 * it back when it leaves.
 */
export function registerAction(id: ActionId, handler: ActionHandler): () => void {
  const stack = handlers.get(id) ?? [];
  stack.push(handler);
  handlers.set(id, stack);
  return () => {
    const current = handlers.get(id);
    if (!current) return;
    const index = current.lastIndexOf(handler);
    if (index >= 0) current.splice(index, 1);
    if (current.length === 0) handlers.delete(id);
  };
}

/** Run an action by name, as the palette and the menus do. */
export function invokeAction(id: ActionId, argument?: number): boolean {
  const stack = handlers.get(id);
  const handler = stack?.at(-1);
  if (!handler) return false;
  handler(argument);
  return true;
}

/** Whether anything currently answers to an action, for greying out a menu row. */
export function actionIsBound(id: ActionId): boolean {
  return (handlers.get(id)?.length ?? 0) > 0;
}

/**
 * The binding an event fires, or `null`.
 *
 * Exported for the tests: the dispatch rule — innermost active context wins,
 * and a binding with no handler is not a match — is the part worth asserting
 * on, and it needs no DOM to assert.
 */
export function resolve(
  event: KeyboardEvent,
  active: ContextId[],
  bindings: Binding[],
  bound: (id: ActionId) => boolean,
  /** Whether the keystroke is going into a text field. */
  typing = false,
): Binding | null {
  for (const context of active) {
    for (const binding of bindings) {
      if (binding.context !== context) continue;
      // A bare letter is a letter while someone is typing one. The tree binds
      // `j`/`k`/`h`/`l` and `Enter` with no modifier, and a filter box inside
      // that same panel would have every one of them taken out of it — the
      // chord and the character are indistinguishable at this level, so the
      // target is what tells them apart.
      if (typing && isBareKey(binding.chord)) continue;
      if (!matches(binding.chord, event)) continue;
      if (!bound(binding.action)) continue;
      return binding;
    }
  }
  return null;
}

/** A chord with no modifier at all: indistinguishable from typing. */
function isBareKey(chord: Chord): boolean {
  return !chord.ctrl && !chord.alt && !chord.meta;
}

/** The `<input>` types that hold no text, so a bare chord is still a chord. */
const NON_TEXT_INPUTS = ["checkbox", "radio", "button", "submit", "reset", "range", "color"];

/**
 * Whether a keystroke is destined for something that takes text.
 *
 * Includes `contenteditable`, which is what CodeMirror's content element is:
 * the editor's own keymap has to see every key it is given, and a bare-letter
 * app chord firing inside it would eat the letter.
 *
 * Duck-typed rather than `instanceof HTMLElement`, which is also what makes it
 * testable under vitest's node environment: `instanceof` is per-realm, and the
 * three properties read here are the whole of the question.
 */
export function isTypingTarget(target: EventTarget | null): boolean {
  const element = target as { tagName?: unknown; isContentEditable?: unknown; type?: unknown };
  if (!element || typeof element.tagName !== "string") return false;
  if (element.isContentEditable === true) return true;
  const tag = element.tagName.toUpperCase();
  if (tag === "TEXTAREA" || tag === "SELECT") return true;
  if (tag !== "INPUT") return false;
  return !NON_TEXT_INPUTS.includes(String(element.type ?? "text").toLowerCase());
}

export function nativeClipboardIn(event: KeyboardEvent): boolean {
  if (!isTypingTarget(event.target)) return false;
  const target = event.target as { classList?: { contains?: (name: string) => boolean } };
  if (target.classList?.contains?.("terminal-keys")) return false;
  if (event.altKey || event.shiftKey) return false;
  const modifier = isMac() ? event.metaKey && !event.ctrlKey : event.ctrlKey && !event.metaKey;
  return modifier && ["c", "v", "x"].includes(event.key.toLowerCase());
}

/**
 * Select the contents of a real text field. Returns whether it did.
 *
 * WKWebView only runs select-all when Edit › Select All is a predefined
 * menu item; a custom Edit menu that omitted it delivered the keydown
 * with no default action, so ⌘A in a field did nothing. Calling `select()`
 * here is what makes the chord work even if the menu is wrong.
 *
 * The terminal's hidden textarea is a typing target, but selecting it
 * selects nothing the user can see — leave that chord alone there.
 * CodeMirror's content element is `contenteditable`, not an `<input>`,
 * so this returns false and the editor's own keymap takes it.
 */
export function selectAllIn(target: EventTarget | null): boolean {
  const element = target as {
    tagName?: unknown;
    classList?: { contains?: (name: string) => boolean };
    select?: () => void;
  };
  if (!element || typeof element.tagName !== "string") return false;
  const tag = element.tagName.toUpperCase();
  if (tag !== "INPUT" && tag !== "TEXTAREA") return false;
  if (element.classList?.contains?.("terminal-keys")) return false;
  if (typeof element.select !== "function") return false;
  element.select();
  return true;
}

/**
 * Install the keymap. Call once, from the shell root.
 *
 * The table is read per event rather than captured at install time, so a
 * rebind in Settings takes effect on the next keystroke instead of on the next
 * launch (§4.1 U1).
 */
export function installKeymap(table: () => Binding[] = mergedBindings): () => void {
  const onKeyDown = (event: KeyboardEvent) => {
    // A composition owns the keyboard; a chord fired mid-word would commit
    // half of it.
    if (event.isComposing || event.keyCode === 229) return;
    // Select-all is the field's, never an app chord. Done before `resolve`
    // so a rebind of `cmd-a` cannot take it away from a focused input.
    if (isSelectAll(event) && selectAllIn(event.target)) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    // Clipboard gestures belong to the focused field, even with a custom app binding.
    if (nativeClipboardIn(event)) return;
    const binding = resolve(
      event,
      contexts(),
      table(),
      actionIsBound,
      isTypingTarget(event.target),
    );
    if (!binding) return;
    event.preventDefault();
    event.stopPropagation();
    invokeAction(binding.action, binding.argument);
  };
  window.addEventListener("keydown", onKeyDown, { capture: true });
  return () => window.removeEventListener("keydown", onKeyDown, { capture: true });
}

/** Test seam: forget every context and handler. */
export function resetActions(): void {
  handlers.clear();
  counts.clear();
  publish();
}
