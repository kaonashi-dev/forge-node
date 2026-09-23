// What a waiting agent is asking, read off the rows the replica already holds.
//
// Provider-agnostic on purpose: no CLI announces its prompts on the wire, so
// the only evidence is the painted text. The parse is deliberately narrow —
// a numbered menu counts only when one of its lines carries a cursor mark —
// because a false menu turns an ordinary numbered list into buttons that type
// digits into an agent.

import type { WireRow } from "../../contracts/terminal";

export type AnswerChoice = {
  /** Exactly what a press sends: the menu's own number. */
  digit: string;
  label: string;
};

export type AnswerPrompt = {
  question: string | null;
  choices: AnswerChoice[];
};

/** Non-blank lines allowed below a menu: key hints, a box's bottom edge. */
const FOOTER_LINES = 4;
const DESCRIPTION_LINES = 2;
/** How far above the bottom a question without a menu is looked for. */
const QUESTION_REACH = 12;
const QUESTION_LINES = 2;
const MAX_TEXT = 280;

const BOX_EDGE = /^[\s│┃║╭╮╰╯┌┐└┘─━═]+|[\s│┃║╭╮╰╯┌┐└┘─━═]+$/gu;
const CHOICE = /^([❯›>▶→➜])?\s*(\d{1,2})[.)]\s+(\S.*)$/u;
const BARE_PROMPT = /^[>❯›$%#]\s*$/u;
const KEY_HINT = /\s*\((?:esc|enter|tab|[a-z]|shift\+tab)\)$/iu;

/** One row as plain text: the runs' own text, in order. */
export function rowText(row: WireRow | null): string {
  if (!row) return "";
  let text = "";
  for (const run of row.r) text += run[0];
  return text;
}

/**
 * Rows to logical lines: a soft-wrapped row is joined to the one after it,
 * so a long question reads as one sentence rather than two fragments.
 */
export function logicalLines(rows: readonly (WireRow | null)[]): string[] {
  const lines: string[] = [];
  let current = "";
  for (const row of rows) {
    current += rowText(row);
    if (row?.w === true) continue;
    lines.push(current);
    current = "";
  }
  if (current) lines.push(current);
  return lines;
}

function clean(line: string): string {
  return line.replace(BOX_EDGE, "").replace(/\s+/gu, " ").trim();
}

function clip(text: string): string {
  return text.length > MAX_TEXT ? `${text.slice(0, MAX_TEXT - 1)}…` : text;
}

type Parsed = { index: number; marked: boolean; digit: number; label: string };

function parseChoice(line: string, index: number): Parsed | null {
  const match = CHOICE.exec(line);
  if (!match) return null;
  return {
    index,
    marked: match[1] !== undefined,
    digit: Number(match[2]),
    label: match[3].replace(KEY_HINT, "").trim(),
  };
}

/**
 * The menu nearest the bottom: numbered `1…n` without a gap, one line marked
 * as the cursor, and nothing but a short footer below it.
 */
function findMenu(lines: string[]): Parsed[] | null {
  let footer = 0;
  let last = -1;
  for (let index = lines.length - 1; index >= 0; index -= 1) {
    if (!lines[index]) continue;
    if (parseChoice(lines[index], index)) {
      last = index;
      break;
    }
    footer += 1;
    if (footer > FOOTER_LINES) return null;
  }
  if (last < 0) return null;

  const found: Parsed[] = [];
  let expected: number | null = null;
  let between = 0;
  for (let index = last; index >= 0; index -= 1) {
    const line = lines[index];
    if (!line) break;
    const choice = parseChoice(line, index);
    if (!choice) {
      // An option's description sits on the lines under its label.
      between += 1;
      if (between <= DESCRIPTION_LINES) continue;
      break;
    }
    if (expected !== null && choice.digit !== expected) break;
    found.unshift(choice);
    expected = choice.digit - 1;
    between = 0;
    if (choice.digit === 1) break;
  }
  if (found.length < 2 || found[0].digit !== 1) return null;
  if (found.filter((choice) => choice.marked).length !== 1) return null;
  return found;
}

function questionAbove(lines: string[], end: number): string | null {
  let index = end - 1;
  while (index >= 0 && !lines[index]) index -= 1;
  const paragraph: string[] = [];
  while (index >= 0 && lines[index] && paragraph.length < QUESTION_LINES) {
    paragraph.unshift(lines[index]);
    index -= 1;
  }
  return paragraph.length > 0 ? clip(paragraph.join(" ")) : null;
}

function trailingQuestion(lines: string[]): string | null {
  const content: string[] = [];
  for (let index = lines.length - 1; index >= 0 && content.length < QUESTION_REACH; index -= 1) {
    const line = lines[index];
    if (line && !BARE_PROMPT.test(line)) content.push(line);
  }
  const asked = content.find((line) => line.endsWith("?"));
  return asked ? clip(asked) : null;
}

/**
 * The question on screen and, when there is a menu under it, its choices.
 *
 * `null` when there is nothing on screen at all. A question with no parsable
 * menu comes back with no choices, and a screen with a menu but no line above
 * it comes back with a `null` question: the card still has something to say.
 */
export function readPrompt(rows: readonly (WireRow | null)[]): AnswerPrompt | null {
  const lines = logicalLines(rows).map(clean);
  if (!lines.some(Boolean)) return null;
  const menu = findMenu(lines);
  if (menu) {
    return {
      question: questionAbove(lines, menu[0].index),
      choices: menu.map((choice) => ({ digit: String(choice.digit), label: clip(choice.label) })),
    };
  }
  return { question: trailingQuestion(lines), choices: [] };
}

/** Structural equality, so a repaint that changed nothing on screen stays quiet. */
export function samePrompt(a: AnswerPrompt | null, b: AnswerPrompt | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  if (a.question !== b.question || a.choices.length !== b.choices.length) return false;
  return a.choices.every(
    (choice, index) =>
      choice.digit === b.choices[index].digit && choice.label === b.choices[index].label,
  );
}
