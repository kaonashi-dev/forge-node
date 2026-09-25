import type { EditorFind, EditorFindCommand, EditorFindFlags } from "../../contracts/runtime";

/** `domain::MAX_EDITOR_FIND_PATTERN_BYTES`: the daemon refuses a longer pattern whole. */
export const MAX_FIND_PATTERN_BYTES = 1024;

const utf8 = new TextEncoder();

/** Whether the daemon would refuse `pattern`; counted in UTF-8 bytes, as it counts. */
export function findPatternTooLong(pattern: string): boolean {
  // Three bytes per UTF-16 unit is the most UTF-8 can take, so short input skips the encode.
  if (pattern.length * 3 <= MAX_FIND_PATTERN_BYTES) return false;
  return utf8.encode(pattern).length > MAX_FIND_PATTERN_BYTES;
}

/** What the counter says: `3 of 12`, `12 results`, `No results`, or why not. */
export function findCounter(find: EditorFind, typed: string): string {
  // The refusal never reaches the editor, so its state still counts the last pattern it took.
  if (findPatternTooLong(typed)) return "Pattern is too long";
  if (find.error) return find.error;
  if (typed === "") return "";
  if (find.total === 0) return "No results";
  const total = `${find.total}${find.capped ? "+" : ""}`;
  if (find.index > 0) return `${find.index} of ${total}`;
  return find.total === 1 && !find.capped ? "1 result" : `${total} results`;
}

export function setFind(pattern: string, flags: EditorFindFlags): EditorFindCommand {
  return {
    Set: {
      pattern,
      case_sensitive: flags.case_sensitive,
      whole_word: flags.whole_word,
      regex: flags.regex,
    },
  };
}

export function flagsOf(find: EditorFind): EditorFindFlags {
  return {
    case_sensitive: find.case_sensitive,
    whole_word: find.whole_word,
    regex: find.regex,
  };
}

/** The keys the field answers itself; anything else types into it. */
export function findKeyCommand(event: {
  key: string;
  shiftKey: boolean;
}): Exclude<EditorFindCommand, { Set: unknown }> | null {
  if (event.key === "Enter") return event.shiftKey ? "Previous" : "Next";
  if (event.key === "Escape") return "Close";
  return null;
}
