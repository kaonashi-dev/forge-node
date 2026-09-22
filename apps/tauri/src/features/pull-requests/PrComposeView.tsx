import { Show, createMemo, createSignal, onMount } from "solid-js";
import { newAgent, selectSession } from "../sessions/commands";
import { sessionStateLabel, sessionTitle, type PullRequest } from "../../contracts/runtime";
import { defaultAgentFrom } from "../settings/defaultAgent";
import { agentVisible } from "../../state/agentVisibility";
import { forgeStore } from "../../state/forgeStore";
import { beginCompose, clearCompose, composeRun } from "./prComposeStore";
import { setNotice } from "../../state/connection";
import { close } from "../editor/tabs";
import { focus, openDiff, openPrCompose, showTerminal } from "../../navigation/viewsStore";
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
import type { WorkspaceDiff } from "../../contracts/workbench";
import { TERMINAL_VIEW } from "../../navigation/views";
import { Button, RadioGroup, TextArea, Tooltip } from "../../ui/index";
import { loadDiff } from "../git/commands";
import { gitStore } from "../git/state";
import { loading, setLoading } from "../../state/loading";
import { activeWorkspace } from "../../state/workspace";

export function openComposeForWorkspace(workspace: string): void {
  openPrCompose();
  setLoading("diff", true);
  void loadDiff(workspace).catch(() => undefined);
}

export function PrComposeView() {
  const [taskId, setTaskId] = createSignal(builtinTasks()[0].id);
  const [promptOpen, setPromptOpen] = createSignal(false);
  const [extra, setExtra] = createSignal("");

  const workspace = () => activeWorkspace();
  const diff = (): WorkspaceDiff | null => gitStore.diff;
  const diffLoading = () => loading.diff;
  const error = () => gitStore.diffError;
  const task = (): PrTask => taskOrDefault(taskId());

  // One launch at a time, and only for the checkout this compose is about —
  // a leftover run from another workspace would block the button for no reason.
  const run = () => {
    const current = composeRun();
    const ws = workspace();
    return current && ws && current.workspace === ws ? current : null;
  };
  const awaiting = () => run() !== null && run()?.session == null;
  const launchedSession = () => run()?.session ?? null;

  onMount(() => {
    const ws = workspace();
    if (ws && !diff() && !diffLoading()) {
      setLoading("diff", true);
      void loadDiff(ws).catch(() => undefined);
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
      setNotice("No installed agent accepts a prompt at launch. Install Claude, Codex, or Cursor.");
      return;
    }
    const ws = workspace();
    if (!ws) return;
    const prompt = composePrompt(task(), d, extra());
    beginCompose(ws);
    void newAgent(provider, null, ws, null, prompt).catch(() => {
      clearCompose();
      setNotice("The daemon refused to launch the agent.");
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
    if (diffLoading() && !d) return { text: "reading the working tree…", tone: "muted" };
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
      diffLoading() && !diff(),
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
              class="pr-compose-field"
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
    agentVisible(forgeStore.app_state, provider.descriptor?.id ?? "") &&
    installed(provider.detection?.status) &&
    (provider.descriptor?.capabilities?.supports_initial_prompt === true ||
      provider.descriptor?.prompt != null);
  // The preference is `provider:<id>` on the wire; a profile or the
  // blank terminal names no provider, so neither steers this pick.
  const preferred = defaultAgentFrom(forgeStore.app_state);
  const preferredId = preferred.kind === "provider" ? preferred.id : null;
  const preferredMatch = forgeStore.providers.find(
    (p) => qualifies(p) && p.descriptor?.id === preferredId,
  );
  if (preferredMatch?.descriptor?.id) return preferredMatch.descriptor.id;
  return forgeStore.providers.find(qualifies)?.descriptor?.id ?? null;
}
