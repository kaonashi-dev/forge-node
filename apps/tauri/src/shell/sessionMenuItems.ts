import { closeSession, killSession, renameSession, restartSession } from "../runtime/api";
import { sessionIsActive, type Session } from "../runtime/types";
import { requestConfirm, requestTextInput } from "../store/runtimeStore";
import type { MenuItem } from "../ui";
import type { TextInputRequest } from "./TextInputDialog";
import { openCheckoutReview, startHandoff, toggleSessionChanges } from "./sessionActions";

/** Confirm before the close command can stop a live process. */
export function closeSessionFromUi(session: Session, displayedTitle: string): void {
  const close = () => void closeSession(session.id).catch(() => undefined);
  if (!sessionIsActive(session.state)) {
    close();
    return;
  }
  requestConfirm({
    title: `${displayedTitle} is still running.`,
    description: "Closing it stops the process. Anything it has not written is lost.",
    confirmLabel: "Close session",
    destructive: true,
    onConfirm: close,
  });
}

/** The session actions shared by tab and sidebar context menus. */
export function sessionMenuItems(session: Session, displayedTitle: string): MenuItem[] {
  const running = sessionIsActive(session.state);

  return [
    {
      kind: "item",
      label: "Rename…",
      icon: "square-terminal",
      run: () => {
        requestTextInput({
          title: "Rename session",
          label: "Session name (empty clears it)",
          value: displayedTitle,
          confirmLabel: "Rename",
          allowEmpty: true,
          onSubmit: (next) => void renameSession(session.id, next || null).catch(() => undefined),
        });
      },
    },
    {
      kind: "item",
      label: "Continue in a New Session…",
      icon: "message-square-plus",
      run: startHandoff,
    },
    {
      kind: "item",
      label: "Session Changes",
      icon: "columns-2",
      run: toggleSessionChanges,
    },
    {
      kind: "item",
      label: "Review This Checkout",
      icon: "list-checks",
      run: openCheckoutReview,
    },
    { kind: "rule" },
    running
      ? {
          kind: "item" as const,
          label: "Kill",
          icon: "close" as const,
          destructive: true,
          run: () => void killSession(session.id).catch(() => undefined),
        }
      : {
          kind: "item" as const,
          label: "Restart",
          icon: "plus" as const,
          run: () => void restartSession(session.id).catch(() => undefined),
        },
    {
      kind: "item",
      label: "Close",
      icon: "trash",
      destructive: true,
      run: () => closeSessionFromUi(session, displayedTitle),
    },
  ];
}
