const COPY_LIMIT = 100;

/**
 * Split a file name into the part a copy renames and the extension it keeps.
 *
 * A leading dot is the name, not an extension: `.env.staging` stays whole.
 * Anything else keeps only the last suffix, the same cut a rename selects.
 */
function stemAndExtension(name: string): { stem: string; extension: string } {
  if (name.startsWith(".")) return { stem: name, extension: "" };
  const dot = name.lastIndexOf(".");
  if (dot <= 0) return { stem: name, extension: "" };
  return { stem: name.slice(0, dot), extension: name.slice(dot) };
}

function copyStem(base: string, n: number): string {
  return n <= 1 ? `${base} copy` : `${base} copy ${n}`;
}

/**
 * The next free sibling name for a copy of `name`.
 *
 * `notes.md` becomes `notes copy.md`, then `notes copy 2.md`. A name that is
 * already a copy increments instead of stacking another "copy". `null` means
 * every candidate through `copy 100` is taken.
 */
export function duplicateFileName(
  name: string,
  taken: (candidate: string) => boolean,
): string | null {
  const { stem, extension } = stemAndExtension(name);
  const numbered = /^(.*) copy (\d+)$/.exec(stem);
  let base = stem;
  let n = 1;
  if (numbered) {
    base = numbered[1];
    n = Number(numbered[2]) + 1;
  } else if (stem.endsWith(" copy")) {
    const trimmed = stem.slice(0, -" copy".length);
    if (trimmed) {
      base = trimmed;
      n = 2;
    }
  }
  if (!Number.isSafeInteger(n) || n < 1) return null;
  for (; n <= COPY_LIMIT; n++) {
    const candidate = `${copyStem(base, n)}${extension}`;
    if (!taken(candidate)) return candidate;
  }
  return null;
}
