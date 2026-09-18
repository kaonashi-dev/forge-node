// Mutation completion is independent of directory reads and generic surface errors.
export type PathOperationResult = {
  operation_id: string;
  workspace: string;
  kind: "create" | "rename" | "delete";
  from?: string | null;
  to?: string | null;
  success: boolean;
  error?: string | null;
  uncertain: boolean;
  directory?: boolean;
};
export type PathOperation = Pick<PathOperationResult, "workspace" | "kind" | "directory"> & {
  from?: string;
  to?: string;
};

export class PathOperationError extends Error {
  readonly uncertain: boolean;
  constructor(readonly result: PathOperationResult) {
    super(result.error ?? "The path operation failed.");
    this.name = "PathOperationError";
    this.uncertain = result.uncertain;
  }
}

export const OPERATION_TIMEOUT_MS = 30_000;
const MAX_OPERATIONS = 128;
let sequence = 0;
const identity = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
export function requestIdentity(): string {
  return `${identity}-${++sequence}`;
}

type Pending = {
  dispatched: boolean;
  operation: PathOperation & { operation_id: string };
  promise: Promise<PathOperationResult>;
  resolve: (result: PathOperationResult) => void;
  reject: (error: PathOperationError) => void;
  timer: ReturnType<typeof setTimeout>;
};

export function createOperationTracker() {
  const pending = new Map<string, Pending>();
  const uncertain = new Map<string, PathOperation & { operation_id: string }>();
  const listeners = new Set<(result: PathOperationResult) => void>();
  const uncertainListeners = new Set<(operation: PathOperation) => void>();
  let overflowed = false;

  function remember(operation: PathOperation & { operation_id: string }): void {
    if (!uncertain.has(operation.operation_id) && uncertain.size >= MAX_OPERATIONS) {
      uncertain.delete(uncertain.keys().next().value!);
      overflowed = true;
    }
    uncertain.set(operation.operation_id, operation);
  }

  function settle(result: PathOperationResult): void {
    const item = pending.get(result.operation_id);
    const operation = item?.operation ?? uncertain.get(result.operation_id);
    if (
      !operation ||
      operation.workspace !== result.workspace ||
      operation.kind !== result.kind ||
      operation.from !== (result.from ?? undefined) ||
      operation.to !== (result.to ?? undefined)
    )
      return;
    if (item) {
      clearTimeout(item.timer);
      pending.delete(result.operation_id);
    }
    if (result.uncertain) {
      remember(operation);
      for (const listener of uncertainListeners) listener(operation);
    } else uncertain.delete(result.operation_id);
    if (result.success && !result.uncertain) {
      const confirmed = {
        ...result,
        from: result.from ?? undefined,
        to: result.to ?? undefined,
        directory: operation.directory,
      };
      item?.resolve(confirmed);
      for (const listener of listeners) listener(confirmed);
    } else item?.reject(new PathOperationError(result));
  }

  function run(
    operation: PathOperation,
    send: (id: string) => Promise<void>,
  ): Promise<PathOperationResult> {
    for (const item of pending.values()) {
      if (
        item.operation.workspace === operation.workspace &&
        item.operation.kind === operation.kind &&
        item.operation.from === operation.from &&
        item.operation.to === operation.to &&
        item.operation.directory === operation.directory
      )
        return item.promise;
    }
    const operation_id = requestIdentity();
    const descriptor = { ...operation, operation_id };
    if (pending.size >= MAX_OPERATIONS)
      return Promise.reject(
        new PathOperationError({
          ...descriptor,
          success: false,
          uncertain: false,
          error: "Too many path operations are pending.",
        }),
      );
    let resolve!: Pending["resolve"];
    let reject!: Pending["reject"];
    const promise = new Promise<PathOperationResult>((yes, no) => {
      resolve = yes;
      reject = no;
    });
    const timer = setTimeout(
      () =>
        settle({
          ...descriptor,
          success: false,
          uncertain: true,
          error: "The operation timed out. Its outcome is unknown; refresh before trying again.",
        }),
      OPERATION_TIMEOUT_MS,
    );
    pending.set(operation_id, {
      operation: descriptor,
      promise,
      resolve,
      reject,
      timer,
      dispatched: false,
    });
    void Promise.resolve()
      .then(() => {
        const item = pending.get(operation_id);
        if (item) {
          item.dispatched = true;
          return send(operation_id);
        }
      })
      .catch((error: unknown) => {
        // Host invoke rejection means the command never entered its work queue.
        settle({
          ...descriptor,
          success: false,
          uncertain: false,
          error: error instanceof Error ? error.message : String(error),
        });
      });
    return promise;
  }

  return {
    run,
    settle,
    onUncertain(listener: (operation: PathOperation) => void): () => void {
      uncertainListeners.add(listener);
      return () => {
        uncertainListeners.delete(listener);
      };
    },
    subscribe(listener: (result: PathOperationResult) => void): () => void {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    disconnect(): void {
      for (const { operation, dispatched } of pending.values())
        settle({
          ...operation,
          success: false,
          uncertain: dispatched,
          error: dispatched
            ? "Disconnected before confirmation. The operation may have completed."
            : "Disconnected before the operation was sent.",
        });
    },
    reconcile(read: (operation: PathOperation) => void, rescan: () => void): void {
      if (overflowed) rescan();
      overflowed = false;
      for (const operation of uncertain.values()) read(operation);
    },
  };
}

export const pathOperations = createOperationTracker();
export const subscribePathOperations = pathOperations.subscribe;
