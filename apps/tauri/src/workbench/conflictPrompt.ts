import type { ShellSnapshot } from "../runtime/types";
import { defaultAgentFrom, resolveDefaultAgent } from "../settings/defaultAgent";
import type { RebaseState } from "./types";

/**
 * The prompt behind "Resolve with AI", and who can be handed it.
 *
 * There is no second kind of agent here: this is the ordinary
 * `CreateAgentSession` the rail's `+` sends, in the same workspace, with the
 * message already typed — the operation, the branches, which side of the
 * conflict is which, and every unmerged path with what each side did to it.
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
 * One side of a conflict, named twice.
 *
 * `short` goes inline in the path list, where it has to fit next to a path;
 * `long` goes in the orientation block, where it is the whole point.
 */
type Side = { short: string; long: string };

/**
 * Which commit each side of the conflict actually holds.
 *
 * Git's own words for the two sides are positional, not semantic: `--ours` is
 * whatever `HEAD` is at the time. During a **rebase** that is the branch being
 * replayed *onto* — the sequencer checks out `onto` and applies your commits on
 * top — so `ours` is the other branch's code and `theirs` is your own. Merge,
 * cherry-pick and revert keep `ours` on the branch the user is standing on.
 *
 * Getting this backwards is the one mistake that produces a resolution which
 * compiles, passes review at a glance, and silently reverts the commit being
 * replayed, so the prompt states it rather than assuming anyone infers it.
 */
type Sides = {
  /** How the operation is spelled in prose and as a git subcommand. */
  noun: string;
  /** The ref naming the incoming commit, when the operation publishes one. */
  incoming: string | null;
  /** What `--ours`, stage `:2:` and `HEAD` hold. */
  ours: Side;
  /** What `--theirs` and stage `:3:` hold. */
  theirs: Side;
  /** Whether git's "us" and "them" read backwards from the user's intent. */
  inverted: boolean;
};

function sidesFor(state: RebaseState): Sides {
  const here: Side = state.branch
    ? { short: `\`${state.branch}\``, long: "the branch you are standing on" }
    : { short: "the current branch", long: "the branch you are standing on" };

  switch (state.operation ?? "Rebase") {
    case "Rebase":
      return {
        noun: "rebase",
        incoming: "REBASE_HEAD",
        ours: {
          short: state.onto ? `\`${state.onto}\`` : "the upstream side",
          long: state.onto
            ? `\`${state.onto}\`, the branch being rebased onto — not your work`
            : "the branch being rebased onto — not your work",
        },
        theirs: {
          short: "the replayed commit",
          long: state.branch
            ? `the commit of \`${state.branch}\` being replayed — this is your work`
            : "the commit being replayed — this is your work",
        },
        inverted: true,
      };
    case "Merge":
      return {
        noun: "merge",
        incoming: "MERGE_HEAD",
        ours: here,
        theirs: { short: "the incoming branch", long: "the branch being merged in" },
        inverted: false,
      };
    case "CherryPick":
      return {
        noun: "cherry-pick",
        incoming: "CHERRY_PICK_HEAD",
        ours: here,
        theirs: { short: "the picked commit", long: "the commit being cherry-picked" },
        inverted: false,
      };
    case "Revert":
      return {
        noun: "revert",
        incoming: "REVERT_HEAD",
        ours: here,
        theirs: {
          short: "the revert",
          long: "the inverse of the commit being reverted",
        },
        inverted: false,
      };
    default:
      // An operation this build does not know the shape of. The positional
      // truth still holds, so say only that and name no branches.
      return {
        noun: String(state.operation).toLowerCase(),
        incoming: null,
        ours: { short: "`HEAD`", long: "what is already in the working tree" },
        theirs: { short: "the incoming side", long: "the side being applied" },
        inverted: false,
      };
  }
}

/** A `git status` code read with the two sides named, rather than "us"/"them". */
function describeConflict(code: string, sides: Sides): string {
  const ours = sides.ours.short;
  const theirs = sides.theirs.short;
  switch (code) {
    case "UU":
      return "changed on both sides";
    case "AA":
      return "added on both sides, differently";
    case "DD":
      return "deleted on both sides";
    case "AU":
      return `added by ${ours}, absent from ${theirs}`;
    case "UA":
      return `added by ${theirs}, absent from ${ours}`;
    case "DU":
      return `deleted by ${ours}, changed by ${theirs}`;
    case "UD":
      return `changed by ${ours}, deleted by ${theirs}`;
    default:
      return `\`${code}\``;
  }
}

/**
 * The message the agent is launched with.
 *
 * Four things travel with it, and all four are load-bearing.
 *
 * It says **which side is which**, because during a rebase `--ours` is the
 * other branch and a resolution built on the opposite assumption is a silent
 * revert. It points at the **commit being replayed**, because that commit's
 * message is what the conflict is actually about and is the one piece of
 * context an agent with a shell can go read that a diff-only tool cannot. It
 * requires the agent to **stage** what it resolves, because staging is what
 * takes a path off the conflict list the panel re-reads. And it forbids
 * `--continue`, `--abort` and every other way of moving or discarding the
 * replay: finishing is a human gesture, in the panel, after the resolutions
 * have been read.
 */
export function conflictPrompt(state: RebaseState): string {
  const sides = sidesFor(state);
  const count = state.conflicts.length;
  const paths = count === 1 ? "1 unmerged path" : `${count} unmerged paths`;

  const replay =
    state.branch && state.onto
      ? `A ${sides.noun} of \`${state.branch}\` onto \`${state.onto}\``
      : `A ${sides.noun} in this checkout`;
  const step = state.step && state.total ? ` at step ${state.step} of ${state.total}` : "";
  const head = state.head
    ? `\nHEAD is detached at ${state.head} — the replay so far, not the commit that failed to apply.`
    : "";

  const files = state.conflicts
    .map((conflict) => `- \`${conflict.path}\` — ${describeConflict(conflict.code, sides)}`)
    .join("\n");
  const truncated = state.truncated
    ? "\nThat list was capped by Forge Node; `git status` has the rest."
    : "";

  const orientation = sides.inverted
    ? `A ${sides.noun} replays your commits onto the other branch, so git's words for the two sides are the reverse of what they usually mean here:`
    : "The two sides are:";

  const digIn = sides.incoming
    ? [
        "",
        `Read \`git log --oneline -1 ${sides.incoming}\` and \`git show ${sides.incoming}\` before`,
        "deciding anything. That commit's message says what its side was trying to do, which is",
        "usually what the conflict is really about; `git log --oneline -5 HEAD` says the same for",
        "the other side.",
      ].join("\n")
    : "";

  return [
    `Resolve the conflicts that stopped a ${sides.noun} in this checkout, and stop there.`,
    "",
    "## What stopped",
    "",
    `${replay}${step} stopped with ${paths}.${head}`,
    "",
    "## Which side is which",
    "",
    orientation,
    "",
    `- \`--ours\`, stage \`:2:\`, \`HEAD\` — ${sides.ours.long}.`,
    `- \`--theirs\`, stage \`:3:\`${sides.incoming ? `, \`${sides.incoming}\`` : ""} — ${sides.theirs.long}.`,
    digIn,
    "",
    `## Unmerged paths (${count})`,
    "",
    files || "- (none reported)",
    truncated,
    "",
    "## How to resolve",
    "",
    "For each path:",
    "",
    "1. Read the whole file, not just the hunk. A resolution that is right in isolation and",
    "   wrong for the file is the usual failure here.",
    "2. When a hunk is not obvious, run `git checkout --conflict=diff3 -- <path>`: it rewrites",
    "   the markers with the common ancestor between them, so you can see what each side",
    "   *changed* rather than only where the two differ.",
    "3. Keep both sides' intent wherever the code means both. Dropping one side to make the",
    "   file compile is a silent bug, not a resolution.",
    "4. Check the rest of the repo before you commit to a reading — whether a symbol one side",
    "   renamed is still referenced elsewhere is a question `grep` answers.",
    "5. `git add <path>` when it is done. Staging is what takes the path off Forge Node's",
    "   conflict list.",
    "",
    "## Rules",
    "",
    "- Never leave a conflict marker in a file you stage. Verify with `git diff --cached --check`.",
    "- Do not edit files outside the list above, and do not reformat, re-sort imports or",
    "  otherwise tidy the parts of a conflicted file that are not in conflict.",
    "- `git checkout --ours <path>` and `--theirs <path>` take a whole file. That is right for",
    "  an add/delete conflict and wrong for a file both sides edited.",
    "- If both sides changed the same behaviour in ways that cannot both hold, say so and leave",
    "  that path unstaged. A guess there fails silently, and a human is already reading this.",
    "",
    `## Do not finish the ${sides.noun}`,
    "",
    `Do not run \`git ${sides.noun} --continue\`, \`--skip\` or \`--abort\`, and no \`git reset\`,`,
    "`git checkout <branch>`, `git stash`, `git commit --amend`, or any push. Continuing the",
    "replay is a human gesture in Forge Node's Git panel, once someone has read your",
    "resolutions. Leaving it stopped is the point, not an unfinished job.",
    "",
    "## Report back",
    "",
    "Per path, one or two lines: what each side wanted, what you kept, and why. Name anything",
    "you were unsure about. Then run the build or test command that covers the files you",
    "touched and say what it did — including if there was none to run.",
  ]
    .join("\n")
    .replace(/\n{3,}/g, "\n\n");
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
