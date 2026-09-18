import { afterAll, afterEach, expect, it } from "vitest";
import { startWorkspaceFocus } from "./workspaceFocus";
import { focusWorkspace } from "../../state/workspace";
import { gitStore, setGitStore } from "../../features/git/state";
import type { JuvaDraft } from "../../contracts/workbench";

const stopWorkspaceFocus = startWorkspaceFocus();

const draft: JuvaDraft = {
  kind: "CommitMessage",
  title: "Change in checkout A",
  body: "A's changes",
  context: {
    branch: "a",
    default_branch: "main",
    dirty: true,
    ahead: null,
    behind: null,
    files: [],
    patch: "",
    truncated: false,
  },
};

afterEach(() => focusWorkspace(null));
afterAll(() => stopWorkspaceFocus());

it("dismisses a workspace's draft before another checkout can apply it", () => {
  focusWorkspace("a");
  setGitStore({ juvaDraft: draft, juvaError: "old error" });
  focusWorkspace("a");
  expect(gitStore.juvaDraft?.title).toBe(draft.title);

  focusWorkspace("b");
  expect(gitStore.juvaDraft).toBeNull();
  expect(gitStore.juvaError).toBeNull();
});

it("dismisses a draft when its workspace loses focus", () => {
  focusWorkspace("a");
  setGitStore("juvaDraft", draft);
  focusWorkspace(null);
  expect(gitStore.juvaDraft).toBeNull();
});
