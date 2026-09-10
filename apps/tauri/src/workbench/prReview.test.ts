import { describe, expect, it } from "vitest";
import type { Launchable, ProviderInfo, PullRequest } from "../runtime/types";
import {
  CUSTOM_RECIPE,
  adoptLaunched,
  builtinRecipes,
  composeReviewPrompt,
  launchHint,
  preferredAgent,
  readyToLaunch,
  recipeOrDefault,
  reviewAgents,
  reviewWorkspace,
} from "./prReview";

function pr(extra: Partial<PullRequest> = {}): PullRequest {
  return {
    project_id: "p1",
    repository: "acme/widget",
    host: "github.com",
    number: 128,
    title: "Add the diff view",
    body: "",
    body_truncated: false,
    url: "https://example.invalid/128",
    author: "rin",
    base_ref: "main",
    head_ref: "diff-view",
    is_draft: false,
    review_decision: null,
    labels: [],
    assignees: [],
    review_requests: [],
    additions: 340,
    deletions: 87,
    changed_files: 12,
    comment_count: 3,
    created_at: "2026-01-01T00:00:00Z",
    updated_at: "2026-01-01T00:00:00Z",
    relations: { assigned: true, review_requested: false, authored: false },
    ...extra,
  };
}

function provider(id: string, extra: Partial<ProviderInfo["descriptor"]> = {}): ProviderInfo {
  return {
    descriptor: {
      id,
      display_name: id,
      capabilities: { supports_initial_prompt: true, supports_review: true },
      prompt: "Positional",
      review: { args: ["--permission-mode", "plan"], label: "plan mode" },
      ...extra,
    },
    detection: { status: { Installed: { executable: `/bin/${id}`, version: "1.0" } } },
  };
}

function launchable(providerId: string, extra: Partial<Launchable> = {}): Launchable {
  return {
    kind: "agent",
    label: providerId,
    detail: null,
    provider: providerId,
    profile: null,
    enabled: true,
    key: providerId,
    supports_initial_prompt: true,
    ...extra,
  };
}

const standard = recipeOrDefault("standard");
const agent = {
  id: "claude",
  provider: "claude",
  profile: null,
  name: "Claude Code",
  mode: "plan mode",
};

describe("review recipes", () => {
  /** The Rust side ships the same ids; a drift here is a prompt nobody meant. */
  it("mirrors the ids `domain::pr_review` declares", () => {
    expect(builtinRecipes().map((recipe) => recipe.id)).toEqual([
      "standard",
      "deep",
      "security",
      CUSTOM_RECIPE,
    ]);
  });

  it("falls back to the first recipe for an unknown remembered id", () => {
    expect(recipeOrDefault("security").id).toBe("security");
    expect(recipeOrDefault("gone").id).toBe("standard");
    expect(recipeOrDefault(null).id).toBe("standard");
  });

  /** `gh` cannot run without the number and the repository, and the model
   *  cannot derive either of them. */
  it("names the pull request for gh, with no placeholder left behind", () => {
    const prompt = composeReviewPrompt(standard, pr(), "");
    expect(prompt).toContain("gh pr diff 128 --repo acme/widget");
    expect(prompt).toContain("gh pr view 128 --repo acme/widget");
    expect(prompt).not.toContain("{");
  });

  /** Prose is the second guard; the read-only flags are the first. */
  it("forbids writing and posting in every recipe", () => {
    for (const recipe of builtinRecipes()) {
      const prompt = composeReviewPrompt(recipe, pr(), "");
      expect(prompt, recipe.id).toContain("Do not edit");
      expect(prompt, recipe.id).toContain("gh pr comment");
    }
  });

  it("appends the user's own words rather than substituting them", () => {
    const prompt = composeReviewPrompt(standard, pr(), "  Focus on the SQL.  ");
    expect(prompt).toContain(standard.body.trim());
    expect(prompt.endsWith("## Also\n\nFocus on the SQL.\n")).toBe(true);
  });

  it("starts the custom recipe with the read instructions and nothing blank", () => {
    const prompt = composeReviewPrompt(recipeOrDefault(CUSTOM_RECIPE), pr(), "look at the locks");
    expect(prompt.startsWith("Read the pull request first")).toBe(true);
    expect(prompt).toContain("look at the locks");
  });
});

describe("review agents", () => {
  it("offers only installed providers that take a prompt and have a read-only mode", () => {
    const providers = [
      provider("claude"),
      provider("nowrite", { review: null }),
      provider("noprompt", { capabilities: { supports_initial_prompt: false }, prompt: undefined }),
      { descriptor: { id: "missing" }, detection: { status: "NotFound" as const } },
    ];
    const launchables = [
      launchable("claude"),
      launchable("nowrite"),
      launchable("noprompt", { supports_initial_prompt: false }),
      launchable("missing", { enabled: false }),
    ];
    expect(reviewAgents(providers, launchables).map((item) => item.id)).toEqual(["claude"]);
  });

  it("lists custom profiles of a reviewable provider next to the bare provider", () => {
    const providers = [provider("claude", { display_name: "Claude Code" })];
    const launchables = [
      launchable("claude", { label: "Claude Code", key: "claude" }),
      launchable("claude", {
        label: "Monato",
        key: "profile:monato",
        profile: "monato",
      }),
    ];
    expect(reviewAgents(providers, launchables)).toEqual([
      {
        id: "claude",
        provider: "claude",
        profile: null,
        name: "Claude Code",
        mode: "plan mode",
      },
      {
        id: "profile:monato",
        provider: "claude",
        profile: "monato",
        name: "Monato",
        mode: "plan mode",
      },
    ]);
  });

  it("names the provider's own mode so the menu can say which posture it is", () => {
    expect(reviewAgents([provider("claude")], [launchable("claude")])[0].mode).toBe("plan mode");
  });

  /** A preference pointing at an uninstalled agent is stale, not a refusal. */
  it("falls back to the first agent when the remembered one is gone", () => {
    const agents = [
      agent,
      { id: "codex", provider: "codex", profile: null, name: "Codex", mode: "read-only sandbox" },
    ];
    expect(preferredAgent(agents, "codex")?.id).toBe("codex");
    expect(preferredAgent(agents, "gone")?.id).toBe("claude");
    expect(preferredAgent([], "claude")).toBeNull();
  });
});

describe("launching a review", () => {
  it("will not start without an agent, and says why", () => {
    expect(readyToLaunch(null, standard, "", false)).toBe(false);
    expect(launchHint(null, standard, "", false)).toContain("read-only mode");
  });

  it("will not start an empty custom recipe", () => {
    const custom = recipeOrDefault(CUSTOM_RECIPE);
    expect(readyToLaunch(agent, custom, "   ", false)).toBe(false);
    expect(readyToLaunch(agent, custom, "check the locks", false)).toBe(true);
  });

  it("will not start a second run while one is going", () => {
    expect(readyToLaunch(agent, standard, "", true)).toBe(false);
    expect(launchHint(agent, standard, "", true)).toContain("already running");
  });

  it("says the review cannot write, naming the provider's mode", () => {
    expect(launchHint(agent, standard, "", false)).toContain("plan mode");
    expect(launchHint(agent, standard, "", false)).toContain("cannot write");
  });
});

describe("adopting the launched session", () => {
  const session = (id: string, created: string, extra = {}) => ({
    id,
    workspace_id: "w1",
    created_at: created,
    agent_provider_id: "claude",
    ...extra,
  });

  /** The command channel is one-way, so the id never comes back; `since` is
   *  what keeps an already-running agent from being mistaken for the new one. */
  it("takes the newest agent session started after the launch", () => {
    const sessions = [
      session("old", "2026-01-01T00:00:00Z"),
      session("new", "2026-01-01T00:00:02Z"),
      session("newest", "2026-01-01T00:00:03Z"),
    ];
    expect(adoptLaunched(sessions, "w1", "2026-01-01T00:00:01Z")?.id).toBe("newest");
  });

  /* `Date.prototype.toISOString` writes three subsecond digits; the daemon's
     RFC-3339 writes as many as it needs. Compared as text, "…00.500Z" sorts
     after "…00.500123Z", and the session would never be adopted. */
  it("compares instants, not the strings they are written as", () => {
    const sessions = [session("agent", "2026-01-01T00:00:00.500123Z")];
    expect(adoptLaunched(sessions, "w1", "2026-01-01T00:00:00.500Z")?.id).toBe("agent");
  });

  it("ignores shells, other checkouts, and anything older than the launch", () => {
    const sessions = [
      session("shell", "2026-01-01T00:00:05Z", { agent_provider_id: null }),
      session("elsewhere", "2026-01-01T00:00:05Z", { workspace_id: "w2" }),
      session("before", "2026-01-01T00:00:00Z"),
    ];
    expect(adoptLaunched(sessions, "w1", "2026-01-01T00:00:01Z")).toBeNull();
  });
});

describe("where a review runs", () => {
  const checkout = (id: string, project: string, kind: string) => ({
    id,
    project_id: project,
    kind,
  });

  it("prefers the checkout already on screen when it belongs to the project", () => {
    const workspaces = [checkout("main", "p1", "Main"), checkout("wt", "p1", "GitWorktree")];
    expect(reviewWorkspace(workspaces, "p1", "wt")?.id).toBe("wt");
  });

  it("falls back to the project's main checkout, then to anything it has", () => {
    const workspaces = [checkout("wt", "p1", "GitWorktree"), checkout("main", "p1", "Main")];
    expect(reviewWorkspace(workspaces, "p1", "elsewhere")?.id).toBe("main");
    expect(reviewWorkspace([checkout("wt", "p1", "GitWorktree")], "p1", null)?.id).toBe("wt");
  });

  /** `gh` needs a directory whose remote is the repository; without a project
   *  Forge has none, and that is the one case a review cannot start. */
  it("has nowhere to run a pull request from an unknown repository", () => {
    expect(reviewWorkspace([checkout("main", "p1", "Main")], null, "main")).toBeNull();
    expect(reviewWorkspace([checkout("main", "p1", "Main")], "p2", "main")).toBeNull();
  });
});
