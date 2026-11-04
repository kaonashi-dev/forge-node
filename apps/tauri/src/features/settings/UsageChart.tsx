import { For, Show, createMemo, createSignal } from "solid-js";
import {
  chartScale,
  compactMoney,
  compactTokens,
  pointValue,
  seriesColor,
  shortDay,
  type StatsChart,
  type StatsMetric,
} from "./usageStats";

const VIEW_WIDTH = 960;
const VIEW_HEIGHT = 232;
const PLOT_TOP = 10;

/** The daily series as layered area lines, one per visible provider. */
export function UsageChart(props: { chart: StatsChart; metric: StatsMetric }) {
  const [hover, setHover] = createSignal<number | null>(null);

  const peak = createMemo(() =>
    props.chart.series.reduce(
      (max, series) =>
        series.points.reduce(
          (inner, point) => Math.max(inner, pointValue(point, props.metric)),
          max,
        ),
      0,
    ),
  );
  const scale = createMemo(() => chartScale(peak()));
  const stepX = createMemo(() =>
    props.chart.dates.length > 1 ? VIEW_WIDTH / (props.chart.dates.length - 1) : 0,
  );
  const formatValue = (value: number) =>
    props.metric === "cost" ? compactMoney(value) : compactTokens(value);
  const toY = (value: number) => {
    const max = scale().max;
    return max === 0 ? VIEW_HEIGHT : VIEW_HEIGHT - (value / max) * (VIEW_HEIGHT - PLOT_TOP);
  };
  const paths = createMemo(() =>
    props.chart.series.map((series) => {
      const points = series.points.map((point, index) => ({
        x: index * stepX(),
        y: toY(pointValue(point, props.metric)),
      }));
      const line = points
        .map(
          (point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(2)},${point.y.toFixed(2)}`,
        )
        .join(" ");
      return {
        ...series,
        line,
        area: `${line} L${VIEW_WIDTH},${VIEW_HEIGHT} L0,${VIEW_HEIGHT} Z`,
      };
    }),
  );
  const hoverIndex = () => hover();
  const hoveredDate = () => {
    const index = hoverIndex();
    return index === null ? null : (props.chart.dates[index] ?? null);
  };
  const hoveredRows = createMemo(() => {
    const index = hoverIndex();
    if (index === null) return [];
    return props.chart.series
      .map((series) => ({
        ...series,
        value: pointValue(series.points[index], props.metric),
      }))
      .sort((left, right) => right.value - left.value);
  });
  const hoveredTotal = () => hoveredRows().reduce((sum, series) => sum + series.value, 0);
  const tooltipLeft = () => {
    const count = props.chart.dates.length;
    const index = hoverIndex();
    if (index === null || count <= 1) return 50;
    return Math.min(88, Math.max(12, (index / (count - 1)) * 100));
  };
  const handleMove = (event: PointerEvent & { currentTarget: HTMLDivElement }) => {
    const count = props.chart.dates.length;
    if (count === 0) return;
    const bounds = event.currentTarget.getBoundingClientRect();
    if (bounds.width === 0) return;
    const fraction = Math.min(1, Math.max(0, (event.clientX - bounds.left) / bounds.width));
    setHover(Math.min(count - 1, Math.max(0, Math.round(fraction * (count - 1)))));
  };

  return (
    <div class="stats-chart">
      <div class="stats-chart-body">
        <div class="stats-chart-axis-y" aria-hidden="true">
          <For each={scale().ticks}>
            {(tick) => (
              <span style={{ top: `${(toY(tick) / VIEW_HEIGHT) * 100}%` }}>
                {formatValue(tick)}
              </span>
            )}
          </For>
        </div>
        <div
          class="stats-chart-plot"
          onPointerMove={handleMove}
          onPointerLeave={() => setHover(null)}
        >
          <svg
            class="stats-chart-svg"
            viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={chartLabel(props.chart, props.metric)}
          >
            <For each={scale().ticks}>
              {(tick) => (
                <line
                  class="stats-chart-grid"
                  x1={0}
                  x2={VIEW_WIDTH}
                  y1={toY(tick)}
                  y2={toY(tick)}
                  vector-effect="non-scaling-stroke"
                />
              )}
            </For>
            <For each={paths()}>
              {(series) => (
                <path
                  class="stats-chart-area"
                  d={series.area}
                  style={{ color: seriesColor(series.colorSlot) }}
                />
              )}
            </For>
            <For each={paths()}>
              {(series) => (
                <path
                  class="stats-chart-line"
                  d={series.line}
                  style={{ color: seriesColor(series.colorSlot) }}
                  vector-effect="non-scaling-stroke"
                />
              )}
            </For>
            <Show when={hoverIndex() !== null}>
              <line
                class="stats-chart-crosshair"
                x1={(hoverIndex() ?? 0) * stepX()}
                x2={(hoverIndex() ?? 0) * stepX()}
                y1={PLOT_TOP}
                y2={VIEW_HEIGHT}
                vector-effect="non-scaling-stroke"
              />
            </Show>
          </svg>
          <Show when={hoveredDate()}>
            <div class="stats-chart-tooltip" style={{ left: `${tooltipLeft()}%` }}>
              <span class="stats-chart-tooltip-date">{shortDay(hoveredDate() ?? "")}</span>
              <For each={hoveredRows()}>
                {(series) => (
                  <span class="stats-chart-tooltip-row">
                    <span
                      class="stats-chart-swatch"
                      style={{ background: seriesColor(series.colorSlot) }}
                    />
                    <span class="stats-chart-tooltip-name forge-truncate">{series.name}</span>
                    <span class="stats-chart-tooltip-value">{formatValue(series.value)}</span>
                  </span>
                )}
              </For>
              <span class="stats-chart-tooltip-total">
                <span>Total</span>
                <span>{formatValue(hoveredTotal())}</span>
              </span>
            </div>
          </Show>
          <Show when={peak() === 0}>
            <p class="stats-chart-empty">
              {props.metric === "cost"
                ? "No priced activity in this window."
                : "No activity in this window."}
            </p>
          </Show>
        </div>
      </div>
      <div class="stats-chart-axis-x" aria-hidden="true">
        <span>{props.chart.dates[0] ? shortDay(props.chart.dates[0]) : ""}</span>
        <span>
          {props.chart.dates.length > 0
            ? shortDay(props.chart.dates[Math.floor(props.chart.dates.length / 2)] ?? "")
            : ""}
        </span>
        <span>
          {props.chart.dates.length > 0
            ? shortDay(props.chart.dates[props.chart.dates.length - 1] ?? "")
            : ""}
        </span>
      </div>
    </div>
  );
}

function chartLabel(chart: StatsChart, metric: StatsMetric): string {
  const reading = metric === "cost" ? "estimated cost" : "tokens";
  return chart.series.length === 1
    ? `Daily ${reading} for ${chart.series[0]?.name ?? "this scope"}`
    : `Daily ${reading} by provider`;
}
