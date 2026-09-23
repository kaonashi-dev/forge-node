// Sessions with a question the GUI has not seen answered.
//
// The shell snapshot's `wants_you` is false for the attached terminal by
// design, so it cannot say that the session on screen is the one waiting. This
// latch mirrors `client::Store`'s own mark from the two edges the WebView sees:
// a `wants_you` in the snapshot or a bell on the attached grid sets it, and
// typing into that terminal — what `answer_attention` spends — clears it.

import { createStore } from "solid-js/store";

const [questions, setQuestions] = createStore<Record<string, true>>({});

export function markQuestion(session: string): void {
  if (!questions[session]) setQuestions(session, true);
}

export function clearQuestion(session: string | null): void {
  if (session && questions[session]) setQuestions(session, undefined!);
}

export function hasQuestion(session: string | null): boolean {
  return session !== null && questions[session] === true;
}

export function questionIds(): string[] {
  return Object.keys(questions);
}
