import { For, Show, createMemo, createSignal } from "solid-js";
import {
  ACTIONS,
  CONTEXT_ORDER,
  actionLabel,
  defaultBindings,
  type ActionId,
  type ContextId,
} from "../../actions/actions";
import {
  bindingConflicts,
  bindings,
  isOverridden,
  rebind,
  resetBinding,
} from "../../actions/bindings";
import { describeChord, specFromEvent, type Chord } from "../../actions/keys";
import { Button, FilterHeader, Kbd } from "../../ui/index";
import { Group, Page } from "./SettingsLayout";

export function KeyboardSection() {
  const [query, setQuery] = createSignal("");
  /** The row currently listening for a keystroke, as `context:action:argument`. */
  const [capturing, setCapturing] = createSignal<string | null>(null);

  /**
   * One row per action, per context it is bound in — plus one row for every
   * action that is bound nowhere, so it can be given a chord.
   */
  const rows = createMemo(() => {
    const table = bindings();
    const out: Array<{
      action: ActionId;
      context: ContextId;
      chord: Chord | null;
      argument?: number;
    }> = [];
    for (const action of ACTIONS) {
      const own = table.filter((binding) => binding.action === action.id);
      if (own.length === 0) {
        out.push({ action: action.id, context: defaultContextFor(action.id), chord: null });
        continue;
      }
      for (const binding of own) {
        out.push({
          action: action.id,
          context: binding.context,
          chord: binding.chord,
          argument: binding.argument,
        });
      }
    }
    const needle = query().trim().toLowerCase();
    if (needle === "") return out;
    return out.filter((row) => {
      const label = actionLabel(row.action, row.argument).toLowerCase();
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
  function capture(
    event: KeyboardEvent,
    action: ActionId,
    context: ContextId,
    argument?: number,
  ): void {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      setCapturing(null);
      return;
    }
    const spec = specFromEvent(event);
    if (!spec) return;
    rebind(action, context, spec, argument);
    setCapturing(null);
  }

  return (
    <Page
      title="Keyboard"
      summary="Every action, the context it belongs to, and the key it answers to. Escape twice leaves Settings — a sequence, not a rebindable chord."
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
                  {conflict.actions.map((action) => actionLabel(action)).join(" · ")}
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
                const id = () => `${row.context}:${row.action}:${row.argument ?? ""}`;
                const listening = () => capturing() === id();
                const label = actionLabel(row.action, row.argument);
                return (
                  <tr
                    classList={{
                      overridden: isOverridden(row.action, row.context, row.argument),
                    }}
                  >
                    <th scope="row" class="keymap-action">
                      {label}
                    </th>
                    <td class="keymap-context">{row.context}</td>
                    <td>
                      <button
                        type="button"
                        class="forge-row keymap-chord"
                        classList={{ listening: listening() }}
                        aria-label={
                          listening()
                            ? `Press a new shortcut for ${label}, or Escape to cancel`
                            : `Change the shortcut for ${label}`
                        }
                        onClick={() => setCapturing(listening() ? null : id())}
                        onBlur={() => listening() && setCapturing(null)}
                        onKeyDown={(event) => {
                          if (!listening()) return;
                          capture(event, row.action, row.context, row.argument);
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
                      <Show when={isOverridden(row.action, row.context, row.argument)}>
                        <Button
                          variant="ghost"
                          size="xs"
                          onClick={() => resetBinding(row.action, row.context, row.argument)}
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
