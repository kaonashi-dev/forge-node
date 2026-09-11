import { Show, createMemo, createSignal } from "solid-js";
import { sendContext } from "../runtime/api";
import { sessionTabLabel } from "../runtime/attention";
import { forgeStore } from "../store/forgeStore";
import type { SendContextRequest } from "../store/runtimeStore";
import { SessionGlyph } from "../theme/icons";
import {
  Button,
  Checkbox,
  Dialog,
  Select,
  TextArea,
  TextField,
  type SelectOption,
  toast,
} from "../ui";
import { sessionsInWorkspace } from "./sessionScope";

type Target = { provider: string; profile: string | null };

const ROLE_OPTIONS: SelectOption<string>[] = [
  { value: "generic", label: "Generic" },
  { value: "planner", label: "Planner" },
  { value: "executor", label: "Executor" },
  { value: "reviewer", label: "Reviewer" },
];

/**
 * Send an auditable context envelope to another session, or spawn a child that
 * starts with it (§8.3).
 */
export function SendContextDialog(props: { request: SendContextRequest; onDismiss: () => void }) {
  const [mode, setMode] = createSignal<"existing" | "spawn">("existing");
  const [targetSession, setTargetSession] = createSignal<string | null>(null);
  const [agentKey, setAgentKey] = createSignal<string | null>(null);
  const [summary, setSummary] = createSignal("");
  const [instructions, setInstructions] = createSignal("");
  const [includeTranscript, setIncludeTranscript] = createSignal(true);
  const [role, setRole] = createSignal("generic");
  const [sending, setSending] = createSignal(false);

  const sessionOptions = createMemo<SelectOption<string>[]>(() =>
    forgeStore.sessions
      .filter((session) => session.id !== props.request.source)
      .map((session) => {
        const label = sessionTabLabel(
          session,
          sessionsInWorkspace(forgeStore.sessions, session.workspace_id),
        );
        return {
          value: session.id,
          label,
          glyph: <SessionGlyph providerId={session.agent_provider_id} size={14} />,
        };
      }),
  );

  const agentOptions = createMemo<SelectOption<string>[]>(() =>
    forgeStore.launchables
      .filter((item) => item.kind === "agent" && item.enabled && item.provider !== null)
      .map((item) => ({
        value: item.key,
        label: item.label,
        disabled: !item.supports_initial_prompt,
        glyph: <SessionGlyph providerId={item.provider} size={14} />,
      })),
  );

  const usableAgents = createMemo(() => agentOptions().filter((option) => !option.disabled));

  function resolveAgent(key: string): Target | null {
    const row = forgeStore.launchables.find((item) => item.key === key);
    if (!row?.provider) return null;
    return { provider: row.provider, profile: row.profile };
  }

  const modeOptions: SelectOption<string>[] = [
    { value: "existing", label: "Existing session" },
    { value: "spawn", label: "Spawn new child" },
  ];

  const blocked = () => {
    if (sending()) return true;
    if (!summary().trim() && !instructions().trim() && !includeTranscript()) return true;
    if (mode() === "existing") {
      return sessionOptions().length === 0 || !(targetSession() ?? sessionOptions()[0]?.value);
    }
    return usableAgents().length === 0 || !(agentKey() ?? usableAgents()[0]?.value);
  };

  function start(): void {
    setSending(true);
    const base = {
      source: props.request.source,
      summary: summary().trim() || null,
      instructions: instructions().trim() || null,
      includeTranscript: includeTranscript(),
    };
    const run =
      mode() === "existing"
        ? sendContext({
            ...base,
            target: targetSession() ?? sessionOptions()[0]?.value ?? null,
          })
        : (() => {
            const key = agentKey() ?? usableAgents()[0]?.value;
            const to = key ? resolveAgent(key) : null;
            if (!to) return Promise.reject(new Error("No agent selected"));
            return sendContext({
              ...base,
              spawnProvider: to.provider,
              profile: to.profile,
              role: role(),
              workspacePolicy: "same",
            });
          })();

    void run
      .then(() => props.onDismiss())
      .catch((error: unknown) => {
        setSending(false);
        toast({
          title: "Could not send context.",
          detail: error instanceof Error ? error.message : undefined,
          tone: "danger",
        });
      });
  }

  return (
    <Dialog
      title="Send context"
      size="sm"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" disabled={sending()} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={blocked()} onClick={start}>
            {sending() ? "Sending…" : mode() === "spawn" ? "Spawn with context" : "Send"}
          </Button>
        </>
      }
    >
      <p class="panel-note">
        From <span class="tree-label">{props.request.title}</span>. Forge stores an envelope and
        either pastes it into a live session or starts a child with it as the prompt.
      </p>

      <Select
        label="Destination"
        value={mode()}
        options={modeOptions}
        onChange={(value) => setMode(value === "spawn" ? "spawn" : "existing")}
      />

      <Show when={mode() === "existing"}>
        <Show
          when={sessionOptions().length > 0}
          fallback={<p class="panel-note">No other sessions to send to.</p>}
        >
          <Select
            label="Session"
            value={targetSession() ?? sessionOptions()[0]?.value ?? null}
            options={sessionOptions()}
            onChange={setTargetSession}
          />
        </Show>
      </Show>

      <Show when={mode() === "spawn"}>
        <Select
          label="Agent"
          value={agentKey() ?? usableAgents()[0]?.value ?? null}
          options={agentOptions()}
          onChange={setAgentKey}
          disabled={usableAgents().length === 0}
        />
        <Select label="Role" value={role()} options={ROLE_OPTIONS} onChange={setRole} />
        <Show when={usableAgents().length === 0}>
          <p class="panel-note">No installed agent can take a launch prompt.</p>
        </Show>
      </Show>

      <TextField
        label="Summary"
        value={summary()}
        onChange={setSummary}
        placeholder="One-line description of what is being handed over"
      />
      <TextArea
        label="Instructions"
        value={instructions()}
        onChange={setInstructions}
        rows={3}
        placeholder="What should the receiver do with this?"
      />
      <Checkbox
        checked={includeTranscript()}
        onChange={setIncludeTranscript}
        label="Include terminal transcript"
      />
    </Dialog>
  );
}
