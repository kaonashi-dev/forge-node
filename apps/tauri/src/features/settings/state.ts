import { createStore } from "solid-js/store";
import type { UsageAnalytics } from "../../contracts/workbench";

export const [settingsStore, setSettingsStore] = createStore({
  usage: null as UsageAnalytics | null,
  usageError: null as string | null,
});
