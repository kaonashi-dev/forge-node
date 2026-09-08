import { describe, expect, it } from "vitest";
import { DEFAULT_LANG_ICON, LANG_ICON_NAMES, langIconFor } from "./langIcons";

/*
 * The vendored set, read off disk by the bundler rather than by `node:fs`:
 * this app deliberately has no `@types/node` (see `vite-env.d.ts`), and a glob
 * is resolved at transform time, so the keys are the file names.
 */
const VARIANTS = {
  dark: import.meta.glob("../../../public/icons/lang/dark/*.svg"),
  light: import.meta.glob("../../../public/icons/lang/light/*.svg"),
};

const svgNames = (variant: "dark" | "light") =>
  Object.keys(VARIANTS[variant])
    .map((path) => path.slice(path.lastIndexOf("/") + 1, -".svg".length))
    .sort();

describe("the vendored set", () => {
  /**
   * The guard the whole table rests on: a name with no file renders an empty
   * box and a file no name reaches is dead weight in the bundle. Neither shows
   * up in a type check, so it has to show up here.
   */
  it("has one file per name, in both variants", () => {
    const names = [...LANG_ICON_NAMES].sort();
    expect(svgNames("dark")).toEqual(names);
    expect(svgNames("light")).toEqual(names);
  });

  it("names no icon twice", () => {
    expect(new Set(LANG_ICON_NAMES).size).toBe(LANG_ICON_NAMES.length);
  });
});

describe("langIconFor", () => {
  it("reads the extension", () => {
    expect(langIconFor("main.rs")).toBe("rust");
    expect(langIconFor("store.ts")).toBe("typescript");
    expect(langIconFor("App.tsx")).toBe("tsx");
    expect(langIconFor("setup.py")).toBe("python");
    expect(langIconFor("style.scss")).toBe("sass");
    expect(langIconFor("run.fish")).toBe("terminal");
  });

  it("takes the last segment of a path", () => {
    expect(langIconFor("crates/daemon/src/core.rs")).toBe("rust");
    expect(langIconFor("./deep/name.with.dots.json")).toBe("json");
  });

  it("is case-insensitive", () => {
    expect(langIconFor("Makefile")).toBe("makefile");
    expect(langIconFor("README.MD")).toBe("markdown");
    expect(langIconFor("Dockerfile.ci")).toBe("docker");
  });

  it("lets a whole name beat the extension", () => {
    expect(langIconFor("Cargo.toml")).toBe("rust");
    expect(langIconFor("other.toml")).toBe("toml");
    expect(langIconFor("package.json")).toBe("npm");
    expect(langIconFor("tsconfig.json")).toBe("typescript");
    expect(langIconFor("openapi.yaml")).toBe("open_api");
  });

  it("lets a compound suffix beat the extension", () => {
    expect(langIconFor("prFilters.test.ts")).toBe("ts_test");
    expect(langIconFor("App.spec.tsx")).toBe("tsx_test");
    expect(langIconFor("types.d.ts")).toBe("typescript");
    expect(langIconFor("plain.ts")).toBe("typescript");
  });

  it("reads a config file from its front, where the name is", () => {
    expect(langIconFor(".env.local")).toBe("env");
    expect(langIconFor("jest.config.mjs")).toBe("jest");
    expect(langIconFor("next.config.ts")).toBe("nextjs");
    expect(langIconFor(".eslintrc.json")).toBe("eslint");
  });

  it("treats a dotfile as a name and not as an extension", () => {
    expect(langIconFor(".gitignore")).toBe("ignored");
    expect(langIconFor(".dockerignore")).toBe("ignored");
    expect(langIconFor(".gitattributes")).toBe("vcs");
    expect(langIconFor(".hushlogin")).toBe(DEFAULT_LANG_ICON);
  });

  it("falls back rather than guessing", () => {
    expect(langIconFor("LICENSE")).toBe(DEFAULT_LANG_ICON);
    expect(langIconFor("data.msgpack")).toBe(DEFAULT_LANG_ICON);
    expect(langIconFor("")).toBe(DEFAULT_LANG_ICON);
  });
});
