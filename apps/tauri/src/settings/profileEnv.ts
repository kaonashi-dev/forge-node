// Parsing the environment block of a launch profile (§13.4).
//
// Its own module and not a helper inside the form: it is the only part of that
// screen with a right and a wrong answer, and a component module cannot be
// imported by a node test without pulling Solid's server build in with it.

/**
 * Variables a profile may never set (`domain::RESERVED_PROFILE_VARS`).
 *
 * Forge owns the terminal contract with the child process (§13.3): a profile
 * that redefined these would break the emulator rather than configure the
 * agent, so the form refuses them here instead of letting the daemon do it.
 */
const RESERVED_VARS = ["TERM", "TERMINFO", "COLORTERM", "FORGE_SESSION_ID", "FORGE_WORKSPACE"];

/** `domain::AgentProfile::is_valid_var_name`. */
function validVarName(name: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(name);
}

/**
 * `NAME=value` lines into pairs, with the first thing wrong about them.
 *
 * A rejected form is better than a profile the daemon silently drops half of:
 * the reserved names in particular fail *after* the process has started, as a
 * terminal that renders wrong rather than as an error anyone connects to this
 * screen.
 */
export function parseEnv(text: string): { pairs: [string, string][]; error: string | null } {
  const pairs: [string, string][] = [];
  for (const raw of text.split("\n")) {
    const line = raw.trim();
    if (line === "") continue;
    const at = line.indexOf("=");
    if (at <= 0) return { pairs, error: `"${line}" is not NAME=value.` };
    const name = line.slice(0, at);
    if (!validVarName(name)) return { pairs, error: `"${name}" is not a variable name.` };
    if (RESERVED_VARS.includes(name)) {
      return { pairs, error: `${name} is Forge Node's to set, not a profile's.` };
    }
    pairs.push([name, line.slice(at + 1)]);
  }
  return { pairs, error: null };
}
