/**
 * Parse an RFC-3339 instant to epoch milliseconds.
 *
 * Never `localeCompare` or `>` on these strings. The daemon writes them with
 * `time`'s RFC-3339, whose subsecond part is as long as it needs to be, and
 * `Date.prototype.toISOString` always writes exactly three digits — so
 * `"…42.123Z"` sorts *after* `"…42.123456Z"`, and a session created a third of
 * a millisecond after the launch would read as older than it.
 */
function instant(value: string): number {
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? 0 : parsed;
}

/**
 * The session a launch produced, adopted by looking for what appeared after it.
 *
 * The runtime command channel is one-way — it is drained on the thread that
 * also carries keystrokes — so the id of the session it just created does not
 * come back. Matching on "an agent session in this checkout, created after we
 * asked" is what the PR compose and review tabs do; `since`
 * is what keeps it from adopting a session that was already there.
 */
export function adoptLaunched<
  T extends {
    id: string;
    workspace_id: string;
    created_at: string;
    agent_provider_id: string | null;
  },
>(sessions: readonly T[], workspace: string, since: string): T | null {
  const floor = instant(since);
  return sessions
    .filter(
      (session) =>
        session.workspace_id === workspace &&
        session.agent_provider_id !== null &&
        instant(session.created_at) >= floor,
    )
    .reduce<T | null>(
      (newest, session) =>
        !newest || instant(session.created_at) > instant(newest.created_at) ? session : newest,
      null,
    );
}
