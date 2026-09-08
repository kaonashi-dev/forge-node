import { beforeEach, describe, expect, it } from "vitest";
import {
  REFRESH_FLOOR_MS,
  applySessionChanges,
  beginSessionChanges,
  failSessionChanges,
  forgetSession,
  mayAutoRefresh,
  setSplitOpen,
  splitEntry,
  splitOpen,
} from "./sessionChangesStore";
import type { SessionChanges } from "../workbench/types";

const SESSION = "s1";

const changes = (): SessionChanges => ({
  session_id: SESSION,
  origin: "Recorded",
  summary: {
    branch: "main",
    base: "abc1234",
    files: [],
    commits: [],
    commit_count: 0,
    truncated: false,
  },
  sharing_sessions: 0,
});

beforeEach(() => forgetSession(SESSION));

describe("splitOpen", () => {
  it("follows the preference until the session says otherwise", () => {
    expect(splitOpen(SESSION, false)).toBe(false);
    expect(splitOpen(SESSION, true)).toBe(true);
    setSplitOpen(SESSION, false);
    // An explicit close outranks a preference that says open.
    expect(splitOpen(SESSION, true)).toBe(false);
  });

  it("is closed when there is no session", () => {
    expect(splitOpen(null, true)).toBe(false);
  });
});

describe("mayAutoRefresh", () => {
  it("allows the first read", () => {
    expect(mayAutoRefresh(SESSION)).toBe(true);
  });

  /* The whole reason the floor exists: a `change_summary` is four
     subprocesses, and a session that bursts and pauses would otherwise turn
     the quiet trigger into a loop. */
  it("refuses a second read inside the floor and allows one past it", () => {
    const now = 1_000_000;
    applySessionChanges(SESSION, changes());
    expect(mayAutoRefresh(SESSION, now)).toBe(false);

    const readAt = splitEntry(SESSION).readAt ?? 0;
    expect(mayAutoRefresh(SESSION, readAt + REFRESH_FLOOR_MS - 1)).toBe(false);
    expect(mayAutoRefresh(SESSION, readAt + REFRESH_FLOOR_MS)).toBe(true);
  });

  /* Answers are not ordered, so a second read landing first would paint an
     older count as the newer one. */
  it("refuses while a read is in flight", () => {
    beginSessionChanges(SESSION);
    expect(mayAutoRefresh(SESSION)).toBe(false);
  });

  it("allows a retry after a failure", () => {
    beginSessionChanges(SESSION);
    failSessionChanges(SESSION, "git said no");
    expect(splitEntry(SESSION).error).toBe("git said no");
    expect(mayAutoRefresh(SESSION)).toBe(true);
  });
});

describe("applySessionChanges", () => {
  it("clears the error and stamps the read", () => {
    failSessionChanges(SESSION, "old failure");
    applySessionChanges(SESSION, changes());
    const entry = splitEntry(SESSION);
    expect(entry.error).toBeNull();
    expect(entry.loading).toBe(false);
    expect(entry.readAt).not.toBeNull();
  });
});
