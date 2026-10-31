export type NameQuery = { workspace: string; needle: string };

/** Busy from the keystroke (debounce pending) until the reply or failure for the query asked. */
export function nameSearchBusy(
  pending: boolean,
  asked: NameQuery | null,
  answered: boolean,
  failed: boolean,
): boolean {
  return pending || (asked !== null && !answered && !failed);
}

export type ShownAnswer<T> = { workspace: string; results: T };

/**
 * The file answer the palette lists: the current one, else the last one for the
 * same workspace while the next is on its way.
 *
 * `current` counts only when it is the active checkout's own: `nameResults()`
 * matches the query asked, and a workspace switch moves the focus before the
 * palette asks again.
 *
 * Returns `previous` itself when nothing changed, so a busy flip does not
 * rebuild every file row.
 */
export function shownAnswer<T extends { workspace_id: string }>(
  previous: ShownAnswer<T> | null,
  workspace: string | null,
  needle: string,
  current: T | null,
  busy: boolean,
): ShownAnswer<T> | null {
  if (!workspace || needle.trim() === "") return null;
  if (current?.workspace_id === workspace) {
    return previous?.workspace === workspace && previous.results === current
      ? previous
      : { workspace, results: current };
  }
  return busy && previous?.workspace === workspace ? previous : null;
}
