import { For } from "solid-js";
import { HUE_COUNT } from "../../theme/tokens";

const ANSI_SLOTS = Array.from({ length: 16 }, (_, index) => index);
const HUE_SLOTS = Array.from({ length: HUE_COUNT }, (_, index) => index);

/**
 * The terminal under the chosen theme: a prompt, a command, its output and the
 * sixteen ANSI slots, then the nine project hues as the rail draws them, all
 * read from the live variables so a custom palette previews as it is edited.
 */
export function ThemePreview() {
  return (
    <section class="theme-preview" aria-label="Theme preview">
      <div class="theme-preview-toolbar">
        <span class="hue-mark" aria-hidden="true" />
        <span class="forge-state-marker forge-state-dot forge-icon-green" aria-hidden="true" />
        Terminal 1
      </div>
      <div class="theme-preview-terminal" aria-label="Terminal color preview">
        <div>
          <span class="theme-terminal-path">forge-node</span> on{" "}
          <span class="theme-terminal-branch">main</span> via{" "}
          <span class="theme-terminal-version">v1.89.0</span>
        </div>
        <div>
          <span class="theme-terminal-prompt">❯</span> scripts/dev check
        </div>
        <div class="theme-terminal-quiet">{"   Finished in 41.2s"}</div>
      </div>
      <div class="theme-terminal-colors" aria-label="Sixteen ANSI colors">
        <For each={ANSI_SLOTS}>
          {(slot) => (
            <span style={{ background: `var(--forge-ansi-${slot})` }} title={`ANSI ${slot}`} />
          )}
        </For>
      </div>
      <div class="theme-preview-hues" aria-label="Project hues">
        <span class="theme-preview-hues-label">Project hues</span>
        <For each={HUE_SLOTS}>
          {(slot) => (
            <span
              class="project-avatar hued"
              style={{ "--project-hue": `var(--hue-${slot})` }}
              aria-hidden="true"
            >
              {String.fromCharCode(65 + slot)}
            </span>
          )}
        </For>
      </div>
    </section>
  );
}
