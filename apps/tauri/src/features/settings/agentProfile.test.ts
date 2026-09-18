import { describe, expect, it } from "vitest";
import { formatArgs, newAgentProfile, normalizeArgs, parseArgs } from "./agentProfile";

describe("newAgentProfile", () => {
  it("creates a profile with the supplied stable id", () => {
    expect(
      newAgentProfile("claude", "00000000-0000-7000-8000-000000000002", "2026-09-05T00:00:00Z"),
    ).toEqual({
      id: "00000000-0000-7000-8000-000000000002",
      provider_id: "claude",
      name: "",
      executable: null,
      config_dir: null,
      args: [],
      created_at: "2026-09-05T00:00:00Z",
    });
  });
});

describe("parseArgs", () => {
  // The reason this module exists: a whole flag reads as one line, and the
  // launch has to pass it as the two arguments the CLI expects.
  it("splits a flag and its value written on one line", () => {
    expect(parseArgs("--model opus")).toEqual({ args: ["--model", "opus"], error: null });
  });

  it("treats every run of whitespace, newlines included, as one separator", () => {
    expect(parseArgs("  --model   opus\n--verbose \n").args).toEqual([
      "--model",
      "opus",
      "--verbose",
    ]);
  });

  it("is empty for empty input", () => {
    expect(parseArgs("   \n  ")).toEqual({ args: [], error: null });
  });

  it("keeps a quoted argument whole", () => {
    expect(parseArgs('--append-system-prompt "be terse, always"').args).toEqual([
      "--append-system-prompt",
      "be terse, always",
    ]);
    expect(parseArgs("--dir 'My Projects'").args).toEqual(["--dir", "My Projects"]);
    // Quotes group, they do not have to wrap the whole argument.
    expect(parseArgs('--dir=" spaced "').args).toEqual(["--dir= spaced "]);
  });

  // No shell runs these, so the characters a shell would eat stay put.
  it("passes shell metacharacters through untouched", () => {
    expect(parseArgs("--prompt '$HOME/*; rm -rf'").args).toEqual(["--prompt", "$HOME/*; rm -rf"]);
  });

  it("escapes with a backslash outside and inside double quotes", () => {
    expect(parseArgs('--dir My\\ Projects "a \\" quote"').args).toEqual([
      "--dir",
      "My Projects",
      'a " quote',
    ]);
    // Single quotes are literal, as in every shell.
    expect(parseArgs("'C:\\\\path'").args).toEqual(["C:\\\\path"]);
  });

  it("reports the two ways the text can be unfinished", () => {
    expect(parseArgs('--prompt "unclosed').error).toBe('An opening " is never closed.');
    expect(parseArgs("--prompt \\").error).toBe("A trailing backslash escapes nothing.");
  });
});

describe("formatArgs", () => {
  it("puts each flag and the values that follow it on one line", () => {
    expect(formatArgs(["--model", "opus"])).toBe("--model opus");
    expect(formatArgs(["--verbose", "--model", "opus", "--dir", "src", "tests"])).toBe(
      "--verbose\n--model opus\n--dir src tests",
    );
  });

  // Quotes are for arguments that really need them, and they get a line to
  // themselves rather than sitting in the middle of a readable one.
  it("quotes only what contains whitespace, on its own line", () => {
    expect(formatArgs(["--append-system-prompt", "be terse, always", "--verbose"])).toBe(
      '--append-system-prompt\n"be terse, always"\n--verbose',
    );
  });

  // Every saved profile has to survive being reopened and saved again, which
  // is exactly parse(format(args)) === args.
  it("round-trips whatever a launch could carry", () => {
    for (const args of [
      ["--model", "opus"],
      ["--append-system-prompt", "be terse, always"],
      ["--dir", "My Projects"],
      ["--prompt", 'say "hi"'],
      ["--path", "C:\\Users\\me"],
      ["--empty", ""],
      ["$HOME/*; rm -rf"],
      ["--flag", "a", "b", "--other"],
    ]) {
      expect(parseArgs(formatArgs(args)).args).toEqual(args);
    }
  });
});

describe("normalizeArgs", () => {
  // What the previous editor saved when a whole flag was typed on one line:
  // one argument with a space in it, which no CLI reads as a flag and a value.
  it("splits a flag that swallowed its value", () => {
    expect(normalizeArgs(["--model claude-opus-4-8"])).toEqual(["--model", "claude-opus-4-8"]);
    expect(formatArgs(normalizeArgs(["--model claude-opus-4-8"]))).toBe("--model claude-opus-4-8");
  });

  it("leaves alone what is one argument on purpose", () => {
    // `--flag=value with a space` is a single argument to a GNU-style parser.
    expect(normalizeArgs(["--prompt=say hello"])).toEqual(["--prompt=say hello"]);
    // A value is not a flag: splitting it would change what the agent is told.
    expect(normalizeArgs(["--prompt", "say hello"])).toEqual(["--prompt", "say hello"]);
    expect(normalizeArgs(["--model", "opus"])).toEqual(["--model", "opus"]);
  });
});
