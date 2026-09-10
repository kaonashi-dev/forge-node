import { Show, createMemo, createSignal } from "solid-js";
import { forgeStore } from "../store/forgeStore";
import type { HandoffRequest } from "../store/runtimeStore";
import { defaultAgentFrom, resolveDefaultAgent } from "../settings/defaultAgent";
import { SessionGlyph } from "../theme/icons";
import { Button, Dialog, Select, Skeleton, toast, type SelectOption } from "../ui";
import { handoffPrompt } from "./handoffPrompt";
import { launchAgent } from "./sessionActions";

/** `provider` and `profile` travel together; the key is what the select holds. */
type Target = { provider: string; profile: string | null };

/**
 * Carry one session's context into a fresh agent (§16.8).
 *
 * The capture is a round trip, so the dialog opens first and fills in: asking
 * someone to wait on a spinner with no dialog around it reads as a click that
 * did nothing.
 *
 * The original session is left alone. It is named in the prompt as read-only
 * context, and the new agent is told the files on disk win wherever they
 * disagree with it.
 */
export function HandoffDialog(props: { request: HandoffRequest; onDismiss: () => void }) {
  const [target, setTarget] = createSignal<string | null>(null);
  const [starting, setStarting] = createSignal(false);

  /**
   * The agents that can actually take this.
   *
   * A provider with no prompt style refuses the launch outright rather than
   * starting empty, so it is listed and disabled with the reason instead of
   * failing on confirm.
   */
  const options = createMemo<SelectOption<string>[]>(() =>
    forgeStore.launchables
      .filter((item) => item.kind === "agent" && item.enabled && item.provider !== null)
      .map((item) => ({
        value: item.key,
        label: item.label,
        disabled: !item.supports_initial_prompt,
        glyph: <SessionGlyph providerId={item.provider} size={14} />,
      })),
  );

  const usable = createMemo(() => options().filter((option) => !option.disabled));

  /** The source session's own agent when it qualifies, else the default. */
  const initial = createMemo(() => {
    const preferred = forgeStore.launchables.find(
      (item) =>
        item.kind === "agent" &&
        item.enabled &&
        item.supports_initial_prompt &&
        item.provider === props.request.sourceAgent,
    );
    if (preferred) return preferred.key;

    const fallback = resolveDefaultAgent(
      defaultAgentFrom(forgeStore.app_state),
      forgeStore.launchables,
    );
    if (fallback.kind === "agent") {
      const row = forgeStore.launchables.find(
        (item) =>
          item.provider === fallback.provider &&
          item.profile === fallback.profile &&
          item.supports_initial_prompt,
      );
      if (row) return row.key;
    }
    return usable()[0]?.value ?? null;
  });

  const chosen = () => target() ?? initial();

  function resolveTarget(key: string): Target | null {
    const row = forgeStore.launchables.find((item) => item.key === key);
    if (!row?.provider) return null;
    return { provider: row.provider, profile: row.profile };
  }

  const prompt = createMemo(() => {
    const transcript = props.request.transcript;
    if (!transcript) return null;
    return handoffPrompt({
      transcript: transcript.text,
      truncated: transcript.truncated,
      sourceAgent: props.request.sourceAgent,
      sourceTitle: props.request.title,
      workingDirectory: props.request.workingDirectory,
      branch: props.request.branch,
    });
  });

  const blocked = () =>
    starting() || prompt() === null || chosen() === null || usable().length === 0;

  function start(): void {
    const key = chosen();
    const text = prompt();
    if (!key || !text) return;
    const to = resolveTarget(key);
    if (!to) return;
    setStarting(true);
    void launchAgent(
      to.provider,
      to.profile,
      props.request.workspace,
      null,
      text,
      // The graph edge: the new run nests under the one it continues. A
      // discovered run is not a node of that graph (`domain::external`), so
      // there is no parent to record.
      props.request.kind === "external" ? null : props.request.session,
    )
      .then(() => props.onDismiss())
      .catch((error: unknown) => {
        setStarting(false);
        toast({
          title: "Could not start the new session.",
          detail: error instanceof Error ? error.message : undefined,
          tone: "danger",
        });
      });
  }

  return (
    <Dialog
      title="Continue in a new session"
      size="sm"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" disabled={starting()} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={blocked()} onClick={start}>
            {starting() ? "Starting…" : "Start new session"}
          </Button>
        </>
      }
    >
      <p class="panel-note">
        A fresh agent starts from this session's stopping point, in the same checkout. The original
        session is left running and untouched.
      </p>

      <div class="handoff-source">
        <SessionGlyph providerId={props.request.sourceAgent} size={14} />
        <span class="tree-label">{props.request.title}</span>
      </div>

      <Select
        label="Agent"
        value={chosen()}
        options={options()}
        onChange={setTarget}
        placeholder="Select an agent"
        disabled={usable().length === 0}
      />
      <Show when={usable().length === 0}>
        <p class="panel-note">
          No installed agent can take a prompt at launch, so there is nothing to hand this to.
        </p>
      </Show>

      <p class="panel-note">
        Starts in <span class="session-changes-base">{props.request.workingDirectory}</span>
      </p>

      <Show when={props.request.error}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      <Show
        when={props.request.transcript}
        fallback={
          <Show when={!props.request.error}>
            <Skeleton label="Reading the session" rows={2} />
          </Show>
        }
      >
        {(transcript) => (
          <p class="panel-note">
            Carrying {transcript().lines} line
            {transcript().lines === 1 ? "" : "s"} ({Math.ceil(transcript().text.length / 1024)} KB)
            {transcript().truncated ? "; older output omitted" : ""}.
          </p>
        )}
      </Show>
      <Show when={props.request.transcript && prompt() === null}>
        <p class="panel-note">This session has produced nothing to carry over yet.</p>
      </Show>
    </Dialog>
  );
}
