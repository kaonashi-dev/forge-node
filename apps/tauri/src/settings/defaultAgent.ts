// The `ui.default_agent` preference.
//
// The wire form matters: the host and the WebView talk to the same daemon
// `app_state` row, so a value written here has to round-trip through the same
// parser. Anything else silently reads back as "ask", which looks like the
// preference forgetting itself.

import type { Launchable } from "../runtime/types";

/** `app_state` key holding the serialized preference. */
export const DEFAULT_AGENT_KEY = "ui.default_agent";

/** What ⌘⇧A ("New agent") starts. */
export type DefaultAgent =
  | { kind: "ask" }
  | { kind: "shell" }
  | { kind: "provider"; id: string }
  | { kind: "profile"; id: string };

/** The resolved outcome of {@link resolveDefaultAgent}. */
export type LaunchDefault =
  | { kind: "ask" }
  | { kind: "shell" }
  | { kind: "agent"; provider: string; profile: string | null };

/**
 * A profile id is a UUID. the host parses it into an `AgentProfileId` and degrades
 * to "ask" when that fails; matching the shape here keeps a malformed value
 * from travelling as far as a launch command.
 */
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * Parse the persisted form.
 *
 * Anything unknown degrades to "ask" instead of throwing: a value written by a
 * newer build must not break this one, and `app_state` is untyped text by
 * design.
 */
export function parseDefaultAgent(value: string | null | undefined): DefaultAgent {
  const trimmed = (value ?? "").trim();
  if (trimmed === "shell") return { kind: "shell" };
  const provider = trimmed.startsWith("provider:") ? trimmed.slice("provider:".length) : null;
  if (provider) return { kind: "provider", id: provider };
  const profile = trimmed.startsWith("profile:") ? trimmed.slice("profile:".length) : null;
  if (profile && UUID.test(profile)) return { kind: "profile", id: profile };
  // "ask", the legacy "auto", the empty string, a malformed id, or a value
  // this build does not know.
  return { kind: "ask" };
}

/** The persisted form. */
export function defaultAgentValue(agent: DefaultAgent): string {
  switch (agent.kind) {
    case "shell":
      return "shell";
    case "provider":
      return `provider:${agent.id}`;
    case "profile":
      return `profile:${agent.id}`;
    default:
      return "ask";
  }
}

/**
 * The `Launchable.key` this preference points at, if any — a provider id, or
 * `profile:<uuid>`, exactly as the host builds them.
 */
export function launchableKey(agent: DefaultAgent): string | null {
  switch (agent.kind) {
    case "provider":
      return agent.id;
    case "profile":
      return `profile:${agent.id}`;
    default:
      return null;
  }
}

/**
 * What choosing "New agent" should actually do right now.
 *
 * A provider that is no longer installed — or a profile that was deleted —
 * falls back to the palette rather than to silence: the preference is stale,
 * not a reason to make the shortcut dead. The launchable list is the whole
 * test, because the host drops deleted profiles from it and marks an
 * uninstalled provider `enabled: false` (`src-tauri/src/runtime/snapshot.rs`).
 */
export function resolveDefaultAgent(
  agent: DefaultAgent,
  launchables: readonly Launchable[],
): LaunchDefault {
  if (agent.kind === "ask") return { kind: "ask" };
  if (agent.kind === "shell") return { kind: "shell" };
  const key = launchableKey(agent);
  const entry = launchables.find((item) => item.key === key);
  if (!entry || !entry.enabled || !entry.provider) return { kind: "ask" };
  return { kind: "agent", provider: entry.provider, profile: entry.profile };
}

/** The preference as it stands in the last snapshot. */
export function defaultAgentFrom(appState: Record<string, string>): DefaultAgent {
  return parseDefaultAgent(appState[DEFAULT_AGENT_KEY]);
}
