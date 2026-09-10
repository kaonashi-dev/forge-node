import { Show, createEffect, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { createWorktree, fetchRemote } from "../runtime/api";
import { listBranches } from "../workbench/api";
import { launchAgent, launchShell } from "./sessionActions";
import { forgeStore } from "../store/forgeStore";
import { setLoading, workbenchStore } from "../store/workbenchStore";
import { runtimeStore, setRuntimeStore } from "../store/runtimeStore";
import { Button, Combobox, Dialog, Select, TextField, type ComboboxOption } from "../ui";
import type { SelectOption } from "../ui/Select";
import { SessionGlyph } from "../theme/icons/SessionGlyph";
import { branchEntries, type BranchPickerEntry } from "./branchPickerEntries";
import {
  createdWorkspace,
  defaultLaunch,
  launchOptions,
  NOTHING,
  parseLaunch,
  slugFromBranch,
} from "./worktreeLaunch";

/**
 * How long to wait for the daemon's `WorkspaceCreated` before giving up on
 * opening something in it.
 *
 * `CreateWorktree` provisions the checkout — `git worktree add`, then the
 * configured copies and the setup script — so the broadcast is not immediate.
 * On the timeout the checkout still exists and the rail already shows it; only
 * the launch is abandoned, and the dialog says so rather than hanging.
 */
const OPEN_TIMEOUT_MS = 30_000;

export type BranchPickerProps = {
  projectId: string;
  projectName: string;
  onDismiss: () => void;
  onCreated?: () => void;
};

/**
 * New-worktree dialog (§14.3) — one searchable list of branches to start from.
 *
 * Same shape as the command palette: a filtering listbox whose cursor moves
 * while focus stays in the query field.
 */
export function BranchPicker(props: BranchPickerProps) {
  const [query, setQuery] = createSignal("");
  const [fetching, setFetching] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [name, setName] = createSignal("");
  const [launch, setLaunch] = createSignal(defaultLaunch(forgeStore.launchables));
  /** The branch a create is in flight for; the effect below opens what lands. */
  const [pending, setPending] = createSignal<string | null>(null);
  /**
   * The row the list's cursor is on.
   *
   * Reported by `Combobox` rather than owned here: the footer's "Create
   * worktree" button acts on the same row `Enter` does, and two sources for
   * "which branch" is how they come to disagree.
   */
  const [chosen, setChosen] = createSignal<BranchPickerEntry | null>(null);

  onMount(() => {
    setLoading("branches", true);
    void listBranches(props.projectId).catch(() => undefined);
  });

  const entries = createMemo(() => branchEntries(query(), workbenchStore.branches, forgeStore));

  const options = createMemo<ComboboxOption<BranchPickerEntry>[]>(() =>
    entries().map((entry) => ({
      value: entry,
      disabled: !entry.enabled,
      render: () => (
        <>
          <span class="palette-label">{entry.label}</span>
          <Show when={entry.note}>{(note) => <span class="palette-note">{note()}</span>}</Show>
        </>
      ),
    })),
  );

  /** One line naming what provisioning will do, or nothing when it will not. */
  const shareSummary = () => {
    const rules = forgeStore.worktree_shares.filter(
      (rule) => rule.project_id === props.projectId && rule.enabled,
    );
    if (rules.length === 0) return "";
    const counts = new Map<string, number>();
    for (const rule of rules) {
      counts.set(rule.strategy.kind, (counts.get(rule.strategy.kind) ?? 0) + 1);
    }
    const words: string[] = [];
    const say = (kind: string, one: string, many: string) => {
      const n = counts.get(kind) ?? 0;
      if (n > 0) words.push(n === 1 ? one : `${n} ${many}`);
    };
    say("link", "one file linked", "files linked");
    say("copy", "one file copied", "files copied");
    say("clone", "one path cloned", "paths cloned");
    say("run", "one command run", "commands run");
    return `This checkout will be set up with ${words.join(", ")}.`;
  };

  /* The placeholder under the name field is what the daemon would derive, so
     the field can stay optional and still be informative. */
  const derived = () => {
    const branch = chosen()?.choice?.branch;
    return branch ? slugFromBranch(branch) : "";
  };

  function choose(entry: BranchPickerEntry | null | undefined): void {
    if (!entry?.enabled || !entry.choice || pending() !== null) return;
    const branch = entry.choice.branch;
    const folder = name().trim();
    setError(null);
    setRuntimeStore("worktreeCreationFailure", null);
    setPending(branch);
    void createWorktree(props.projectId, branch, entry.choice.base, folder || null).catch(
      (reason: unknown) => {
        setPending(null);
        setError(String(reason));
      },
    );
  }

  createEffect(() => {
    const failure = runtimeStore.worktreeCreationFailure;
    if (!failure || failure.project !== props.projectId || failure.branch !== pending()) {
      return;
    }
    setPending(null);
    setError(failure.reason);
    setRuntimeStore("worktreeCreationFailure", null);
  });

  /*
   * Open the checkout the moment the daemon says it exists.
   *
   * Keyed on the branch rather than on a returned id because `CreateWorktree`
   * acks and then broadcasts, like every other mutation here — the id does not
   * exist at the call site. Git will not check one branch out twice in a
   * repository, so the branch identifies the row it produced.
   */
  createEffect(() => {
    const branch = pending();
    if (branch === null) return;
    const workspace = createdWorkspace(forgeStore, props.projectId, branch);
    if (!workspace) return;

    setPending(null);
    props.onCreated?.();
    const choice = parseLaunch(launch(), forgeStore.launchables);
    if (choice.kind === "shell") {
      void launchShell(workspace.id).catch(() => undefined);
    } else if (choice.kind === "agent") {
      void launchAgent(choice.provider, choice.profile, workspace.id).catch(() => undefined);
    }
    props.onDismiss();
  });

  // A provisioning step that never finishes must not leave a dialog that never
  // closes: the checkout is made either way, so only the launch is dropped.
  createEffect(() => {
    if (pending() === null) return;
    const timer = window.setTimeout(() => {
      setPending(null);
      setError("The checkout was created, but it did not arrive in time to open.");
    }, OPEN_TIMEOUT_MS);
    onCleanup(() => window.clearTimeout(timer));
  });

  function onFetch(): void {
    if (fetching()) return;
    setFetching(true);
    setError(null);
    void fetchRemote(props.projectId)
      .then(() => {
        setLoading("branches", true);
        return listBranches(props.projectId);
      })
      .catch(() => setError("Fetch failed."))
      .finally(() => setFetching(false));
  }

  const hasRemote = () => (workbenchStore.branches?.remotes.length ?? 0) > 0;
  const loading = () => workbenchStore.loading.branches;

  const launchSelectOptions = createMemo<SelectOption<string>[]>(() => {
    const base = launchOptions(forgeStore.launchables);
    return base.map((option) => {
      if (option.value === NOTHING) return option;
      const launchable = forgeStore.launchables.find((item) => item.key === option.value);
      if (!launchable) return option;
      if (launchable.kind === "shell") {
        return { ...option, icon: "square-terminal" as const };
      }
      return {
        ...option,
        glyph: <SessionGlyph providerId={launchable.provider ?? null} size={14} />,
      };
    });
  });

  return (
    <Dialog
      title="New worktree"
      hideTitle
      flush
      class="branch-picker"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Show when={error()}>{(message) => <span class="panel-error">{message()}</span>}</Show>
          <Show when={hasRemote()}>
            <Button
              variant="secondary"
              disabled={fetching() || pending() !== null}
              onClick={onFetch}
            >
              {fetching() ? "Fetching…" : "Fetch from remote"}
            </Button>
          </Show>
          <Button variant="secondary" disabled={pending() !== null} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button
            variant="primary"
            disabled={!chosen()?.enabled || pending() !== null}
            onClick={() => choose(chosen())}
          >
            {pending() === null ? "Create worktree" : "Creating…"}
          </Button>
        </>
      }
    >
      <Combobox
        options={options()}
        query={query()}
        onQuery={setQuery}
        onCursor={setChosen}
        onChoose={choose}
        label="Branch name or filter"
        placeholder="Branch name or filter…"
        inputClass="branch-picker-input"
        empty={<p class="empty-copy">{loading() ? "Loading branches…" : "No branches match."}</p>}
        leading={
          <>
            <span class="palette-scope">New worktree</span>
            <span class="palette-note">{props.projectName}</span>
          </>
        }
      />
      {/* Both answers up front — what to call it and what to start in it — so
          creating a checkout and getting to work in it is one gesture rather
          than a dialog followed by a hunt through the rail. */}
      <div class="branch-picker-fields">
        <TextField
          label="Folder name"
          description={derived() ? `Optional — defaults to ${derived()}` : "Optional"}
          placeholder={derived()}
          value={name()}
          disabled={pending() !== null}
          onChange={setName}
        />
        <Select
          label="Open with"
          value={launch()}
          options={launchSelectOptions()}
          onChange={setLaunch}
          disabled={pending() !== null}
        />
      </div>
      {/* What the new checkout will arrive with (§14.2). Read-only: the rules
          are edited in Settings → Shared files, and a creation dialog is the
          place to *notice* they exist, not to maintain them. */}
      <Show when={shareSummary()}>
        {(summary) => <p class="branch-picker-shares">{summary()}</p>}
      </Show>
    </Dialog>
  );
}
