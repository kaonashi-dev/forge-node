import { For, createMemo } from "solid-js";
import { themeBase } from "../../theme/ThemeProvider";
import { editorPalette } from "../../theme/editorTheme";
import { palettes } from "../../theme/tokens";

/**
 * Both surfaces a theme has to answer for, at once.
 *
 * Not a Code/Terminal switch: the two share one palette, and someone choosing a
 * colour needs the diff and the ANSI row in the same glance rather than a
 * toggle that hides whichever of them the change just broke.
 */
export function ThemePreview(props: { contrast: number }) {
  const syntax = createMemo(() => editorPalette(themeBase()).scopes);
  const palette = () => palettes[themeBase()];
  const styles = createMemo(() => ({
    "--preview-keyword": syntax().keyword,
    "--preview-type": syntax().type,
    "--preview-string": syntax().string,
    "--preview-number": syntax().number,
    "--preview-comment": syntax().comment,
  }));

  return (
    <section class="theme-preview" aria-label="Theme preview" style={styles()}>
      <div class="theme-preview-toolbar">
        <span class="theme-preview-caption">Preview</span>
        <span class="theme-preview-filename">theme.ts</span>
      </div>
      <div class="theme-preview-diff" aria-label="Code diff color preview">
        <For each={[false, true]}>
          {(after) => (
            <div class="theme-preview-code" aria-label={after ? "After" : "Before"}>
              <div class="theme-preview-line">
                <span class="theme-preview-line-number">1</span>
                <code>
                  <span class="theme-preview-keyword">const</span> themePreview:{" "}
                  <span class="theme-preview-type">ThemeConfig</span> = {"{"}
                </code>
              </div>
              <For each={["surface", "accent", "contrast"] as const}>
                {(field, index) => (
                  <div
                    class={`theme-preview-line ${after ? "theme-preview-added" : "theme-preview-removed"}`}
                  >
                    <span class="theme-preview-line-number">{index() + 2}</span>
                    <span class="theme-preview-sign" aria-hidden="true">
                      {after ? "+" : "−"}
                    </span>
                    <code>
                      {"  "}
                      {field}:{" "}
                      {field === "contrast" ? (
                        <span class="theme-preview-number">{after ? props.contrast : 42}</span>
                      ) : (
                        <span class="theme-preview-string">
                          "
                          {field === "surface"
                            ? after
                              ? "sidebar-elevated"
                              : "sidebar"
                            : after
                              ? palette().accent
                              : palette().blue}
                          "
                        </span>
                      )}
                      ,
                    </code>
                  </div>
                )}
              </For>
              <div class="theme-preview-line">
                <span class="theme-preview-line-number">5</span>
                <code>{"};"}</code>
              </div>
            </div>
          )}
        </For>
      </div>
      <div class="theme-preview-toolbar">
        <span class="theme-preview-caption">Terminal</span>
        <span class="theme-preview-filename">zsh</span>
      </div>
      <div class="theme-preview-terminal" aria-label="Terminal color preview">
        <div>
          <span class="theme-terminal-path">~/forge</span>{" "}
          <span class="theme-terminal-branch">main</span>{" "}
          <span class="theme-terminal-prompt">❯</span> git status --short
        </div>
        <div>
          <span class="theme-terminal-modified"> M</span> src/theme.ts
        </div>
        <div>
          <span class="theme-terminal-added">??</span> src/preview.ts
        </div>
        <div>
          <span class="theme-terminal-path">~/forge</span>{" "}
          <span class="theme-terminal-branch">main</span>{" "}
          <span class="theme-terminal-prompt">❯</span> bun test
        </div>
        <div>
          <span class="theme-terminal-success">✓</span> Theme colors look good{" "}
          <span class="theme-preview-comment">[12 ms]</span>
        </div>
        <div class="theme-terminal-colors" aria-label="Sixteen ANSI colors">
          <For each={palette().ansi}>
            {(color, index) => (
              <span style={{ background: color }} title={`ANSI ${index()}: ${color}`} />
            )}
          </For>
        </div>
      </div>
    </section>
  );
}
