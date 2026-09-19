import { For, Show } from "solid-js";
import type { ActionId } from "../actions/actions";
import { invokeAction } from "../actions/dispatch";
import { Icon, type ForgeIconName } from "../theme/icons/index";
import { Kbd } from "./Kbd";

export type EmptyAction = {
  action: ActionId;
  label: string;
  icon?: ForgeIconName;
};

export type EmptyStateProps = {
  /** What is not here, in a sentence. */
  message: string;
  /** Action IDs keep dispatch and displayed shortcuts aligned with user bindings. */
  actions?: ReadonlyArray<EmptyAction>;
};

export function EmptyState(props: EmptyStateProps) {
  return (
    <div class="empty-state">
      <p class="empty-copy">{props.message}</p>
      <Show when={(props.actions?.length ?? 0) > 0}>
        <ul class="empty-state-rows">
          <For each={props.actions}>
            {(row) => (
              <li>
                <button
                  type="button"
                  class="forge-row empty-state-row"
                  onClick={() => invokeAction(row.action)}
                >
                  <Show when={row.icon}>
                    {(icon) => <Icon name={icon()} class="forge-icon-muted" size={14} />}
                  </Show>
                  <span class="empty-state-label">{row.label}</span>
                  <Kbd action={row.action} />
                </button>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </div>
  );
}
