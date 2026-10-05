// Built-in PR compose tasks — port of `domain::pr_task`.

import type { WorkspaceDiff } from "../../contracts/workbench";
import { diffTotals } from "../../contracts/workbench";

export type TaskMode = "Draft" | "Implement";

export type PrTask = {
  id: string;
  name: string;
  detail: string;
  mode: TaskMode;
  body: string;
};

const MAX_PROMPT_PATCH = 24 * 1024;

const DESCRIBE_INTRO = `Write the pull request for the uncommitted changes in this checkout, summarised below with their diff. Say what changed and why; do not list the files, the diff already does.

`;

const REVIEW_INTRO = `Read the diff below as a reviewer would before writing anything. If you find something that would block a merge — a bug, a missing test, a leak — say so plainly at the top of the body, under a \`## Concerns\` heading. Then write the pull request for these uncommitted changes.

`;

const DRAFT_HANDOVER = `Do not edit files, commit, push or open the pull request: Forge Node does that once the user has read your text.

Write it the way this team does. Read what the repository says — CONTRIBUTING, AGENTS.md or CLAUDE.md, any commit or PR guide under docs/, \`.github/pull_request_template.md\` — and its recent history: \`git log -n 20\` and \`gh pr list --state merged --limit 5 --json title,body\`. If this repository has too little history to show a convention, read recent merged pull requests in other repositories of the same owner. A convention you already remember for this repository or team counts too; the repository's written rules win.

All the changes go into one commit, so the commit message describes them all, in this repository's commit style. The branch is used only when this checkout is on its default branch; name it the way this repository names branches.

Hand the result over with this command, quoting each value for the shell, and stop:

\`\`\`sh
forgectl pr draft --title '<title>' --branch '<new-branch>' --commit '<commit subject>' [--commit-body '<commit body>'] <<'EOF'
<pull request body in Markdown>
EOF
\`\`\``;

export function builtinTasks(): PrTask[] {
  return [
    {
      id: "describe",
      name: "Describe the change",
      detail:
        "The agent writes the PR and commit text; you edit it, Forge Node commits and opens the PR.",
      mode: "Draft",
      body: DESCRIBE_INTRO + DRAFT_HANDOVER,
    },
    {
      id: "review",
      name: "Review, then describe",
      detail:
        "The agent reviews the diff first, then writes the text; Forge Node commits and opens the PR.",
      mode: "Draft",
      body: REVIEW_INTRO + DRAFT_HANDOVER,
    },
    {
      id: "finish",
      name: "Finish and open the PR",
      detail:
        "The agent reviews, commits in few groups, pushes and opens the PR in the repo's style.",
      mode: "Implement",
      body: `You are working in a git checkout that already has uncommitted changes, summarised below. Take them to an open pull request.

1. Learn how this team writes commits and pull requests before writing either. Read what the repository says — CONTRIBUTING, AGENTS.md or CLAUDE.md, any commit or PR guide under docs/, \`.github/pull_request_template.md\` — then the recent history: \`git log -n 20\` and \`gh pr list --state merged --limit 5 --json title,body\`. If this repository has too little history to show a convention, read recent merged pull requests in other repositories of the same owner (\`gh repo list <owner> --limit 5\`, then \`gh pr list -R <owner>/<repo> --state merged --limit 3 --json title,body\`). A convention you already remember for this repository or team counts too. Where these disagree, the repository's own written rules win.
2. Review the changes as a reviewer would. Fix what would block a merge — a bug, a broken build, a failing test — and nothing else: do not refactor or extend the work. Run the project's checks.
3. If the checked-out branch is the default branch, create a branch for this work first, named the way this repository names its branches. Never commit or push to the default branch.
4. Commit in as few commits as tell the story honestly: related changes together, one commit per independent concern, never one commit per file. Follow the commit convention from step 1.
5. Push with \`git push -u origin HEAD\`, then open the pull request with \`gh pr create --assignee @me\` so it is assigned to whoever \`gh\` is signed in as. Its title and body follow the convention from step 1; fill the repository's template when it has one. Report the pull request URL when you are done.

Do not force-push and do not touch any other branch. If something stops you — a check that fails for reasons outside these changes, a convention you cannot satisfy — stop and say so instead of working around it.`,
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
    return "You edit what the agent writes; Forge Node then commits, pushes and opens the PR.";
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
