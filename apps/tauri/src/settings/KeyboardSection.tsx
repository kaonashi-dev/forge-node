import { For, Show, createMemo, createSignal } from "solid-js";
import {
  ACTIONS,
  CONTEXT_ORDER,
  defaultBindings,
  type ActionId,
  type ContextId,
} from "../actions/actions";
import {
  bindingConflicts,
  bindings,
  isOverridden,
  rebind,
  resetBinding,
} from "../actions/bindings";
import { describeChord, keyIsKnown, type Chord } from "../actions/keys";
import { Button, FilterHeader, Kbd } from "../ui";
import { Group, Page } from "./SettingsLayout";

/**
 * Settings › Keyboard (§4.1 U2).
 *
 * Every action, the context it lives in, and the chord it currently answers
 * to — including the ones that ship unbound, which is the half a shortcut list
 * usually leaves out and the half someone is looking for. Rebinding captures a
 * real keystroke rather than parsing a typed string: `⌥⌘F` is a chord you press
 * and a puzzle you spell.
 */
export function KeyboardSection() {
  const [query, setQuery] = createSignal("");
  /** The row currently listening for a keystroke, as `context:action`. */
  const [capturing, setCapturing] = createSignal<string | null>(null);

  /**
   * One row per action, per context it is bound in — plus one row for every
   * action that is bound nowhere, so it can be given a chord.
   */
  const rows = createMemo(() => {
    const table = bindings();
    const out: Array<{ action: ActionId; context: ContextId; chord: Chord | null }> = [];
    for (const action of ACTIONS) {
      const own = table.filter((binding) => binding.action === action.id);
      if (own.length === 0) {
        out.push({ action: action.id, context: defaultContextFor(action.id), chord: null });
        continue;
      }
      for (const binding of own) {
        out.push({ action: action.id, context: binding.context, chord: binding.chord });
      }
    }
    const needle = query().trim().toLowerCase();
    if (needle === "") return out;
    return out.filter((row) => {
      const label = labelFor(row.action).toLowerCase();
      const chord = row.chord ? describeChord(row.chord).toLowerCase() : "";
      return label.includes(needle) || row.action.includes(needle) || chord.includes(needle);
    });
  });

  /**
   * Capture the next keystroke as a chord spec.
   *
   * `keydown` on the row's own button, in the bubble phase — the shell's
   * dispatcher runs on capture and would have taken `⌘K` before this ever saw
   * it, so the handler stops propagation on the way in.
   */
  function capture(event: KeyboardEvent, action: ActionId, context: ContextId): void {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setCapturing(null);
      return;
    }
    // A modifier alone is the first half of a chord, not a chord.
    if (["Shift", "Control", "Alt", "Meta"].includes(event.key)) return;
    const key = normaliseKey(event);
    if (!keyIsKnown(key)) return;
    const spec = [
      event.ctrlKey ? "ctrl" : "",
      event.altKey ? "alt" : "",
      event.shiftKey ? "shift" : "",
      event.metaKey ? "cmd" : "",
      key,
    ]
      .filter(Boolean)
      .join("-");
    rebind(action, context, spec);
    setCapturing(null);
  }

  return (
    <Page
      title="Keyboard"
      summary="Every action, the context it belongs to, and the key it answers to."
    >
      <Show when={bindingConflicts().length > 0}>
        <Group
          title="Conflicts"
          description="Two actions answer to the same chord in the same context. Only the first one fires; the other is unreachable until one of them moves."
        >
          <For each={bindingConflicts()}>
            {(conflict) => (
              <div class="keymap-conflict">
                <Kbd chord={conflict.chord} />
                <span class="keymap-conflict-context">{conflict.context}</span>
                <span class="keymap-conflict-actions">
                  {conflict.actions.map(labelFor).join(" · ")}
                </span>
              </div>
            )}
          </For>
        </Group>
      </Show>

      <Group title="Shortcuts">
        <FilterHeader
          label="Filter shortcuts"
          placeholder="Filter by name or chord…"
          query={query()}
          onQuery={setQuery}
        />
        <table class="keymap-table">
          <thead>
            <tr>
              <th scope="col">Action</th>
              <th scope="col">Context</th>
              <th scope="col">Chord</th>
              <th scope="col">
                <span class="forge-visually-hidden">Reset</span>
              </th>
            </tr>
          </thead>
          <tbody>
            <For each={rows()}>
              {(row) => {
                const id = () => `${row.context}:${row.action}`;
                const listening = () => capturing() === id();
                return (
                  <tr classList={{ overridden: isOverridden(row.action, row.context) }}>
                    <th scope="row" class="keymap-action">
                      {labelFor(row.action)}
                    </th>
                    <td class="keymap-context">{row.context}</td>
                    <td>
                      <button
                        type="button"
                        class="forge-row keymap-chord"
                        classList={{ listening: listening() }}
                        aria-label={
                          listening()
                            ? `Press a new shortcut for ${labelFor(row.action)}, or Escape to cancel`
                            : `Change the shortcut for ${labelFor(row.action)}`
                        }
                        onClick={() => setCapturing(listening() ? null : id())}
                        onBlur={() => listening() && setCapturing(null)}
                        onKeyDown={(event) => {
                          if (!listening()) return;
                          capture(event, row.action, row.context);
                        }}
                      >
                        <Show
                          when={!listening()}
                          fallback={<span class="keymap-listening">Press a key…</span>}
                        >
                          <Show
                            when={row.chord}
                            fallback={<span class="keymap-unbound">Not bound</span>}
                          >
                            {(chord) => <Kbd chord={chord()} />}
                          </Show>
                        </Show>
                      </button>
                    </td>
                    <td>
                      <Show when={isOverridden(row.action, row.context)}>
                        <Button
                          variant="ghost"
                          size="xs"
                          onClick={() => resetBinding(row.action, row.context)}
                        >
                          Reset
                        </Button>
                      </Show>
                    </td>
                  </tr>
                );
              }}
            </For>
          </tbody>
        </table>
      </Group>
    </Page>
  );
}

function labelFor(action: ActionId): string {
  return ACTIONS.find((item) => item.id === action)?.label ?? action;
}

/**
 * Where an unbound action would live if it were given a chord.
 *
 * Read from the shipped table when the action has a default somewhere, and
 * `App` otherwise — which is the right answer for everything that is unbound
 * today, all of which is app-level.
 */
function defaultContextFor(action: ActionId): ContextId {
  const shipped = defaultBindings().find((binding) => binding.action === action);
  return shipped?.context ?? CONTEXT_ORDER[CONTEXT_ORDER.length - 1];
}

/** `KeyboardEvent.key` as a chord spec writes it. */
function normaliseKey(event: KeyboardEvent): string {
  const named: Record<string, string> = {
    arrowup: "up",
    arrowdown: "down",
    arrowleft: "left",
    arrowright: "right",
    " ": "space",
    esc: "escape",
  };
  const key = event.key.toLowerCase();
  return named[key] ?? key;
}
