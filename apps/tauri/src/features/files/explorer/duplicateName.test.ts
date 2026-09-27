import { describe, expect, it } from "vitest";
import { duplicateFileName } from "./duplicateName";

const free = () => false;

describe("duplicateFileName", () => {
  it("inserts copy before the last extension", () => {
    expect(duplicateFileName("notes.md", free)).toBe("notes copy.md");
    expect(duplicateFileName("vite.config.ts", free)).toBe("vite.config copy.ts");
    expect(duplicateFileName("Makefile", free)).toBe("Makefile copy");
  });

  it("keeps a dotfile whole, including a dotted env name", () => {
    expect(duplicateFileName(".env.staging", free)).toBe(".env.staging copy");
    expect(duplicateFileName(".gitignore", free)).toBe(".gitignore copy");
  });

  it("increments a name that is already a copy", () => {
    expect(duplicateFileName("notes copy.md", free)).toBe("notes copy 2.md");
    expect(duplicateFileName("notes copy 2.md", free)).toBe("notes copy 3.md");
    expect(duplicateFileName(".env.staging copy", free)).toBe(".env.staging copy 2");
  });

  it("skips names that are already in the folder", () => {
    const taken = new Set(["notes copy.md", "notes copy 2.md"]);
    expect(duplicateFileName("notes.md", (candidate) => taken.has(candidate))).toBe(
      "notes copy 3.md",
    );
  });

  it("gives up once every numbered copy through 100 is taken", () => {
    expect(duplicateFileName("notes copy 100.md", free)).toBeNull();
    expect(duplicateFileName("notes.md", () => true)).toBeNull();
  });
});
