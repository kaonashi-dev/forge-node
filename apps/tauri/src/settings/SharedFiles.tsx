import { For, Show, createEffect, createMemo, createSignal } from "solid-js";
import { AlertDialog, Badge, Button, Menu, Select, Switch, TextField } from "../ui";
import { Icon } from "../theme/icons";
import { Group, Page, Row } from "./SettingsLayout";
import { forgeStore } from "../store/forgeStore";
import { workbenchStore } from "../store/workbenchStore";
import { sharesStore } from "../store/sharesStore";
import { pickFile, removeShareRule, setProjectShares } from "../runtime/api";
import {
  adoptIntoShareStore,
  applyShares,
  detectShareCandidates,
  loadShareStatus,
  previewShares,
} from "../workbench/api";
import type {
  ShareCandidate,
  ShareCleanup,
  ShareRule,
  ShareState,
  ShareStrategy,
} from "../runtime/types";

/** The strategies a rule can take, in the order the picker offers them. */
const STRATEGIES: { value: ShareStrategy["kind"]; label: string; help: string }[] = [
  {
    value: "link",
    label: "Link",
    help: "One file, every worktree — an edit is visible everywhere. Some editors replace a link when they save; Forge Node notices and says so.",
  },
  {
    value: "copy",
    label: "Copy",
    help: "An independent copy per worktree. Nothing a worktree does can reach the others.",
  },
  {
    value: "clone",
    label: "Clone",
    help: "A copy-on-write clone: instant and free for a big directory, and independent afterwards. Falls back to a full copy where the filesystem cannot.",
  },
  {
    value: "run",
    label: "Run a command",
    help: "Produce it instead of sharing it — the honest answer for dependencies, because two branches can disagree on a lockfile.",
  },
];

const CLASS_LABEL: Record<ShareCandidate["class"], string> = {
  Secret: "secret",
  Dependencies: "dependencies",
  BuildOutput: "build output",
  Cache: "cache",
  EditorState: "editor state",
  Other: "other",
};

function strategyKind(strategy: ShareStrategy): ShareStrategy["kind"] {
  return strategy.kind;
}

function strategyLabel(strategy: ShareStrategy): string {
  if (strategy.kind === "run") return `Run: ${strategy.command}`;
  return STRATEGIES.find((item) => item.value === strategy.kind)?.label ?? strategy.kind;
}

function size(bytes: number | null): string {
  if (bytes === null) return "—";
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}

function stateLabel(state: ShareState): string {
  switch (state.state) {
    case "applied":
      return "applied";
    case "missing":
      return "missing";
    case "severed":
      return "link replaced by a real file";
    case "diverged":
      return "changed since it was written";
    case "skipped":
      return state.reason;
    default:
      return state.message;
  }
}

function stateTone(state: ShareState): "good" | "warn" | "bad" | "neutral" {
  switch (state.state) {
    case "applied":
      return "good";
    case "missing":
      return "neutral";
    case "severed":
    case "failed":
      return "bad";
    default:
      return "warn";
  }
}

export function SharedFiles(props: { projectId?: string; workspaceId?: string } = {}) {
  const workspaceId = () =>
    props.workspaceId ?? (props.projectId ? null : workbenchStore.workspace);
  const workspace = () => forgeStore.workspaces.find((item) => item.id === workspaceId()) ?? null;
  const project = () => props.projectId ?? workspace()?.project_id ?? null;
  const projectName = () =>
    forgeStore.projects.find((item) => item.id === project())?.name ?? "this project";

  const rules = createMemo(() =>
    forgeStore.worktree_shares
      .filter((rule) => rule.project_id === project())
      .slice()
      .sort((a, b) => a.position - b.position),
  );
  const checkouts = createMemo(() =>
    forgeStore.workspaces.filter((item) => item.project_id === project()),
  );
  const candidates = createMemo(() => {
    const id = project();
    return id ? (sharesStore.candidates[id] ?? []) : [];
  });

  const [draftPath, setDraftPath] = createSignal("");
  const [pathError, setPathError] = createSignal<string | null>(null);
  const [removing, setRemoving] = createSignal<ShareRule | null>(null);
  const [cleanup, setCleanup] = createSignal<ShareCleanup>("leave");

  // One scan per project, when the section is first opened on it: a
  // `git status --ignored` plus a bounded walk is not something to repeat on
  // every keystroke in the path field.
  let scanned: string | null = null;
  createEffect(() => {
    const id = project();
    if (!id || scanned === id) return;
    scanned = id;
    void detectShareCandidates(id).catch(() => undefined);
    for (const checkout of checkouts()) {
      void loadShareStatus(checkout.id).catch(() => undefined);
    }
  });

  function save(next: ShareRule[]): void {
    const id = project();
    if (!id) return;
    void setProjectShares(
      id,
      next.map((rule, position) => ({ ...rule, position })),
    ).catch(() => undefined);
  }

  function add(path: string, strategy: ShareStrategy): void {
    const id = project();
    if (!id) return;
    const clean = path.trim().replace(/^\/+/, "").replace(/\/+$/, "");
    if (!clean) {
      setPathError("Type a path first.");
      return;
    }
    if (clean.startsWith("..") || clean.includes("/../")) {
      setPathError("The path must stay inside the repository.");
      return;
    }
    if (rules().some((rule) => rule.path === clean)) {
      setPathError("That path already has a rule.");
      return;
    }
    setPathError(null);
    setDraftPath("");
    save([
      ...rules(),
      {
        id: crypto.randomUUID(),
        project_id: id,
        path: clean,
        strategy,
        enabled: true,
        position: rules().length,
        created_at: new Date().toISOString(),
      },
    ]);
  }

  function retarget(rule: ShareRule, kind: ShareStrategy["kind"]): void {
    const strategy: ShareStrategy =
      kind === "run" ? { kind: "run", command: "", timeout_secs: 600 } : { kind };
    save(rules().map((item) => (item.id === rule.id ? { ...item, strategy } : item)));
    // Linking a path that exists as a real file has to move it into the store,
    // which is the one operation that changes the primary checkout — so it is
    // asked for, not implied.
    if (kind === "link" && project()) {
      void adoptIntoShareStore(project() as string, rule.path).catch(() => undefined);
    }
  }

  function move(rule: ShareRule, by: number): void {
    const list = rules();
    const from = list.findIndex((item) => item.id === rule.id);
    const to = from + by;
    if (from < 0 || to < 0 || to >= list.length) return;
    const next = [...list];
    const [moved] = next.splice(from, 1);
    next.splice(to, 0, moved);
    save(next);
  }

  function stateFor(rule: ShareRule, workspaceId: string): ShareState | null {
    const entries = sharesStore.status[workspaceId];
    return entries?.find((entry) => entry.rule_id === rule.id)?.state ?? null;
  }

  function summary(rule: ShareRule): { applied: number; total: number; worst: ShareState | null } {
    let applied = 0;
    let worst: ShareState | null = null;
    for (const checkout of checkouts()) {
      const state = stateFor(rule, checkout.id);
      if (!state) continue;
      if (state.state === "applied") applied += 1;
      else if (!worst || stateTone(state) === "bad") worst = state;
    }
    return { applied, total: checkouts().length, worst };
  }

  async function browse(): Promise<void> {
    const root = forgeStore.projects.find((item) => item.id === project())?.root_path;
    const picked = await pickFile("Share a file between worktrees", root ?? undefined);
    if (!picked) return;
    if (root && picked.startsWith(`${root}/`)) {
      setDraftPath(picked.slice(root.length + 1));
      setPathError(null);
      return;
    }
    setPathError("Pick a file inside the repository — a path outside it cannot be shared.");
  }

  return (
    <Page
      title="Shared files"
      summary="What a new worktree needs on disk that git does not carry: .env, installed dependencies, a local certificate."
    >
      <Show
        when={project()}
        fallback={<p class="empty-copy">Open a checkout to reach its shared files.</p>}
      >
        {(id) => (
          <>
            <Show when={props.workspaceId}>
              {(target) => (
                <Group
                  title="This worktree"
                  description="Apply project rules here. Copies can be edited independently; linked files share edits across checkouts."
                  aside={
                    <Button
                      variant="secondary"
                      disabled={sharesStore.applying.includes(target())}
                      onClick={() => void applyShares(target()).catch(() => undefined)}
                    >
                      Apply missing files here
                    </Button>
                  }
                >
                  <For each={rules()} fallback={<p class="empty-copy">No project rules yet.</p>}>
                    {(rule) => (
                      <Row
                        label={rule.path}
                        description={
                          stateFor(rule, target())
                            ? stateLabel(stateFor(rule, target()) as ShareState)
                            : "Not checked"
                        }
                        control={
                          <Button
                            variant="secondary"
                            disabled={!rule.enabled || sharesStore.applying.includes(target())}
                            onClick={() =>
                              void applyShares(target(), [rule.id]).catch(() => undefined)
                            }
                          >
                            Re-apply here
                          </Button>
                        }
                      />
                    )}
                  </For>
                  <p class="settings-note">
                    Re-applying backs up and replaces existing files. Editing or removing project
                    rules below affects all worktrees.
                  </p>
                </Group>
              )}
            </Show>
            <Show when={sharesStore.error}>
              {(error) => (
                <p class="settings-note" role="alert">
                  {error()}
                </p>
              )}
            </Show>
            <Group
              title={`Project rules · ${projectName()}`}
              description={
                rules().length > 0
                  ? `They apply to every checkout of ${projectName()}, the main one included.`
                  : `${projectName()} has no rules of its own, so the global [worktrees] list in config.toml is what runs.`
              }
              aside={
                <Button
                  variant="secondary"
                  onClick={() => {
                    for (const checkout of checkouts()) {
                      void applyShares(checkout.id).catch(() => undefined);
                    }
                  }}
                >
                  Apply to all checkouts
                </Button>
              }
            >
              <Show
                when={rules().length > 0}
                fallback={<p class="empty-copy">Nothing shared yet. Add a path below.</p>}
              >
                <ul class="share-rules">
                  <For each={rules()}>
                    {(rule) => {
                      const counts = () => summary(rule);
                      return (
                        <li class="share-rule" classList={{ off: !rule.enabled }}>
                          <div class="share-rule-main">
                            <code class="share-rule-path">{rule.path}</code>
                            <Select<ShareStrategy["kind"]>
                              aria-label={`Strategy for ${rule.path}`}
                              class="share-rule-strategy"
                              value={strategyKind(rule.strategy)}
                              options={STRATEGIES.map((item) => ({
                                value: item.value,
                                label: item.label,
                              }))}
                              onChange={(kind) => retarget(rule, kind)}
                            />
                            <Show when={rule.strategy.kind === "run"}>
                              <TextField
                                aria-label={`Command for ${rule.path}`}
                                class="share-rule-command"
                                size="sm"
                                placeholder="pnpm install"
                                value={rule.strategy.kind === "run" ? rule.strategy.command : ""}
                                onChange={(command) =>
                                  save(
                                    rules().map((item) =>
                                      item.id === rule.id
                                        ? {
                                            ...item,
                                            strategy: {
                                              kind: "run",
                                              command,
                                              timeout_secs:
                                                item.strategy.kind === "run"
                                                  ? item.strategy.timeout_secs
                                                  : 600,
                                            },
                                          }
                                        : item,
                                    ),
                                  )
                                }
                              />
                            </Show>
                          </div>

                          <div class="share-rule-state">
                            <Badge
                              tone={
                                counts().worst ? stateTone(counts().worst as ShareState) : "good"
                              }
                            >
                              {counts().worst
                                ? stateLabel(counts().worst as ShareState)
                                : `applied in ${counts().applied}/${counts().total}`}
                            </Badge>
                          </div>

                          <div class="share-rule-actions">
                            <Switch
                              label="Enabled"
                              hideLabel
                              size="sm"
                              checked={rule.enabled}
                              onChange={(enabled) =>
                                save(
                                  rules().map((item) =>
                                    item.id === rule.id ? { ...item, enabled } : item,
                                  ),
                                )
                              }
                            />
                            <Menu
                              triggerLabel={`Actions for ${rule.path}`}
                              trigger={<Icon name="settings" size={14} />}
                              placement="bottom-end"
                              items={[
                                {
                                  kind: "item",
                                  label: "Re-apply everywhere",
                                  run: () => {
                                    for (const checkout of checkouts()) {
                                      void applyShares(checkout.id, [rule.id]).catch(
                                        () => undefined,
                                      );
                                    }
                                  },
                                },
                                {
                                  kind: "item",
                                  label: "Move up",
                                  run: () => move(rule, -1),
                                },
                                {
                                  kind: "item",
                                  label: "Move down",
                                  run: () => move(rule, 1),
                                },
                                {
                                  kind: "item",
                                  label: "Remove…",
                                  destructive: true,
                                  run: () => {
                                    setCleanup("leave");
                                    setRemoving(rule);
                                  },
                                },
                              ]}
                            />
                          </div>
                        </li>
                      );
                    }}
                  </For>
                </ul>
              </Show>

              <Row
                label="Add a path"
                description="Anything in the repository, whether or not the scan proposed it."
              >
                <div class="share-add">
                  <TextField
                    aria-label="Path to share"
                    placeholder=".env.local"
                    value={draftPath()}
                    onChange={setDraftPath}
                    invalid={pathError() !== null}
                    errorMessage={pathError() ?? undefined}
                  />
                  <Button variant="secondary" onClick={() => void browse()}>
                    Browse…
                  </Button>
                  <Button onClick={() => add(draftPath(), { kind: "copy" })}>Add</Button>
                </div>
              </Row>
            </Group>

            <Group
              title="Detected, not shared yet"
              description="What this repository ignores, with what Forge Node would do about it."
              aside={
                <Button
                  variant="secondary"
                  onClick={() => void detectShareCandidates(id()).catch(() => undefined)}
                >
                  Scan again
                </Button>
              }
            >
              <Show
                when={candidates().some((candidate) => !candidate.already_ruled)}
                fallback={
                  <p class="empty-copy">
                    {sharesStore.scanning ? "Scanning…" : "Nothing left to propose."}
                  </p>
                }
              >
                <ul class="share-candidates">
                  <For each={candidates().filter((candidate) => !candidate.already_ruled)}>
                    {(candidate) => (
                      <li class="share-candidate">
                        <code class="share-rule-path">{candidate.path}</code>
                        <span class="share-candidate-class">{CLASS_LABEL[candidate.class]}</span>
                        <span class="share-candidate-size">{size(candidate.size_bytes)}</span>
                        <Button
                          variant="secondary"
                          onClick={() => add(candidate.path, candidate.suggested)}
                        >
                          Share as {strategyLabel(candidate.suggested).toLowerCase()}
                        </Button>
                      </li>
                    )}
                  </For>
                </ul>
              </Show>
              <Show when={sharesStore.truncated[id()]}>
                <p class="settings-note">
                  The scan stopped at its budget, so this list is a floor, not the whole tree.
                </p>
              </Show>
            </Group>

            <Group title="What each strategy means">
              <dl class="settings-facts">
                <For each={STRATEGIES}>
                  {(item) => (
                    <>
                      <dt>{item.label}</dt>
                      <dd>{item.help}</dd>
                    </>
                  )}
                </For>
              </dl>
              <div class="settings-actions">
                <Button
                  variant="secondary"
                  onClick={() => {
                    const current = workspaceId();
                    if (current) void previewShares(current).catch(() => undefined);
                  }}
                >
                  Preview this checkout
                </Button>
              </div>
            </Group>

            <Show when={removing()}>
              {(rule) => (
                <AlertDialog
                  open
                  title={`Stop sharing ${rule().path}?`}
                  description="The rule goes either way. What happens to the files it already put in each checkout is up to you."
                  onDismiss={() => setRemoving(null)}
                  footer={
                    <>
                      <Button variant="secondary" onClick={() => setRemoving(null)}>
                        Cancel
                      </Button>
                      <Button
                        variant="danger"
                        onClick={() => {
                          void removeShareRule(id(), rule().id, cleanup()).catch(() => undefined);
                          setRemoving(null);
                        }}
                      >
                        Remove the rule
                      </Button>
                    </>
                  }
                >
                  <Select<ShareCleanup>
                    label="In every checkout"
                    value={cleanup()}
                    options={[
                      { value: "leave", label: "Leave the files as they are" },
                      {
                        value: "remove_injected",
                        label: "Delete what Forge Node wrote (never a file you changed)",
                      },
                      {
                        value: "materialize",
                        label: "Turn links into real files",
                        disabled: rule().strategy.kind !== "link",
                      },
                    ]}
                    onChange={setCleanup}
                  />
                </AlertDialog>
              )}
            </Show>
          </>
        )}
      </Show>
    </Page>
  );
}
