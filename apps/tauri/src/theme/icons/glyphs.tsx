import { For, splitProps, type Component } from "solid-js";
import type { LucideProps } from "lucide-solid";

/**
 * One stroke on Forge's own 16px grid. `region` is the part that fills at 20%
 * when the glyph names the chosen thing (`.glyph-region` in `base.css`), so a
 * selected view never depends on colour alone.
 */
type Shape =
  | { path: string; region?: boolean }
  | { circle: readonly [cx: number, cy: number, r: number]; region?: boolean }
  | { rect: readonly [x: number, y: number, w: number, h: number, rx: number]; region?: boolean };

function glyph(shapes: readonly Shape[]): Component<LucideProps> {
  return (props) => {
    const [local, rest] = splitProps(props, [
      "size",
      "strokeWidth",
      "color",
      "class",
      "absoluteStrokeWidth",
      "children",
    ]);
    return (
      <svg
        xmlns="http://www.w3.org/2000/svg"
        width={local.size ?? 16}
        height={local.size ?? 16}
        viewBox="0 0 16 16"
        fill="none"
        stroke={local.color ?? "currentColor"}
        stroke-width={local.strokeWidth ?? 1.5}
        stroke-linecap="round"
        stroke-linejoin="round"
        class={`forge-glyph ${local.class ?? ""}`}
        {...rest}
      >
        <For each={shapes}>
          {(shape) => {
            const region = shape.region ? "glyph-region" : undefined;
            if ("path" in shape) return <path d={shape.path} class={region} />;
            if ("circle" in shape) {
              const [cx, cy, r] = shape.circle;
              return <circle cx={cx} cy={cy} r={r} class={region} />;
            }
            const [x, y, width, height, rx] = shape.rect;
            return <rect x={x} y={y} width={width} height={height} rx={rx} class={region} />;
          }}
        </For>
      </svg>
    );
  };
}

export const GLYPHS = {
  projects: glyph([
    { path: "M8 2L1.8 5.2 8 8.4l6.2-3.2z", region: true },
    { path: "M1.8 8L8 11.2 14.2 8M1.8 10.8L8 14l6.2-3.2" },
  ]),
  files: glyph([
    {
      path: "M1.9 4.1a1.2 1.2 0 011.2-1.2h2.6l1.4 1.8h5a1.2 1.2 0 011.2 1.2v6.2a1.2 1.2 0 01-1.2 1.2H3.1a1.2 1.2 0 01-1.2-1.2z",
      region: true,
    },
  ]),
  history: glyph([{ circle: [8, 8, 6], region: true }, { path: "M8 4.6V8l2.3 1.6" }]),
  pullRequest: glyph([
    { circle: [4.2, 3.8, 1.7] },
    { circle: [4.2, 12.2, 1.7] },
    { path: "M4.2 5.5v5" },
    { circle: [11.8, 12.2, 1.7], region: true },
    { path: "M11.8 10.5V6.4a2 2 0 00-2-2H7.6M9.2 3l-1.6 1.4 1.6 1.4" },
  ]),
  git: glyph([
    { circle: [4.4, 3.9, 1.8] },
    { circle: [4.4, 12.1, 1.8] },
    { circle: [11.6, 6.6, 1.8], region: true },
    { path: "M4.4 5.7v4.6M9.9 8a5 5 0 01-4.1 2.3" },
  ]),
  worktree: glyph([
    { rect: [7.4, 8.4, 6.6, 5.4, 1.4], region: true },
    { circle: [4, 3.6, 1.6] },
    { path: "M4 5.2v4.4a1.4 1.4 0 001.4 1.4h2" },
  ]),
  terminal: glyph([
    { rect: [1.8, 2.6, 12.4, 10.8, 2.2], region: true },
    { path: "M4.6 6l2 2-2 2M8.4 10.2h3" },
  ]),
  code: glyph([
    {
      path: "M9.4 1.9H4.2a1.4 1.4 0 00-1.4 1.4v9.4a1.4 1.4 0 001.4 1.4h7.6a1.4 1.4 0 001.4-1.4V5.9z",
      region: true,
    },
    { path: "M9.4 1.9v4h3.8M6.4 8.8L5.2 10l1.2 1.2M9.6 8.8l1.2 1.2-1.2 1.2" },
  ]),
  commit: glyph([{ circle: [8, 8, 2.6], region: true }, { path: "M1.6 8h3.8M10.6 8h3.8" }]),
  fetch: glyph([
    { path: "M8 2.4v7.4M5 6.8L8 9.8l3-3" },
    { path: "M2.6 10.6v1.6a1.4 1.4 0 001.4 1.4h8a1.4 1.4 0 001.4-1.4v-1.6", region: true },
  ]),
  push: glyph([
    { path: "M8 10.2V2.8M5 5.8L8 2.8l3 3" },
    { path: "M2.6 10.6v1.6a1.4 1.4 0 001.4 1.4h8a1.4 1.4 0 001.4-1.4v-1.6", region: true },
  ]),
  patch: glyph([
    { rect: [2, 2, 12, 12, 2.4], region: true },
    { path: "M5.2 6h3M6.7 4.5v3M5.2 10.6h3" },
  ]),
  review: glyph([{ path: "M2.4 4l1.3 1.3L6 3M2.4 9.4l1.3 1.3L6 8.4M8.6 4.4h5M8.6 9.8h5" }]),
  merged: glyph([
    { circle: [4.4, 3.8, 1.7] },
    { circle: [4.4, 12.2, 1.7] },
    { circle: [11.8, 8, 1.7], region: true },
    { path: "M4.4 5.5v5M4.6 5.6c.6 1.6 2.4 2.4 5.5 2.4" },
  ]),
  conflict: glyph([
    { path: "M2.6 4.4h4.6l2 3.6-2 3.6H2.6M13.4 4.4H11M13.4 11.6H11", region: true },
  ]),
  handoff: glyph([
    {
      path: "M13.4 9.6a1.4 1.4 0 01-1.4 1.4H5.2l-2.6 2.4V3.8a1.4 1.4 0 011.4-1.4h8a1.4 1.4 0 011.4 1.4z",
      region: true,
    },
    { path: "M6 6.8h4M8.4 5l1.8 1.8-1.8 1.8" },
  ]),
  spawnChild: glyph([
    { circle: [4.4, 3.8, 1.7] },
    { path: "M4.4 5.5v3a2 2 0 002 2h1.6" },
    { rect: [8.6, 8.2, 5.4, 4.6, 1.2], region: true },
    { path: "M11.3 9.4v2.2M10.2 10.5h2.2" },
  ]),
  sharedFiles: glyph([
    {
      path: "M6.6 9.4l2.8-2.8M5.4 7.4L4.2 8.6a2.4 2.4 0 003.4 3.4l1.2-1.2M10.6 8.6l1.2-1.2a2.4 2.4 0 00-3.4-3.4L7.2 5.2",
    },
  ]),
  needsYou: glyph([
    { path: "M4 11.2V7.4a4 4 0 018 0v3.8l1.2 1.4H2.8z", region: true },
    { path: "M6.6 13.6a1.6 1.6 0 002.8 0" },
  ]),
  idle: glyph([{ path: "M12.8 9.8A5.6 5.6 0 016.2 3.2a5.6 5.6 0 106.6 6.6z", region: true }]),
  exited: glyph([{ rect: [3.4, 3.4, 9.2, 9.2, 2], region: true }]),
  failed: glyph([
    { circle: [8, 8, 6], region: true },
    { path: "M5.8 5.8l4.4 4.4M10.2 5.8l-4.4 4.4" },
  ]),
  split: glyph([{ rect: [1.8, 2.6, 12.4, 10.8, 2], region: true }, { path: "M8 2.6v10.8" }]),
  palette: glyph([
    { rect: [2, 2, 12, 12, 3], region: true },
    { path: "M5.2 6.4l1.6 1.6-1.6 1.6M8.4 10.2h2.4" },
  ]),
  done: glyph([{ path: "M3.4 8.4l3 3 6.2-6.6" }]),
} as const;
