import {
  For,
  Show,
  createEffect,
  createMemo,
  createResource,
  createSignal,
  type JSX,
} from "solid-js";
import {
  BarChart3,
  Bot,
  FolderSymlink,
  Keyboard as KeyboardIcon,
  Palette,
  Settings as SettingsIcon,
  Workflow,
} from "lucide-solid";
import { Badge, Button, Disclosure, IconButton, RadioGroup, Switch, Tabs, TextField } from "../ui";
import { Icon, SessionGlyph } from "../theme/icons";
import {
  configPaths,
  factoryReset,
  openInFileManager,
  refreshDetection,
  refreshSnapshot,
  setAppState,
  setProviderExecutable,
  stopDaemon,
} from "../runtime/api";
import { AgentProfiles } from "./AgentProfiles";
import { HarnessSection } from "./HarnessSection";
import { SharedFiles } from "./SharedFiles";
import { KeyboardSection } from "./KeyboardSection";
import { StatsSection } from "./StatsSection";
import {
  DEFAULT_AGENT_KEY,
  defaultAgentFrom,
  defaultAgentValue,
  type DefaultAgent,
} from "./defaultAgent";
import { forgeStore } from "../store/forgeStore";
import {
  providerDetectionLabel,
  providerExecutable,
  providerId,
  providerInstalled,
  providerName,
  type ProviderInfo,
} from "../runtime/types";
import { Group, Page, Row } from "./SettingsLayout";
// §5.2: this route's own paint, so a window that never opens it never loads it.
import "../styles/settings.css";
import { requestConfirm, runtimeStore } from "../store/runtimeStore";
import { terminalStore } from "../store/terminalStore";
import { ThemeSettings } from "./ThemeSettings";
import { DENSITIES, applyDensity, type Density } from "../theme/density";
import {
  AUTOSAVE_KEY,
  DENSITY_KEY,
  EDITOR_KEYMAP_KEY,
  readChoice,
  readFlag,
  writeChoice,
  writeFlag,
} from "../shell/layout";
import { EDITOR_KEYMAPS } from "../workbench/editor/createEditor";

/** Sections, in `apps/tauri order. */
const SECTIONS = [
  "General",
  "Agents",
  "Keyboard",
  "Shared files",
  "Harness",
  "Stats & Usage",
  "Personalization",
] as const;
export type Section = (typeof SECTIONS)[number];

const SECTION_ICONS: Record<Section, typeof SettingsIcon> = {
  General: SettingsIcon,
  Agents: Bot,
  Keyboard: KeyboardIcon,
  "Shared files": FolderSymlink,
  Harness: Workflow,
  "Stats & Usage": BarChart3,
  Personalization: Palette,
};

export type SettingsRouteProps = {
  onClose: () => void;
  /** Section to open on. Re-reading it moves the rail, which is how About lands on General. */
  section?: Section;
};

/**
 * The settings screen.
 *
 * Replaces the body rather than floating over it: these are decisions taken
 * away from the terminal, and a dialog over a live grid invites reading the
 * output behind it instead of the choice in front.
 *
 * *The whole body*, though — it used to replace the centre column alone, which
 * left the project rail and the inspector framing it. That reads as a panel
 * that lost its terminal rather than as a screen, and it left the way out at
 * the bottom of the section rail, under five section names, where nothing else
 * in the app puts a close. The layer covers everything below the title bar and
 * the close sits in its own header, top right, where a screen's close lives.
 */
export function SettingsRoute(props: SettingsRouteProps) {
  const [section, setSection] = createSignal<Section>(props.section ?? "General");

  // Opening settings *at* a section is a different act from opening settings:
  // "About Forge Node" has to land on General even when the screen was last left on
  // Personalization.
  createEffect(() => {
    if (props.section) setSection(props.section);
  });

  const PANELS: Record<Section, () => JSX.Element> = {
    General: () => <General />,
    Agents: () => <Agents />,
    Keyboard: () => <KeyboardSection />,
    "Shared files": () => <SharedFiles />,
    Harness: () => <HarnessSection />,
    "Stats & Usage": () => <StatsSection />,
    Personalization: () => <Personalization />,
  };

  return (
    <div class="settings-layer">
      <header class="settings-header">
        <span class="settings-header-title">Settings</span>
        <IconButton label="Close settings" onClick={props.onClose}>
          <Icon name="close" class="forge-icon-muted" />
        </IconButton>
      </header>
      <Tabs
        class="settings-route"
        aria-label="Settings"
        orientation="vertical"
        value={section()}
        onChange={(value) => setSection(value as Section)}
        listClass="settings-rail"
        triggerClass="settings-nav"
        contentClass="settings-body"
        tabs={SECTIONS.map((item) => {
          const IconComp = SECTION_ICONS[item];
          return {
            value: item,
            label: (
              <span class="settings-nav-label">
                <IconComp size={16} class="settings-nav-icon" strokeWidth={1.85} />
                <span>{item}</span>
              </span>
            ),
            content: PANELS[item],
          };
        })}
      />
    </div>
  );
}

function General() {
  const connection = () => runtimeStore.connection;
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

  return (
    <Page title="General" summary="What this install is running, and where it keeps its files.">
      <Group title="Status">
        <dl class="settings-facts">
          <dt>Version</dt>
          <dd>{paths()?.app_version ? `v${paths()?.app_version}` : "—"}</dd>
          <dt>Daemon</dt>
          <dd>
            {connection().kind === "connected"
              ? `${(connection() as { version: string }).version} · ${(connection() as { instanceId: string }).instanceId.slice(0, 8)}`
              : connection().kind}
          </dd>
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
        title="Files"
        description="Forge Node only ever reads these; edit them with your own editor. Changes need a restart — nothing here hot-reloads."
      >
        <ul class="settings-paths">
          <li>
            <span class="settings-path-name">
              config.toml
              <span class="settings-row-note">terminal, sessions, worktrees, daemon</span>
            </span>
            <span class="settings-path-value">
              {paths()?.config_file ?? "no config directory on this platform"}
            </span>
            <span class="settings-row-note">
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
            <span class="settings-row-note">app.log, daemon.log</span>
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
        <div class="settings-actions">
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
        description="Reset Forge Node's own state without modifying your repositories, configuration file, or logs."
      >
        <Row
          label="Reset to factory defaults"
          description="Stops every session and job, forgets all projects and preferences, and force-removes worktrees created by Forge Node. Branches, commits, config.toml, logs, and repository harness files stay in place."
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
  // Every launchable is a candidate, not just the bare providers: the + menu offers
  // the blank terminal and each launch profile here too, and they share one
  // key space with `ui.agent_visible.*`.
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
      label: "Ask",
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
      label: item.kind === "shell" ? "No agent (blank terminal)" : item.label,
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
    <Page title="Agents" summary="Pick what ⌘⇧A starts, and how each provider is run.">
      <Group
        title="Default agent"
        description={
          "Detection is the daemon's: a provider counts as installed when its CLI answered a " +
          "version probe. Providers that are not installed stay listed, so this also answers " +
          '"what could run here?".'
        }
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
                <span class="agent-choice-label">{choice.label}</span>
                <Show when={defaultAgentValue(preferred()) === defaultAgentValue(choice.value)}>
                  <Icon name="check" class="forge-icon-accent" size={13} />
                </Show>
              </>
            ),
          }))}
        />
      </Group>

      <Group
        title="Installed"
        aside={
          <>
            <Badge label={`${installed()} of ${forgeStore.providers.length} detected`}>
              {installed()} detected
            </Badge>
            <Button
              variant="ghost"
              size="xs"
              onClick={() => void refreshDetection().catch(() => undefined)}
            >
              <Icon name="refresh" class="forge-icon-muted" size={13} />
              Refresh
            </Button>
          </>
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

/**
 * One provider: what detection found, and the binary it is pinned to.
 *
 * The executable override used to live in a separate list further down the
 * page, keyed by a name that also appeared up here — two rows for one
 * provider, and the field was write-only: it opened blank whether or not
 * anything was pinned. Folded into the row it belongs to, the field can show
 * the binary that is actually resolved.
 */
function ProviderRow(props: { provider: ProviderInfo; preferred: DefaultAgent }) {
  const [path, setPath] = createSignal("");
  const id = () => providerId(props.provider);
  const executable = () => providerExecutable(props.provider);
  const isDefault = () =>
    defaultAgentValue(props.preferred) === defaultAgentValue({ kind: "provider", id: id() });

  return (
    <Disclosure
      class="provider-row"
      summary={
        <>
          <SessionGlyph providerId={id()} size={16} />
          <span class="provider-name">{providerName(props.provider)}</span>
          <span class="provider-id">{id()}</span>
          <Badge tone={providerInstalled(props.provider) ? "good" : "warn"}>
            {providerDetectionLabel(props.provider)}
          </Badge>
        </>
      }
      actions={
        <Button
          variant="secondary"
          size="xs"
          disabled={isDefault() || !providerInstalled(props.provider)}
          onClick={() =>
            void setAppState(
              DEFAULT_AGENT_KEY,
              defaultAgentValue({ kind: "provider", id: id() }),
            ).catch(() => undefined)
          }
        >
          {isDefault() ? "Default" : "Set default"}
        </Button>
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
            class="panel-search provider-command-field"
            aria-label={`Executable for ${providerName(props.provider)}`}
            value={path()}
            placeholder={executable()?.path ?? "Path to the binary"}
            onChange={setPath}
          />
          <Button
            variant="secondary"
            size="xs"
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

function Personalization() {
  const density = () => readChoice(DENSITY_KEY, DENSITIES, "default");

  function chooseDensity(next: Density): void {
    applyDensity(next);
    writeChoice(DENSITY_KEY, next);
  }

  return (
    <Page title="Personalization" summary="How the shell looks.">
      <ThemeSettings />

      <Group
        title="Editor"
        description="How the in-app editor behaves. Both are off the beaten path by default: the platform's own chords, and no writing without being asked."
      >
        <Row
          label="Autosave"
          description="Write the open file when the editor loses focus, and after a second of stillness. Off by default because agents write these same files, and a save on a pause races whatever one is doing in the same checkout."
          control={
            <Switch
              label="Autosave"
              hideLabel
              checked={readFlag(AUTOSAVE_KEY, false)}
              onChange={(on) => writeFlag(AUTOSAVE_KEY, on)}
            />
          }
        />
        <Row
          label="Editing model"
          description="Vim is the `@replit/codemirror-vim` keymap. Helix is this repository's own, over CodeMirror's selection model — the motions select and the verbs act on what is selected."
          control={
            <RadioGroup
              label="Editing model"
              class="theme-choices"
              orientation="horizontal"
              itemClass="forge-chip"
              value={readChoice(EDITOR_KEYMAP_KEY, EDITOR_KEYMAPS, "default")}
              onChange={(next) => writeChoice(EDITOR_KEYMAP_KEY, next)}
              options={EDITOR_KEYMAPS.map((value) => ({
                value,
                label: value,
                render: () => value,
              }))}
            />
          }
        />
      </Group>

      <Group
        title="Density"
        description="One multiplier on the row height and the control ladder. Nothing else moves: the type scale and the spacing grid are the same at every density, so a compact window is the same layout drawn tighter."
      >
        <RadioGroup
          label="Density"
          class="theme-choices"
          orientation="horizontal"
          itemClass="forge-chip"
          value={density()}
          onChange={(next) => chooseDensity(next as Density)}
          options={DENSITIES.map((value) => ({ value, label: value, render: () => value }))}
        />
      </Group>
    </Page>
  );
}
