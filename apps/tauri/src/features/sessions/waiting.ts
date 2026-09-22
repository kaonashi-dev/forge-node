import { createEffect, untrack } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import { sessionIsActive, type Session } from "../../contracts/runtime";
import { clearQuestion, hasQuestion, markQuestion, questionIds } from "../terminal/questions";

/**
 * Feed the question latch from the snapshot and retire it with its session.
 *
 * Runs on the shell rung — a snapshot, a few times a minute — never per frame.
 * Only agents ask: a shell's bell is a completion beep, the same rule
 * `projects/tree.ts` `waiting` applies.
 */
export function trackQuestions(): void {
  createEffect(() => {
    const asking = new Set<string>();
    for (const session of forgeStore.sessions) {
      if (forgeStore.session_attention[session.id]?.wants_you) asking.add(session.id);
    }
    const live = new Set(
      forgeStore.sessions
        .filter((session) => session.agent_provider_id !== null && sessionIsActive(session.state))
        .map((session) => session.id),
    );
    untrack(() => {
      for (const id of asking) markQuestion(id);
      for (const id of questionIds()) if (!live.has(id)) clearQuestion(id);
    });
  });
}

/** Every session with an open question, the one that has waited longest first. */
export function waitingSessions(): Session[] {
  return forgeStore.sessions
    .filter((session) => hasQuestion(session.id))
    .sort((a, b) =>
      (a.last_activity_at ?? a.created_at).localeCompare(b.last_activity_at ?? b.created_at),
    );
}

export { hasQuestion };
