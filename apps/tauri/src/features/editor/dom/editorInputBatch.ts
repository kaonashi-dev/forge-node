import type { EditorInputEvent } from "../../../contracts/editor";

const INPUT_BATCH_MS = 8;

export function editorInputBatch(
  session: string,
  send: (session: string, events: EditorInputEvent[]) => Promise<void>,
) {
  let outbox: EditorInputEvent[] = [];
  let timer: ReturnType<typeof setTimeout> | undefined;

  function flush(): void {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
    if (outbox.length === 0) return;
    const events = outbox;
    outbox = [];
    void send(session, events).catch(() => undefined);
  }

  return {
    push(event: EditorInputEvent): void {
      outbox.push(event);
      timer ??= setTimeout(flush, INPUT_BATCH_MS);
    },
    flush,
  };
}
