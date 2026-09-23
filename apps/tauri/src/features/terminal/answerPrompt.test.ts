import { describe, expect, it } from "vitest";
import type { WireRow } from "../../contracts/terminal";
import { logicalLines, readPrompt, samePrompt } from "./answerPrompt";

const row = (text: string, wrapped = false): WireRow => ({
  w: wrapped,
  r: text ? [[text, [...text].length, -1, -2, 0]] : [],
});
const screen = (...lines: string[]) => lines.map((line) => row(line));

describe("readPrompt", () => {
  it("reads a boxed permission menu and its question", () => {
    const prompt = readPrompt(
      screen(
        "╭──────────────────────────────╮",
        "│ Bash command                 │",
        "│   scripts/dev check          │",
        "│                              │",
        "│ Do you want to proceed?      │",
        "│ ❯ 1. Yes                     │",
        "│   2. Yes, and don't ask again │",
        "│   3. No, and tell me (esc)   │",
        "╰──────────────────────────────╯",
        "",
      ),
    );
    expect(prompt?.question).toBe("Do you want to proceed?");
    expect(prompt?.choices).toEqual([
      { digit: "1", label: "Yes" },
      { digit: "2", label: "Yes, and don't ask again" },
      { digit: "3", label: "No, and tell me" },
    ]);
  });

  it("takes a menu under a key-hint footer, marked with a different cursor", () => {
    const prompt = readPrompt(
      screen(
        "Would you like to run the following command?",
        "",
        "› 1. Yes, proceed (y)",
        "  2. No (esc)",
        "",
        "  Press enter to confirm or esc to cancel",
      ),
    );
    expect(prompt?.question).toBe("Would you like to run the following command?");
    expect(prompt?.choices.map((choice) => choice.label)).toEqual(["Yes, proceed", "No"]);
  });

  it("allows an option's description between two options", () => {
    const prompt = readPrompt(
      screen("Pick one", "❯ 1. Fast", "     skips the tests", "  2. Safe", "     runs everything"),
    );
    expect(prompt?.choices.map((choice) => choice.digit)).toEqual(["1", "2"]);
  });

  // A numbered list in an agent's answer is not a menu: nothing marks a cursor.
  it("does not turn an ordinary numbered list into choices", () => {
    const prompt = readPrompt(
      screen("Here is the plan:", "1. Read the file", "2. Edit it", "", "Shall I start?", ">"),
    );
    expect(prompt?.choices).toEqual([]);
    expect(prompt?.question).toBe("Shall I start?");
  });

  it("refuses a menu that does not start at one", () => {
    const prompt = readPrompt(screen("❯ 2. Later", "  3. Never"));
    expect(prompt?.choices).toEqual([]);
  });

  it("refuses a menu buried under more than a short footer", () => {
    const prompt = readPrompt(screen("Continue?", "❯ 1. Yes", "  2. No", "a", "b", "c", "d", "e"));
    expect(prompt?.choices).toEqual([]);
  });

  it("answers null for a blank screen and no question when nothing asks", () => {
    expect(readPrompt(screen("", "  ", "╰────╯"))).toBeNull();
    expect(readPrompt(screen("compiling…", "done"))?.question).toBeNull();
  });

  it("joins a soft-wrapped question before reading it", () => {
    const rows = [
      row("Should I also update the ", true),
      row("snapshots?"),
      row("❯ 1. Yes"),
      row("  2. No"),
    ];
    expect(logicalLines(rows)[0]).toBe("Should I also update the snapshots?");
    expect(readPrompt(rows)?.question).toBe("Should I also update the snapshots?");
  });
});

describe("samePrompt", () => {
  it("compares by content", () => {
    const a = { question: "Go?", choices: [{ digit: "1", label: "Yes" }] };
    expect(samePrompt(a, { question: "Go?", choices: [{ digit: "1", label: "Yes" }] })).toBe(true);
    expect(samePrompt(a, { question: "Go?", choices: [] })).toBe(false);
    expect(samePrompt(null, null)).toBe(true);
  });
});
