// Owns bounded directory observations, read identities and reconciliation debt.
// The Solid adapter publishes snapshots; filesystem IO stays in the host reader.
import type { FileEntry, FileTree } from "../../../contracts/workbench";
import { sameListing } from "../index/fileInvalidation";
import { parentPath, retargetPath, validatePath } from "../../../shared/paths";
import { requestIdentity, type PathOperationResult } from "../operations/operations";

export type DirectoryRequest = {
  workspace: string;
  path: string;
  request_id: string;
  generation: number;
};
export type DirectoryAnswer = DirectoryRequest & { entries: FileEntry[]; truncated: boolean };
export type DirectoryFailure = DirectoryRequest & { message: string };
export type DirectoryState = {
  entries: FileEntry[];
  status: "unloaded" | "loading" | "loaded" | "error";
  error: string | null;
  stale: boolean;
  truncated: boolean;
  version: number;
  request_id: string | null;
};
export type DirectorySnapshot = {
  workspace: string | null;
  generation: number;
  tree: FileTree | null;
  directories: Record<string, DirectoryState>;
};
export const DIRECTORY_TIMEOUT_MS = 15_000;
export const DIRECTORY_DEBOUNCE_MS = 100;
const MAX_DIRECTORIES = 256;
const MAX_ENTRIES = 20_000;
const MAX_CONCURRENT = 4;
const MAX_RETRIES = 2;

type RecordState = DirectoryState & {
  loaded: boolean;
  readingVersion: number;
  retries: number;
  timer?: ReturnType<typeof setTimeout>;
  retry?: ReturnType<typeof setTimeout>;
};

export function createDirectoryController(publish: (snapshot: DirectorySnapshot) => void) {
  let workspace: string | null = null;
  let generation = 0;
  let connected = true;
  let tree: FileTree | null = null;
  let reader: ((request: DirectoryRequest) => Promise<void>) | undefined;
  let debounce: ReturnType<typeof setTimeout> | undefined;
  const records = new Map<string, RecordState>();
  let interests = new Set<string>();
  let active = 0;
  let cacheTruncated = false;
  let treeDirty = true;

  function emit(): void {
    if (treeDirty) {
      const entries = [...records.values()].flatMap((record) => record.entries);
      const byPath = new Map(entries.map((entry) => [entry.path, entry]));
      const next: FileTree | null =
        workspace && records.size
          ? {
              workspace_id: workspace,
              entries: [...byPath.values()].sort((a, b) =>
                a.path < b.path ? -1 : a.path > b.path ? 1 : 0,
              ),
              loadedDirectories: [...records]
                .filter(([, record]) => record.loaded)
                .map(([path]) => path)
                .sort(),
              truncated: cacheTruncated || [...records.values()].some((record) => record.truncated),
            }
          : null;
      if (!next || !sameListing(tree, next)) tree = next;
      treeDirty = false;
    }
    publish({
      workspace,
      generation,
      tree,
      directories: Object.fromEntries(
        [...records].map(([path, record]) => [
          path,
          {
            entries: record.entries,
            status: record.status,
            error: record.error,
            stale: record.stale,
            truncated: record.truncated,
            version: record.version,
            request_id: record.request_id,
          },
        ]),
      ),
    });
  }

  function cancel(record: RecordState): void {
    clearTimeout(record.timer);
    clearTimeout(record.retry);
    record.retry = undefined;
    if (record.request_id) active--;
    record.request_id = null;
  }

  function get(path: string): RecordState | undefined {
    let record = records.get(path);
    if (record) return record;
    if (path !== "" && validatePath(path)) return;
    if (records.size >= MAX_DIRECTORIES) {
      cacheTruncated = true;
      treeDirty = true;
      const victim = [...records].find(([key, value]) => !interests.has(key) && !value.request_id);
      if (!victim) return;
      cancel(victim[1]);
      records.delete(victim[0]);
    }
    record = {
      entries: [],
      status: "unloaded",
      error: null,
      stale: true,
      truncated: false,
      version: 0,
      request_id: null,
      readingVersion: 0,
      retries: 0,
      loaded: false,
    };
    records.set(path, record);
    treeDirty = true;
    return record;
  }

  function schedule(): void {
    if (debounce !== undefined || !connected) return;
    // The deadline stays anchored to the first change, even under continuous writes.
    debounce = setTimeout(() => {
      debounce = undefined;
      drain();
    }, DIRECTORY_DEBOUNCE_MS);
  }

  function matching(request: DirectoryRequest): RecordState | undefined {
    if (workspace !== request.workspace || generation !== request.generation) return;
    const record = records.get(request.path);
    return record?.request_id === request.request_id ? record : undefined;
  }

  function fail(failure: DirectoryFailure): void {
    const record = matching(failure);
    if (!record) return;
    cancel(record);
    record.status = "error";
    record.error = failure.message;
    record.stale = true;
    if (record.retries < MAX_RETRIES && interests.has(failure.path) && connected) {
      const delay = 500 * 2 ** record.retries++;
      record.retry = setTimeout(() => {
        record.retry = undefined;
        record.status = "unloaded";
        schedule();
      }, delay);
    }
    emit();
    schedule();
  }

  function drain(): void {
    if (!workspace || !connected || !reader) return;
    for (const path of interests) {
      if (active >= MAX_CONCURRENT) break;
      const record = records.get(path);
      if (
        !record ||
        !record.stale ||
        record.request_id ||
        record.retry ||
        (record.status === "error" && record.retries >= MAX_RETRIES)
      )
        continue;
      const request = { workspace, path, generation, request_id: requestIdentity() };
      record.request_id = request.request_id;
      record.readingVersion = record.version;
      record.status = "loading";
      record.error = null;
      active++;
      record.timer = setTimeout(
        () => fail({ ...request, message: "Directory read timed out. Retry to refresh." }),
        DIRECTORY_TIMEOUT_MS,
      );
      void Promise.resolve()
        .then(() => {
          if (matching(request)) return reader!(request);
        })
        .catch((error: unknown) =>
          fail({ ...request, message: error instanceof Error ? error.message : String(error) }),
        );
    }
    emit();
  }

  function invalidate(source: string, paths?: readonly string[]): void {
    if (source !== workspace) return;
    for (const path of paths ?? [...records.keys()]) {
      const record = get(path);
      if (!record) continue;
      record.version++;
      record.stale = true;
      record.retries = 0;
    }
    emit();
    schedule();
  }

  function removeSubtree(path: string): void {
    for (const [key, record] of records) {
      if (key === path || key.startsWith(`${path}/`)) {
        cancel(record);
        records.delete(key);
        treeDirty = true;
      }
    }
  }

  function accept(answer: DirectoryAnswer): void {
    const record = matching(answer);
    if (!record) return;
    cancel(record);
    const invalidated = record.readingVersion !== record.version;
    let retained = [...records.values()].reduce(
      (sum, item) => sum + (item === record ? 0 : item.entries.length),
      0,
    );
    // Closed observations must not prevent a newly opened folder from using the budget.
    for (const [path, cached] of records) {
      if (retained + answer.entries.length <= MAX_ENTRIES) break;
      if (cached === record || interests.has(path) || cached.request_id) continue;
      retained -= cached.entries.length;
      cancel(cached);
      records.delete(path);
    }
    const remaining = Math.max(0, MAX_ENTRIES - retained);
    const entries = answer.entries
      .filter((entry) => parentPath(entry.path) === answer.path)
      .slice(0, remaining);
    const partial = answer.truncated || entries.length !== answer.entries.length;
    if (!partial && !invalidated) {
      const directories = new Set(
        entries.filter((entry) => entry.kind === "Directory").map((entry) => entry.path),
      );
      for (const path of records.keys()) {
        const relative = answer.path
          ? path.startsWith(`${answer.path}/`)
            ? path.slice(answer.path.length + 1)
            : null
          : path;
        if (!relative) continue;
        const child = (answer.path ? `${answer.path}/` : "") + relative.split("/")[0];
        if (!directories.has(child)) removeSubtree(child);
      }
    }
    const byPath = new Map(
      (partial || invalidated ? record.entries : []).map((entry) => [entry.path, entry]),
    );
    for (const entry of entries) byPath.set(entry.path, entry);
    record.entries = [...byPath.values()].slice(0, remaining);
    record.truncated = partial;
    record.status = "loaded";
    record.loaded = true;
    record.error = null;
    record.stale = invalidated;
    record.retries = 0;
    treeDirty = true;
    emit();
    schedule();
  }

  function ensure(source: string, path = "", force = false): void {
    if (source !== workspace) return;
    const record = get(path);
    if (!record) return;
    const reopened = !interests.has(path);
    interests.add(path);
    if (reopened) {
      record.version++;
      record.stale = true;
    }
    if (force) {
      record.version++;
      record.stale = true;
    }
    if (record.status === "error" || force) {
      clearTimeout(record.retry);
      record.retry = undefined;
      record.retries = 0;
      record.stale = true;
    }
    emit();
    schedule();
  }

  return {
    accept,
    fail,
    ensure,
    invalidate,
    configure(read: (request: DirectoryRequest) => Promise<void>): void {
      reader = read;
    },
    focus(source: string | null): void {
      if (workspace === source) return;
      for (const record of records.values()) cancel(record);
      clearTimeout(debounce);
      debounce = undefined;
      records.clear();
      interests.clear();
      workspace = source;
      generation++;
      tree = null;
      cacheTruncated = false;
      treeDirty = true;
      emit();
    },
    setInterests(source: string, paths: readonly string[]): void {
      if (source !== workspace) return;
      const previous = interests;
      interests = new Set(paths.slice(0, MAX_DIRECTORIES));
      for (const path of interests) {
        const record = get(path);
        if (record && !previous.has(path)) {
          clearTimeout(record.retry);
          record.retry = undefined;
          record.retries = 0;
          record.version++;
          record.stale = true;
        }
      }
      emit();
      schedule();
    },
    changed(source: string, path: string): void {
      if (!path) {
        invalidate(source);
        return;
      }
      const affected = [parentPath(path)];
      for (const known of records.keys())
        if (known === path || known.startsWith(`${path}/`)) affected.push(known);
      invalidate(source, affected);
    },
    operation(result: PathOperationResult, directory = false): void {
      if (result.workspace !== workspace || !result.success || result.uncertain) return;
      const affected = new Set<string>();
      // Ancestor reads may predate newly created intermediate directories too.
      for (const path of [result.from, result.to]) {
        if (!path) continue;
        const immediate = parentPath(path);
        affected.add(immediate);
        for (const [key, parent] of records) {
          if (key !== "" && key !== immediate && !immediate.startsWith(`${key}/`)) continue;
          if (parent.request_id) {
            cancel(parent);
            parent.status = parent.loaded ? "loaded" : "unloaded";
            affected.add(key);
          }
        }
      }
      treeDirty = true;
      const moved: FileEntry[] = [];
      for (const record of records.values()) {
        record.entries = record.entries.filter((entry) => {
          if (
            !result.from ||
            (entry.path !== result.from && !entry.path.startsWith(`${result.from}/`))
          )
            return true;
          if (result.to)
            moved.push({ ...entry, path: retargetPath(entry.path, result.from, result.to) });
          return false;
        });
      }
      if (result.from) removeSubtree(result.from);
      if (result.to && !moved.some((entry) => entry.path === result.to))
        moved.push({ path: result.to, kind: directory ? "Directory" : "File", ignored: false });
      let entryCount = [...records.values()].reduce(
        (sum, record) => sum + record.entries.length,
        0,
      );
      for (const entry of moved) {
        const parent = get(parentPath(entry.path));
        if (!parent) continue;
        const existing = parent.entries.findIndex((item) => item.path === entry.path);
        if (existing >= 0)
          parent.entries = parent.entries.map((item, index) => (index === existing ? entry : item));
        else {
          if (entryCount >= MAX_ENTRIES) {
            parent.truncated = true;
            if (!parent.entries.length) continue;
            parent.entries = parent.entries.slice(1);
            entryCount--;
          }
          parent.entries = [...parent.entries, entry];
          entryCount++;
        }
      }
      invalidate(result.workspace, [...affected]);
    },
    connection(value: boolean): void {
      connected = value;
      generation++;
      clearTimeout(debounce);
      debounce = undefined;
      for (const record of records.values()) {
        cancel(record);
        record.version++;
        record.stale = true;
        record.retries = 0;
        if (record.status === "loading") record.status = "unloaded";
      }
      emit();
      if (connected) schedule();
    },
    dispose(): void {
      clearTimeout(debounce);
      for (const record of records.values()) cancel(record);
      records.clear();
      interests.clear();
    },
  };
}
