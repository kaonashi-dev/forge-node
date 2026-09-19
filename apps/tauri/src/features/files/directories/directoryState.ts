import { batch, createMemo, createRoot, createSignal, untrack } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { createDirectoryController, type DirectorySnapshot } from "./directoryController";
import { parentPath } from "../../../shared/paths";
import { sameListing } from "../index/fileInvalidation";
export type {
  DirectoryState,
  DirectoryAnswer,
  DirectoryFailure,
  DirectoryRequest,
} from "./directoryController";

const [tree, setTree] = createSignal<DirectorySnapshot["tree"]>(null);
const [state, setState] = createStore<Omit<DirectorySnapshot, "tree">>({
  workspace: null,
  generation: 0,
  directories: Object.create(null) as DirectorySnapshot["directories"],
});
export const directories = createDirectoryController((snapshot) =>
  batch(() =>
    untrack(() => {
      setState("workspace", snapshot.workspace);
      setState("generation", snapshot.generation);
      setTree(snapshot.tree);
      setState("directories", reconcile(snapshot.directories));
    }),
  ),
);
export const directoryTree = tree;
export const directoryStatus = (path: string) => {
  const record = state.directories[path];
  return Object.hasOwn(state.directories, path) ? record : undefined;
};
/** Stale cached children cannot override a newer navigation-index observation. */
export const freshDirectoryTree = createRoot(() =>
  createMemo<DirectorySnapshot["tree"]>((previous) => {
    const current = tree();
    if (!current) return null;
    const next = {
      ...current,
      entries: current.entries.filter((entry) => {
        const parent = directoryStatus(parentPath(entry.path));
        return parent?.status === "loaded" && !parent.stale;
      }),
    };
    return sameListing(previous ?? null, next) ? previous : next;
  }, null),
);
export const directoryError = (path = "") => directoryStatus(path)?.error ?? null;
export const directoryLoading = (path = "") => directoryStatus(path)?.status === "loading";
