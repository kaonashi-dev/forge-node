import { describe, expect, it } from "vitest";
import { parseEnv } from "./profileEnv";

describe("parseEnv", () => {
  it("reads NAME=value lines and ignores blank ones", () => {
    expect(parseEnv("A=1\n\n  B=two  ")).toEqual({
      pairs: [
        ["A", "1"],
        ["B", "two"],
      ],
      error: null,
    });
  });

  // A value is whatever follows the first `=`; splitting on every one would
  // truncate a connection string at its first parameter.
  it("keeps everything after the first equals sign", () => {
    expect(parseEnv("URL=postgres://h/db?a=1&b=2").pairs).toEqual([
      ["URL", "postgres://h/db?a=1&b=2"],
    ]);
  });

  it("rejects a line that is not an assignment", () => {
    expect(parseEnv("JUST_A_NAME").error).toBe('"JUST_A_NAME" is not NAME=value.');
    expect(parseEnv("=novalue").error).toBe('"=novalue" is not NAME=value.');
  });

  it("rejects a name the shell could not carry", () => {
    expect(parseEnv("2BAD=x").error).toBe('"2BAD" is not a variable name.');
    expect(parseEnv("has-dash=x").error).toBe('"has-dash" is not a variable name.');
  });

  // These fail *after* the process starts, as a terminal that renders wrong
  // rather than as an error anyone connects to the profile screen.
  it("refuses the variables Forge owns", () => {
    for (const name of ["TERM", "TERMINFO", "COLORTERM", "FORGE_SESSION_ID", "FORGE_WORKSPACE"]) {
      expect(parseEnv(`${name}=x`).error).toBe(`${name} is Forge Node's to set, not a profile's.`);
    }
  });

  it("accepts an empty value", () => {
    expect(parseEnv("EMPTY=")).toEqual({ pairs: [["EMPTY", ""]], error: null });
  });

  it("is empty for empty input", () => {
    expect(parseEnv("")).toEqual({ pairs: [], error: null });
  });
});
