import type { Session } from "./types";

/**
 * A session row for tests.
 *
 * One builder rather than one per test file: `Session` mirrors a domain struct
 * the daemon owns, so it gains fields, and four copies of the same literal
 * means four test files break on every one of them for no reason of their own.
 *
 * Defaults describe a running generic shell — a graph root with no parent and
 * no profile — because that is the row every test that does not care about
 * roles is describing anyway.
 *
 * Never imported by application code, so it is not in the bundle.
 */
export function sessionFixture(partial: Partial<Session> = {}): Session {
  const id = partial.id ?? "session";
  return {
    id,
    workspace_id: "workspace",
    kind: "Shell",
    role: "Generic",
    parent_session_id: null,
    root_session_id: id,
    title: { user: null, terminal: null },
    state: "Running",
    terminal_id: `terminal-${id}`,
    agent_provider_id: null,
    agent_profile_id: null,
    created_at: "2026-01-01T00:00:00Z",
    ...partial,
  };
}
