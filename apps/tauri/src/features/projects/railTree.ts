import { reconcile, type SetStoreFunction } from "solid-js/store";
import type { GroupNode } from "./tree";

/**
 * Write a freshly built rail into the store without replacing unchanged rows.
 *
 * `buildTree` always allocates new wrappers. `<For>` keys by identity, so
 * without this an OSC title change remounts every card and replays
 * `ws-list-in`. Same `id` as `applyShellSnapshot`: a row that did not change
 * keeps the DOM node it already has.
 */
export function applyRailTree(setTree: SetStoreFunction<GroupNode[]>, next: GroupNode[]): void {
  setTree(reconcile(next, { key: "id" }));
}
