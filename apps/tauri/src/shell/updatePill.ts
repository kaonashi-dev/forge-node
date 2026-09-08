// What the status bar says about a pending update, as a pure function of the
// last `shell:update` event. Its own module so a node test can read it without
// Solid's server build.

import type { UpdateState } from "../runtime/types";

export type PillTone = "neutral" | "accent" | "warn";

export type Pill = {
  label: string;
  title: string;
  tone: PillTone;
  /** Whether clicking starts the download. `false` while one is in flight. */
  actionable: boolean;
};

/**
 * `null` means "show nothing".
 *
 * A background check that found nothing is not news, and neither is one still
 * running: the bar stays as it was rather than flashing a spinner every six
 * hours. Only `failed` after an attempt the user started is worth a pill, and
 * the caller decides that by passing `userAsked`.
 */
export function updatePill(state: UpdateState | null, userAsked = false): Pill | null {
  if (!state) return null;
  switch (state.state) {
    case "checking":
      return userAsked
        ? {
            label: "Checking…",
            title: "Looking for a newer release",
            tone: "neutral",
            actionable: false,
          }
        : null;
    case "uptodate":
      return userAsked
        ? {
            label: "Up to date",
            title: "You are on the newest release",
            tone: "neutral",
            actionable: false,
          }
        : null;
    case "available":
      return state.kind === "hard"
        ? {
            label: `${state.version} available`,
            title:
              `${state.version} also replaces the runtime, so it cannot be applied in place. ` +
              `Download it and install it over the app.`,
            tone: "warn",
            actionable: false,
          }
        : {
            label: `Update to ${state.version}`,
            title:
              state.notes || `Install ${state.version} and reload — your sessions keep running`,
            tone: "accent",
            actionable: true,
          };
    case "downloading":
      return {
        label: `Downloading ${state.percent}%`,
        title: "Fetching the update in the background",
        tone: "accent",
        actionable: false,
      };
    case "installing":
      return {
        label: "Applying…",
        title: "Swapping the app and reloading",
        tone: "accent",
        actionable: false,
      };
    case "failed":
      return userAsked
        ? { label: "Update failed", title: state.message, tone: "warn", actionable: true }
        : null;
  }
}
