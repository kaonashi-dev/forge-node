import { describe, expect, it } from "vitest";
import {
  groupStreamLines,
  jobPreviewLine,
  streamLineBody,
  streamLineTone,
  turnHeadline,
} from "./stream";

describe("stream line tone", () => {
  // What it ran and what went wrong are the two things a reader scans for.
  it("lifts commands and failures out of the muted body", () => {
    expect(streamLineTone("$ cargo test")).toBe("command");
    expect(streamLineTone("error: no such file")).toBe("error");
    expect(streamLineTone("  ⤷ FAILED (exit 1)")).toBe("error");
    expect(streamLineTone("✎ crates/daemon/src/core.rs")).toBe("edit");
    expect(streamLineTone("assistant: running the gate")).toBe("message");
    expect(streamLineTone("assistant  I'll write the spec")).toBe("message");
    expect(streamLineTone("thinking  checking the theme tokens")).toBe("thinking");
    expect(streamLineTone("thinking: weighing the options")).toBe("thinking");
    expect(streamLineTone('{"type":"unparsed"}')).toBe("muted");
  });
});

describe("groupStreamLines", () => {
  it("groups a Codex-shaped turn into conversation and terminal blocks", () => {
    expect(
      groupStreamLines([
        "turn started",
        "thinking  checking the theme tokens",
        "assistant  I'll write the spec",
        "assistant  then run the gate",
        "$ rg theme crates",
        "  ⤷ ok (0)  b",
        "✎ edited  features.json (update)",
        "turn completed  in 10 · out 2 · reasoning 1",
      ]),
    ).toEqual([
      {
        kind: "turn",
        started: "turn started",
        completed: "turn completed  in 10 · out 2 · reasoning 1",
        items: [
          { kind: "thinking", lines: ["thinking  checking the theme tokens"] },
          {
            kind: "message",
            lines: ["assistant  I'll write the spec", "assistant  then run the gate"],
          },
          { kind: "command", command: "$ rg theme crates", result: "  ⤷ ok (0)  b" },
          { kind: "edit", lines: ["✎ edited  features.json (update)"] },
        ],
      },
    ]);
  });

  it("keeps holes as gaps and does not invent text", () => {
    expect(groupStreamLines(["assistant  hi", undefined, undefined, "assistant  there"])).toEqual([
      { kind: "message", lines: ["assistant  hi"] },
      { kind: "gap" },
      { kind: "message", lines: ["assistant  there"] },
    ]);
  });

  it("treats leftover JSON as muted meta, without parsing it", () => {
    const leftover = '{ "type": "item.completed" }';
    expect(groupStreamLines([leftover])).toEqual([{ kind: "meta", lines: [leftover] }]);
    expect(streamLineTone(leftover)).toBe("muted");
  });

  it("closes a turn at the next turn started when no completed line arrived", () => {
    expect(
      groupStreamLines(["turn started", "assistant  a", "turn started", "assistant  b"]),
    ).toEqual([
      {
        kind: "turn",
        started: "turn started",
        completed: undefined,
        items: [{ kind: "message", lines: ["assistant  a"] }],
      },
      {
        kind: "turn",
        started: "turn started",
        completed: undefined,
        items: [{ kind: "message", lines: ["assistant  b"] }],
      },
    ]);
  });

  it("attaches a FAILED result to the command that produced it", () => {
    expect(groupStreamLines(["$ cargo test", "  ⤷ FAILED (1)  panicked"])).toEqual([
      { kind: "command", command: "$ cargo test", result: "  ⤷ FAILED (1)  panicked" },
    ]);
  });

  it("groups thread, todo, search and tool lines as meta", () => {
    expect(
      groupStreamLines([
        "thread started  t1",
        "todo  2 item(s)  write the spec",
        "⌕ search  theme tokens",
        "→ mcp/read",
      ]),
    ).toEqual([
      {
        kind: "meta",
        lines: [
          "thread started  t1",
          "todo  2 item(s)  write the spec",
          "⌕ search  theme tokens",
          "→ mcp/read",
        ],
      },
    ]);
  });

  it("keeps top-level meta on either side of a turn", () => {
    expect(
      groupStreamLines([
        "thread started  t1",
        "turn started",
        "assistant  hi",
        "turn completed  in 1 · out 1 · reasoning 0",
        "todo  1 item(s)  next",
      ]),
    ).toEqual([
      { kind: "meta", lines: ["thread started  t1"] },
      {
        kind: "turn",
        started: "turn started",
        completed: "turn completed  in 1 · out 1 · reasoning 0",
        items: [{ kind: "message", lines: ["assistant  hi"] }],
      },
      { kind: "meta", lines: ["todo  1 item(s)  next"] },
    ]);
  });
});

describe("stream line display", () => {
  it("strips the summariser prefix so assistant text reads as prose", () => {
    expect(streamLineBody("assistant  I'll write the spec")).toBe("I'll write the spec");
    expect(streamLineBody("assistant: running the gate")).toBe("running the gate");
    expect(streamLineBody("thinking  checking the theme tokens")).toBe("checking the theme tokens");
    expect(streamLineBody("assistant  thinking: weighing the options")).toBe(
      "weighing the options",
    );
  });

  it("names a completed turn by its usage tail", () => {
    expect(turnHeadline(undefined)).toBe("Turn");
    expect(turnHeadline("turn completed  in 10 · out 2 · reasoning 1")).toBe(
      "Turn  in 10 · out 2 · reasoning 1",
    );
  });
});

describe("job preview line", () => {
  // A card is not the log: leftover provider JSON is unreadable at 96 chars.
  it("hides leftover JSON and empty lines in favour of the summary", () => {
    expect(jobPreviewLine('{"type":"turn.completed","usage":{}}', "spec finished")).toBe(
      "spec finished",
    );
    expect(jobPreviewLine('[{"type":"item.started"}]', "working")).toBe("working");
    expect(jobPreviewLine("   ", "waiting")).toBe("waiting");
    expect(jobPreviewLine(null, "waiting")).toBe("waiting");
  });

  it("keeps a summarised line", () => {
    expect(jobPreviewLine("assistant  writing the spec", "working")).toBe(
      "assistant  writing the spec",
    );
    expect(jobPreviewLine("$ cargo test", "")).toBe("$ cargo test");
  });
});
