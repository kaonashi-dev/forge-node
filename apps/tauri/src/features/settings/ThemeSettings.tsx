import { For, Show, createMemo, createSignal } from "solid-js";
import { setAppState } from "./commands";
import { forgeStore } from "../../state/forgeStore";
import { THEME_BASE_KEY } from "../../state/preferences";
import {
  applyThemeBase,
  themeBase,
  themePreference,
  type ThemePreference,
} from "../../theme/ThemeProvider";
import { MAX_THEME_BYTES, parseCustomTheme, type CustomTheme } from "../../theme/customTheme";
import { hex, on, parseHex } from "../../theme/mix";
import {
  DEFAULT_CONTRAST,
  normalizeThemeColor,
  themeVariant,
  withThemeColor,
  type ThemeColor,
} from "../../theme/themeVariants";
import {
  baseIsLight,
  palettes,
  THEME_LABELS,
  type Palette,
  type ThemeBaseId,
} from "../../theme/tokens";
import { Button, Disclosure, Select, TextArea } from "../../ui/index";
// Not through `ui/index`: the barrel is in the initial chunk, and the colour
// primitives belong with Settings, which is loaded when it is first opened.
import { ColorPicker } from "../../ui/ColorPicker";
import { Group } from "./SettingsLayout";
import { ThemePreview } from "./ThemePreview";

const CUSTOM_KEY = "ui.custom_theme";
const COLORS: { key: ThemeColor; label: string }[] = [
  { key: "accent", label: "Accent" },
  { key: "bg", label: "Background" },
  { key: "text", label: "Foreground" },
];

export function ThemeSettings() {
  let fileInput: HTMLInputElement | undefined;
  let persistedPreference = themePreference();
  const [busy, setBusy] = createSignal(false);
  const [error, setError] = createSignal("");
  const [status, setStatus] = createSignal("");
  const [source, setSource] = createSignal("");
  const [savedCustom, setSavedCustom] = createSignal(
    themePreference().startsWith("{")
      ? themePreference()
      : (forgeStore.app_state[CUSTOM_KEY] ?? ""),
  );
  const custom = createMemo(() => {
    const saved = themePreference().startsWith("{") ? themePreference() : savedCustom();
    try {
      return { source: saved as ThemePreference, theme: parseCustomTheme(saved) };
    } catch {
      return undefined;
    }
  });
  const active = createMemo<CustomTheme>(() => {
    if (themePreference().startsWith("{")) return parseCustomTheme(themePreference());
    const id = themeBase();
    return {
      name: THEME_LABELS[id],
      mode: baseIsLight(id) ? "light" : "dark",
      palette: palettes[id],
    };
  });
  const colors = () => active().adjustments?.palette ?? active().palette;
  const contrast = () => active().adjustments?.contrast ?? DEFAULT_CONTRAST;
  const selected = () => (themePreference().startsWith("{") ? "custom" : themePreference());
  const bases = Object.keys(palettes) as ThemeBaseId[];
  const options = createMemo(() => [
    { value: "system", label: "Match system" },
    ...bases.map((id) => ({ value: id, label: THEME_LABELS[id] })),
    ...(custom() ? [{ value: "custom", label: `${custom()!.theme.name} · Custom` }] : []),
  ]);

  async function choose(next: ThemePreference): Promise<void> {
    if (busy()) return;
    setBusy(true);
    setError("");
    setStatus("");
    applyThemeBase(next);
    try {
      await setAppState(THEME_BASE_KEY, next);
      persistedPreference = next;
      if (next.startsWith("{")) {
        setSavedCustom(next);
        await setAppState(CUSTOM_KEY, next);
      }
    } catch (cause) {
      applyThemeBase(persistedPreference);
      setError(`Could not save theme: ${String(cause)}`);
    } finally {
      setBusy(false);
    }
  }

  function preview(
    key?: ThemeColor,
    value?: string,
    nextContrast = contrast(),
    origin = colors(),
  ): void {
    if (busy()) return;
    try {
      const palette = key && value ? withThemeColor(origin, key, value) : origin;
      applyThemeBase(
        JSON.stringify(themeVariant(active(), palette, nextContrast)) as ThemePreference,
      );
      setError("");
      setStatus("");
    } catch (cause) {
      setError(String(cause));
    }
  }

  function commit(): void {
    if (themePreference() !== persistedPreference) void choose(themePreference());
  }

  function editHex(key: ThemeColor, input: HTMLInputElement): void {
    const value = normalizeThemeColor(input.value);
    if (!value) {
      input.value = colors()[key].toUpperCase();
      setError("Use a six-digit hex color, such as #458588.");
      return;
    }
    preview(key, value);
    commit();
  }

  async function importTheme(text: string): Promise<void> {
    if (busy()) return;
    try {
      const next = JSON.stringify(parseCustomTheme(text)) as ThemePreference;
      await choose(next);
      if (persistedPreference === next) setSource("");
    } catch (cause) {
      setError(String(cause));
    }
  }

  async function copyTheme(): Promise<void> {
    try {
      await navigator.clipboard.writeText(JSON.stringify(active(), null, 2));
      setStatus("Theme copied.");
      setError("");
    } catch (cause) {
      setError(`Could not copy theme: ${String(cause)}`);
    }
  }

  function downloadTemplate(): void {
    const url = URL.createObjectURL(
      new Blob([JSON.stringify(active(), null, 2)], { type: "application/json" }),
    );
    const link = document.createElement("a");
    link.href = url;
    link.download = "forge-theme.json";
    link.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  }

  return (
    <div class="theme-editor">
      <ThemePreview contrast={contrast()} />
      <Group class="theme-controls">
        <div class="theme-control-row theme-selector-row">
          <span class="theme-control-label">
            {baseIsLight(themeBase()) ? "Light theme" : "Dark theme"}
          </span>
          <div class="theme-selector-actions">
            <Button disabled={busy()} onClick={() => fileInput?.click()}>
              Import
            </Button>
            <Button onClick={() => void copyTheme()}>Copy theme</Button>
          </div>
          <div class="theme-selector">
            <span class="theme-type-sample" aria-hidden="true">
              Aa
            </span>
            <Select
              aria-label="Color theme"
              value={selected()}
              options={options()}
              disabled={busy()}
              onChange={(value) => {
                // The select also reconciles its value when a live preview becomes custom.
                if (value !== selected()) {
                  void choose(value === "custom" ? custom()!.source : (value as ThemePreference));
                }
              }}
            />
          </div>
        </div>
        <For each={COLORS}>
          {({ key, label }) => {
            let colorOrigin: Palette | undefined;
            return (
              <div class="theme-control-row">
                <label class="theme-control-label" for={`theme-${key}-hex`}>
                  {label}
                </label>
                <div
                  class="theme-color-field"
                  style={{
                    background: colors()[key],
                    color: hex(on(parseHex(colors()[key]), 0, 0xffffff)),
                  }}
                >
                  <ColorPicker
                    label={label}
                    value={colors()[key]}
                    disabled={busy()}
                    swatches={colors().ansi}
                    onInput={(value) => {
                      colorOrigin ??= colors();
                      preview(key, value, contrast(), colorOrigin);
                    }}
                    onCommit={(value) => {
                      preview(key, value, contrast(), colorOrigin ?? colors());
                      colorOrigin = undefined;
                      commit();
                    }}
                  />
                  <input
                    id={`theme-${key}-hex`}
                    class="theme-color-hex"
                    type="text"
                    spellcheck={false}
                    autocomplete="off"
                    maxLength={7}
                    value={colors()[key].toUpperCase()}
                    disabled={busy()}
                    onChange={(event) => editHex(key, event.currentTarget)}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") event.currentTarget.blur();
                      if (event.key === "Escape") {
                        event.currentTarget.value = colors()[key].toUpperCase();
                        event.currentTarget.blur();
                      }
                    }}
                  />
                </div>
              </div>
            );
          }}
        </For>
        <div class="theme-control-row">
          <label class="theme-control-label" for="theme-contrast">
            Contrast
          </label>
          <div class="theme-contrast-control">
            <input
              id="theme-contrast"
              type="range"
              min="0"
              max="100"
              step="1"
              value={contrast()}
              disabled={busy()}
              aria-describedby="theme-contrast-description"
              onInput={(event) => preview(undefined, undefined, event.currentTarget.valueAsNumber)}
              onChange={commit}
            />
            <output for="theme-contrast">{contrast()}</output>
          </div>
        </div>
      </Group>
      <p class="theme-editor-hint" id="theme-contrast-description">
        Contrast adjusts panel separation and secondary text. Choose the original theme to reset
        your changes.
      </p>
      <Show when={error()}>
        <p class="theme-editor-error" role="alert">
          {error()}
        </p>
      </Show>
      <span class="theme-editor-status" role="status">
        {busy() ? "Saving theme…" : status()}
      </span>
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
      <Disclosure summary="Theme JSON">
        <div class="theme-import">
          <Button onClick={downloadTemplate}>Download theme</Button>
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
        </div>
      </Disclosure>
    </div>
  );
}
