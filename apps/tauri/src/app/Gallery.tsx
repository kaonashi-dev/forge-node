import { For, Show, createSignal, type JSX } from "solid-js";
import {
  AlertDialog,
  Badge,
  Button,
  Checkbox,
  Dialog,
  Disclosure,
  Menu,
  RadioGroup,
  SearchField,
  Breadcrumbs,
  buttonClass,
  Card,
  IconButton,
  Kbd,
  ListCard,
  Progress,
  Select,
  Skeleton,
  Switch,
  Tabs,
  TextField,
  Tooltip,
  toast,
} from "../ui/index";
import { Icon, type ForgeIconName } from "../theme/icons/index";
import { playOnce } from "../theme/motion";
import { HUE_COUNT } from "../theme/tokens";
import { WorkMarker } from "../features/sessions/markers";
import { applyThemeBase, themeBase } from "../theme/ThemeProvider";
import { palettes, THEME_LABELS, type ThemeBaseId } from "../theme/tokens";
import { DENSITIES, applyDensity, type Density } from "../theme/density";
import "./gallery.css";

/** `?gallery`: every token and kit control on one page, painted from the live tokens. */

const bases = Object.keys(palettes) as ThemeBaseId[];

type Sample = readonly [token: string, label: string];

function sample(token: string): JSX.CSSProperties {
  return { "--sample": `var(${token})` };
}

function resolved(name: string): string {
  if (typeof window === "undefined") return "";
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

function Section(props: { title: string; children: JSX.Element }) {
  return (
    <section class="gallery-section">
      <h2 class="forge-section-label gallery-section-label">{props.title}</h2>
      {props.children}
    </section>
  );
}

function Swatch(props: { token: string; ring?: boolean }) {
  // `applyTheme` rewrites the inline vars before `themeBase` settles, so reading
  // the signal here is what keeps the caption in step with the base.
  const value = () => {
    themeBase();
    return resolved(props.token);
  };
  return (
    <div class="gallery-tile">
      <div
        class="gallery-swatch"
        data-ring={props.ring ? "" : undefined}
        style={sample(props.token)}
      />
      <span class="forge-caption">{props.token}</span>
      <span class="gallery-value">{value()}</span>
    </div>
  );
}

function Grid(props: { wide?: boolean; children: JSX.Element }) {
  return (
    <div class={`gallery-grid ${props.wide ? "gallery-grid-wide" : ""}`}>{props.children}</div>
  );
}

function FillChip(props: { fill: string; fg: string; label: string }) {
  return (
    <div
      class="gallery-fill"
      style={{ "--sample": `var(${props.fill})`, "--sample-fg": `var(${props.fg})` }}
    >
      {props.label}
    </div>
  );
}

function TintChip(props: { tone: string }) {
  return (
    <div class="gallery-tile">
      <div
        class="gallery-tint"
        style={{
          "--sample": `var(--${props.tone}-tint-bg)`,
          "--sample-edge": `var(--${props.tone}-tint-border)`,
        }}
      >
        {props.tone}
      </div>
      <span class="forge-caption">--{props.tone}-tint-bg / -border</span>
    </div>
  );
}

const SURFACES = [
  "--bg-panel",
  "--bg-window",
  "--bg-sidebar",
  "--bg-strip",
  "--bg-sunken",
  "--bg-float",
  "--bg-raised",
  "--bg-overlay",
  "--bg-terminal",
  "--forge-editor",
];
const FOREGROUNDS = ["--fg-strong", "--fg-default", "--fg-muted", "--fg-subtle", "--forge-faint"];
const INTERACTION = [
  "--border-subtle",
  "--border-default",
  "--border-strong",
  "--line",
  "--bg-hover",
  "--bg-selected",
  "--bg-pressed",
];
const RADII: Sample[] = [
  ["--radius-2xs", "2xs"],
  ["--radius-xs", "xs"],
  ["--radius-sm", "sm"],
  ["--radius-md", "md"],
  ["--radius-list", "list"],
  ["--radius-lg", "lg"],
  ["--radius-palette", "palette"],
  ["--radius-full", "full"],
];
const CONTROL_HEIGHTS: Sample[] = [
  ["--control-xs", "control-xs"],
  ["--control-sm", "control-sm"],
  ["--control-md", "control-md"],
  ["--control-lg", "control-lg"],
  ["--row-h", "row-h"],
  ["--tree-row-h", "tree-row-h"],
  ["--tab-h", "tab-h"],
  ["--location-h", "location-h"],
  ["--strip-h", "strip-h"],
];
const TYPE_SCALE: Sample[] = [
  ["--text-mono", "mono — counts, keys, branches"],
  ["--text-xs", "xs — captions, group labels, footer"],
  ["--text-sm", "sm — rows, tabs, buttons, body"],
  ["--text-md", "md — a section heading"],
  ["--text-lg", "lg — a dialog title"],
  ["--text-xl", "xl — the page title"],
];
const WEIGHTS: Sample[] = [
  ["--weight-regular", "regular"],
  ["--weight-medium", "medium"],
  ["--weight-semibold", "semibold"],
  ["--weight-bold", "bold"],
];
const LEADINGS: Sample[] = [
  ["--leading-none", "none"],
  ["--leading-title", "title"],
  ["--leading-heading", "heading"],
  ["--leading-label", "label"],
  ["--leading-card", "card"],
  ["--leading-caption", "caption"],
  ["--leading-body", "body"],
  ["--leading-prose", "prose"],
];
const TRACKINGS: Sample[] = [["--tracking-tight", "tight — the page title"]];
const OPACITIES: Sample[] = [
  ["--opacity-disabled", "disabled"],
  ["--opacity-dim", "dim"],
  ["--opacity-drag", "drag"],
];
const SHADOWS: Sample[] = [
  ["--shadow-sm", "shadow-sm"],
  ["--shadow-md", "shadow-md"],
  ["--shadow-lg", "shadow-lg"],
  ["--depth-2", "depth-2"],
  ["--depth-3", "depth-3"],
];

type SyntaxRun = readonly [scope: string, text: string];

const CODE: SyntaxRun[][] = [
  [["comment", "// the budget a frame may spend"]],
  [
    ["keyword", "import "],
    ["punctuation", "{ "],
    ["variable", "lazy "],
    ["punctuation", "} "],
    ["keyword", "from "],
    ["string", '"solid-js"'],
    ["punctuation", ";"],
  ],
  [
    ["keyword", "const "],
    ["constant", "FRAME_MS "],
    ["operator", "= "],
    ["number", "8"],
    ["punctuation", ";"],
  ],
  [
    ["keyword", "export function "],
    ["function", "paint"],
    ["punctuation", "("],
    ["variable", "cell"],
    ["punctuation", ": "],
    ["type", "Cell"],
    ["punctuation", ") { "],
    ["controlKeyword", "return "],
    ["variable", "cell"],
    ["punctuation", "."],
    ["property", "text"],
    ["punctuation", "; }"],
  ],
  [
    ["tag", "<Kbd "],
    ["attribute", "action"],
    ["operator", "="],
    ["string", '"save_file" '],
    ["tag", "/> "],
    ["invalid", "@@"],
  ],
];

const HUE_SLOTS = Array.from({ length: HUE_COUNT }, (_, index) => index);

/** The glyphs Forge draws itself, as opposed to the lucide remainder. */
const GLYPH_NAMES: ForgeIconName[] = [
  "layers",
  "folder",
  "history",
  "git-pull-request",
  "git-branch",
  "worktree",
  "square-terminal",
  "file-code",
  "commit",
  "fetch",
  "push",
  "patch",
  "list-checks",
  "merged",
  "conflict",
  "message-square-plus",
  "spawn-child",
  "folder-symlink",
  "bell",
  "idle",
  "exited",
  "failed",
  "columns-2",
  "command-palette",
];

/** The five icons that move once, each named for the event that plays it. */
const ONE_SHOTS: {
  icon: ForgeIconName;
  play: string;
  on: "glyph" | "host";
  tone: string;
  label: string;
}[] = [
  {
    icon: "bell",
    play: "glyph-ring-once",
    on: "glyph",
    tone: "forge-icon-needs-you",
    label: "Starts waiting",
  },
  {
    icon: "worktree",
    play: "glyph-draw-long",
    on: "glyph",
    tone: "forge-icon-accent",
    label: "New worktree",
  },
  {
    icon: "fetch",
    play: "glyph-drop",
    on: "glyph",
    tone: "forge-icon-muted",
    label: "Fetch acked",
  },
  { icon: "check", play: "glyph-draw", on: "glyph", tone: "forge-icon-green", label: "Agent done" },
  { icon: "commit", play: "settle-in", on: "host", tone: "forge-icon-muted", label: "Committed" },
];

export function Gallery() {
  const [density, setDensity] = createSignal<Density>("compact");
  return (
    <div class="gallery">
      <header class="gallery-header">
        <strong class="gallery-title">Design gallery</strong>
        {/* No `system` entry: this page looks at one specific base. */}
        <div class="gallery-toggles">
          <For each={bases}>
            {(id) => (
              <Button
                variant="secondary"
                size="sm"
                selected={themeBase() === id}
                onClick={() => applyThemeBase(id)}
              >
                {THEME_LABELS[id]}
              </Button>
            )}
          </For>
        </div>
        <div class="gallery-toggles gallery-toggles-end">
          <For each={DENSITIES}>
            {(id) => (
              <Button
                variant="secondary"
                size="sm"
                selected={density() === id}
                onClick={() => {
                  setDensity(id);
                  applyDensity(id);
                }}
              >
                {id}
              </Button>
            )}
          </For>
        </div>
      </header>

      <div class="gallery-body">
        <Section title="Surfaces">
          <Grid>
            <For each={SURFACES}>{(token) => <Swatch token={token} ring />}</For>
          </Grid>
        </Section>

        <Section title="Foreground">
          <Grid>
            <For each={FOREGROUNDS}>{(token) => <Swatch token={token} ring />}</For>
          </Grid>
        </Section>

        <Section title="Borders & interaction">
          <Grid>
            <For each={INTERACTION}>{(token) => <Swatch token={token} ring />}</For>
          </Grid>
        </Section>

        <Section title="Accent & status — fill with derived foreground">
          <Grid>
            <FillChip fill="--accent-solid" fg="--accent-fg" label="Accent" />
            <FillChip fill="--attention-solid" fg="--attention-fg" label="Attention" />
            <FillChip fill="--success-solid" fg="--success-fg" label="Success" />
            <FillChip fill="--danger-solid" fg="--danger-fg" label="Danger" />
            <FillChip fill="--warning-solid" fg="--warning-fg" label="Warning" />
            <FillChip fill="--info-solid" fg="--info-fg" label="Info" />
          </Grid>
        </Section>

        <Section title="Project hues — a square or a line, never a circle">
          <div class="gallery-stack">
            <For each={HUE_SLOTS}>
              {(slot) => (
                <div
                  class="gallery-row gallery-row-tight hued"
                  style={{ "--project-hue": `var(--hue-${slot})` }}
                >
                  <span class="project-avatar" aria-hidden="true">
                    {String.fromCharCode(65 + slot)}
                  </span>
                  <span class="hue-mark" aria-hidden="true" />
                  <span class="gallery-hue-guide" aria-hidden="true" />
                  <span class="gallery-value">--hue-{slot}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Glyphs — at rest, then chosen (one region at 20%)">
          <div class="gallery-row">
            <For each={GLYPH_NAMES}>
              {(name) => (
                <div class="gallery-tile gallery-tile-center">
                  <span class="gallery-row gallery-row-tight">
                    <Icon name={name} size={18} />
                    <span class="gallery-chosen" data-selected="">
                      <Icon name={name} size={18} />
                    </span>
                  </span>
                  <span class="gallery-value">{name}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Motion — loops only for live state; one-shots on an event">
          <div class="gallery-row gallery-row-loose">
            <div class="gallery-tile gallery-tile-center">
              <WorkMarker work="working" size="md" />
              <span class="gallery-value">working · 900ms</span>
            </div>
            <div class="gallery-tile gallery-tile-center">
              <WorkMarker work="needs-you" size="md" />
              <span class="gallery-value">needs you · breathe</span>
            </div>
            <For each={ONE_SHOTS}>
              {(shot) => {
                let host: HTMLSpanElement | undefined;
                return (
                  <div class="gallery-tile gallery-tile-center">
                    <span ref={host} class={shot.tone}>
                      <Icon name={shot.icon} size={18} />
                    </span>
                    <Button
                      variant="ghost"
                      size="sm"
                      onClick={() =>
                        playOnce(
                          shot.on === "glyph" ? (host?.firstElementChild ?? undefined) : host,
                          shot.play,
                        )
                      }
                    >
                      {shot.label}
                    </Button>
                  </div>
                );
              }}
            </For>
          </div>
        </Section>

        <Section title="Tints — a wash and its edge">
          <Grid wide>
            <TintChip tone="accent" />
            <TintChip tone="danger" />
            <TintChip tone="attention" />
          </Grid>
        </Section>

        <Section title="Opacity">
          <Grid>
            <For each={OPACITIES}>
              {([token, label]) => (
                <div class="gallery-tile">
                  <div class="gallery-opacity" style={sample(token)} />
                  <span class="forge-caption">{label}</span>
                  <span class="gallery-value">{token}</span>
                </div>
              )}
            </For>
          </Grid>
        </Section>

        <Section title="Radius">
          <div class="gallery-row gallery-row-loose">
            <For each={RADII}>
              {([token, label]) => (
                <div class="gallery-tile gallery-tile-center">
                  <div class="gallery-radius" style={sample(token)} />
                  <span class="forge-caption">{label}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Control heights — move with density">
          <div class="gallery-row">
            <For each={CONTROL_HEIGHTS}>
              {([token, label]) => (
                <div class="gallery-control" style={sample(token)}>
                  {label}
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Type scale">
          <div class="gallery-stack gallery-stack-tight">
            <For each={TYPE_SCALE}>
              {([token, label]) => (
                <div class="gallery-type" style={sample(token)}>
                  {label}
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Weight & tracking">
          <div class="gallery-row gallery-row-loose">
            <For each={WEIGHTS}>
              {([token, label]) => (
                <div class="gallery-tile">
                  <span class="gallery-weight" style={sample(token)}>
                    Forge {label}
                  </span>
                  <span class="gallery-value">{token}</span>
                </div>
              )}
            </For>
          </div>
          <div class="gallery-stack gallery-stack-tight">
            <For each={TRACKINGS}>
              {([token, label]) => (
                <div class="gallery-tile">
                  <span class="gallery-tracking" style={sample(token)}>
                    {label}
                  </span>
                  <span class="gallery-value">{token}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Line height">
          <Grid wide>
            <For each={LEADINGS}>
              {([token, label]) => (
                <div class="gallery-tile">
                  <p class="gallery-leading" style={sample(token)}>
                    {label}: two lines of chrome text, wrapped to show the gap between them.
                  </p>
                  <span class="gallery-value">{token}</span>
                </div>
              )}
            </For>
          </Grid>
        </Section>

        <Section title="Elevation">
          <div class="gallery-row gallery-row-loose">
            <For each={SHADOWS}>
              {([token, label]) => (
                <div class="gallery-tile gallery-tile-center">
                  <div class="gallery-shadow" style={sample(token)} />
                  <span class="forge-caption">{label}</span>
                </div>
              )}
            </For>
          </div>
        </Section>

        <Section title="Syntax — data-syntax scopes">
          <pre class="gallery-code">
            <For each={CODE}>
              {(line) => (
                <>
                  <For each={line}>
                    {([scope, text]) => <span data-syntax={scope}>{text}</span>}
                  </For>
                  {"\n"}
                </>
              )}
            </For>
          </pre>
        </Section>

        <Primitives />
      </div>
    </div>
  );
}

const ROW_STATES = [
  { label: "at rest — move the pointer here for hover", selected: false, attention: false },
  { label: "selected — data-selected", selected: true, attention: false },
  { label: "waiting on you — .forge-attention-row", selected: false, attention: true },
] as const;

function Primitives() {
  const [text, setText] = createSignal("");
  const [search, setSearch] = createSignal("bran");
  const [dialog, setDialog] = createSignal(false);
  const [alert, setAlert] = createSignal(false);
  const [checked, setChecked] = createSignal(true);
  const [choice, setChoice] = createSignal("all");
  const [picked, setPicked] = createSignal("claude");
  const [autosave, setAutosave] = createSignal(true);
  const [density, setDensity] = createSignal("compact");
  const [scope, setScope] = createSignal("workspace");
  const [pinned, setPinned] = createSignal(true);
  const [tab, setTab] = createSignal("files");
  const [picked42, setPicked42] = createSignal(true);
  const [cardOn, setCardOn] = createSignal(false);
  return (
    <>
      <Section title="Buttons — hierarchy">
        <div class="gallery-row">
          <Button variant="primary">Primary</Button>
          <Button variant="secondary">Secondary</Button>
          <Button variant="ghost">Ghost</Button>
          <Button variant="danger">Danger</Button>
          <Button variant="danger-ghost">Danger ghost</Button>
          <Button variant="attention">Answer now</Button>
          <Button variant="attention-soft">2 waiting</Button>
        </div>
        <div class="gallery-row">
          <Button variant="secondary" shape="pill">
            Pill
          </Button>
          <Button variant="attention-soft" shape="pill" size="sm">
            Needs you
          </Button>
          <Button variant="ghost" shape="pill" size="sm">
            Ghost pill
          </Button>
        </div>
        <div class="gallery-row">
          <Button variant="primary" size="lg" iconLeading={<Icon name="plus" size={14} />}>
            New session
          </Button>
          <Button variant="secondary" size="lg" loading>
            Detecting…
          </Button>
          <IconButton label="Split the pane">
            <Icon name="columns-2" size={15} />
          </IconButton>
          <IconButton label="Search" variant="secondary">
            <Icon name="search" size={15} />
          </IconButton>
          <IconButton
            label="Pin this session"
            selected={pinned()}
            onClick={() => setPinned(!pinned())}
          >
            <Icon name="layers" size={15} />
          </IconButton>
          <IconButton label="Close">
            <Icon name="close" size={15} />
          </IconButton>
          <RadioGroup
            label="Row density"
            variant="segmented"
            value={density()}
            onChange={setDensity}
            options={[
              { value: "compact", label: "Compact" },
              { value: "cozy", label: "Cozy" },
            ]}
          />
          <RadioGroup
            label="Scope"
            variant="segmented"
            size="md"
            value={scope()}
            onChange={setScope}
            options={[
              { value: "workspace", label: "Workspace" },
              { value: "session", label: "Session" },
            ]}
          />
        </div>
        <div class="gallery-row">
          <Button variant="primary" loading>
            Loading
          </Button>
          <Button variant="secondary" selected>
            Selected
          </Button>
          <Button variant="secondary" disabled>
            Disabled
          </Button>
          <Button variant="primary" disabled>
            Disabled
          </Button>
        </div>
        <div class="gallery-row">
          <Button variant="secondary" size="xs">
            xs
          </Button>
          <Button variant="secondary" size="sm">
            sm
          </Button>
          <Button variant="secondary" size="md">
            md
          </Button>
          <Button variant="secondary" size="lg">
            lg
          </Button>
        </div>
      </Section>

      <Section title="Fields">
        <div class="gallery-row gallery-wide">
          <TextField label="Default" value={text()} onChange={setText} placeholder="Type…" />
          <TextField
            label="Invalid"
            value={text()}
            onChange={setText}
            invalid
            errorMessage="Something is off"
          />
          <div class="gallery-field">
            <SearchField value={search()} onChange={setSearch} onClear={() => setSearch("")} />
          </div>
        </div>
      </Section>

      <Section title="Interactions — open one of each to see the motion">
        <div class="gallery-row">
          <Menu
            triggerLabel="Open menu"
            trigger={<span>Menu ▾</span>}
            triggerClass={buttonClass("md", "secondary")}
            items={[
              { kind: "heading", label: "Session" },
              { kind: "item", label: "Refresh status", icon: "refresh", run: () => undefined },
              { kind: "item", label: "Copy path", icon: "copy", run: () => undefined },
              { kind: "rule" },
              {
                kind: "submenu",
                label: "Open in",
                icon: "folder-open",
                items: [{ kind: "item", label: "Finder", run: () => undefined }],
              },
              {
                kind: "item",
                label: "Remove",
                icon: "trash",
                destructive: true,
                run: () => undefined,
              },
            ]}
          />
          <div class="gallery-field">
            <Select
              aria-label="Provider"
              value={picked()}
              onChange={setPicked}
              options={[
                { value: "claude", label: "Claude" },
                { value: "codex", label: "Codex" },
                { value: "opencode", label: "opencode" },
              ]}
            />
          </div>
          <Tooltip label="A tooltip only fades — a scale would read as a twitch">
            <Button variant="ghost" iconLeading={<Icon name="search" size={14} />}>
              Hover me
            </Button>
          </Tooltip>
        </div>

        <div class="gallery-row">
          <Checkbox checked={checked()} onChange={setChecked} label="Checkbox" />
          <RadioGroup
            label="Filter"
            variant="chips"
            value={choice()}
            onChange={setChoice}
            options={[
              { value: "all", label: "All" },
              { value: "mine", label: "Mine" },
              { value: "open", label: "Open" },
            ]}
          />
        </div>

        <div class="gallery-row">
          <Badge>draft</Badge>
          <Badge tone="accent">reviewing</Badge>
          <Badge tone="good">ready</Badge>
          <Badge tone="warn">review required</Badge>
          <Badge tone="bad">changes requested</Badge>
          <Badge tone="attention">2 waiting</Badge>
        </div>

        <div class="gallery-list gallery-medium">
          <span class="forge-section-label">Row · the real state hooks</span>
          <For each={ROW_STATES}>
            {(row) => (
              <button
                type="button"
                class={`forge-row empty-state-row ${row.attention ? "forge-attention-row" : ""}`}
                data-selected={row.selected ? "" : undefined}
              >
                <span class={row.attention ? "forge-attention-dot" : "status-dot"} />
                <span class="empty-state-label">{row.label}</span>
              </button>
            )}
          </For>
        </div>

        <div class="gallery-medium">
          <Tabs
            aria-label="Kit example"
            value={tab()}
            onChange={setTab}
            tabs={[
              { value: "files", label: "Files changed", content: () => <p>Four files.</p> },
              { value: "checks", label: "Checks", content: () => <p>Six checks green.</p> },
              { value: "commits", label: "Commits", content: () => <p>Seven commits.</p> },
            ]}
          />
        </div>

        <div class="gallery-list gallery-medium">
          <span class="forge-section-label">Markers sit at the end of a row, after the label</span>
          <For each={["working", "starting", "needs-you", "idle", "exited", "failed"] as const}>
            {(work) => (
              <button type="button" class="forge-row empty-state-row">
                <Icon name="square-terminal" class="forge-icon-muted" size={14} />
                <span class="empty-state-label">{work}</span>
                <WorkMarker work={work} />
              </button>
            )}
          </For>
        </div>

        <div class="gallery-medium">
          <Disclosure summary={<span>A disclosure — this one animates its height</span>}>
            <p class="gallery-note">
              Kobalte publishes the measured height as `--kb-collapsible-content-height`, which is
              what the keyframe interpolates to. `auto` does not animate.
            </p>
          </Disclosure>
        </div>

        <div class="gallery-list gallery-medium">
          <For each={["A row", "Another row", "A third"]}>
            {(label) => (
              <button type="button" class="forge-row empty-state-row">
                <Icon name="square-terminal" class="forge-icon-muted" size={14} />
                <span>{label}</span>
              </button>
            )}
          </For>
        </div>
      </Section>

      <Section title="Shell — the card's strip and location, a pinned wait, tree rows">
        <div class="gallery-shell">
          <div class="work-card">
            <div class="card-strip">
              <div class="session-tab-wrap code-tab-wrap">
                <span class="session-tab">
                  <Icon name="file-code" size={14} class="forge-icon-muted" />
                  <span class="session-tab-label">Code</span>
                  <span class="session-tab-count">3</span>
                </span>
              </div>
              <div class="session-tab-wrap">
                <span class="session-tab">
                  <WorkMarker work="idle" />
                  <span class="session-tab-label">Terminal 1</span>
                </span>
              </div>
              <div class="session-tab-wrap active">
                <span class="session-tab">
                  <WorkMarker work="working" />
                  <span class="session-tab-label">Calm redesign</span>
                </span>
              </div>
              <span class="tab-strip-spacer" />
              <button type="button" class="waiting-pill">
                <span class="waiting-pill-dot" aria-hidden="true" />1 waiting
              </button>
              <Button variant="ghost" size="md">
                Changes <span class="session-actions-count">0</span>
              </Button>
            </div>
            <div class="card-location">
              <span class="card-location-place">forge-node</span>
              <span class="card-location-sep">/</span>
              <span class="card-location-mono card-location-strong">main</span>
              <span class="card-location-sep">·</span>
              <span>no changes</span>
              <span class="card-location-spacer" />
              <span class="card-location-mono">132 × 42</span>
            </div>
            <div class="gallery-shell-body" />
          </div>
          <div class="gallery-shell-side">
            <button type="button" class="needs-you-card">
              <span class="needs-you-line">
                <span class="forge-attention-dot" aria-hidden="true" />
                <span class="needs-you-title forge-truncate">Migrate the admin portal</span>
                <span class="needs-you-waited">12m</span>
              </span>
              <span class="needs-you-place forge-truncate">admin-portal · waiting on you</span>
            </button>
            <button type="button" class="forge-row tree-row project">
              <span class="project-avatar current">F</span>
              <span class="tree-label">forge-node</span>
              <span class="tree-meta">main</span>
            </button>
            <div class="tree-guide">
              <button type="button" class="forge-row tree-row">
                <Icon name="square-terminal" size={14} class="forge-icon-muted" />
                <span class="tree-label">
                  Shell <span class="tree-path">~/forge-node</span>
                </span>
                <WorkMarker work="idle" />
              </button>
              <button type="button" class="forge-row tree-row" aria-current="true">
                <Icon name="agent" size={14} class="forge-icon-muted" />
                <span class="tree-label">Calm redesign</span>
                <WorkMarker work="working" />
              </button>
            </div>
          </div>
        </div>
      </Section>

      <Section title="Dialogs">
        <div class="gallery-row">
          <Button variant="secondary" onClick={() => setDialog(true)}>
            Open dialog
          </Button>
          <Button variant="danger-ghost" onClick={() => setAlert(true)}>
            Open alert
          </Button>
        </div>
        <Show when={dialog()}>
          <Dialog
            title="A dialog"
            description="Sized from a token, body scrolls, animates in and out."
            align="center"
            size="md"
            onDismiss={() => setDialog(false)}
            footer={
              <>
                <Button variant="secondary" onClick={() => setDialog(false)}>
                  Cancel
                </Button>
                <Button variant="primary" onClick={() => setDialog(false)}>
                  Confirm
                </Button>
              </>
            }
          >
            <p class="forge-dialog-copy">
              The header, this body and the footer are three regions; only the body scrolls.
            </p>
          </Dialog>
        </Show>
        <Show when={alert()}>
          <AlertDialog
            title="Delete this?"
            description="AlertDialog: the destructive role, no dismiss on click-outside."
            onDismiss={() => setAlert(false)}
            footer={
              <>
                <Button variant="secondary" onClick={() => setAlert(false)}>
                  Cancel
                </Button>
                <Button variant="danger" onClick={() => setAlert(false)}>
                  Delete
                </Button>
              </>
            }
          />
        </Show>
      </Section>

      <Section title="Booleans, progress and placeholders">
        <div class="gallery-stack gallery-medium">
          <Switch
            label="Autosave"
            description="Write on blur, and after a second of stillness."
            checked={autosave()}
            onChange={setAutosave}
          />
          <Switch label="Disabled, on" checked disabled onChange={() => undefined} />
          <Switch label="Small" size="sm" checked={autosave()} onChange={setAutosave} />
          <Progress label="Reading the checkout" />
          <Progress label="Steps" value={40} detail="2 of 5" />
          <Progress label="Weekly allowance" value={71} tone="warning" />
          <Skeleton label="Loading the file tree" rows={4} />
        </div>
      </Section>

      <Section title="Chords, crumbs and cards">
        <div class="gallery-stack gallery-medium">
          <div class="gallery-row gallery-row-tight">
            <Kbd action="open_command_palette" />
            <Kbd action="new_terminal" />
            <Kbd action="save_file" />
            <Kbd>↑↓</Kbd>
            <Kbd>esc</Kbd>
          </div>
          <Breadcrumbs
            label="File path"
            crumbs={[
              { label: "src", value: "src" },
              { label: "workbench", value: "src/workbench" },
            ]}
            onChoose={() => undefined}
          />
          <ListCard
            ident="#42"
            selected={picked42()}
            onOpen={() => setPicked42(!picked42())}
            openLabel="Select pull request 42"
            title="Open Markdown source and reflow the preview"
            meta={
              <>
                <span>md-preview → main</span>
                <span>6 checks</span>
              </>
            }
          />
          <Card selected>A selected card: accent edge and an outer halo.</Card>
          <Card selected={cardOn()} onClick={() => setCardOn(!cardOn())}>
            A card that is one button: click to toggle aria-pressed.
          </Card>
          <ListCard
            glyph={<Icon name="git-pull-request" size={14} />}
            title="Replace the textarea editor with CodeMirror"
            aside={<Badge tone="good">open</Badge>}
            meta={
              <>
                <span>#128</span>
                <span>2 files</span>
                <span>opened 3h ago</span>
              </>
            }
            actions={
              <>
                <Button variant="secondary" size="xs">
                  Check out
                </Button>
                <Button variant="ghost" size="xs">
                  Open on GitHub
                </Button>
              </>
            }
          >
            <p class="gallery-note">
              The body slot, for whatever a particular list wants under the metadata.
            </p>
          </ListCard>
          <div class="gallery-stack gallery-stack-tight">
            <ListCard
              variant="flat"
              glyph={<Icon name="git-pull-request" size={14} />}
              title="Flat: a row in a list, not a card on a board"
              meta={<span>#131 · hairline between rows</span>}
            />
            <ListCard
              variant="flat"
              glyph={<Icon name="git-pull-request" size={14} />}
              title="Another flat row"
              meta={<span>#132 · hover for the row wash</span>}
            />
          </div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() =>
              toast({ title: "Saved", detail: "src/app/Gallery.tsx", tone: "success" })
            }
          >
            Fire a toast
          </Button>
        </div>
      </Section>
    </>
  );
}
