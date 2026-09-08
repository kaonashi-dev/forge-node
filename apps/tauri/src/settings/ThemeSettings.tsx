import { For, Show, createMemo, createSignal } from "solid-js";
import { setAppState } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import { THEME_BASE_KEY } from "../shell/layout";
import {
  applyThemeBase,
  themeBase,
  themePreference,
  type ThemePreference,
} from "../theme/ThemeProvider";
import { MAX_THEME_BYTES, parseCustomTheme } from "../theme/customTheme";
import { baseIsLight, palettes, type ThemeBaseId } from "../theme/tokens";
import { Button, RadioGroup, TextArea } from "../ui";
import { Group } from "./SettingsLayout";

const CUSTOM_KEY = "ui.custom_theme";

export function ThemeSettings() {
  let fileInput: HTMLInputElement | undefined;
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [source, setSource] = createSignal("");
  const [savedCustom, setSavedCustom] = createSignal(
    themePreference().startsWith("{")
      ? themePreference()
      : (forgeStore.app_state[CUSTOM_KEY] ?? ""),
  );
  const custom = createMemo(() => {
    const saved = savedCustom();
    try {
      return { source: saved as ThemePreference, theme: parseCustomTheme(saved) };
    } catch {
      return undefined;
    }
  });
  const bases = Object.keys(palettes) as ThemeBaseId[];

  async function choose(next: ThemePreference): Promise<boolean> {
    if (busy()) return false;
    const previous = themePreference();
    setBusy(true);
    setError("");
    applyThemeBase(next);
    try {
      await setAppState(THEME_BASE_KEY, next);
      return true;
    } catch (cause) {
      applyThemeBase(previous);
      setError(`Could not request theme change: ${String(cause)}`);
      return false;
    } finally {
      setBusy(false);
    }
  }

  async function importTheme(text: string): Promise<void> {
    if (busy()) return;
    setError("");
    try {
      const next = JSON.stringify(parseCustomTheme(text)) as ThemePreference;
      const previous = savedCustom();
      setSavedCustom(next);
      if (!(await choose(next))) {
        setSavedCustom(previous);
        return;
      }
      setSource("");
      setBusy(true);
      await setAppState(CUSTOM_KEY, next);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  function downloadTemplate(): void {
    const data = {
      name: "My theme",
      mode: baseIsLight(themeBase()) ? "light" : "dark",
      palette: palettes[themeBase()],
    };
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(data, null, 2)], { type: "application/json" }),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = "forge-theme.json";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  return (
    <>
      <Group
        title="Color theme"
        description="Choose a palette for the shell, editor and terminal. System follows your device’s light or dark appearance."
      >
        <RadioGroup
          label="Color theme"
          class="theme-choices"
          itemClass="theme-choice"
          orientation="horizontal"
          value={themePreference()}
          onChange={(value) => void choose(value as ThemePreference)}
          options={[
            {
              value: "system",
              name: "Match system",
              colors: [palettes["gruvbox-hard"].bg, palettes["gruvbox-light"].bg],
            },
            ...bases.map((id) => ({
              value: id,
              name: id.replaceAll("-", " "),
              colors: [
                palettes[id].bg,
                palettes[id].accent,
                palettes[id].green,
                palettes[id].red,
                palettes[id].termFg,
              ],
            })),
            ...(custom()
              ? [
                  {
                    value: custom()!.source,
                    name: custom()!.theme.name,
                    colors: [
                      custom()!.theme.palette.bg,
                      custom()!.theme.palette.accent,
                      custom()!.theme.palette.text,
                    ],
                  },
                ]
              : []),
          ].map((option) => ({
            value: option.value,
            label: option.name,
            get disabled() {
              return busy();
            },
            render: () => (
              <>
                <span class="theme-swatches" aria-hidden="true">
                  <For each={option.colors}>
                    {(color) => <span class="theme-swatch" style={{ background: color }} />}
                  </For>
                </span>
                <span class="theme-name">{option.name}</span>
                <span class="theme-active">
                  {themePreference() === option.value ? "✓ Active" : "Select theme"}
                </span>
              </>
            ),
          }))}
        />
      </Group>
      <Group
        title="Custom theme"
        description="Import a Forge theme JSON file, or paste JSON below. Download the current palette as a template to edit its colors. One custom palette is saved; importing another replaces it."
      >
        <div class="theme-import">
          <Button onClick={downloadTemplate}>Download template</Button>
          <Button disabled={busy()} onClick={() => fileInput?.click()}>
            Import JSON file
          </Button>
          <input
            ref={fileInput}
            hidden
            aria-label="Import theme JSON file"
            type="file"
            accept=".json,application/json"
            disabled={busy()}
            onChange={async (event) => {
              const file = event.currentTarget.files?.[0];
              event.currentTarget.value = "";
              if (!file) return;
              if (file.size > MAX_THEME_BYTES) {
                setError("Theme JSON must be 16 KB or smaller.");
                return;
              }
              try {
                await importTheme(await file.text());
              } catch (cause) {
                setError(String(cause));
              }
            }}
          />
          <TextArea
            label="Theme JSON"
            class="theme-json"
            rows={7}
            maxLength={MAX_THEME_BYTES}
            value={source()}
            disabled={busy()}
            spellcheck={false}
            onChange={setSource}
          />
          <Button disabled={busy() || !source().trim()} onClick={() => void importTheme(source())}>
            Import and apply
          </Button>
          <Show when={error()}>
            <p role="alert">{error()}</p>
          </Show>
          <Show when={busy()}>
            <p role="status">Applying theme…</p>
          </Show>
        </div>
      </Group>
    </>
  );
}
