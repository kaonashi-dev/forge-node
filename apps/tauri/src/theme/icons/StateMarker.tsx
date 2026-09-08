import { now } from "../../runtime/clock";
import { sessionAttention } from "../../runtime/attention";
import { sessionWork, type SessionWork } from "../../runtime/work";
import type { Session } from "../../runtime/types";
import { Tooltip } from "../../ui";

export type MarkerSize = "sm" | "md";

/**
 * Session state is the app's own vocabulary, so it is a geometric marker rather
 * than an icon: a filled dot for a settled state, a hollow ring for one still
 * in motion, a pulsing ring for one waiting on a person. Three shapes, five
 * states — the colour carries the rest.
 */
type MarkerShape = "dot" | "ring" | "pulse";

function shapeFor(work: SessionWork): MarkerShape {
  switch (work) {
    case "needs-you":
      return "pulse";
    case "working":
    case "starting":
      return "ring";
    default:
      return "dot";
  }
}

/**
 * The colour is the whole message.
 *
 * Green is *done* and blue is *still going* — not the other way around, and
 * not one green for both. A rail of four agents was four green dots that said
 * only "four processes are alive", which is the one thing nobody was wondering
 * about. Amber is starting, orange is waiting on a person, red is a crash.
 */
function workColorClass(work: SessionWork): string {
  switch (work) {
    case "starting":
      return "forge-icon-amber";
    case "working":
      return "forge-icon-blue";
    case "idle":
    case "running":
      return "forge-icon-green";
    case "needs-you":
      return "forge-icon-needs-you";
    case "failed":
      return "forge-icon-red";
    default:
      return "forge-icon-muted";
  }
}

/** What the marker means, for the tooltip a hover gets. */
function workLabel(work: SessionWork): string {
  switch (work) {
    case "starting":
      return "Starting";
    case "working":
      return "Working";
    case "idle":
      return "Finished — waiting for you";
    case "running":
      return "Running";
    case "needs-you":
      return "Needs you";
    case "failed":
      return "Failed";
    default:
      return "Exited";
  }
}

function Marker(props: {
  shape: MarkerShape;
  colorClass: string;
  label: string;
  size?: MarkerSize;
}) {
  return (
    <Tooltip label={props.label} contents>
      <span
        class={`forge-state-marker forge-state-${props.shape} ${props.colorClass}`}
        classList={{ "forge-state-lg": props.size === "md" }}
        role="img"
        aria-label={props.label}
      />
    </Tooltip>
  );
}

/**
 * The marker for a state that was rolled up rather than read off one session —
 * what a folded checkout card shows for everything under it.
 */
export function WorkMarker(props: { work: SessionWork; size?: MarkerSize }) {
  return (
    <Marker
      shape={shapeFor(props.work)}
      colorClass={workColorClass(props.work)}
      label={workLabel(props.work)}
      size={props.size}
    />
  );
}

type StateMarkerProps = {
  session: Session;
};

/**
 * The marker in front of a session row, tab or card.
 *
 * Reads the clock and the attention map itself rather than taking them as
 * props: it is painted in three places and every one of them would otherwise
 * have to thread the same two values through for one glyph.
 */
export function StateMarker(props: StateMarkerProps) {
  const work = () => sessionWork(props.session, sessionAttention(props.session.id), now());

  return (
    <Marker
      shape={shapeFor(work())}
      colorClass={workColorClass(work())}
      label={workLabel(work())}
    />
  );
}

export function AttentionMarker() {
  return <Marker shape="pulse" colorClass="forge-icon-needs-you" label="Needs you" />;
}
