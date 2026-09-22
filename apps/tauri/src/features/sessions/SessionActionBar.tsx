import { Show } from "solid-js";
import { Icon } from "../../theme/icons/index";
import { IconButton } from "../../ui/index";
import {
  activeCheckout,
  activeSession,
  openCheckoutReview,
  sessionChangesOpen,
  startHandoff,
  toggleSessionChanges,
} from "./sessionActions";
import { handoffJob, setHandoffProgressOpen } from "./handoffJobStore";

export function SessionActionBar() {
  /** All three need a checkout: a folder workspace has no diff and no branch. */
  const ready = () => activeCheckout() !== null;
  const job = () => {
    const current = handoffJob();
    const session = activeSession();
    if (!current || !session) return null;
    return current.returnSession === session.id ? current : null;
  };

  return (
    <Show when={activeSession()}>
      <div class="session-actions" role="toolbar" aria-label="Session actions">
        <Show
          when={job()}
          fallback={
            <IconButton
              label="Continue in a new session…"
              size="sm"
              onClick={startHandoff}
              disabled={!ready()}
            >
              <Icon name="message-square-plus" class="forge-icon-muted" size={14} />
            </IconButton>
          }
        >
          <IconButton
            label="Open session handoff"
            size="sm"
            onClick={() => setHandoffProgressOpen(true)}
          >
            <Icon name="loader" class="forge-icon-muted forge-icon-spin" size={14} />
          </IconButton>
        </Show>
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
