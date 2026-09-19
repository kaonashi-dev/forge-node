// How one line of a step's transcript reads.

/** The classes a stream line is painted in; the CSS carries the colours. */
export type StreamTone = "error" | "command" | "edit" | "message" | "thinking" | "muted";

export type StreamItem =
  | { kind: "message"; lines: string[] }
  | { kind: "thinking"; lines: string[] }
  | { kind: "command"; command: string; result: string | undefined }
  | { kind: "edit"; lines: string[] }
  | { kind: "error"; lines: string[] }
  | { kind: "meta"; lines: string[] }
  | { kind: "gap" };

export type StreamTurn = {
  kind: "turn";
  started: string;
  completed: string | undefined;
  items: StreamItem[];
};

export type StreamBlock = StreamTurn | StreamItem;

type LineKind =
  | "turn-start"
  | "turn-end"
  | "message"
  | "thinking"
  | "command"
  | "result"
  | "edit"
  | "error"
  | "meta";

type MergeKind = "message" | "thinking" | "edit" | "error" | "meta";

/**
 * The tone one stream line gets, from the shape the summariser gave it.
 *
 * Reading a stream is scanning for two things — what it ran, and what went
 * wrong — so those two are the ones that are not muted. Matching on the
 * rendered prefix keeps this on the GUI side of the line: the vocabulary is
 * `agents::summarize_stream_line`'s, and nothing here parses the provider's
 * JSON a second time.
 */
export function streamLineTone(line: string): StreamTone {
  switch (classifyStreamLine(line)) {
    case "error":
      return "error";
    case "result":
      return streamResultFailed(line) ? "error" : "muted";
    case "command":
      return "command";
    case "edit":
      return "edit";
    case "message":
      return "message";
    case "thinking":
      return "thinking";
    default:
      return "muted";
  }
}

/** Walk once; keep the original strings. Holes stay holes. */
export function groupStreamLines(lines: Array<string | undefined>): StreamBlock[] {
  const out: StreamBlock[] = [];
  const top: StreamItem[] = [];
  let turn: StreamTurn | undefined;

  const items = (): StreamItem[] => (turn !== undefined ? turn.items : top);

  const flushTop = (): void => {
    if (top.length === 0) return;
    for (const item of top) out.push(item);
    top.length = 0;
  };

  const closeTurn = (completed?: string): void => {
    if (turn === undefined) return;
    if (completed !== undefined) turn.completed = completed;
    out.push(turn);
    turn = undefined;
  };

  for (const line of lines) {
    if (line === undefined) {
      const list = items();
      const last = list[list.length - 1];
      if (last?.kind !== "gap") list.push({ kind: "gap" });
      continue;
    }

    const kind = classifyStreamLine(line);
    if (kind === "turn-start") {
      closeTurn();
      flushTop();
      turn = { kind: "turn", started: line, completed: undefined, items: [] };
      continue;
    }
    if (kind === "turn-end") {
      if (turn !== undefined) {
        closeTurn(line);
      } else {
        pushMerged(items(), "meta", line);
      }
      continue;
    }
    if (kind === "result") {
      const list = items();
      const last = list[list.length - 1];
      if (last?.kind === "command" && last.result === undefined) {
        last.result = line;
        continue;
      }
      pushMerged(list, streamResultFailed(line) ? "error" : "meta", line);
      continue;
    }
    if (kind === "command") {
      items().push({ kind: "command", command: line, result: undefined });
      continue;
    }
    pushMerged(items(), kind, line);
  }

  closeTurn();
  flushTop();
  return out;
}

export function streamResultFailed(result: string): boolean {
  return result.trimStart().startsWith("⤷ FAILED");
}

/** Prefix stripped so a message or thought reads as prose, not as a log line. */
export function streamLineBody(line: string): string {
  const trimmed = line.trimStart();
  if (trimmed.startsWith("assistant  ")) return stripThinkingLabel(trimmed.slice(11));
  if (trimmed.startsWith("assistant:")) {
    return stripThinkingLabel(trimmed.slice("assistant:".length).trimStart());
  }
  if (trimmed.startsWith("thinking  ")) return trimmed.slice(10);
  if (trimmed.startsWith("thinking:")) return trimmed.slice("thinking:".length).trimStart();
  return trimmed;
}

export function turnHeadline(completed: string | undefined): string {
  if (completed === undefined) return "Turn";
  const trimmed = completed.trimStart();
  if (trimmed.startsWith("turn completed")) {
    const rest = trimmed.slice("turn completed".length).trim();
    return rest.length > 0 ? `Turn  ${rest}` : "Turn";
  }
  return "Turn";
}

function classifyStreamLine(line: string): LineKind {
  const trimmed = line.trimStart();
  // Leftover provider JSON: a bug elsewhere. Do not parse it here (C4).
  if (trimmed.startsWith("{")) return "meta";
  if (trimmed.startsWith("turn started")) return "turn-start";
  if (trimmed.startsWith("turn completed")) return "turn-end";
  if (trimmed.startsWith("error")) return "error";
  if (trimmed.startsWith("$")) return "command";
  if (trimmed.startsWith("⤷")) return "result";
  if (trimmed.startsWith("✎")) return "edit";
  if (trimmed.startsWith("thinking")) return "thinking";
  if (trimmed.startsWith("assistant")) {
    const body = assistantPayload(trimmed);
    if (body.startsWith("thinking:") || body.startsWith("thinking ")) return "thinking";
    if (body.startsWith("→")) return "meta";
    return "message";
  }
  return "meta";
}

function assistantPayload(trimmed: string): string {
  if (trimmed.startsWith("assistant  ")) return trimmed.slice(11);
  if (trimmed.startsWith("assistant:")) return trimmed.slice("assistant:".length).trimStart();
  return trimmed.slice("assistant".length).trimStart();
}

function stripThinkingLabel(body: string): string {
  if (body.startsWith("thinking:")) return body.slice("thinking:".length).trimStart();
  if (body.startsWith("thinking  ")) return body.slice(10);
  return body;
}

function pushMerged(items: StreamItem[], kind: MergeKind, line: string): void {
  const last = items[items.length - 1];
  if (last !== undefined && last.kind === kind) {
    last.lines.push(line);
    return;
  }
  items.push({ kind, lines: [line] });
}

/**
 * What a card prints for a job's latest line.
 *
 * Raw JSON is the provider's stream; a card is not the log. The daemon is
 * supposed to have summarised `last_line` already — if it did not, the
 * summary field is the thing a person can read.
 */
export function jobPreviewLine(lastLine: string | null | undefined, fallback: string): string {
  const line = lastLine?.trim() ?? "";
  if (line === "") return fallback;
  if (line.startsWith("{") || line.startsWith("[")) return fallback;
  return line;
}
