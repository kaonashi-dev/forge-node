import type { EditorFind, EditorFindCommand, EditorFindFlags } from "../../contracts/runtime";

/** What the counter says: `3 of 12`, `12 results`, `No results`, or why not. */
export function findCounter(find: EditorFind, typed: string): string {
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
