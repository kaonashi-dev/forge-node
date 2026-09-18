import type { AgentProfile } from "../../contracts/runtime";
import { sendRuntimeCommand } from "../../runtime/host";
import { sendWorkbenchCommand } from "../../runtime/workbench";

export async function loadUsageAnalytics(windowDays: number | null = null): Promise<void> {
  await sendWorkbenchCommand({ type: "load_usage_analytics", window_days: windowDays });
}

// --- Agents -----------------------------------------------------------------

/** `null` refreshes every provider. */
export async function refreshDetection(provider: string | null = null): Promise<void> {
  await sendRuntimeCommand({ type: "refresh_detection", provider });
}

export async function saveAgentProfile(profile: AgentProfile): Promise<void> {
  await sendRuntimeCommand({ type: "save_agent_profile", profile });
}

export async function removeAgentProfile(profile: string): Promise<void> {
  await sendRuntimeCommand({ type: "remove_agent_profile", profile });
}

/** Pin the executable used for a provider, or clear the override with `null`. */
export async function setProviderExecutable(provider: string, path: string | null): Promise<void> {
  await sendRuntimeCommand({ type: "set_provider_executable", provider, path });
}

/**
 * Ask the daemon to stop.
 *
 * The connection goes with it, so the shell reconnects afterwards: it finds
 * nothing, says so, and keeps retrying until a daemon is started again.
 */
export async function stopDaemon(killSessions = false): Promise<void> {
  await sendRuntimeCommand({ type: "stop_daemon", kill_sessions: killSessions });
}

/**
 * Persist one GUI preference.
 *
 * The daemon owns it: this writes, and the value comes back on the next
 * snapshot rather than the WebView keeping a second copy.
 */
export async function setAppState(key: string, value: string): Promise<void> {
  await sendRuntimeCommand({ type: "set_app_state", key, value });
}

/** Restore Forge-owned state while retaining config, logs, and repository files. */
export async function factoryReset(): Promise<void> {
  await sendRuntimeCommand({ type: "factory_reset" });
}
