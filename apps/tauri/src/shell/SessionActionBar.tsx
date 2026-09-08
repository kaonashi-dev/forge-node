import { Show } from "solid-js";
import { Icon } from "../theme/icons";
import { IconButton } from "../ui";
import {
  activeCheckout,
  activeSession,
  openCheckoutReview,
  sessionChangesOpen,
  startHandoff,
  toggleSessionChanges,
} from "./sessionActions";

/**
 * The three things you can do to the session on screen (§16.7, §16.8).
 *
 * An overlay on the terminal rather than a row above it: the terminal is the
 * whole centre column, and a permanent strip would cost every session a line
 * of grid to hold three buttons most of them never need. It appears on hover
 * and on focus, so it is reachable from the keyboard as well as findable.
 *
 * All three actions live in `sessionActions.ts`, which the command palette
 * calls too — a palette entry that did something slightly different from the
 * button beside it is a bug nobody reports.
 */
export function SessionActionBar() {
  /** All three need a checkout: a folder workspace has no diff and no branch. */
  const ready = () => activeCheckout() !== null;

  return (
    <Show when={activeSession()}>
      <div class="session-actions" role="toolbar" aria-label="Session actions">
        <IconButton
          label="Continue in a new session…"
          size="sm"
          onClick={startHandoff}
          disabled={!ready()}
        >
          <Icon name="message-square-plus" class="forge-icon-muted" size={14} />
        </IconButton>
        <IconButton
          label={sessionChangesOpen() ? "Hide session changes" : "Show session changes"}
          size="sm"
          selected={sessionChangesOpen()}
          onClick={toggleSessionChanges}
          disabled={!ready()}
        >
          <Icon name="columns-2" class="forge-icon-muted" size={14} />
        </IconButton>
        <IconButton
          label="Review this checkout"
          size="sm"
          onClick={openCheckoutReview}
          disabled={!ready()}
        >
          <Icon name="list-checks" class="forge-icon-muted" size={14} />
        </IconButton>
      </div>
    </Show>
  );
}
