import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { setProfilesStore } from "../../../features/settings/profiles";

export async function bindSettingsEvents(): Promise<UnlistenFn[]> {
  const unlisten = await listen<{ profile: string; error: string | null }>(
    "runtime:profile_save",
    (event) => {
      setProfilesStore("profileSave", event.payload);
    },
  );
  return [unlisten];
}
