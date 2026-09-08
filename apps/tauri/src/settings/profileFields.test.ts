import { describe, expect, it } from "vitest";
import type { AgentProfile, ProfileField } from "../runtime/types";
import {
  applyProfileFields,
  newAgentProfile,
  profileFieldKey,
  splitProfileFields,
} from "./profileFields";

const fields: ProfileField[] = [
  {
    label: "Config directory",
    help: "Separate account",
    effect: { Env: { name: "CODEX_HOME", is_directory: true } },
  },
  {
    label: "Config profile",
    help: "Named Codex profile",
    effect: { Flag: { flag: "--profile" } },
  },
];

function profile(): AgentProfile {
  return {
    id: "00000000-0000-7000-8000-000000000001",
    provider_id: "codex",
    name: "Review",
    executable: null,
    args: ["--sandbox", "workspace-write", "--profile", "review"],
    env: [
      ["RUST_LOG", "info"],
      ["CODEX_HOME", "/tmp/codex-review"],
    ],
    created_at: "2026-09-05T00:00:00Z",
  };
}

describe("profile fields", () => {
  it("creates a profile with the supplied stable id", () => {
    expect(
      newAgentProfile("claude", "00000000-0000-7000-8000-000000000002", "2026-09-05T00:00:00Z"),
    ).toMatchObject({
      id: "00000000-0000-7000-8000-000000000002",
      provider_id: "claude",
      created_at: "2026-09-05T00:00:00Z",
    });
  });

  it("lifts declared values out of the advanced lists", () => {
    expect(splitProfileFields(profile(), fields)).toEqual({
      values: {
        [profileFieldKey(fields[0])]: "/tmp/codex-review",
        [profileFieldKey(fields[1])]: "review",
      },
      args: ["--sandbox", "workspace-write"],
      env: [["RUST_LOG", "info"]],
    });
  });

  it("rebuilds declared fields without losing advanced values", () => {
    const base = profile();
    const result = applyProfileFields(base, fields, {
      [profileFieldKey(fields[0])]: "/tmp/codex-work",
      [profileFieldKey(fields[1])]: "work",
    });
    expect(result.args).toEqual(["--sandbox", "workspace-write", "--profile", "work"]);
    expect(result.env).toEqual([
      ["RUST_LOG", "info"],
      ["CODEX_HOME", "/tmp/codex-work"],
    ]);
  });
});
