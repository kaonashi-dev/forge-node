import { sessionIsActive, type Session } from "./types";

export function createEditorAutosaveSync(
  send: (session: string, enabled: boolean) => Promise<void>,
) {
  const applied = new Map<string, boolean>();
  return (sessions: Session[], enabled: boolean, reconnect = false): void => {
    if (reconnect) applied.clear();
    const live = new Set<string>();
    for (const session of sessions) {
      if (session.kind !== "Editor" || !sessionIsActive(session.state)) continue;
      live.add(session.id);
      if (applied.get(session.id) === enabled) continue;
      applied.set(session.id, enabled);
      void send(session.id, enabled).catch(() => {
        if (applied.get(session.id) === enabled) applied.delete(session.id);
      });
    }
    for (const id of applied.keys()) if (!live.has(id)) applied.delete(id);
  };
}
