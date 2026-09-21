import { ColorArea } from "@kobalte/core/color-area";
import { ColorSlider } from "@kobalte/core/color-slider";
import { parseColor, type Color } from "@kobalte/core/colors";
import { Popover } from "@kobalte/core/popover";
import { For, Show, createMemo } from "solid-js";
import { Button } from "./Button";

export type ColorPickerProps = {
  /** Six-digit hex, `#458588`. */
  value: string;
  /** Names the colour being edited, for the trigger and the dialog. */
  label: string;
  disabled?: boolean;
  /** Quick picks — usually the palette this colour belongs to. */
  swatches?: readonly string[];
  /** Live, on every step of a drag. */
  onInput: (value: string) => void;
  /** The end of an interaction: the value worth persisting. */
  onCommit: (value: string) => void;
};

const FALLBACK = "#000000";

/**
 * A colour well that opens a picker and stays open.
 *
 * `<input type="color">` hands the choice to the platform, and on macOS that
 * panel closes on every pick — which makes comparing two shades a fight. This
 * owns the surface instead: saturation area, hue slider and the palette's own
 * swatches, all live against the page behind them, dismissed only by Escape or
 * a click outside. The system panel stays one button away, because it is where
 * the eyedropper lives.
 */
export function ColorPicker(props: ColorPickerProps) {
  let native: HTMLInputElement | undefined;
  const color = createMemo(() => toHsl(props.value));
  const emit = (next: Color, done: boolean) => {
    const hex = next.toFormat("rgb").toString("hex");
    if (done) props.onCommit(hex);
    else props.onInput(hex);
  };

  return (
    <Popover placement="bottom-start" gutter={8}>
      <Popover.Trigger
        class="forge-color-well"
        aria-label={`${props.label} color`}
        disabled={props.disabled}
        style={{ background: props.value }}
      />
      <Popover.Portal>
        <Popover.Content class="forge-color-picker" aria-label={`${props.label} color picker`}>
          <ColorArea
            class="forge-color-area"
            colorSpace="hsl"
            xChannel="saturation"
            yChannel="lightness"
            value={color()}
            onChange={(next) => emit(next, false)}
            onChangeEnd={(next) => emit(next, true)}
          >
            <ColorArea.Background class="forge-color-area-bg">
              <ColorArea.Thumb class="forge-color-thumb">
                <ColorArea.HiddenInputX />
                <ColorArea.HiddenInputY />
              </ColorArea.Thumb>
            </ColorArea.Background>
          </ColorArea>
          <ColorSlider
            class="forge-color-slider"
            channel="hue"
            colorSpace="hsl"
            value={color()}
            onChange={(next) => emit(next, false)}
            onChangeEnd={(next) => emit(next, true)}
          >
            <ColorSlider.Track class="forge-color-track">
              <ColorSlider.Thumb class="forge-color-thumb">
                <ColorSlider.Input />
              </ColorSlider.Thumb>
            </ColorSlider.Track>
          </ColorSlider>
          <Show when={props.swatches?.length}>
            <div class="forge-color-swatches" role="group" aria-label="Palette colors">
              <For each={props.swatches}>
                {(swatch) => (
                  <button
                    type="button"
                    class="forge-color-swatch"
                    title={swatch}
                    aria-label={swatch}
                    aria-pressed={swatch.toLowerCase() === props.value.toLowerCase()}
                    style={{ background: swatch }}
                    onClick={() => props.onCommit(swatch)}
                  />
                )}
              </For>
            </div>
          </Show>
          <div class="forge-color-footer">
            <span class="forge-color-value">{props.value.toUpperCase()}</span>
            {/* The one thing this picker cannot draw: the system eyedropper. */}
            <Button size="sm" onClick={() => native?.click()}>
              System picker…
            </Button>
            <input
              ref={native}
              class="forge-visually-hidden"
              type="color"
              tabindex={-1}
              aria-hidden="true"
              value={props.value}
              onInput={(event) => props.onInput(event.currentTarget.value)}
              onChange={(event) => props.onCommit(event.currentTarget.value)}
            />
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover>
  );
}

/** Kobalte works in the space its channels name, and a hex parses as `rgb`. */
function toHsl(value: string): Color {
  try {
    return parseColor(value).toFormat("hsl");
  } catch {
    return parseColor(FALLBACK).toFormat("hsl");
  }
}
