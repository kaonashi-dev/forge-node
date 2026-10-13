import { createSignal } from "solid-js";
import { parseTabOrder, type TabOrderMap } from "./sessionScope";
import { setAppState } from "../settings/commands";
import { forgeStore } from "../../state/forgeStore";

const TAB_ORDER_KEY = "ui.tab_order";

/** Module-level so the tab strip and the rail paint one order, not two that drift. */
const [tabOrder, setTabOrder] = createSignal<TabOrderMap>({});

/** Seed from the daemon's stored copy once it lands; a later snapshot must not undo a drag. */
export function seedTabOrder(): void {
  setTabOrder(parseTabOrder(forgeStore.app_state[TAB_ORDER_KEY]));
}

export function persistTabOrder(workspace: string, order: string[]): void {
  // Pruned to live checkouts: app state is written back on every drag, and a
  // per-checkout map would otherwise keep a row for every worktree ever removed.
  const live = new Set(forgeStore.workspaces.map((item) => item.id));
  const next: TabOrderMap = { [workspace]: order };
  for (const [id, ids] of Object.entries(tabOrder())) {
    if (id !== workspace && live.has(id)) next[id] = ids;
  }
  setTabOrder(next);
  void setAppState(TAB_ORDER_KEY, JSON.stringify(next)).catch(() => undefined);
}

export { tabOrder };
