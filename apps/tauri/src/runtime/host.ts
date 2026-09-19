import { invoke } from "@tauri-apps/api/core";
import type { ConfigPaths, ConnectedPayload, HostStatus, UpdateInfo } from "../contracts/runtime";

export async function hostStatus(): Promise<HostStatus> {
  return invoke<HostStatus>("host_status");
}

export async function connect(): Promise<ConnectedPayload | null> {
  return invoke<ConnectedPayload | null>("connect");
}

export async function reconnect(): Promise<void> {
  await invoke("reconnect");
}

/**
 * Where Forge reads its configuration from.
 *
 * Asked for on demand rather than carried on the snapshot: it is host state,
 * not daemon state, and only the settings screen ever wants it.
 */
export async function configPaths(): Promise<ConfigPaths> {
  return invoke<ConfigPaths>("config_paths");
}

/** Resolves on queue acceptance; execution failures arrive through runtime events. */
export async function sendRuntimeCommand(command: Record<string, unknown>): Promise<void> {
  await invoke("send_runtime_command", { command });
}

export async function refreshSnapshot(): Promise<void> {
  await sendRuntimeCommand({ type: "refresh_snapshot" });
}

export async function openInEditor(editor: string, path: string): Promise<void> {
  await sendRuntimeCommand({ type: "open_in_editor", editor, path });
}

export async function openInFileManager(path: string): Promise<void> {
  await sendRuntimeCommand({ type: "open_in_file_manager", path });
}

export async function openUrl(url: string): Promise<void> {
  await sendRuntimeCommand({ type: "open_url", url });
}

export async function pickDirectory(title?: string): Promise<string | null> {
  return (await invoke<string | null>("pick_directory", { title })) ?? null;
}

/** Ask the platform for a file to share. `null` means the user cancelled. */
export async function pickFile(title?: string, directory?: string): Promise<string | null> {
  return (await invoke<string | null>("pick_file", { title, directory })) ?? null;
}

/**
 * Force an update check, for the `Check for Updates…` menu item.
 *
 * `null` means "already the newest". The background schedule in the host stays
 * quiet about that; a check the user asked for is the one case worth saying.
 */
export async function checkForUpdate(): Promise<UpdateInfo | null> {
  return invoke<UpdateInfo | null>("check_for_update");
}

/**
 * Download the pending update, swap the bundle and relaunch.
 *
 * Resolves only on failure: on success the process is replaced. Progress
 * arrives on `shell:update`, not here.
 */
export async function installUpdate(): Promise<void> {
  await invoke("install_update");
}
