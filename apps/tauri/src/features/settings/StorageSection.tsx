import { For, Show, createResource, createSignal } from "solid-js";
import type { MigrationReport } from "../../contracts/runtime";
import { Button } from "../../ui/index";
import {
  configPaths,
  migrateHomePreview,
  openInFileManager,
  reconnect,
  removeLegacyData,
  sendRuntimeCommand,
} from "../../runtime/host";
import { connectionStore } from "../../state/connection";
import { requestConfirm } from "../../state/dialogs";
import { forgeStore } from "../../state/forgeStore";
import { Group, Page } from "./SettingsLayout";
import { legacyWorktreeCount } from "./storage";

// A preview copies the database and rescans every agent transcript, so it is
// computed once per connection and legacy-worktree count, not on every visit.
let cachedPreview: { key: string; report: Promise<MigrationReport> } | null = null;

function previewFor(key: string): Promise<MigrationReport> {
  if (cachedPreview?.key === key) return cachedPreview.report;
  const report = migrateHomePreview();
  cachedPreview = { key, report };
  report.catch(() => {
    if (cachedPreview?.report === report) cachedPreview = null;
  });
  return report;
}

export function StorageSection() {
  const connection = () => connectionStore.connection;
  const storage = () => {
    const state = connection();
    return state.kind === "connected" ? state.storage : null;
  };
  const [removing, setRemoving] = createSignal(false);
  const [removeError, setRemoveError] = createSignal<string | null>(null);
  const [fallback] = createResource(() => configPaths().catch(() => undefined));

  const legacyWorktrees = () =>
    legacyWorktreeCount(forgeStore.workspaces, storage()?.paths.legacy ?? null);
  const isLegacy = () => storage()?.paths.layout === "Legacy";
  const migratedHome = () => storage()?.paths.layout === "Home" && storage()?.paths.legacy != null;
  const worktreesLeftBehind = () => migratedHome() && legacyWorktrees() > 0;

  const [preview] = createResource(
    () =>
      isLegacy() || worktreesLeftBehind()
        ? `${connectionStore.connectionGeneration}:${legacyWorktrees()}`
        : null,
    previewFor,
  );

  const moves = () => preview()?.worktrees.filter((item) => item.outcome === "move") ?? [];
  const skips = () => preview()?.worktrees.filter((item) => item.outcome === "skip") ?? [];
  const transcripts = () => moves().reduce((total, item) => total + item.transcripts, 0);

  const home = () => storage()?.paths.home ?? fallback()?.config_dir ?? null;
  const configFile = () => storage()?.paths.config_file ?? fallback()?.config_file ?? null;
  const logs = () => storage()?.paths.logs ?? fallback()?.logs_dir ?? null;
  const worktreesRoot = () => storage()?.worktrees_root ?? null;

  const reveal = (path: string | null | undefined) => {
    if (path) void openInFileManager(path).catch(() => undefined);
  };

  function confirmMigration(moveWorktrees: boolean) {
    const count = moves().length;
    const staying = preview()?.worktrees.length ?? 0;
    const affected = transcripts();
    const label = !isLegacy()
      ? "Move worktrees"
      : moveWorktrees
        ? "Move data and worktrees"
        : "Move data";
    requestConfirm({
      title: `${label}?`,
      description: moveWorktrees
        ? `${count} managed worktree${count === 1 ? "" : "s"} will move to the visible folder with git worktree move. Live sessions end, as on any restart. ${affected} agent transcript${affected === 1 ? "" : "s"} will no longer match their worktree.`
        : `${staying} managed worktree${staying === 1 ? "" : "s"} stay where they are and keep working; new ones go under the visible folder. Live sessions end, as on any restart.`,
      confirmLabel: label,
      onConfirm: () => {
        void sendRuntimeCommand({
          type: "migrate_home",
          move_worktrees: moveWorktrees,
        }).catch(() => undefined);
      },
    });
  }

  function confirmRemove() {
    const legacy = storage()?.paths.legacy;
    requestConfirm({
      title: "Remove old Forge data?",
      description: `Deletes ${legacy ?? "the old"} app.db and agent-plugins. Worktrees, config.toml, logs and runs are kept.`,
      confirmLabel: "Remove old data",
      destructive: true,
      onConfirm: () => {
        setRemoving(true);
        setRemoveError(null);
        void removeLegacyData()
          .then(() => reconnect())
          .catch((error: unknown) => setRemoveError(String(error)))
          .finally(() => setRemoving(false));
      },
    });
  }

  const plan = () => (
    <Show
      when={preview()}
      fallback={
        <p class="forge-body">
          {preview.error
            ? `Could not read the migration plan: ${String(preview.error)}`
            : "Reading the migration plan…"}
        </p>
      }
    >
      <ul class="settings-paths">
        <li>
          <span class="settings-path-name">worktrees to move</span>
          <span class="settings-path-value">{moves().length}</span>
        </li>
        <li>
          <span class="settings-path-name">skipped</span>
          <span class="settings-path-value">{skips().length}</span>
        </li>
        <li>
          <span class="settings-path-name">
            transcripts
            <span class="forge-option-note">that would stop matching their worktree</span>
          </span>
          <span class="settings-path-value">{transcripts()}</span>
        </li>
      </ul>
      <Show when={skips().length > 0}>
        <ul class="settings-paths">
          <For each={skips()}>
            {(item) => (
              <li>
                <span class="settings-path-name">{item.path}</span>
                <span class="settings-path-value">{item.reason ?? "skipped"}</span>
              </li>
            )}
          </For>
        </ul>
      </Show>
    </Show>
  );

  return (
    <Page title="Storage" summary="Where Forge Node keeps its config, state and managed worktrees.">
      <Show
        when={connection().kind === "connected"}
        fallback={
          <Group title="Forge data">
            <p class="forge-body">Connect to the daemon to see where Forge stores data.</p>
          </Group>
        }
      >
        <Group
          title="Forge data"
          description="Hidden. Config, database and logs; not a folder to open in an editor."
        >
          <ul class="settings-paths">
            <li>
              <span class="settings-path-name">
                home
                <span class="forge-option-note">hidden · config, database, logs, runs</span>
              </span>
              <span class="settings-path-value">{home() ?? "unavailable"}</span>
            </li>
            <li>
              <span class="settings-path-name">config.toml</span>
              <span class="settings-path-value">{configFile() ?? "unavailable"}</span>
            </li>
            <li>
              <span class="settings-path-name">database</span>
              <span class="settings-path-value">{storage()?.paths.db ?? "unavailable"}</span>
            </li>
            <li>
              <span class="settings-path-name">logs</span>
              <span class="settings-path-value">{logs() ?? "unavailable"}</span>
            </li>
            <li>
              <span class="settings-path-name">runs</span>
              <span class="settings-path-value">{storage()?.paths.runs ?? "unavailable"}</span>
            </li>
          </ul>
          <div class="settings-actions">
            <Button variant="secondary" disabled={!home()} onClick={() => reveal(home())}>
              Show {home()?.split("/").pop() ?? "home"} in Finder
            </Button>
          </div>
        </Group>

        <Group title="Worktrees" description="Visible to you and to your editor.">
          <ul class="settings-paths">
            <li>
              <span class="settings-path-name">
                worktrees
                <Show
                  when={storage() && worktreesRoot() !== storage()?.paths.default_worktrees_root}
                >
                  <span class="forge-option-note">set by [worktrees] root in config.toml</span>
                </Show>
              </span>
              <span class="settings-path-value">{worktreesRoot() ?? "unavailable"}</span>
            </li>
          </ul>
          <div class="settings-actions">
            <Button
              variant="secondary"
              disabled={!worktreesRoot()}
              onClick={() => reveal(worktreesRoot())}
            >
              Show worktrees in Finder
            </Button>
          </div>
        </Group>

        <Show when={isLegacy()}>
          <Group
            title="Old location"
            description="Your data still lives in the old hidden folder, and new managed worktrees are created there. Move it to finish the upgrade."
          >
            {plan()}
            <Show when={preview()}>
              <div class="settings-actions">
                <Button variant="secondary" onClick={() => confirmMigration(false)}>
                  Move data
                </Button>
                <Button variant="secondary" onClick={() => confirmMigration(true)}>
                  Move data and worktrees
                </Button>
              </div>
            </Show>
          </Group>
        </Show>

        <Show when={worktreesLeftBehind()}>
          <Group
            title="Old worktrees"
            description={`${legacyWorktrees()} managed worktree${legacyWorktrees() === 1 ? " still lives" : "s still live"} in the old folder. Move them to finish the upgrade; the old data can be removed once none are left.`}
          >
            {plan()}
            <Show when={preview()}>
              <div class="settings-actions">
                <Button
                  variant="secondary"
                  disabled={moves().length === 0}
                  onClick={() => confirmMigration(true)}
                >
                  Move worktrees
                </Button>
              </div>
            </Show>
          </Group>
        </Show>

        <Show when={migratedHome() && legacyWorktrees() === 0}>
          <Group
            title="Old data"
            description="Every managed worktree has moved. The old database and agent plugins can go."
          >
            <Show when={removeError()}>{(message) => <p class="panel-error">{message()}</p>}</Show>
            <div class="settings-actions">
              <Button variant="secondary" disabled={removing()} onClick={confirmRemove}>
                Remove old data
              </Button>
            </div>
          </Group>
        </Show>
      </Show>
    </Page>
  );
}
