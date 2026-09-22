import { Show, createEffect, onCleanup, untrack } from "solid-js";
import {
  cancelHandoffJob,
  completeHandoff,
  finishHandoffWithSummary,
  handoffJob,
  MAX_HANDOFF_SUMMARY_LENGTH,
  openHandoffSession,
  refreshHandoffProgress,
  retryHandoffProgress,
  setHandoffProgressOpen,
  setHandoffSummary,
} from "./handoffJobStore";
import { Button, Dialog, Disclosure, Skeleton, TextArea } from "../../ui/index";
import { connectionStore } from "../../state/connection";

const POLL_MS = 1500;

export function HandoffProgressDialog() {
  const job = handoffJob;

  createEffect(() => {
    if (
      connectionStore.connection.kind !== "connected" ||
      !job()?.summarizerSession ||
      job()?.finishing ||
      job()?.cancelled ||
      job()?.targetSession
    )
      return;
    untrack(refreshHandoffProgress);
    const timer = window.setInterval(refreshHandoffProgress, POLL_MS);
    onCleanup(() => window.clearInterval(timer));
  });

  const blocked = () =>
    !job()?.summarizerSession ||
    !!job()?.launch ||
    job()?.finishing ||
    job()?.cancelled ||
    job()?.uncertain ||
    !!job()?.targetSession;

  async function openSummarizer(): Promise<void> {
    const session = job()?.summarizerSession;
    if (!session) return;
    if (await openHandoffSession(session)) setHandoffProgressOpen(false);
  }

  return (
    <Show when={job()?.progressOpen}>
      <Dialog
        title="Review session handoff"
        size="lg"
        align="center"
        onDismiss={() => setHandoffProgressOpen(false)}
        footer={
          <>
            <Button variant="secondary" disabled={job()?.finishing} onClick={cancelHandoffJob}>
              {job()?.cancelled && job()?.launch ? "Cancelling…" : "Cancel handoff"}
            </Button>
            <Button
              variant="secondary"
              disabled={!job()?.summarizerSession || job()?.finishing}
              onClick={() => void openSummarizer()}
            >
              Open summarizer
            </Button>
            <Button
              variant="primary"
              disabled={
                blocked() ||
                !job()?.summary.trim() ||
                (job()?.summary.length ?? 0) > MAX_HANDOFF_SUMMARY_LENGTH
              }
              loading={job()?.finishing}
              onClick={() => void finishHandoffWithSummary(job()?.summary ?? "")}
            >
              Continue with summary
            </Button>
          </>
        }
      >
        <p class="panel-note">
          Review the brief before continuing. Terminal output can include prompts and unfinished
          drafts, so a detected block never starts the destination automatically. You can edit the
          brief or paste one here if the summarizer omitted its markers.
        </p>
        <Show when={job()?.focus}>
          {(focus) => (
            <p class="panel-note">
              Focus: <span class="session-changes-base">{focus()}</span>
            </p>
          )}
        </Show>
        <TextArea
          label="Continuation brief"
          value={job()?.summary ?? ""}
          onChange={setHandoffSummary}
          rows={10}
          maxLength={MAX_HANDOFF_SUMMARY_LENGTH}
          disabled={blocked()}
          placeholder="The summarizer's marked brief will appear here. You can also paste or write it yourself."
        />
        <Disclosure summary="Summarizer terminal capture">
          <Show
            when={job()?.progressText}
            fallback={<Skeleton label="Waiting for the summarizer" rows={4} />}
          >
            {(text) => <pre class="handoff-progress-body">{text()}</pre>}
          </Show>
          <Show when={job()?.progressTruncated}>
            <p class="panel-note">Older output was omitted from this capture.</p>
          </Show>
        </Disclosure>
        <Show when={job()?.error}>{(error) => <p class="panel-error">{error()}</p>}</Show>
        <Show when={job()?.targetSession && !job()?.finishing}>
          <Button onClick={() => void completeHandoff()}>
            Open destination and finish handoff
          </Button>
        </Show>
        <Show when={job()?.readError}>
          {(error) => (
            <>
              <p class="panel-error">{error()}</p>
              <Button
                onClick={retryHandoffProgress}
                disabled={job()?.finishing || job()?.cancelled}
              >
                Retry capture
              </Button>
            </>
          )}
        </Show>
      </Dialog>
    </Show>
  );
}
