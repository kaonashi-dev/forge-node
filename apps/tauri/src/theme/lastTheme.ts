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
