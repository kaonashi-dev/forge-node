import { For, Show, createEffect, createMemo, createSignal, onMount } from "solid-js";
import { newAgent, selectSession } from "../runtime/api";
import { sessionStateLabel, sessionTitle, type PullRequest, type Session } from "../runtime/types";
import { defaultAgentFrom } from "../settings/defaultAgent";
import { forgeStore } from "../store/forgeStore";
import { setRuntimeStore } from "../store/runtimeStore";
import { setLoading, workbenchStore } from "../store/workbenchStore";
import { close, focus, openDiff, openPrCompose, showTerminal } from "../store/viewsStore";
import { loadDiff } from "./api";
import {
  builtinTasks,
  composePrompt,
  diffIsEmpty,
  diffSummary,
  launchHint,
  taskAction,
  taskOrDefault,
  type PrTask,
} from "./prTasks";
import type { WorkspaceDiff } from "./types";
import { TERMINAL_VIEW } from "./views";
import { Button, RadioGroup, TextArea, Tooltip } from "../ui";

export function openComposeForWorkspace(workspace: string): void {
  openPrCompose();
  setLoading("diff", true);
  void loadDiff(workspace).catch(() => undefined);
}

export function PrComposeView() {
  const [taskId, setTaskId] = createSignal(builtinTasks()[0].id);
  const [promptOpen, setPromptOpen] = createSignal(false);
  const [extra, setExtra] = createSignal("");
  const [awaiting, setAwaiting] = createSignal(false);
  const [launchedSession, setLaunchedSession] = createSignal<string | null>(null);

  const workspace = () => workbenchStore.workspace;
  const diff = (): WorkspaceDiff | null => workbenchStore.diff;
  const loading = () => workbenchStore.loading.diff;
  const error = () => workbenchStore.diffError;
  const task = (): PrTask => taskOrDefault(taskId());

  onMount(() => {
    const ws = workspace();
    if (ws && !diff() && !loading()) {
      setLoading("diff", true);
      void loadDiff(ws).catch(() => undefined);
    }
  });

  // Adopt the newest agent session in this checkout once a launch is in flight.
  createEffect(() => {
    if (!awaiting()) return;
    const ws = workspace();
    if (!ws) return;
    const session = forgeStore.sessions
      .filter((item) => item.workspace_id === ws && item.agent_provider_id != null)
      .reduce<Session | null>(
        (newest, item) => (!newest || item.created_at > newest.created_at ? item : newest),
        null,
      );
    if (session) {
      setLaunchedSession(session.id);
      setAwaiting(false);
    }
  });

  const launched = createMemo(() => {
    const id = launchedSession();
    return id ? (forgeStore.sessions.find((item) => item.id === id) ?? null) : null;
  });

  const sessionStatus = createMemo(() => {
    const running = launched();
    if (running) return `${sessionTitle(running)} · ${sessionStateLabel(running.state)}`;
    if (awaiting()) return "starting the agent…";
    return null;
  });

  const pullRequest = createMemo((): PullRequest | null => {
    const ws = workspace();
    if (!ws) return null;
    const checkout = forgeStore.workspaces.find((item) => item.id === ws);
    const branch = checkout?.branch;
    if (!checkout || !branch) return null;
    return (
      forgeStore.pull_requests.pull_requests.find(
        (pr) => pr.head_ref === branch && pr.project_id === checkout.project_id,
      ) ?? null
    );
  });

  function reload(): void {
    const ws = workspace();
    if (!ws) return;
    setLoading("diff", true);
    void loadDiff(ws).catch(() => undefined);
  }

  function launch(): void {
    const d = diff();
    if (!d || diffIsEmpty(d)) return;
    const provider = promptCapableProvider();
    if (!provider) {
      setRuntimeStore(
        "notice",
        "No installed agent accepts a prompt at launch. Install Claude, Codex, or Cursor.",
      );
      return;
    }
    const ws = workspace();
    if (!ws) return;
    const prompt = composePrompt(task(), d, extra());
    setAwaiting(true);
    void newAgent(provider, null, ws, null, prompt).catch(() => {
      setAwaiting(false);
      setRuntimeStore("notice", "The daemon refused to launch the agent.");
    });
  }

  function showLaunched(): void {
    const id = launchedSession();
    if (id) void selectSession(id).catch(() => undefined);
    showTerminal();
    focus(TERMINAL_VIEW);
  }

  const ready = () => {
    const d = diff();
    return d && !diffIsEmpty(d) && !awaiting() && !launchedSession();
  };

  const headerLine = () => {
    const d = diff();
    if (loading() && !d) return { text: "reading the working tree…", tone: "muted" };
    if (error()) return { text: error() ?? "failed to read diff", tone: "error" };
    if (!d || diffIsEmpty(d)) return { text: "no changes to work from", tone: "error" };
    const branch = d.branch ?? "detached";
    return { text: `${branch} · ${diffSummary(d)}`, tone: "text" };
  };

  const hint = () =>
    launchHint(
      task(),
      diff(),
      launchedSession() != null,
      awaiting(),
      error(),
      loading() && !diff(),
    );

  return (
    <div class="pr-compose">
      <header class="pr-compose-head">
        <span class={`pr-compose-summary tone-${headerLine().tone}`}>{headerLine().text}</span>
        <div class="pr-compose-actions">
          <Button variant="secondary" onClick={() => openDiff()}>
            View diff
          </Button>
          <Button variant="secondary" onClick={reload}>
            Reload
          </Button>
          <Tooltip label="Show prompt" contents>
            <Button
              variant="secondary"
              class={promptOpen() ? "active" : ""}
              aria-expanded={promptOpen()}
              aria-label="Show prompt"
              onClick={() => setPromptOpen((open) => !open)}
            >
              ⚙
            </Button>
          </Tooltip>
          <Button variant="secondary" onClick={() => close({ kind: "pr_compose" })}>
            Close
          </Button>
        </div>
      </header>

      <div class="pr-compose-body">
        <section class="pr-compose-tasks">
          <RadioGroup
            label="Task"
            itemClass="pr-task-row"
            value={taskId()}
            onChange={setTaskId}
            options={builtinTasks().map((item) => ({
              value: item.id,
              label: item.name,
              render: () => (
                <>
                  <span class="pr-task-name">{item.name}</span>
                  <span class="pr-task-badge">
                    {item.mode === "Draft" ? "writes the PR" : "changes the code"}
                  </span>
                  <span class="pr-task-detail">{item.detail}</span>
                </>
              ),
            }))}
          />
        </section>

        <Show when={promptOpen()}>
          <section class="pr-compose-prompt">
            <p class="settings-hint">This is what the agent will be sent.</p>
            <pre class="pr-prompt-preview">
              {diff() ? composePrompt(task(), diff()!, extra()) : task().body}
            </pre>
            <TextArea
              class="settings-hint"
              label="Also"
              rows={3}
              value={extra()}
              onChange={setExtra}
              placeholder="Anything to append to the prompt…"
            />
          </section>
        </Show>

        <section class="pr-compose-launch">
          <Button variant="primary" disabled={!ready()} onClick={launch}>
            {awaiting() ? "Launching…" : taskAction(task().mode)}
          </Button>
          <p class="settings-hint pr-compose-hint">{hint()}</p>
          <Show when={launchedSession()}>
            <Button variant="secondary" onClick={showLaunched}>
              Show agent session
            </Button>
          </Show>
        </section>

        <Show when={sessionStatus()}>
          {(status) => (
            <button type="button" class="forge-row pr-compose-progress" onClick={showLaunched}>
              <span>{status()}</span>
              <span class="settings-row-note">open the session</span>
            </button>
          )}
        </Show>

        <Show when={pullRequest()}>
          {(pr) => (
            <div class="pr-compose-result">
              <div class="pr-compose-result-head">
                <span>#{pr().number}</span>
                <span class="pr-compose-result-title">{pr().title}</span>
              </div>
              <span class="settings-hint">{pr().url}</span>
            </div>
          )}
        </Show>
      </div>
    </div>
  );
}

function promptCapableProvider(): string | null {
  const installed = (status: unknown): boolean =>
    typeof status === "object" && status !== null && "Installed" in status;
  const qualifies = (provider: (typeof forgeStore.providers)[number]) =>
    installed(provider.detection?.status) &&
    (provider.descriptor?.capabilities?.supports_initial_prompt === true ||
      provider.descriptor?.prompt != null);
  // The preference is `provider:<id>` on the wire (§15.2); a profile or the
  // blank terminal names no provider, so neither steers this pick.
  const preferred = defaultAgentFrom(forgeStore.app_state);
  const preferredId = preferred.kind === "provider" ? preferred.id : null;
  const preferredMatch = forgeStore.providers.find(
    (p) => qualifies(p) && p.descriptor?.id === preferredId,
  );
  if (preferredMatch?.descriptor?.id) return preferredMatch.descriptor.id;
  return forgeStore.providers.find(qualifies)?.descriptor?.id ?? null;
}
