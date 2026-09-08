import { describe, expect, it } from "vitest";
import { extensionOf, grammarFor } from "./language";

describe("grammarFor", () => {
  // One place decides what a `.rs` is, and it is not this file.
  it("believes the daemon's hint first", () => {
    expect(grammarFor("rust", "crates/client/src/lib.rs")).toBe("rust");
    expect(grammarFor("tsx", "src/App.tsx")).toBe("tsx");
  });

  // `fs-service`'s table stops at seven languages; a stylesheet is not one.
  it("reads the extension when the daemon had nothing to say", () => {
    expect(grammarFor("", "src/styles.css")).toBe("css");
    expect(grammarFor("", "scripts/init.sh")).toBe("shellscript");
    expect(grammarFor("", ".github/workflows/ci.yml")).toBe("yaml");
  });

  it("knows the names that carry no suffix", () => {
    expect(grammarFor("", "Makefile")).toBe("shellscript");
    expect(grammarFor("", "docker/Dockerfile")).toBe("shellscript");
  });

  // A wrong grammar paints confident, wrong colour over half a file. Grey is
  // the honest answer.
  it("leaves an unknown file unpainted", () => {
    expect(grammarFor("", "assets/icon.png")).toBeNull();
    expect(grammarFor("", "LICENSE")).toBeNull();
    expect(grammarFor("", "bun.lock")).toBeNull();
  });

  it("takes the basename, not the path", () => {
    expect(grammarFor("", "some.rs.dir/notes.md")).toBe("markdown");
  });
});

describe("extensionOf", () => {
  it("lowercases what it finds, and takes the last of several", () => {
    expect(extensionOf("README.MD")).toBe("md");
    expect(extensionOf("pnpm-lock.yaml")).toBe("yaml");
    expect(extensionOf("vite.config.ts")).toBe("ts");
  });

  // `.gitignore` is a dotfile, not a `gitignore` file.
  it("treats a leading dot as part of the name", () => {
    expect(extensionOf(".gitignore")).toBe("");
    expect(extensionOf(".env.local")).toBe("local");
    expect(extensionOf("Makefile")).toBe("");
  });
});
