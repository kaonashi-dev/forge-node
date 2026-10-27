import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { createStore, produce } from "solid-js/store";
import type { ActionKind, ActionReady, ProjectAction } from "../../contracts/runtime";
import { forgeStore } from "../../state/forgeStore";
import { Icon } from "../../theme/icons";
import {
  Button,
  Checkbox,
  Dialog,
  Disclosure,
  IconButton,
  Select,
  Switch,
  TextField,
} from "../../ui/index";
import { buildDraft, duplicateLabels, toActionSet, type DraftRow } from "./actionDraft";
import { KIND_LABELS } from "./actionState";
import { actionsStore, type ActionsDialogTarget } from "./actionsStore";
import { setProjectActions, suggestActions } from "./commands";

const KINDS = (Object.keys(KIND_LABELS) as ActionKind[]).map((value) => ({
  value,
  label: KIND_LABELS[value],
}));

type ReadyKind = ActionReady["kind"];
const READY_OPTIONS: { value: ReadyKind; label: string }[] = [
  { value: "none", label: "Running is enough" },
  { value: "port", label: "Its port answers" },
  { value: "output", label: "Its output says…" },
];
const DEFAULT_READY_TIMEOUT_SECS = 60;

/**
 * A project's actions: what is saved, what its files suggest, and what its
 * `.forge/actions.toml` offers. Nothing reaches the daemon until Save, which
 * sends the whole set.
 */
export function ActionsDialog(props: { target: ActionsDialogTarget; onDismiss: () => void }) {
  const [rows, setRows] = createStore<DraftRow[]>([]);
  const [drafted, setDrafted] = createSignal(false);
  const [saving, setSaving] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  void suggestActions(props.target.workspaceId).catch((e: unknown) => setError(String(e)));

  // Drafted once, when the suggestions land: a later broadcast must not wipe
  // what the user is editing.
  createEffect(
    on(
      () => actionsStore.suggestions[props.target.workspaceId],
      (suggestions) => {
        if (drafted() || !suggestions) return;
        const saved = forgeStore.project_actions.filter(
          (action) => action.project_id === props.target.projectId,
        );
        setRows(
          buildDraft(
            props.target.projectId,
            saved,
            suggestions,
            actionsStore.repoFile[props.target.workspaceId] ?? null,
          ),
        );
        setDrafted(true);
      },
    ),
  );

  const repoFile = () => actionsStore.repoFile[props.target.workspaceId] ?? null;
  const sources = createMemo(() => [
    ...new Set(rows.filter((row) => row.detectedFrom).map((row) => row.detectedFrom!)),
  ]);
  const duplicates = createMemo(() => duplicateLabels(rows));
  const incomplete = createMemo(() =>
    rows.some((row) => row.include && (!row.action.label.trim() || !row.action.command.trim())),
  );

  function patch(index: number, change: Partial<ProjectAction>): void {
    setRows(index, "action", (action) => ({ ...action, ...change }));
  }

  function move(index: number, by: number): void {
    const target = index + by;
    if (target < 0 || target >= rows.length) return;
    setRows(
      produce((list) => {
        const [row] = list.splice(index, 1);
        list.splice(target, 0, row!);
      }),
    );
  }

  function add(): void {
    setRows(rows.length, {
      action: {
        id: crypto.randomUUID(),
        project_id: props.target.projectId,
        label: "",
        command: "",
        kind: "custom",
        cwd: null,
        single_instance: true,
        pinned: false,
        ready: { kind: "none" },
        port: null,
        depends_on: [],
        source: { kind: "manual" },
        position: rows.length,
        created_at: new Date().toISOString(),
      },
      origin: "saved",
      include: true,
      detectedFrom: null,
      dependsOnLabels: [],
      drift: false,
    });
  }

  function save(): void {
    setSaving(true);
    setError(null);
    setProjectActions(props.target.projectId, toActionSet(rows))
      .then(props.onDismiss)
      .catch((e: unknown) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setSaving(false));
  }

  const has = (origin: DraftRow["origin"]) => rows.some((row) => row.origin === origin);

  // Iterates the store's own rows, so typing in a field never remounts it.
  const section = (
    origin: DraftRow["origin"],
    includeLabel: (row: DraftRow) => string,
    movable: boolean,
  ) => (
    <For each={rows}>
      {(row, index) => (
        <Show when={row.origin === origin}>
          <ActionRow
            row={row}
            rows={rows}
            includeLabel={includeLabel(row)}
            onInclude={(include) => setRows(index(), "include", include)}
            onPatch={(change) => patch(index(), change)}
            onDepends={(ids) =>
              setRows(
                index(),
                produce((draft) => {
                  draft.action.depends_on = ids;
                  draft.dependsOnLabels = [];
                }),
              )
            }
            onMove={movable ? (by) => move(index(), by) : undefined}
          />
        </Show>
      )}
    </For>
  );

  return (
    <Dialog
      title={`Actions · ${props.target.name}`}
      description={sources().length > 0 ? `Detected from ${sources().join(" · ")}` : undefined}
      size="lg"
      onDismiss={props.onDismiss}
      footer={
        <div class="actions-dialog-footer">
          <Button variant="secondary" onClick={add}>
            Add action
          </Button>
          <span class="actions-dialog-spacer" />
          <Button variant="secondary" onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button
            onClick={save}
            loading={saving()}
            disabled={!drafted() || duplicates().length > 0 || incomplete()}
          >
            Save
          </Button>
        </div>
      }
    >
      <Show when={error() ?? actionsStore.error}>
        {(message) => <p class="forge-dialog-copy actions-dialog-error">{message()}</p>}
      </Show>
      <Show when={duplicates().length > 0}>
        <p class="forge-dialog-copy actions-dialog-error">
          Two actions are called “{duplicates()[0]}”. Labels must differ, ignoring case.
        </p>
      </Show>
      <Show
        when={drafted()}
        fallback={<p class="forge-dialog-copy">Reading the project’s manifests…</p>}
      >
        <Show when={has("saved")}>
          <h3 class="actions-dialog-heading">Saved</h3>
          {section("saved", () => "Keep", true)}
        </Show>
        <Show when={has("suggested")}>
          <h3 class="actions-dialog-heading">Suggested</h3>
          {section("suggested", () => "Add", false)}
        </Show>
        <Show when={repoFile()}>
          {(file) => (
            <>
              <h3 class="actions-dialog-heading">From .forge/actions.toml</h3>
              <Show
                when={!file().error}
                fallback={<p class="forge-dialog-copy actions-dialog-error">{file().error}</p>}
              >
                <p class="forge-dialog-copy">
                  The repository offers these. Importing copies them into this project: what runs is
                  the copy you approve here, never the file as it changes.
                </p>
                <Show when={file().drift.length > 0}>
                  <p class="forge-dialog-copy actions-dialog-warn">
                    The file changed since{" "}
                    {file().drift.length === 1 ? "an action was" : "actions were"} imported. Saved
                    copies keep running as approved; tick an update below to take it.
                  </p>
                </Show>
                {section(
                  "repo",
                  (row) =>
                    rows.some((r) => r.origin === "saved" && r.action.id === row.action.id)
                      ? "Update"
                      : "Import",
                  false,
                )}
              </Show>
            </>
          )}
        </Show>
        <Show when={rows.length === 0}>
          <p class="forge-dialog-copy">
            Nothing to suggest at this checkout’s root. Add an action for any command you run often.
          </p>
        </Show>
      </Show>
    </Dialog>
  );
}

function ActionRow(props: {
  row: DraftRow;
  rows: readonly DraftRow[];
  includeLabel: string;
  onInclude: (include: boolean) => void;
  onPatch: (change: Partial<ProjectAction>) => void;
  onDepends: (ids: string[]) => void;
  onMove?: (by: number) => void;
}) {
  const action = () => props.row.action;
  const ready = () => action().ready ?? ({ kind: "none" } as ActionReady);
  const others = createMemo(() =>
    props.rows.filter(
      (row) => row.include && row.action.id !== action().id && row.action.label.trim(),
    ),
  );
  /** By id once edited; a suggestion still names its dependencies by label. */
  const dependsOn = createMemo(() => {
    const labels = props.row.dependsOnLabels.map((label) => label.toLowerCase());
    if (labels.length === 0) return action().depends_on ?? [];
    return others()
      .filter((other) => labels.includes(other.action.label.toLowerCase()))
      .map((other) => other.action.id);
  });

  function setReady(kind: ReadyKind): void {
    const timeout_secs =
      ready().kind === "none"
        ? DEFAULT_READY_TIMEOUT_SECS
        : (ready() as { timeout_secs: number }).timeout_secs;
    if (kind === "none") props.onPatch({ ready: { kind: "none" } });
    else if (kind === "port")
      props.onPatch({
        ready: { kind: "port", timeout_secs },
        port: action().port ?? { base: 3000, per_worktree: true },
      });
    else props.onPatch({ ready: { kind: "output", pattern: "localhost:", timeout_secs } });
  }

  function toggleDependency(id: string, on: boolean): void {
    props.onDepends(on ? [...dependsOn(), id] : dependsOn().filter((dep) => dep !== id));
  }

  return (
    <div class="action-row" classList={{ excluded: !props.row.include }}>
      <div class="action-row-main">
        <Checkbox
          checked={props.row.include}
          onChange={props.onInclude}
          label={`${props.includeLabel} ${action().label || "this action"}`}
          hideLabel
        />
        <TextField
          class="action-row-label"
          value={action().label}
          onChange={(label) => props.onPatch({ label })}
          aria-label="Label"
          placeholder="label"
        />
        <TextField
          class="action-row-command"
          value={action().command}
          onChange={(command) => props.onPatch({ command })}
          aria-label="Command"
          placeholder="command"
        />
        <Select
          class="action-row-kind"
          value={action().kind}
          options={KINDS}
          onChange={(kind) => props.onPatch({ kind })}
          aria-label="Kind"
        />
        <Switch
          size="sm"
          label="Pinned"
          hideLabel
          checked={action().pinned}
          onChange={(pinned) => props.onPatch({ pinned })}
        />
        <Show when={props.onMove}>
          {(move) => (
            <span class="action-row-order">
              <IconButton label="Move up" size="xs" onClick={() => move()(-1)}>
                <Icon name="chevron-up" size={12} />
              </IconButton>
              <IconButton label="Move down" size="xs" onClick={() => move()(1)}>
                <Icon name="chevron-down" size={12} />
              </IconButton>
            </span>
          )}
        </Show>
      </div>
      <Show when={props.row.drift || props.row.detectedFrom}>
        <div class="action-row-meta">
          <Show when={props.row.detectedFrom}>{(from) => <span>{from()}</span>}</Show>
          <Show when={props.row.drift}>
            <span class="actions-dialog-warn">
              the repository file has changed since this import
            </span>
          </Show>
        </div>
      </Show>
      <Disclosure summary={<span class="action-row-advanced">Advanced</span>}>
        <div class="action-row-advanced-body">
          <TextField
            label="Directory"
            description="Relative to the checkout; empty is its root."
            value={action().cwd ?? ""}
            onChange={(cwd) => props.onPatch({ cwd: cwd.trim() || null })}
          />
          <Switch
            label="One at a time"
            description="A second click shows the running one instead of starting another."
            checked={action().single_instance}
            onChange={(single_instance) => props.onPatch({ single_instance })}
          />
          <Select
            label="Ready when"
            value={ready().kind}
            options={READY_OPTIONS}
            onChange={setReady}
          />
          <Show when={ready().kind === "output"}>
            <TextField
              label="Output contains"
              description="Plain text, any case. Not a regular expression."
              value={(ready() as { pattern: string }).pattern}
              onChange={(pattern) =>
                props.onPatch({
                  ready: {
                    kind: "output",
                    pattern,
                    timeout_secs: (ready() as { timeout_secs: number }).timeout_secs,
                  },
                })
              }
            />
          </Show>
          <TextField
            label="Port"
            description="Injected as PORT and substituted for {port} in the command."
            value={action().port ? String(action().port!.base) : ""}
            onChange={(value) => {
              const base = Number.parseInt(value, 10);
              props.onPatch({
                port:
                  Number.isInteger(base) && base > 0 && base < 65536
                    ? { base, per_worktree: action().port?.per_worktree ?? true }
                    : null,
              });
            }}
          />
          <Show when={action().port}>
            {(port) => (
              <Switch
                label="A port per worktree"
                description="Take the next free port above this one, so each checkout can serve at once."
                checked={port().per_worktree}
                onChange={(per_worktree) => props.onPatch({ port: { ...port(), per_worktree } })}
              />
            )}
          </Show>
          <Show when={others().length > 0}>
            <fieldset class="action-row-deps">
              <legend>Starts after</legend>
              <For each={others()}>
                {(other) => (
                  <Checkbox
                    label={other.action.label}
                    checked={dependsOn().includes(other.action.id)}
                    onChange={(on) => toggleDependency(other.action.id, on)}
                  />
                )}
              </For>
            </fieldset>
          </Show>
        </div>
      </Disclosure>
    </div>
  );
}
