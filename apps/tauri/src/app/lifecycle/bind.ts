import type { UnlistenFn } from "@tauri-apps/api/event";

type Binding = UnlistenFn | UnlistenFn[];

export type Binder = () => Binding | Promise<Binding>;

function normalize(binding: Binding): UnlistenFn[] {
  return Array.isArray(binding) ? binding : [binding];
}

export async function bindAll(binders: Binder[]): Promise<UnlistenFn[]> {
  const settled = await Promise.allSettled(binders.map((bind) => Promise.resolve().then(bind)));
  const unlisteners = settled.flatMap((result) =>
    result.status === "fulfilled" ? normalize(result.value) : [],
  );
  const failed = settled.find((result) => result.status === "rejected");
  if (failed) {
    for (const unlisten of unlisteners) unlisten();
    throw failed.reason;
  }
  return unlisteners;
}
