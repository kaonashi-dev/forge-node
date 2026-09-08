// Built-in pull-request review recipes — port of `domain::pr_review` (§16.9).
//
// The wording is the daemon's, kept here so the panel can show the prompt
// before it is sent and remember an edited one; the Rust tests assert the same
// ids and the same two guards this file does.
//
// The recipe is only half of what makes a review a review. The other half is
// the provider's own read-only mode, applied by the launch — see
// `providerReviews` in `runtime/types`.

import type { ProviderInfo, PullRequest } from "../runtime/types";
import { providerName, providerReviewMode, providerReviews } from "../runtime/types";

export type ReviewRecipe = {
  id: string;
  name: string;
  detail: string;
  /** Empty for {@link CUSTOM_RECIPE}, which is the user's own text and nothing else. */
  body: string;
};

/** The id of the recipe that carries no wording of its own. */
export const CUSTOM_RECIPE = "custom";

/** `app_state` keys: the last recipe, the last agent, and the custom wording. */
export const REVIEW_RECIPE_KEY = "ui.pr_review.recipe";
export const REVIEW_PROVIDER_KEY = "ui.pr_review.provider";
export const REVIEW_CUSTOM_KEY = "ui.pr_review.custom";

/**
 * How the prompt tells the agent to read the pull request.
 *
 * `gh` rather than a checkout: it reaches a fork's head as easily as a branch
 * of the same repository, and it reads without touching the working tree the
 * user left behind. `{number}` and `{repo}` are the two things the model
 * cannot derive.
 */
const READ_INSTRUCTIONS = `Read the pull request first, with the GitHub CLI:

- \`gh pr view {number} --repo {repo} --comments\`
- \`gh pr diff {number} --repo {repo}\`

You are a reader here. Do not edit, stage, commit, checkout or push anything, and do not post to GitHub — no \`gh pr review\`, no \`gh pr comment\`. Write your findings in this session and stop; the person watching decides what reaches the pull request.`;

export function builtinRecipes(): ReviewRecipe[] {
  return [
    {
      id: "standard",
      name: "Review the change",
      detail: "Correctness, missing tests, and anything that would block a merge.",
      body: `Review this pull request the way a careful colleague would before approving it.

Lead with a verdict — approve, approve with comments, or request changes — and one sentence saying why. Then list what you found, most serious first, each as \`path:line\` plus what breaks and when. Say plainly when a change needs a test it does not have.

Do not summarise the diff back: the person reading you can already see it. Report only what they would have missed.`,
    },
    {
      id: "deep",
      name: "Deep review",
      detail: "Reads the surrounding code too — design, edge cases, cost.",
      body: `Review this pull request against the code around it, not only against itself. Open the files it touches and their callers.

Look for: a case the change does not handle, an invariant it breaks somewhere else, an error path that silently swallows a failure, work that grows with the input where the caller assumed it would not, and a public contract that changed without its callers changing.

Lead with a verdict and one sentence. Then the findings, most serious first, each as \`path:line\` with the concrete input or state that makes it go wrong. A finding you cannot make concrete is a question, not a finding — say so.`,
    },
    {
      id: "security",
      name: "Security review",
      detail: "Input handling, secrets, authorization, injection, resource limits.",
      body: `Review this pull request for security defects only.

Look for: input that reaches a shell, a query or a path without being bounded or escaped; a secret, token or key in the diff or in a log line; an authorization check that is missing or applied after the effect; an allocation sized by something the caller controls; and a dependency added or bumped without a reason in the description.

Report each finding as \`path:line\`, what an attacker controls, and what they get. Rank by what the attacker gets, not by how easy it was to spot. If you find nothing, say so — do not pad the list.`,
    },
    {
      id: CUSTOM_RECIPE,
      name: "Custom",
      detail: "Your own wording, remembered for next time.",
      body: "",
    },
  ];
}

/**
 * The recipe with `id`, or the first built-in when there is no such recipe.
 *
 * Never `null`: a remembered id that no longer exists is a stale preference,
 * not an error worth putting on screen.
 */
export function recipeOrDefault(id: string | null, recipes = builtinRecipes()): ReviewRecipe {
  return recipes.find((recipe) => recipe.id === id) ?? recipes[0];
}

/**
 * The whole prompt: the recipe, how to read the pull request, what the pull
 * request is, and whatever the user added.
 *
 * The identity block is small on purpose. The agent is about to read the title,
 * the body and the diff itself through `gh`; repeating them here would spend
 * context on a second, staler copy.
 */
export function composeReviewPrompt(recipe: ReviewRecipe, pr: PullRequest, extra: string): string {
  let prompt = recipe.body.trim();
  if (prompt) prompt += "\n\n";

  prompt += READ_INSTRUCTIONS.replaceAll("{number}", String(pr.number)).replaceAll(
    "{repo}",
    pr.repository,
  );

  prompt += "\n\n## The pull request\n\n";
  prompt += `- ${pr.host}/${pr.repository} #${pr.number}\n`;
  prompt += `- ${pr.title}\n`;
  prompt += `- opened by ${pr.author}\n`;
  prompt += `- ${pr.head_ref} → ${pr.base_ref}\n`;
  prompt += `- ${pr.changed_files} file${pr.changed_files === 1 ? "" : "s"}, +${pr.additions} −${pr.deletions}\n`;
  if (pr.is_draft) prompt += "- still a draft\n";

  const trimmed = extra.trim();
  if (trimmed) {
    prompt += "\n## Also\n\n";
    prompt += trimmed;
    prompt += "\n";
  }

  return prompt;
}

export type ReviewAgent = {
  /** Provider id, as the launch names it. */
  id: string;
  name: string;
  /** What the provider calls its read-only mode — "plan mode", "ask mode". */
  mode: string;
};

/**
 * The agents that can actually be sent to review, in the snapshot's order.
 *
 * Both halves of the launch are checked, because the daemon refuses the whole
 * request when either spelling is missing: offering a row that would be turned
 * down is a button that does nothing.
 */
export function reviewAgents(providers: readonly ProviderInfo[]): ReviewAgent[] {
  return providers.filter(providerReviews).map((provider) => ({
    id: provider.descriptor?.id ?? "",
    name: providerName(provider),
    mode: providerReviewMode(provider) ?? "read-only",
  }));
}

/**
 * Which agent a review should start with.
 *
 * The remembered choice when it is still installed, else the first that can
 * review, else nothing — a preference pointing at an agent that was
 * uninstalled is stale, not a reason to refuse to start.
 */
export function preferredAgent(
  agents: readonly ReviewAgent[],
  remembered: string | null,
): ReviewAgent | null {
  return agents.find((agent) => agent.id === remembered) ?? agents[0] ?? null;
}

/** The line under the button: what pressing it will do, or why it cannot. */
export function launchHint(
  agent: ReviewAgent | null,
  recipe: ReviewRecipe,
  custom: string,
  running: boolean,
): string {
  if (!agent) {
    return "No installed agent has a read-only mode. Install Claude, Codex, OpenCode, or Cursor.";
  }
  if (running) return "A review is already running for this pull request.";
  if (recipe.id === CUSTOM_RECIPE && !custom.trim()) {
    return "Write what to ask for, and the review will start with it.";
  }
  return `${agent.name} reads the pull request with \`gh\` in ${agent.mode} — it cannot write to this checkout.`;
}

/** Whether the launch button can fire. */
export function readyToLaunch(
  agent: ReviewAgent | null,
  recipe: ReviewRecipe,
  custom: string,
  running: boolean,
): boolean {
  if (!agent || running) return false;
  return recipe.id !== CUSTOM_RECIPE || custom.trim().length > 0;
}

/**
 * Parse an RFC-3339 instant to epoch milliseconds.
 *
 * Never `localeCompare` or `>` on these strings. The daemon writes them with
 * `time`'s RFC-3339, whose subsecond part is as long as it needs to be, and
 * `Date.prototype.toISOString` always writes exactly three digits — so
 * `"…42.123Z"` sorts *after* `"…42.123456Z"`, and a session created a third of
 * a millisecond after the launch would read as older than it.
 */
function instant(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * The session a launch produced, adopted by looking for what appeared after it.
 *
 * The runtime command channel is one-way — it is drained on the thread that
 * also carries keystrokes — so the id of the session it just created does not
 * come back. Matching on "an agent session in this checkout, created after we
 * asked" is what the PR compose tab already does; `since` is what keeps it from
 * adopting a session that was already there.
 */
export function adoptLaunched<
  T extends {
    id: string;
    workspace_id: string;
    created_at: string;
    agent_provider_id: string | null;
  },
>(sessions: readonly T[], workspace: string, since: string): T | null {
  const floor = instant(since);
  return sessions
    .filter(
      (session) =>
        session.workspace_id === workspace &&
        session.agent_provider_id !== null &&
        instant(session.created_at) >= floor,
    )
    .reduce<T | null>(
      (newest, session) =>
        !newest || instant(session.created_at) > instant(newest.created_at) ? session : newest,
      null,
    );
}

/**
 * The checkout a review of `pr` should run in.
 *
 * Any checkout of the right repository will do — the agent reads the pull
 * request through `gh`, not through the working tree — so the choice is about
 * not surprising the reader: the checkout already on screen when it belongs to
 * that project, then the project's main one, then whatever it has. `null` when
 * the pull request came from a repository Forge does not know, which is the one
 * case a review cannot start: `gh` needs a directory whose remote is the
 * repository to be authenticated against it.
 */
export function reviewWorkspace<T extends { id: string; project_id: string; kind: string }>(
  workspaces: readonly T[],
  project: string | null,
  focused: string | null,
): T | null {
  if (project === null) return null;
  const owned = workspaces.filter((workspace) => workspace.project_id === project);
  return (
    owned.find((workspace) => workspace.id === focused) ??
    owned.find((workspace) => workspace.kind === "Main") ??
    owned[0] ??
    null
  );
}
