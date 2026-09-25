import { sessionIsActive, type Session } from "../../contracts/runtime";

/** A session with no terminal to attach to: its process is gone. */
export function hasExited(row: Pick<Session, "state" | "terminal_id">): boolean {
  return row.terminal_id === null && !sessionIsActive(row.state);
}
