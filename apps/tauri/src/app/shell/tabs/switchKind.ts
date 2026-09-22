import type { WorkbenchView } from "../../../navigation/views";

/** The one word the Ctrl+Tab list prints after a pane's name, so its kind never rides on the glyph alone. */
export function viewKindWord(view: WorkbenchView): string {
  switch (view.kind) {
    case "terminal":
      return "terminal";
    case "diff":
      return "diff";
    case "editor-terminal":
      return "editor";
    case "preview":
      return "preview";
    case "pr_detail":
    case "pr_compose":
      return "pull request";
    case "pr_review":
    case "review":
      return "review";
    case "search":
      return "search";
  }
}

export function sessionKindWord(agent: boolean): string {
  return agent ? "session" : "terminal";
}
