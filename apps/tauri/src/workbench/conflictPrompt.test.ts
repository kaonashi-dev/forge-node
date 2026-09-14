import { describe, expect, it } from "vitest";
import { emptySnapshot } from "../store/forgeStore";
import type { Launchable, ProviderInfo, ShellSnapshot } from "../runtime/types";
import type { RebaseState } from "./types";
import { conflictLabel, conflictPrompt, resolverFor } from "./conflictPrompt";

function state(partial: Partial<RebaseState> = {}): RebaseState {
  return {
    workspace_id: "w1",
    operation: "Rebase",
    head: "018895e",
    branch: "feat/strategy",
    onto: "origin/develop",
    step: 3,
    total: 7,
    conflicts: [
      { path: "src/main/StrategySpecController.kt", code: "UU" },
      { path: "src/test/StrategySpecUseCaseTest.kt", code: "DU" },
    ],
    truncated: false,
    ...partial,
  };
}

function provider(id: string, supports: boolean): ProviderInfo {
  return {
    descriptor: { id, display_name: id, capabilities: { supports_initial_prompt: supports } },
  };
}

function launchable(partial: Partial<Launchable> = {}): Launchable {
  return {
    kind: "agent",
    label: "Claude",
    detail: null,
    provider: "claude",
    profile: null,
    enabled: true,
    key: "claude",
    supports_initial_prompt: true,
    ...partial,
  };
}

function store(partial: Partial<ShellSnapshot> = {}): ShellSnapshot {
  return { ...emptySnapshot(), ...partial };
}

describe("conflictLabel", () => {
  it("says what each side did", () => {
    expect(conflictLabel("UU")).toBe("both modified");
    expect(conflictLabel("DU")).toBe("deleted by us, modified by them");
    expect(conflictLabel("UD")).toBe("modified by us, deleted by them");
  });

  it("passes an unknown code through rather than inventing a meaning", () => {
    expect(conflictLabel("ZZ")).toBe("ZZ");
  });
});

describe("conflictPrompt", () => {
  const prompt = conflictPrompt(state());

  it("names the replay, the branches and the step", () => {
    expect(prompt).toContain("A rebase of `feat/strategy` onto `origin/develop` at step 3 of 7");
    expect(prompt).toContain("2 unmerged paths");
  });

  /*
   * `head` is the tip of the replay so far — on a rebase that is the `onto`
   * side, not the commit that failed to apply. Calling it "the commit it
   * stopped on" is how an agent ends up reading the wrong commit message.
   */
  it("calls HEAD the replay so far rather than the commit that failed", () => {
    expect(prompt).toContain("HEAD is detached at 018895e");
    expect(prompt).toContain("not the commit that failed to apply");
  });

  /*
   * The one mistake that produces a resolution which compiles and silently
   * reverts the commit being replayed. Verified against git itself: during a
   * rebase, stage `:2:` holds the `onto` branch and `:3:` holds your own
   * commit, which is the reverse of what "ours" and "theirs" suggest.
   */
  it("says which side is which, and that a rebase inverts them", () => {
    expect(prompt).toContain("git's words for the two sides are the reverse");
    expect(prompt).toContain(
      "`--ours`, stage `:2:`, `HEAD` — `origin/develop`, the branch being rebased onto — not your work.",
    );
    expect(prompt).toContain(
      "`--theirs`, stage `:3:`, `REBASE_HEAD` — the commit of `feat/strategy` being replayed — this is your work.",
    );
  });

  it("points at the replayed commit's message, which is what the conflict is about", () => {
    expect(prompt).toContain("git log --oneline -1 REBASE_HEAD");
    expect(prompt).toContain("git show REBASE_HEAD");
  });

  it("reads each status code with the sides named, not as `us` and `them`", () => {
    expect(prompt).toContain("- `src/main/StrategySpecController.kt` — changed on both sides");
    expect(prompt).toContain(
      "- `src/test/StrategySpecUseCaseTest.kt` — deleted by `origin/develop`, changed by the replayed commit",
    );
  });

  it("tells the agent to stage what it resolves, which is what clears the list", () => {
    expect(prompt).toContain("git add <path>");
  });

  it("forbids continuing the replay, which stays a human gesture", () => {
    expect(prompt).toContain("Do not run `git rebase --continue`, `--skip` or `--abort`");
    expect(prompt).toContain("Forge Node's Git panel");
  });

  it("forbids the other ways of moving or throwing away the replay", () => {
    for (const escape of [
      "git reset",
      "git checkout <branch>",
      "git stash",
      "git commit --amend",
    ]) {
      expect(prompt).toContain(escape);
    }
  });

  it("refuses a guess on a conflict that cannot be resolved both ways", () => {
    expect(prompt).toContain("leave");
    expect(prompt).toContain("unstaged");
  });

  it("requires no conflict marker survives into the index", () => {
    expect(prompt).toContain("git diff --cached --check");
  });

  it("admits a capped list rather than looking exhaustive", () => {
    expect(conflictPrompt(state({ truncated: true }))).toContain("capped");
    expect(prompt).not.toContain("capped");
  });

  it("puts `ours` on the current branch for the operations that do not invert", () => {
    const merge = conflictPrompt(state({ operation: "Merge", branch: "main", onto: null }));
    expect(merge).toContain("The two sides are:");
    expect(merge).toContain("`HEAD` — the branch you are standing on.");
    expect(merge).toContain("`MERGE_HEAD` — the branch being merged in.");
    expect(merge).not.toContain("the reverse");

    const pick = conflictPrompt(state({ operation: "CherryPick", onto: null }));
    expect(pick).toContain("`CHERRY_PICK_HEAD` — the commit being cherry-picked.");
    expect(pick).toContain("Do not run `git cherry-pick --continue`");
  });

  it("names no branch for an operation it does not know the shape of", () => {
    const odd = conflictPrompt(state({ operation: "Bisect" }));
    expect(odd).toContain("`HEAD` — what is already in the working tree.");
    expect(odd).toContain("the side being applied.");
    expect(odd).not.toContain("REBASE_HEAD");
  });

  it("still reads as prose with the facts missing", () => {
    const bare = conflictPrompt(
      state({ operation: null, branch: null, onto: null, head: null, step: null, total: null }),
    );
    expect(bare).toContain("A rebase in this checkout stopped with 2 unmerged paths.");
    expect(bare).toContain("the branch being rebased onto — not your work");
    expect(bare).not.toContain("HEAD is detached");
    expect(bare).not.toContain("undefined");
    expect(bare).not.toContain("null");
    expect(bare).not.toContain("``");
  });

  it("counts one path in the singular", () => {
    const one = conflictPrompt(state({ conflicts: [{ path: "a.txt", code: "UU" }] }));
    expect(one).toContain("stopped with 1 unmerged path.");
    expect(one).toContain("## Unmerged paths (1)");
  });

  it("never leaves a blank run where an absent fact was", () => {
    expect(conflictPrompt(state({ head: null, step: null, total: null }))).not.toMatch(/\n\n\n/);
    expect(prompt).not.toMatch(/\n\n\n/);
  });
});

describe("resolverFor", () => {
  it("hands the prompt to the default agent when it takes one", () => {
    const snapshot = store({
      app_state: { "ui.default_agent": "provider:claude" },
      launchables: [launchable()],
      providers: [provider("claude", true)],
    });
    expect(resolverFor(snapshot)).toEqual({ kind: "agent", provider: "claude", profile: null });
  });

  it("refuses here, naming what to change, when the default cannot take one", () => {
    const snapshot = store({
      app_state: { "ui.default_agent": "provider:opencode" },
      launchables: [launchable({ provider: "opencode", label: "OpenCode", key: "opencode" })],
      providers: [provider("opencode", false)],
    });
    const resolver = resolverFor(snapshot);
    expect(resolver.kind).toBe("unavailable");
    expect(resolver.kind === "unavailable" && resolver.reason).toContain("Settings → Agents");
  });

  it("uses the only prompt-capable agent when no default is set", () => {
    const snapshot = store({
      app_state: {},
      launchables: [
        launchable({ provider: "opencode", key: "opencode" }),
        launchable({ provider: "claude", key: "claude" }),
      ],
      providers: [provider("opencode", false), provider("claude", true)],
    });
    expect(resolverFor(snapshot)).toEqual({ kind: "agent", provider: "claude", profile: null });
  });

  it("refuses to guess between two agents when no default is set", () => {
    const snapshot = store({
      app_state: {},
      launchables: [
        launchable({ provider: "claude", key: "claude" }),
        launchable({ provider: "codex", key: "codex" }),
      ],
      providers: [provider("claude", true), provider("codex", true)],
    });
    const resolver = resolverFor(snapshot);
    expect(resolver.kind).toBe("unavailable");
    expect(resolver.kind === "unavailable" && resolver.reason).toContain("No default agent is set");
  });

  it("does not count a profile as a second agent to choose between", () => {
    const snapshot = store({
      app_state: {},
      launchables: [
        launchable({ provider: "claude", key: "claude" }),
        launchable({ provider: "claude", key: "profile:x", profile: "x", label: "Reviewer" }),
      ],
      providers: [provider("claude", true)],
    });
    expect(resolverFor(snapshot)).toEqual({ kind: "agent", provider: "claude", profile: null });
  });

  it("never picks one that is not installed", () => {
    const snapshot = store({
      app_state: {},
      launchables: [launchable({ enabled: false })],
      providers: [provider("claude", true)],
    });
    expect(resolverFor(snapshot).kind).toBe("unavailable");
  });

  it("says so when nothing can be handed a prompt", () => {
    expect(resolverFor(store()).kind).toBe("unavailable");
  });

  /*
   * The keys below are the ones the host actually emits
   * (`src-tauri/src/runtime/snapshot.rs`): a provider launchable is keyed by
   * the provider id, a profile one by `profile:<uuid>`. A mismatch here would
   * read as the preference being ignored, which is the whole point of these
   * two cases.
   */
  const PROFILE = "6f1b0f52-0a1e-4a41-9d0c-2a2b7f9e1c33";

  it("launches the configured profile, carrying the profile through", () => {
    const snapshot = store({
      app_state: { "ui.default_agent": `profile:${PROFILE}` },
      launchables: [
        launchable({ key: "claude" }),
        launchable({ key: `profile:${PROFILE}`, label: "Reviewer", profile: PROFILE }),
      ],
      providers: [provider("claude", true)],
    });
    expect(resolverFor(snapshot)).toEqual({
      kind: "agent",
      provider: "claude",
      profile: PROFILE,
    });
  });

  it("prefers the configured default over an agent listed before it", () => {
    const snapshot = store({
      app_state: { "ui.default_agent": "provider:codex" },
      launchables: [
        launchable({ key: "claude" }),
        launchable({ key: "codex", label: "Codex", provider: "codex" }),
      ],
      providers: [provider("claude", true), provider("codex", true)],
    });
    expect(resolverFor(snapshot)).toEqual({ kind: "agent", provider: "codex", profile: null });
  });

  it("falls back when the configured default is no longer installed", () => {
    const snapshot = store({
      app_state: { "ui.default_agent": "provider:codex" },
      launchables: [
        launchable({ key: "codex", provider: "codex", enabled: false }),
        launchable({ key: "claude" }),
      ],
      providers: [provider("codex", true), provider("claude", true)],
    });
    expect(resolverFor(snapshot)).toEqual({ kind: "agent", provider: "claude", profile: null });
  });
});
