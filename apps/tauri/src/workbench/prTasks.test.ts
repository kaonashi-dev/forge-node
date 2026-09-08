import { describe, expect, it } from "vitest";
import { builtinTasks, composePrompt, diffSummary, launchHint, taskOrDefault } from "./prTasks";
import type { WorkspaceDiff } from "./types";

function sampleDiff(): WorkspaceDiff {
  return {
    workspace_id: "w1",
    branch: "feature/x",
    files: [
      {
        path: "src/main.rs",
        status: "Modified",
        additions: 3,
        deletions: 1,
        patch: "diff --git a/src/main.rs\n+fn main() {}\n",
        binary: false,
        truncated: false,
      },
    ],
    truncated: false,
  };
}

describe("prTasks", () => {
  it("ships three built-in tasks with distinct ids", () => {
    const tasks = builtinTasks();
    expect(tasks).toHaveLength(3);
    expect(new Set(tasks.map((task) => task.id)).size).toBe(3);
  });

  it("falls back to the first task for unknown ids", () => {
    expect(taskOrDefault("describe").id).toBe("describe");
    expect(taskOrDefault("missing").id).toBe(builtinTasks()[0].id);
  });

  it("summarises a diff in one line", () => {
    expect(diffSummary(sampleDiff())).toBe("1 file · +3 −1");
  });

  it("builds a prompt with change context and optional extra", () => {
    const task = taskOrDefault("describe");
    const prompt = composePrompt(task, sampleDiff(), "Ship it Tuesday.");
    expect(prompt).toContain("## The change");
    expect(prompt).toContain("feature/x");
    expect(prompt).toContain("## Also");
    expect(prompt).toContain("Ship it Tuesday.");
  });

  it("describes what each launch mode will do", () => {
    const draft = taskOrDefault("describe");
    const implement = taskOrDefault("finish");
    expect(launchHint(draft, sampleDiff(), false, false, null, false)).toContain(
      "Forge Node pushes",
    );
    expect(launchHint(implement, sampleDiff(), false, false, null, false)).toContain(
      "opens the PR itself",
    );
    expect(launchHint(draft, sampleDiff(), true, false, null, false)).toContain("already running");
  });
});
