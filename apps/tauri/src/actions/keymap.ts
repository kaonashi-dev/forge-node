// U1: the person's own bindings, over the defaults.
//
// Where they live. `plan-ui-ux.md` proposed `~/…/Forge/keymap.json` "read by
// the daemon, same path family as `theme.json`" — but there is no such family:
// `theme.json` is a protocol fixture, and the daemon has no config-directory
// reader to add a second file to. What it does have is `app_state`, the
// key-value store that already holds every other `ui.*` preference and is
// already shared with any other shell on the same daemon. So the overrides go
// there, under one key, as the same JSON a file would have held — and if a
// file reader is ever added, it produces this shape and nothing below changes.
//
// Pure, and its own module: merging and conflict detection are the parts with
// a right and a wrong answer, and both are answerable without a DOM.

import { defaultBindings, type ActionId, type Binding, type ContextId } from "./actions";
import { describeChord, keyIsKnown, parseChord, type Chord } from "./keys";

/** Where the merged table is stored, alongside `ui.sidebar.width` and friends. */
export const KEYMAP_KEY = "ui.keymap";

/**
 * One person's decision about one binding.
 *
 * `chord: null` is an unbind, and it has to be representable: "this shortcut
 * should do nothing" is a real preference, and leaving it out would mean the
 * only way to express it is to bind the action to a key nobody presses.
 */
export type KeymapOverride = {
  action: ActionId;
  context: ContextId;
  /** A chord spec — `cmd-shift-p` — or `null` to unbind the action there. */
  chord: string | null;
};

/**
 * Read the stored overrides.
 *
 * Anything malformed is dropped rather than thrown: this value is read on the
 * way to installing the keymap, and a bad entry written by an older build must
 * cost that one binding, not every shortcut in the app.
 */
export function parseKeymap(raw: string | undefined): KeymapOverride[] {
  if (!raw) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return [];
  }
  if (!Array.isArray(parsed)) return [];
  const out: KeymapOverride[] = [];
  for (const entry of parsed) {
    if (typeof entry !== "object" || entry === null) continue;
    const row = entry as Record<string, unknown>;
    if (typeof row.action !== "string" || typeof row.context !== "string") continue;
    if (row.chord !== null && typeof row.chord !== "string") continue;
    if (typeof row.chord === "string" && !chordIsValid(row.chord)) continue;
    out.push({
      action: row.action as ActionId,
      context: row.context as ContextId,
      chord: row.chord as string | null,
    });
  }
  return out;
}

export function serializeKeymap(overrides: KeymapOverride[]): string {
  return JSON.stringify(overrides);
}

function chordIsValid(spec: string): boolean {
  try {
    // Parsing alone is not enough: `parseChord` takes everything after the
    // modifiers as the key, so any string at all yields a binding — one that
    // can never fire. `keyIsKnown` is what makes an unusable chord a rejected
    // one rather than a shortcut that quietly does nothing.
    return keyIsKnown(parseChord(spec).key);
  } catch {
    return false;
  }
}

/**
 * The defaults with the overrides applied.
 *
 * An override replaces the default for its `(action, context)` pair rather
 * than adding to it, because that is what a rebind means. An action bound to
 * two chords by default — the file tree answers to both `↓` and `j` — keeps
 * whichever it was not asked about, so rebinding `j` does not silently take
 * the arrow away too.
 *
 * `argument` is preserved from the default it replaces, so rebinding
 * "focus tab 3" still focuses tab 3.
 */
export function mergeBindings(
  overrides: KeymapOverride[],
  defaults: Binding[] = defaultBindings(),
): Binding[] {
  const decided = new Map<string, KeymapOverride>();
  for (const override of overrides) decided.set(pairKey(override), override);

  const merged: Binding[] = [];
  const applied = new Set<string>();

  for (const binding of defaults) {
    const key = pairKey(binding);
    const override = decided.get(key);
    if (!override) {
      merged.push(binding);
      continue;
    }
    // An action with two default chords is replaced once, by the first of
    // them; the rest are dropped, because two chords for one rebind is a
    // second binding nobody asked for.
    if (applied.has(key)) continue;
    applied.add(key);
    if (override.chord === null) continue;
    merged.push({ ...binding, chord: parseChord(override.chord) });
  }

  // An override for something with no default — a chord for an action that
  // ships unbound — is an addition, not a replacement.
  for (const override of overrides) {
    if (applied.has(pairKey(override)) || override.chord === null) continue;
    if (defaults.some((binding) => pairKey(binding) === pairKey(override))) continue;
    merged.push({
      chord: parseChord(override.chord),
      action: override.action,
      context: override.context,
    });
  }
  return merged;
}

function pairKey(item: { action: ActionId; context: ContextId }): string {
  return `${item.context}:${item.action}`;
}

/** A chord claimed by more than one action in the same context. */
export type Conflict = {
  context: ContextId;
  chord: Chord;
  label: string;
  actions: ActionId[];
};

/**
 * Every chord two actions both answer to in one context.
 *
 * Not an error, and deliberately reported rather than resolved: `resolve`
 * returns whichever comes first in the table, so the second binding is dead —
 * silently — and the only way anyone finds out is that a shortcut they set
 * does nothing. Settings › Keyboard shows this list.
 */
export function conflicts(bindings: Binding[]): Conflict[] {
  const byChord = new Map<string, Conflict>();
  for (const binding of bindings) {
    const key = `${binding.context}|${chordSignature(binding.chord)}|${binding.argument ?? ""}`;
    const existing = byChord.get(key);
    if (existing) {
      if (!existing.actions.includes(binding.action)) existing.actions.push(binding.action);
      continue;
    }
    byChord.set(key, {
      context: binding.context,
      chord: binding.chord,
      label: describeChord(binding.chord),
      actions: [binding.action],
    });
  }
  return [...byChord.values()].filter((entry) => entry.actions.length > 1);
}

export function chordSignature(chord: Chord): string {
  return [
    chord.ctrl ? "ctrl" : "",
    chord.alt ? "alt" : "",
    chord.shift ? "shift" : "",
    chord.meta ? "cmd" : "",
    chord.key,
  ]
    .filter(Boolean)
    .join("-");
}
