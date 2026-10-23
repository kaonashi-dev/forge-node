import type { ThemePreference } from "./ThemeProvider";

/**
 * A browser-local echo of the daemon's `ui.theme_base`, read before the first
 * paint. The daemon stays authoritative: its value replaces this one as soon
 * as the snapshot lands, so a stale or missing echo costs a flash, not a theme.
 */
const KEY = "forge.theme.last";

type Storage = Pick<globalThis.Storage, "getItem" | "setItem" | "removeItem">;

function local(): Storage | undefined {
  try {
    return globalThis.localStorage;
  } catch {
    return undefined;
  }
}

export function recallTheme(storage: Storage | undefined = local()): ThemePreference | undefined {
  try {
    return (storage?.getItem(KEY) as ThemePreference | null) ?? undefined;
  } catch {
    return undefined;
  }
}

/** `undefined` forgets the echo, for a daemon that has no stored theme. */
export function rememberTheme(
  preference: ThemePreference | undefined,
  storage: Storage | undefined = local(),
): void {
  try {
    if (preference) storage?.setItem(KEY, preference);
    else storage?.removeItem(KEY);
  } catch {
    // Private windows and blocked storage only lose the head start.
  }
}

/** The finishes echoed beside the base, so the first paint is already at their size. */
export type Finishes = { glass: boolean; trim: boolean };

const FINISHES_KEY = "forge.theme.finishes";

export function recallFinishes(storage: Storage | undefined = local()): Finishes {
  try {
    const raw = storage?.getItem(FINISHES_KEY);
    const parsed: unknown = raw ? JSON.parse(raw) : undefined;
    if (parsed && typeof parsed === "object") {
      const { glass, trim } = parsed as Record<string, unknown>;
      return { glass: glass === true, trim: trim === true };
    }
  } catch {
    // A corrupt echo is the same as none.
  }
  return { glass: false, trim: false };
}

export function rememberFinishes(finishes: Finishes, storage: Storage | undefined = local()): void {
  try {
    storage?.setItem(FINISHES_KEY, JSON.stringify(finishes));
  } catch {
    // Private windows and blocked storage only lose the head start.
  }
}
