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
