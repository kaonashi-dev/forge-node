import { For, Show, createMemo, createResource, createSignal, type JSX } from "solid-js";
import { Minus, Plus } from "lucide-solid";
import {
  Badge,
  Button,
  Disclosure,
  IconButton,
  RadioGroup,
  Switch,
  TextField,
} from "../../ui/index";
import { Icon } from "../../theme/icons/index";
import { configPaths, openInFileManager, reconnect, refreshSnapshot } from "../../runtime/host";
import {
  factoryReset,
  refreshDetection,
  setAppState,
  setProviderExecutable,
  stopDaemon,
} from "./commands";
import { SessionGlyph } from "../sessions/SessionGlyph";
import { AgentProfiles } from "./AgentProfiles";
import { AgentVisibilitySwitch } from "./AgentVisibilitySwitch";
import { SharedFiles } from "../projects/SharedFiles";
import { KeyboardSection } from "./KeyboardSection";
import { StatsSection } from "./StatsSection";
import {
  DEFAULT_AGENT_KEY,
  defaultAgentFrom,
  defaultAgentValue,
  type DefaultAgent,
} from "./defaultAgent";
import { forgeStore } from "../../state/forgeStore";
import { agentVisible } from "../../state/agentVisibility";
import {
  providerDetectionLabel,
  providerExecutable,
  providerId,
  providerInstalled,
  providerName,
  type ProviderInfo,
} from "../../contracts/runtime";
import { Group, Page, Row } from "./SettingsLayout";
import type { Section } from "./sections";
import { Dynamic } from "solid-js/web";
// Lazy route styling avoids loading settings CSS in windows that never open it.
import "../../styles/settings.css";
import { requestConfirm } from "../../state/dialogs";
import { connectionStore } from "../../state/connection";
import { terminalStore } from "../terminal/terminalStore";
import { ThemeSettings } from "./ThemeSettings";
import { ThemePreview } from "./ThemePreview";
import {
  DENSITIES,
  DENSITY_LABELS,
  applyDensity,
  readDensity,
  type Density,
} from "../../theme/density";
import { UI_FONT_SIZE_RANGE, applyUiFont } from "../../theme/uiFont";
import { applyReduceMotion } from "../../theme/motion";
import { applyGlass, glassPreference, themeBase } from "../../theme/ThemeProvider";
import { baseIsLight } from "../../theme/tokens";
import { DENSITY_NOTES, canStep, stepValue, type StepRange } from "./typeSteps";
import {
  AUTOSAVE_KEY,
  DENSITY_KEY,
  EDITOR_FONT_SIZE_KEY,
  EDITOR_FONT_SIZE_RANGE,
  EDITOR_LINE_HEIGHT_KEY,
  EDITOR_LINE_HEIGHT_RANGE,
  REDUCE_MOTION_KEY,
  GLASS_KEY,
  TERMINAL_ZOOM_KEY,
  TERMINAL_ZOOM_RANGE,
  TERMINAL_ZOOM_STEP,
  UI_FONT_SIZE_KEY,
  readFlag,
  readScale,
  writeChoice,
  writeFlag,
} from "../../state/preferences";

/** Line spacing, named rather than numbered — nobody wants to pick `1.35`. */
const LINE_HEIGHTS = [
  { value: 1.2, label: "Tight" },
  { value: 1.35, label: "Default" },
  { value: 1.5, label: "Relaxed" },
  { value: 1.8, label: "Loose" },
] as const;

export type { Section } from "./sections";

export type SettingsRouteProps = {
  section: Section;
};

/** The chosen settings page. The sections themselves are listed by the sidebar. */
export function SettingsRoute(props: SettingsRouteProps) {
  const PANELS: Record<Section, () => JSX.Element> = {
    General: () => <General />,
    Agents: () => <Agents />,
    Keyboard: () => <KeyboardSection />,
    "Shared files": () => <SharedFiles />,
    "Stats & usage": () => <StatsSection />,
    Appearance: () => <Appearance />,
  };

  return (
    <div class="settings-layer">
      <div class="settings-body">
        <Dynamic component={PANELS[props.section]} />
      </div>
    </div>
  );
}

function General() {
  const connection = () => connectionStore.connection;
  // Host state, not daemon state, so it is fetched here instead of read off
  // the snapshot. A failure leaves it undefined and the group says so rather
  // than the page refusing to render.
  const [paths] = createResource(() => configPaths().catch(() => undefined));

  function confirmFactoryReset(): void {
    const projects = forgeStore.projects.length;
    const sessions = forgeStore.live_sessions;
    const worktrees = forgeStore.workspaces.filter((workspace) => workspace.managed_by_app).length;
    requestConfirm({
      title: "Reset Forge Node to factory defaults?",
      description: `${projects} project${projects === 1 ? "" : "s"}, ${sessions} live session${sessions === 1 ? "" : "s"}, and ${worktrees} worktree${worktrees === 1 ? "" : "s"} managed by Forge Node will be removed. Dirty managed worktrees are deleted. Forge Node then reconnects as a fresh install.`,
      confirmLabel: "Reset Forge Node",
      destructive: true,
      onConfirm: () => void factoryReset().catch(() => undefined),
    });
  }

  function confirmReconnect(): void {
    requestConfirm({
      title: "Reconnect to the daemon?",
      description:
        "Closes the current connection and opens a new one. Live sessions keep running on the daemon.",
      confirmLabel: "Reconnect",
      onConfirm: () => void reconnect().catch(() => undefined),
    });
  }

  const daemonTone = () => {
    switch (connection().kind) {
      case "connected":
        return "online";
      case "connecting":
        return "idle";
      case "disconnected":
        return "error";
      default:
        return "idle";
    }
  };

  const daemonTitle = () => {
    switch (connection().kind) {
      case "connected":
        return "Connected";
      case "connecting":
        return "Connecting…";
      case "disconnected":
        return "Disconnected";
      default:
        return "Offline";
    }
  };

  const daemonDetail = () => {
    const state = connection();
    switch (state.kind) {
      case "connected":
        return `v${state.version} · instance ${state.instanceId.slice(0, 8)}`;
      case "disconnected":
        return state.reason;
      case "connecting":
        return "Looking for a running daemon";
      default:
        return "No connection yet";
    }
  };

  return (
    <Page title="General" summary="Application details, configuration files, and daemon controls.">
      <Group title="Overview">
        <dl class="settings-facts">
          <dt>Version</dt>
          <dd>{paths()?.app_version ? `v${paths()?.app_version}` : "—"}</dd>
          <dt>Sessions</dt>
          <dd>
            {forgeStore.live_sessions} live of {forgeStore.sessions.length}
          </dd>
          <dt>Terminal</dt>
          <dd>
            {terminalStore.cols}×{terminalStore.rows}
            {terminalStore.latencyP95 === null
              ? ""
              : ` · ${Math.round(terminalStore.latencyP95)} ms p95`}
          </dd>
          <dt>Projects</dt>
          <dd>
            {forgeStore.projects.length} in {forgeStore.workspaces.length} checkouts
          </dd>
        </dl>
      </Group>

      <Group
        title="Configuration & logs"
        description="Edit config.toml in your editor, then restart Forge Node to apply changes."
      >
        <ul class="settings-paths">
          <li>
            <span class="settings-path-name">
              config.toml
              <span class="forge-option-note">terminal, sessions, worktrees, daemon</span>
            </span>
            <span class="settings-path-value">
              {paths()?.config_file ?? "no config directory on this platform"}
            </span>
            <span class="forge-option-note">
              {paths() === undefined
                ? ""
                : paths()?.config_exists
                  ? "present"
                  : "not created yet — Forge Node is running on the defaults"}
            </span>
          </li>
          <li>
            <span class="settings-path-name">logs</span>
            <span class="settings-path-value">
              {paths()?.logs_dir ?? "no data directory on this platform"}
            </span>
            <span class="forge-option-note">daemon.&lt;date&gt;.log, last 5 days</span>
          </li>
        </ul>
        <div class="settings-actions">
          <Button
            variant="secondary"
            // The directory, not the file: config.toml need not exist yet, and a
            // file manager cannot reveal what is not there.
            disabled={!paths()?.config_dir}
            onClick={() => {
              const dir = paths()?.config_dir;
              if (dir) void openInFileManager(dir).catch(() => undefined);
            }}
          >
            Reveal config folder
          </Button>
          <Button
            variant="secondary"
            disabled={!paths()?.logs_dir}
            onClick={() => {
              const dir = paths()?.logs_dir;
              if (dir) void openInFileManager(dir).catch(() => undefined);
            }}
          >
            Reveal logs
          </Button>
        </div>
      </Group>

      <Group
        title="Daemon"
        description="The daemon owns every PTY, so stopping it takes all running sessions with it."
      >
        <div class="daemon-status" role="status">
          <span class={`status-dot ${daemonTone()}`} />
          <div class="daemon-status-copy">
            <span class="daemon-status-title">{daemonTitle()}</span>
            <span class="daemon-status-detail">{daemonDetail()}</span>
          </div>
        </div>
        <div class="settings-actions">
          <Button variant="secondary" onClick={confirmReconnect}>
            Reconnect
          </Button>
          <Button variant="secondary" onClick={() => void refreshSnapshot().catch(() => undefined)}>
            Reload from the daemon
          </Button>
          {/* Stopping takes the connection with it, so the shell reconnects
            afterwards: it finds nothing, says so, and keeps retrying. Sessions
            are left running unless the second button is used. */}
          <Button variant="secondary" onClick={() => void stopDaemon(false).catch(() => undefined)}>
            Stop the daemon
          </Button>
          <Button variant="danger" onClick={() => void stopDaemon(true).catch(() => undefined)}>
            Stop and kill sessions
          </Button>
        </div>
      </Group>

      <Group
        title="Danger zone"
        class="settings-danger"
        description="Reset Forge Node's own state without modifying your repositories, configuration file, or logs."
      >
        <Row
          label="Reset to factory defaults"
          description="Stops every session, forgets all projects and preferences, and force-removes worktrees created by Forge Node. Branches, commits, config.toml and logs stay in place."
          control={
            <Button variant="danger" onClick={confirmFactoryReset}>
              Reset Forge Node
            </Button>
          }
        />
      </Group>
    </Page>
  );
}

function Agents() {
  const preferred = () => defaultAgentFrom(forgeStore.app_state);
  const choices = createMemo<
    {
      value: DefaultAgent;
      label: string;
      note: string | null;
      enabled: boolean;
      provider: string | null;
    }[]
  >(() => [
    {
      value: { kind: "ask" } as DefaultAgent,
      label: "Ask each time",
      note: "Open the launch palette and pick.",
      enabled: true,
      provider: null,
    },
    ...forgeStore.launchables.map((item) => ({
      value: (item.kind === "shell"
        ? { kind: "shell" }
        : item.profile
          ? { kind: "profile", id: item.profile }
          : { kind: "provider", id: item.provider ?? "" }) as DefaultAgent,
      label: item.kind === "shell" ? "Blank terminal" : item.label,
      note: item.detail,
      enabled: item.enabled,
      provider: item.kind === "shell" ? null : item.provider,
    })),
  ]);

  const installed = createMemo(() => forgeStore.providers.filter(providerInstalled).length);

  function choose(value: string): void {
    // The prefixed form the parser expects: one shared app_state row,
    // and a bare id reads back there as "Ask".
    void setAppState(DEFAULT_AGENT_KEY, value).catch(() => undefined);
  }

  return (
    <Page title="Agents" summary="Choose the default agent and how each one launches.">
      <Group
        title="Default agent"
        description="Starts with ⌘⇧A. Ask opens the launch palette each time."
        plain
      >
        <RadioGroup
          label="Default agent"
          class="agent-choices"
          orientation="horizontal"
          itemClass="agent-choice"
          value={defaultAgentValue(preferred())}
          onChange={choose}
          options={choices().map((choice) => ({
            value: defaultAgentValue(choice.value),
            label: choice.label,
            disabled: !choice.enabled,
            render: () => (
              <>
                <Show
                  when={choice.provider}
                  fallback={<SessionGlyph providerId={null} size={14} />}
                >
                  {(provider) => <SessionGlyph providerId={provider()} size={14} />}
                </Show>
                <span class="agent-choice-label forge-truncate">{choice.label}</span>
              </>
            ),
          }))}
        />
      </Group>

      <Group
        title="Providers"
        description={`${installed()} detected. Off hides a provider from every agent picker.`}
        aside={
          <Button
            variant="ghost"
            size="xs"
            iconLeading={<Icon name="refresh" class="forge-icon-muted" size={13} />}
            onClick={() => void refreshDetection().catch(() => undefined)}
          >
            Rescan
          </Button>
        }
      >
        <For
          each={forgeStore.providers}
          fallback={<p class="empty-copy">No providers in this snapshot.</p>}
        >
          {(provider) => <ProviderRow provider={provider} preferred={preferred()} />}
        </For>
      </Group>

      <AgentProfiles />
    </Page>
  );
}

function ProviderRow(props: { provider: ProviderInfo; preferred: DefaultAgent }) {
  const [path, setPath] = createSignal("");
  const id = () => providerId(props.provider);
  const executable = () => providerExecutable(props.provider);
  const visible = () => agentVisible(forgeStore.app_state, id());
  const isDefault = () =>
    defaultAgentValue(props.preferred) === defaultAgentValue({ kind: "provider", id: id() });

  return (
    <Disclosure
      class={visible() ? "provider-row" : "provider-row off"}
      summary={
        <span class="provider-identity">
          <span class="provider-name">
            {providerName(props.provider)}
            <Show when={isDefault()}>
              <Badge tone="accent">Default</Badge>
            </Show>
          </span>
          <span class="provider-id">
            {id()} · {providerDetectionLabel(props.provider)}
          </span>
        </span>
      }
      actions={
        <>
          <Show when={!isDefault() && visible() && providerInstalled(props.provider)}>
            <Button
              variant="ghost"
              size="sm"
              onClick={() =>
                void setAppState(
                  DEFAULT_AGENT_KEY,
                  defaultAgentValue({ kind: "provider", id: id() }),
                ).catch(() => undefined)
              }
            >
              Make default
            </Button>
          </Show>
          <AgentVisibilitySwitch agentKey={id()} label={providerName(props.provider)} />
        </>
      }
    >
      <Row
        label="Command"
        description={
          executable()?.resolved
            ? "Detection resolved this path on PATH. Pin another to override it."
            : "Nothing was resolved; this is the name detection looks for."
        }
      >
        <div class="provider-command">
          <TextField
            class="provider-command-field"
            aria-label={`Executable for ${providerName(props.provider)}`}
            value={path()}
            placeholder={executable()?.path ?? "Path to the binary"}
            onChange={setPath}
          />
          <Button
            variant="secondary"
            size="sm"
            onClick={() =>
              void setProviderExecutable(id(), path().trim() || null).catch(() => undefined)
            }
          >
            {path().trim() === "" ? "Clear override" : "Pin"}
          </Button>
        </div>
      </Row>
      <Show when={(props.provider.descriptor?.default_args ?? []).length > 0}>
        <Row
          label="Default arguments"
          description="What the daemon passes at launch. Change them per profile, below."
        >
          <code class="provider-args">
            {(props.provider.descriptor?.default_args ?? []).join(" ")}
          </code>
        </Row>
      </Show>
    </Disclosure>
  );
}

function Appearance() {
  const density = () => readDensity(forgeStore.app_state[DENSITY_KEY]);
  const reduceMotion = () => readFlag(REDUCE_MOTION_KEY, false);

  function chooseDensity(next: Density): void {
    applyDensity(next);
    writeChoice(DENSITY_KEY, next);
  }

  function setReduceMotion(next: boolean): void {
    applyReduceMotion(next);
    writeFlag(REDUCE_MOTION_KEY, next);
  }

  const lineHeight = () =>
    readScale(
      EDITOR_LINE_HEIGHT_KEY,
      EDITOR_LINE_HEIGHT_RANGE.min,
      EDITOR_LINE_HEIGHT_RANGE.max,
      EDITOR_LINE_HEIGHT_RANGE.fallback,
    );

  return (
    <Page
      title="Appearance"
      summary="Theme, accent, density and type. Changes apply as you pick them."
    >
      <div class="appearance-grid">
        <div class="appearance-main">
          <ThemeSettings />
          <section class="appearance-row" aria-labelledby="density-title">
            <h3 id="density-title" class="appearance-row-title">
              Density
            </h3>
            <RadioGroup
              label="Row density"
              hideLabel
              variant="segmented"
              size="md"
              value={density()}
              options={DENSITIES.map((value) => ({ value, label: DENSITY_LABELS[value] }))}
              onChange={(value) => chooseDensity(readDensity(value))}
            />
            <span class="history-spacer" />
            <Switch label="Reduce motion" checked={reduceMotion()} onChange={setReduceMotion} />
          </section>
          <section class="appearance-row" aria-labelledby="glass-title">
            <h3 id="glass-title" class="appearance-row-title">
              Glass
            </h3>
            <Switch
              label="Frosted sidebar and pill tabs"
              checked={glassPreference()}
              disabled={baseIsLight(themeBase())}
              onChange={(next) => {
                applyGlass(next);
                writeFlag(GLASS_KEY, next);
              }}
            />
          </section>
          <p class="appearance-row-note">
            {baseIsLight(themeBase())
              ? "Glass is drawn for dark themes; pick one to use it."
              : "A lit backdrop, a translucent sidebar and a blue accent over the current theme."}
          </p>
          <p class="appearance-row-note">{DENSITY_NOTES[density()]}</p>
        </div>

        <div class="appearance-side">
          <ThemePreview />
          <Group title="Type size" plain>
            <div class="settings-steppers">
              <TypeStepper
                label="Chrome"
                noun="chrome text"
                range={UI_FONT_SIZE_RANGE}
                value={readScale(
                  UI_FONT_SIZE_KEY,
                  UI_FONT_SIZE_RANGE.min,
                  UI_FONT_SIZE_RANGE.max,
                  UI_FONT_SIZE_RANGE.fallback,
                )}
                format={(value) => `${value}`}
                onChange={(next) => {
                  applyUiFont(next);
                  writeChoice(UI_FONT_SIZE_KEY, String(next));
                }}
              />
              <TypeStepper
                label="Editor"
                noun="editor text"
                range={{ ...EDITOR_FONT_SIZE_RANGE, step: 1 }}
                value={readScale(
                  EDITOR_FONT_SIZE_KEY,
                  EDITOR_FONT_SIZE_RANGE.min,
                  EDITOR_FONT_SIZE_RANGE.max,
                  EDITOR_FONT_SIZE_RANGE.fallback,
                )}
                format={(value) => `${value}`}
                onChange={(next) => writeChoice(EDITOR_FONT_SIZE_KEY, String(next))}
              />
              <TypeStepper
                label="Terminal"
                noun="terminal text"
                range={{ ...TERMINAL_ZOOM_RANGE, step: TERMINAL_ZOOM_STEP }}
                value={readScale(
                  TERMINAL_ZOOM_KEY,
                  TERMINAL_ZOOM_RANGE.min,
                  TERMINAL_ZOOM_RANGE.max,
                  TERMINAL_ZOOM_RANGE.fallback,
                )}
                format={(value) => `${Math.round(value * 100)}%`}
                onChange={(next) => writeChoice(TERMINAL_ZOOM_KEY, String(next))}
              />
            </div>
          </Group>
          <Group title="Editor" plain>
            <div class="settings-stepper">
              <span class="settings-stepper-label">Autosave</span>
              <Switch
                label="Autosave"
                hideLabel
                checked={readFlag(AUTOSAVE_KEY, false)}
                onChange={(on) => writeFlag(AUTOSAVE_KEY, on)}
              />
            </div>
            <p class="appearance-row-note">
              Off by default: agents write these same files, and a save on a pause races them.
            </p>
            <span class="settings-stepper-label">Line spacing</span>
            <RadioGroup
              label="Line spacing"
              hideLabel
              variant="segmented"
              class="appearance-line-spacing"
              value={String(lineHeight())}
              onChange={(next) => writeChoice(EDITOR_LINE_HEIGHT_KEY, next)}
              options={LINE_HEIGHTS.map(({ value, label }) => ({ value: String(value), label }))}
            />
          </Group>
        </div>
      </div>
    </Page>
  );
}

function TypeStepper(props: {
  label: string;
  /** Completes "Smaller …" and "Larger …" for the buttons' accessible names. */
  noun: string;
  range: StepRange;
  value: number;
  format: (value: number) => string;
  onChange: (next: number) => void;
}) {
  const step = (direction: 1 | -1) => {
    if (canStep(props.value, direction, props.range)) {
      props.onChange(stepValue(props.value, direction, props.range));
    }
  };
  return (
    <div class="settings-stepper">
      <span class="settings-stepper-label">{props.label}</span>
      <div class="settings-stepper-controls">
        <IconButton
          size="sm"
          label={`Smaller ${props.noun}`}
          disabled={!canStep(props.value, -1, props.range)}
          onClick={() => step(-1)}
        >
          <Minus size={12} strokeWidth={1.8} aria-hidden="true" />
        </IconButton>
        <output class="settings-stepper-value" aria-live="polite">
          {props.format(props.value)}
        </output>
        <IconButton
          size="sm"
          label={`Larger ${props.noun}`}
          disabled={!canStep(props.value, 1, props.range)}
          onClick={() => step(1)}
        >
          <Plus size={12} strokeWidth={1.8} aria-hidden="true" />
        </IconButton>
      </div>
    </div>
  );
}
