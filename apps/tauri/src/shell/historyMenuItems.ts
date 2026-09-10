import type { ExternalAgentSession } from "../runtime/types";
import type { MenuItem } from "../ui/types";
import { startExternalHandoff } from "./sessionActions";

/**
 * Why a handoff is not offered.
 *
 * A run can be recorded in a directory Forge maps to a project but not to a
 * workspace row; there is then no checkout to start the new agent in, and
 * guessing one would start it somewhere the conversation never happened.
 */
export function handoffBlocked(session: ExternalAgentSession): string | null {
  return session.workspace_id ? null : "This run was recorded outside a checkout Forge tracks";
}

/**
 * The actions behind a History card's `⋯`.
 *
 * `onDelete` is the caller's, so this module stays free of the shell's dialog
 * and toast layers and a node test can import it. The wording matches the live
 * session menu verbatim: a card whose action reads differently from the button
 * beside it is a bug nobody reports.
 */
export function historyMenuItems(session: ExternalAgentSession, onDelete: () => void): MenuItem[] {
  const blocked = handoffBlocked(session);
  const shared = session.store === "SharedDatabase";
  return [
    {
      kind: "item",
      label: "Continue in a New Session…",
      icon: "message-square-plus",
      disabled: Boolean(blocked),
      detail: blocked ?? undefined,
      run: () => startExternalHandoff(session),
    },
    { kind: "rule" },
    {
      kind: "item",
      label: "Delete Transcript…",
      icon: "trash",
      destructive: true,
      disabled: shared,
      detail: shared ? "opencode records this run in a shared database" : undefined,
      run: onDelete,
    },
  ];
}
