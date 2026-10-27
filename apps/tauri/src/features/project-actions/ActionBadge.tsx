import { Show } from "solid-js";
import type { Session } from "../../contracts/runtime";
import { Icon } from "../../theme/icons";
import { sessionPhase, urlBadge } from "./actionState";

/**
 * What a task's terminal row adds to its state marker: starting, the address
 * it serves on, or a warning. A plain span, because the row is a button.
 */
export function ActionBadge(props: { session: Session }) {
  const state = () => sessionPhase(props.session);
  const text = () => {
    const current = state();
    switch (current.phase) {
      case "waiting":
        return { text: "waiting", tone: "muted" };
      case "ready": {
        const badge = urlBadge(current.url, props.session.action?.port ?? null);
        return badge ? { text: badge, tone: "good" } : null;
      }
      case "timed_out":
        return { text: "not ready", tone: "warn" };
      case "port_in_use":
        return { text: `:${current.port} taken`, tone: "warn" };
      default:
        return null;
    }
  };
  return (
    <>
      <Show when={state().phase === "probing" || state().phase === "waiting"}>
        <Icon name="loader" size={11} class="forge-icon-muted forge-icon-spin" />
      </Show>
      <Show when={text()}>
        {(badge) => (
          <span class="action-badge" data-tone={badge().tone}>
            {badge().text}
          </span>
        )}
      </Show>
    </>
  );
}
