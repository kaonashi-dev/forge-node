import { describe, expect, it } from "vitest";
import type { Launchable } from "../runtime/types";
import {
  defaultAgentValue,
  parseDefaultAgent,
  resolveDefaultAgent,
  type DefaultAgent,
} from "./defaultAgent";

const PROFILE = "6b7f1a52-6e1c-4a2f-9f3a-0d7c2b8e5a41";

function launchable(over: Partial<Launchable>): Launchable {
  return {
    kind: "agent",
    label: "Claude",
    detail: null,
    provider: "claude",
    profile: null,
    enabled: true,
    key: "claude",
    supports_initial_prompt: true,
    ...over,
  };
}

describe("the default-agent preference", () => {
  // The wire form is shared with the shell (`apps/tauri so a
  // value this shell writes has to survive that parser unchanged.
  it("round-trips every form", () => {
    const cases: DefaultAgent[] = [
      { kind: "ask" },
      { kind: "shell" },
      { kind: "provider", id: "claude" },
      { kind: "profile", id: PROFILE },
    ];
    for (const value of cases) {
      expect(parseDefaultAgent(defaultAgentValue(value))).toEqual(value);
    }
  });

  it("writes the prefixed form a bare id would not match", () => {
    expect(defaultAgentValue({ kind: "provider", id: "claude" })).toBe("provider:claude");
  });

  it("degrades anything unknown to asking", () => {
    for (const value of [
      "",
      "  ",
      "auto",
      "claude", // the bare id this shell used to write
      "provider:",
      "something-new",
      "profile:",
      "profile:not-a-uuid",
      null,
      undefined,
    ]) {
      expect(parseDefaultAgent(value)).toEqual({ kind: "ask" });
    }
  });
});

describe("resolving the preference against the launchables", () => {
  it("launches a provider that is installed", () => {
    expect(resolveDefaultAgent({ kind: "provider", id: "claude" }, [launchable({})])).toEqual({
      kind: "agent",
      provider: "claude",
      profile: null,
    });
  });

  it("launches a profile with its provider", () => {
    const entry = launchable({ key: `profile:${PROFILE}`, profile: PROFILE, label: "Work" });
    expect(resolveDefaultAgent({ kind: "profile", id: PROFILE }, [entry])).toEqual({
      kind: "agent",
      provider: "claude",
      profile: PROFILE,
    });
  });

  // Stale, not fatal: the shortcut opens the palette instead of doing nothing.
  it("falls back to asking when the provider is not installed", () => {
    expect(
      resolveDefaultAgent({ kind: "provider", id: "claude" }, [launchable({ enabled: false })]),
    ).toEqual({ kind: "ask" });
  });

  it("falls back to asking when the profile was deleted", () => {
    expect(resolveDefaultAgent({ kind: "profile", id: PROFILE }, [launchable({})])).toEqual({
      kind: "ask",
    });
  });

  it("passes shell and ask through without consulting the list", () => {
    expect(resolveDefaultAgent({ kind: "shell" }, [])).toEqual({ kind: "shell" });
    expect(resolveDefaultAgent({ kind: "ask" }, [])).toEqual({ kind: "ask" });
  });
});
