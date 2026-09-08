// Built-in PR compose tasks — port of `domain::pr_task`.

import type { WorkspaceDiff } from "./types";
import { diffTotals } from "./types";

export type TaskMode = "Draft" | "Implement";

export type PrTask = {
  id: string;
  name: string;
  detail: string;
  mode: TaskMode;
  body: string;
};

const MAX_PROMPT_PATCH = 24 * 1024;

export function builtinTasks(): PrTask[] {
  return [
    {
      id: "describe",
      name: "Describe the change",
      detail: "The agent writes the title and body; Forge Node opens the PR.",
      mode: "Draft",
      body: `Read the diff below and write a pull request for it.

Answer with the title on the first line, then a blank line, then the body in Markdown. Say what changed and why; do not list the files, the diff already does. Do not write anything else — no preamble, no closing remark.`,
    },
    {
      id: "review",
      name: "Review, then describe",
      detail: "The agent reads the diff critically first, then writes the PR.",
      mode: "Draft",
      body: `Read the diff below as a reviewer would. If you find something that would block a merge — a bug, a missing test, a leak — say so plainly at the top of the body, under a \`## Concerns\` heading.

Then write the pull request: title on the first line, a blank line, then the body in Markdown.`,
    },
    {
      id: "finish",
      name: "Finish and open the PR",
      detail: "The agent works in this checkout, commits, pushes and opens the PR.",
      mode: "Implement",
      body: `You are working in a git checkout that already has uncommitted changes, summarised below.

Finish the work: make it build and pass its tests, then commit it, push the branch, and open a pull request with \`gh\`. Report the pull request URL when you are done. Do not force-push and do not touch any branch other than the one checked out here.`,
    },
  ];
}

export function taskOrDefault(id: string | null, tasks = builtinTasks()): PrTask {
  return tasks.find((task) => task.id === id) ?? tasks[0];
}

export function taskAction(mode: TaskMode): string {
  return mode === "Draft" ? "Draft with agent" : "Run agent";
}

export function forgeOpensThePr(mode: TaskMode): boolean {
  return mode === "Draft";
}

export function launchHint(
  task: PrTask,
  diff: WorkspaceDiff | null,
  launched: boolean,
  awaiting: boolean,
  error: string | null,
  loading: boolean,
): string {
  if (launched || awaiting) return "An agent is already running for this checkout.";
  if (error) return error;
  if (loading) return "";
  if (!diff || diff.files.length === 0) return "Nothing uncommitted here.";
  if (forgeOpensThePr(task.mode)) {
    return "Forge Node pushes the branch and opens the PR with what the agent writes.";
  }
  return "The agent works in this checkout and opens the PR itself.";
}

export function diffSummary(diff: WorkspaceDiff): string {
  const { additions, deletions } = diffTotals(diff);
  const files = diff.files.length;
  const plural = files === 1 ? "file" : "files";
  return `${files} ${plural} · +${additions} −${deletions}`;
}

export function composePrompt(task: PrTask, diff: WorkspaceDiff, extra: string): string {
  let prompt = task.body;

  prompt += "\n\n## The change\n\n";
  if (diff.branch) prompt += `Branch \`${diff.branch}\`, `;
  prompt += `${diffSummary(diff)}.\n`;
  for (const file of diff.files) {
    prompt += `\n- \`${file.path}\` (${file.status}, +${file.additions} −${file.deletions})`;
  }
  prompt += "\n";

  if (task.mode === "Draft") {
    let patch = "";
    let truncated = diff.truncated;
    for (const file of diff.files) {
      if (patch.length + file.patch.length > MAX_PROMPT_PATCH) {
        truncated = true;
        break;
      }
      patch += file.patch;
    }
    if (patch) {
      prompt += "\n## The diff\n\n```diff\n";
      prompt += patch;
      prompt += "\n```\n";
    }
    if (truncated) {
      prompt +=
        "\nThe diff above is incomplete: some files were left out for length. Describe what you can see and say that it is partial.\n";
    }
  }

  const trimmed = extra.trim();
  if (trimmed) {
    prompt += "\n## Also\n\n";
    prompt += trimmed;
    prompt += "\n";
  }

  return prompt;
}

export function diffIsEmpty(diff: WorkspaceDiff | null): boolean {
  return !diff || diff.files.length === 0;
}
