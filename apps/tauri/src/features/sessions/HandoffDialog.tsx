import { Show, createMemo, createSignal } from "solid-js";
import { forgeStore } from "../../state/forgeStore";
import type { HandoffRequest } from "./dialogs";
import { defaultAgentFrom, resolveDefaultAgent } from "../settings/defaultAgent";
import { SessionGlyph } from "./SessionGlyph";
import {
  Button,
  Dialog,
  Disclosure,
  Select,
  Skeleton,
  TextArea,
  toast,
  type SelectOption,
} from "../../ui/index";
import { handoffPrompt, summarizerPrompt } from "./handoffPrompt";
import { beginHandoffJob, handoffJob, setHandoffProgressOpen } from "./handoffJobStore";
import { launchAgent } from "./sessionActions";
import { providerReviews } from "../../contracts/runtime";
import { readChoice, writeChoice } from "../../state/preferences";

/** `provider` and `profile` travel together; the key is what the select holds. */
type Target = { provider: string; profile: string | null };

/** Remembered summarizer launchable key — a flash/low-effort profile when set. */
export const HANDOFF_SUMMARIZER_KEY = "ui.handoff.summarizer";

export function HandoffDialog(props: { request: HandoffRequest; onDismiss: () => void }) {
  const [target, setTarget] = createSignal<string | null>(null);
  const [summarizer, setSummarizer] = createSignal<string | null>(null);
  const [focus, setFocus] = createSignal("");
  const [rawCopy, setRawCopy] = createSignal(false);
  const [starting, setStarting] = createSignal(false);

  // Providers without prompt support must be disabled, not launched with empty context.
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
  const initialTarget = createMemo(() => {
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

  const initialSummarizer = createMemo(() => {
    const keys = usable().map((option) => option.value);
    if (keys.length === 0) return null;
    return readChoice(HANDOFF_SUMMARIZER_KEY, keys, keys[0]!);
  });

  const chosenTarget = () => target() ?? initialTarget();
  const chosenSummarizer = () => summarizer() ?? initialSummarizer();

  function resolveTarget(key: string): Target | null {
    const row = forgeStore.launchables.find((item) => item.key === key);
    if (!row?.provider) return null;
    return { provider: row.provider, profile: row.profile };
  }

  const source = createMemo(() => {
    const transcript = props.request.transcript;
    if (!transcript) return null;
    return {
      transcript: transcript.text,
      truncated: transcript.truncated,
      sourceAgent: props.request.sourceAgent,
      sourceTitle: props.request.title,
      workingDirectory: props.request.workingDirectory,
      branch: props.request.branch,
    };
  });

  const hasCapture = () => source() !== null && source()!.transcript.trim() !== "";

  const blocked = () =>
    starting() ||
    handoffJob() !== null ||
    !hasCapture() ||
    chosenTarget() === null ||
    (!rawCopy() && chosenSummarizer() === null) ||
    usable().length === 0;

  function startRaw(): void {
    if (blocked()) return;
    const key = chosenTarget();
    const capture = source();
    if (!key || !capture) return;
    const text = handoffPrompt(capture);
    if (!text) return;
    const to = resolveTarget(key);
    if (!to) return;
    setStarting(true);
    void launchAgent(
      to.provider,
      to.profile,
      props.request.workspace,
      null,
      text,
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

  function startSummarize(): void {
    if (blocked()) return;
    const targetKey = chosenTarget();
    const summarizerKey = chosenSummarizer();
    const capture = source();
    if (!targetKey || !summarizerKey || !capture) return;
    const prompt = summarizerPrompt(capture, focus().trim() || null);
    if (!prompt) return;
    const dest = resolveTarget(targetKey);
    const sum = resolveTarget(summarizerKey);
    if (!dest || !sum) return;

    writeChoice(HANDOFF_SUMMARIZER_KEY, summarizerKey);
    const parent = props.request.kind === "external" ? null : props.request.session;
    const provider = forgeStore.providers.find((row) => row.descriptor?.id === sum.provider);
    const readOnly = provider ? providerReviews(provider) : false;

    const started = beginHandoffJob(
      {
        returnSession: props.request.kind === "session" ? props.request.session : null,
        workspace: props.request.workspace,
        workingDirectory: props.request.workingDirectory,
        branch: props.request.branch,
        sourceAgent: props.request.sourceAgent,
        sourceTitle: props.request.title,
        summarizerProvider: sum.provider,
        summarizerProfile: sum.profile,
        targetProvider: dest.provider,
        targetProfile: dest.profile,
        parent,
        focus: focus().trim() || null,
      },
      prompt,
      readOnly,
    );
    if (started) props.onDismiss();
  }

  function start(): void {
    if (rawCopy()) startRaw();
    else startSummarize();
  }

  return (
    <Dialog
      title="Continue in a new session"
      size="md"
      align="center"
      onDismiss={props.onDismiss}
      footer={
        <>
          <Button variant="secondary" disabled={starting()} onClick={props.onDismiss}>
            Cancel
          </Button>
          <Button variant="primary" disabled={blocked()} onClick={start}>
            {starting()
              ? rawCopy()
                ? "Starting…"
                : "Starting summarizer…"
              : rawCopy()
                ? "Start new session"
                : "Start summarizing"}
          </Button>
        </>
      }
    >
      <p class="panel-note">
        A fresh agent starts from this session's stopping point, in the same checkout. By default a
        separate agent compresses the capture into an editable brief first. Review and confirm it
        before starting the destination agent.
      </p>

      <Show when={handoffJob()}>
        <Button
          onClick={() => {
            props.onDismiss();
            setHandoffProgressOpen(true);
          }}
        >
          Open the handoff already in progress
        </Button>
      </Show>

      <div class="handoff-source">
        <SessionGlyph providerId={props.request.sourceAgent} size={14} />
        <span class="tree-label">{props.request.title}</span>
      </div>

      <Select
        label="Agent"
        value={chosenTarget()}
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

      <TextArea
        label="Focus"
        value={focus()}
        onChange={setFocus}
        placeholder="Continue with the implementation focusing on…"
        rows={3}
        disabled={rawCopy()}
      />
      <p class="panel-note">
        Optional. Orients the summarizer toward a fork of the conversation; leave blank to continue
        the same thread.
      </p>

      <Disclosure summary={<span class="tree-label">Summarizer and options</span>}>
        <Select
          label="Summarizer"
          value={chosenSummarizer()}
          options={options()}
          onChange={setSummarizer}
          placeholder="Select a summarizer"
          disabled={usable().length === 0 || rawCopy()}
        />
        <p class="panel-note">
          Prefer a lower-effort or flash profile here — it only writes the continuation brief.
        </p>
        <label class="handoff-raw">
          <input
            type="checkbox"
            checked={rawCopy()}
            onChange={(event) => setRawCopy(event.currentTarget.checked)}
          />
          <span>Paste the raw transcript instead (no summary)</span>
        </label>
      </Disclosure>

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
            Capture is {transcript().lines} line
            {transcript().lines === 1 ? "" : "s"} ({Math.ceil(transcript().text.length / 1024)} KB)
            {transcript().truncated ? "; older output omitted" : ""}
            {rawCopy() ? " — will be pasted whole." : " — will be summarized first."}
          </p>
        )}
      </Show>
      <Show when={props.request.transcript && !hasCapture()}>
        <p class="panel-note">This session has produced nothing to carry over yet.</p>
      </Show>
    </Dialog>
  );
}
