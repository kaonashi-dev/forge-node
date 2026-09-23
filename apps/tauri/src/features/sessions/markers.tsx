import { Tooltip } from "../../ui/index";
import { now } from "../../runtime/clock";
import type { Session } from "../../contracts/runtime";
import { sessionAttention } from "./attention";
import { sessionWork, type SessionWork } from "./work";

export type MarkerSize = "sm" | "md";

/**
 * Session state is the app's own vocabulary, so it is a geometric marker rather
 * than an icon: a filled dot for a settled state, a hollow ring for one still
 * in motion, a haloed dot for one waiting on a person. Three shapes, and the
 * tooltip and accessible name say the state in words.
 */
type MarkerShape = "dot" | "ring" | "halo";

function shapeFor(work: SessionWork): MarkerShape {
  switch (work) {
    case "needs-you":
      return "halo";
    case "working":
    case "starting":
      return "ring";
    default:
      return "dot";
  }
}

/**
 * Green is *done* and accent is *still going*, not one green for both: a rail
 * of four green dots said only that four processes were alive. Amber is
 * starting, ember is waiting on a person and nothing else, red is a crash.
 */
function workColorClass(work: SessionWork): string {
  switch (work) {
    case "starting":
      return "forge-icon-amber";
    case "working":
      return "forge-icon-accent";
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
      return "Waiting on you";
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
  return <Marker shape="halo" colorClass="forge-icon-needs-you" label="Waiting on you" />;
}
