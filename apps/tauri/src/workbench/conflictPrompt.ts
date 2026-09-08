import type { ShellSnapshot } from "../runtime/types";
import { defaultAgentFrom, resolveDefaultAgent } from "../settings/defaultAgent";
import type { RebaseState } from "./types";

/**
 * The prompt behind "Resolve with AI", and who can be handed it.
 *
 * There is no second kind of agent here: this is the ordinary
 * `CreateAgentSession` the rail's `+` sends, in the same workspace, with the
 * message already typed — the operation, the branch being replayed, the commit
 * it stopped on, and every unmerged path with what each side did to it.
 */

/** What a two-letter `git status` code means, in words. */
export function conflictLabel(code: string): string {
  switch (code) {
    case "UU":
      return "both modified";
    case "AA":
      return "both added";
    case "DD":
      return "both deleted";
    case "AU":
      return "added by us, modified by them";
    case "UA":
      return "modified by us, added by them";
    case "DU":
      return "deleted by us, modified by them";
    case "UD":
      return "modified by us, deleted by them";
    default:
      return code;
  }
}

/**
 * The message the agent is launched with.
 *
 * Two rules travel with it, and both are load-bearing. It must stage what it
 * resolves, because staging is what takes a path off the conflict list the
 * panel re-reads. And it must not run `--continue` or `--abort`: continuing
 * the replay stays a human gesture, in the panel, after the resolutions have
 * been read.
 */
export function conflictPrompt(state: RebaseState): string {
  const operation = state.operation ?? "rebase";
  const onto = state.branch && state.onto ? `${state.branch} onto ${state.onto}` : "this checkout";
  const step = state.step && state.total ? ` (step ${state.step} of ${state.total})` : "";
  const head = state.head ? ` It stopped on commit ${state.head}.` : "";

  const files = state.conflicts
    .map((conflict) => `- ${conflict.path} — ${conflictLabel(conflict.code)}`)
    .join("\n");
  const truncated = state.truncated
    ? "\nThe list above was capped; run `git status` for the rest.\n"
    : "";

  return [
    `A ${operation} of ${onto}${step} has stopped on conflicts.${head}`,
    "",
    `Unmerged paths (${state.conflicts.length}):`,
    files || "- (none reported)",
    truncated,
    "Resolve each one in the working tree, keeping both sides' intent where that",
    "is what the code means. Then stage each file you have resolved with",
    "`git add <path>`.",
    "",
    "Do not run `git rebase --continue`, `git rebase --abort`, `git merge --continue`",
    "or `git cherry-pick --continue`. Continuing the replay is done by hand in",
    "Forge Node's Git panel once the resolutions have been read.",
  ].join("\n");
}

/** Who "Resolve with AI" would launch, or why it cannot. */
export type Resolver =
  | { kind: "agent"; provider: string; profile: string | null }
  | { kind: "unavailable"; reason: string };

/**
 * Pick the agent to hand the prompt to.
 *
 * The configured default first — `ui.default_agent`, carrying its profile if
 * it names one — because that is the agent the user already chose and the one
 * `⌘⇧A` starts. An agent that cannot be handed a prompt is refused *here*,
 * with a sentence naming what to change, rather than at the daemon where the
 * refusal would arrive as a launch that did nothing.
 *
 * With no agent configured — `ask`, the shipped value, or a terminal — there
 * is a default only when there is nothing to choose between: one installed,
 * prompt-capable agent is used, and two or more is a guess this refuses to
 * make. Silently picking the first of several is how a button ends up starting
 * an agent the user does not use.
 */
export function resolverFor(store: ShellSnapshot): Resolver {
  const preferred = resolveDefaultAgent(defaultAgentFrom(store.app_state), store.launchables);
  if (preferred.kind === "agent") {
    if (acceptsPrompt(store, preferred.provider)) {
      return { kind: "agent", provider: preferred.provider, profile: preferred.profile };
    }
    return {
      kind: "unavailable",
      reason: `${preferred.provider} cannot be started with a prompt — pick another default agent in Settings → Agents.`,
    };
  }

  // Providers only: two profiles of one agent are not two choices about which
  // agent to start, and picking a profile nobody asked for is its own guess.
  const capable = store.launchables.filter(
    (launchable) =>
      launchable.kind === "agent" &&
      launchable.enabled &&
      launchable.profile === null &&
      launchable.provider !== null &&
      acceptsPrompt(store, launchable.provider),
  );
  if (capable.length === 1 && capable[0].provider) {
    return { kind: "agent", provider: capable[0].provider, profile: capable[0].profile };
  }
  if (capable.length > 1) {
    return {
      kind: "unavailable",
      reason: "No default agent is set — choose one in Settings → Agents.",
    };
  }
  return {
    kind: "unavailable",
    reason: "No installed agent can be started with a prompt.",
  };
}

/** Whether the provider's descriptor says it takes an initial prompt. */
function acceptsPrompt(store: ShellSnapshot, providerId: string): boolean {
  const provider = store.providers.find((item) => item.descriptor?.id === providerId);
  return provider?.descriptor?.capabilities?.supports_initial_prompt === true;
}
