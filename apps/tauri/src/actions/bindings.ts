// The binding table the running app actually uses.
//
// `defaultBindings()` is what ships; this is that, with the person's overrides
// on top (§4.1 U1). One reactive source so the dispatcher, the palette, every
// tooltip and Settings › Keyboard all read the same table — a chord shown in a
// tooltip that a rebind has already moved is worse than no tooltip.

import { createMemo } from "solid-js";
import { defaultBindings, type ActionId, type Binding } from "./actions";
import { forgeStore } from "../store/forgeStore";
import { setAppState } from "../runtime/api";
import {
  KEYMAP_KEY,
  conflicts,
  mergeBindings,
  parseKeymap,
  serializeKeymap,
  type Conflict,
  type KeymapOverride,
} from "./keymap";
import type { Chord } from "./keys";

/** The overrides as stored, for Settings › Keyboard to edit. */
export const overrides = createMemo(() => parseKeymap(forgeStore.app_state[KEYMAP_KEY]));

/** Defaults plus overrides. Everything that reads a chord reads this. */
export const bindings = createMemo(() => mergeBindings(overrides()));

/** Chords two actions in one context both answer to; the second one is dead. */
export const bindingConflicts = createMemo((): Conflict[] => conflicts(bindings()));

/**
 * The chord an action is drawn with right now.
 *
 * Replaces `chordFor(action)` at every call site that renders one. The
 * `actions.ts` version stays for tests, which want the shipped table and not
 * whatever this install has been configured to.
 */
export function boundChord(action: ActionId): Chord | null {
  return bindings().find((binding) => binding.action === action)?.chord ?? null;
}

/** Every binding for one action, for the Settings table's rows. */
export function bindingsFor(action: ActionId): Binding[] {
  return bindings().filter((binding) => binding.action === action);
}

/** Store a new set of overrides. The daemon owns them, like every `ui.*` key. */
export function writeOverrides(next: KeymapOverride[]): void {
  void setAppState(KEYMAP_KEY, serializeKeymap(next)).catch(() => undefined);
}

/**
 * Rebind one action in one context, or unbind it with `chord: null`.
 *
 * Replaces any previous decision about the same pair rather than stacking a
 * second one, so the stored table never grows a history.
 */
export function rebind(action: ActionId, context: Binding["context"], chord: string | null): void {
  const next = overrides().filter((item) => !(item.action === action && item.context === context));
  next.push({ action, context, chord });
  writeOverrides(next);
}

/** Drop the override for one action, restoring what ships. */
export function resetBinding(action: ActionId, context: Binding["context"]): void {
  writeOverrides(
    overrides().filter((item) => !(item.action === action && item.context === context)),
  );
}

/** Whether an action is currently carrying an override in a context. */
export function isOverridden(action: ActionId, context: Binding["context"]): boolean {
  return overrides().some((item) => item.action === action && item.context === context);
}

export { defaultBindings };
