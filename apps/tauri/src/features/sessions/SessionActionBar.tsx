import { Show, createMemo } from "solid-js";
import { Icon } from "../../theme/icons/index";
import { Button, IconButton, Menu } from "../../ui/index";
import { sessionTitle } from "../../contracts/runtime";
import {
  activeCheckout,
  activeSession,
  openCheckoutReview,
  sessionChangesOpen,
  startHandoff,
  toggleSessionChanges,
} from "./sessionActions";
import { handoffJob, setHandoffProgressOpen } from "./handoffJobStore";
import { sessionMenuItems } from "./sessionMenuItems";
import { splitEntry } from "../git/sessionChangesStore";
import { gitStore } from "../git/state";

/**
 * How many files the session header can honestly count: the session's own
 * changes once they have been read, else the checkout's diff when that is the
 * one on screen. `null` is "not read", never zero.
 */
function changedFileCount(session: string, workspace: string): number | null {
  const own = splitEntry(session).changes;
  if (own) return own.summary.files.length;
  if (gitStore.diff?.workspace_id === workspace) return gitStore.diff.files.length;
  return null;
}

/** The session header's trailing controls. */
export function SessionActionBar() {
  /** All three need a checkout: a folder workspace has no diff and no branch. */
  const ready = () => activeCheckout() !== null;
  const job = () => {
    const current = handoffJob();
    const session = activeSession();
    if (!current || !session) return null;
    return current.returnSession === session.id ? current : null;
  };
  const changed = createMemo(() => {
    const session = activeSession();
    return session ? changedFileCount(session.id, session.workspace_id) : null;
  });

  return (
    <Show when={activeSession()}>
      {(session) => (
        <div class="session-actions" role="toolbar" aria-label="Session actions">
          <Button
            variant="secondary"
            size="sm"
            selected={sessionChangesOpen()}
            disabled={!ready()}
            onClick={toggleSessionChanges}
            aria-label={changed() === null ? "Changes" : `Changes, ${changed()} files`}
            iconLeading={<Icon name="columns-2" size={13} class="forge-icon-muted" />}
          >
            Changes
            <Show when={changed() !== null}>
              <span class="session-actions-count" aria-hidden="true">
                {changed()}
              </span>
            </Show>
          </Button>
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
            label="Review this checkout"
            size="sm"
            onClick={openCheckoutReview}
            disabled={!ready()}
          >
            <Icon name="list-checks" class="forge-icon-muted" size={14} />
          </IconButton>
          <Menu
            triggerLabel="Session menu"
            triggerClass="forge-control forge-control-sm forge-btn forge-btn-ghost forge-icon-button"
            trigger={<Icon name="more-horizontal" class="forge-icon-muted" size={14} />}
            items={sessionMenuItems(session(), sessionTitle(session()))}
          />
        </div>
      )}
    </Show>
  );
}
