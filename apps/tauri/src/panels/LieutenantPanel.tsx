import { For, Show, createEffect, createSignal } from "solid-js";
import { askLieutenant } from "../runtime/api";
import { forgeStore } from "../store/forgeStore";
import {
  clearLieutenant,
  focusLieutenantProject,
  lieutenantStore,
  setLieutenantError,
  startLieutenantTurn,
} from "../store/lieutenantStore";
import { workbenchStore } from "../store/workbenchStore";
import { Button, TextArea } from "../ui";

function currentProject(): string | null {
  const workspace = forgeStore.workspaces.find((item) => item.id === workbenchStore.workspace);
  return workspace?.project_id ?? null;
}

export function LieutenantPanel() {
  const [question, setQuestion] = createSignal("");
  let transcript!: HTMLDivElement;

  createEffect(() => {
    focusLieutenantProject(currentProject());
  });

  // Keep a live answer in view unless the user deliberately scrolled back.
  createEffect(() => {
    lieutenantStore.turns.map((turn) => `${turn.answer.length}:${turn.running}`).join(",");
    if (!transcript) return;
    const distanceFromBottom =
      transcript.scrollHeight - transcript.scrollTop - transcript.clientHeight;
    if (distanceFromBottom < 48) transcript.scrollTop = transcript.scrollHeight;
  });

  function ask(): void {
    if (lieutenantStore.jobId) return;
    const project = currentProject();
    const text = question().trim();
    if (!project) {
      setLieutenantError("Open a project first.");
      return;
    }
    if (!text) return;

    setQuestion("");
    startLieutenantTurn(text);
    void askLieutenant(project, text, lieutenantStore.providerSession).catch((error: unknown) => {
      setLieutenantError(
        error instanceof Error ? error.message : "Could not ask Lieutenant.",
        project,
      );
    });
  }

  function onKeyDown(event: KeyboardEvent): void {
    if (event.key === "Enter" && !event.shiftKey) {
      event.preventDefault();
      ask();
    }
  }

  return (
    <div class="panel-body lieutenant-panel">
      <div class="panel-heading">
        <span>
          Lieutenant
          <Show when={lieutenantStore.jobId}>
            <span class="panel-note-inline"> · thinking...</span>
          </Show>
          <Show when={!lieutenantStore.jobId && lieutenantStore.providerSession}>
            <span class="panel-note-inline"> · continuing</span>
          </Show>
        </span>
        <Button
          variant="secondary"
          size="xs"
          class="lieutenant-clear"
          onClick={clearLieutenant}
          disabled={lieutenantStore.turns.length === 0}
        >
          Clear
        </Button>
      </div>

      <div ref={transcript} class="lieutenant-transcript" aria-live="polite">
        <For
          each={lieutenantStore.turns}
          fallback={
            <p class="empty-copy">
              Ask about the harness: which feature is stuck, what a review said, or what to do next.
            </p>
          }
        >
          {(turn) => (
            <article class="lieutenant-turn">
              <div class="lieutenant-question">{turn.question}</div>
              <Show
                when={turn.answer.length > 0}
                fallback={
                  <p class="panel-note">
                    {turn.running ? "..." : "(the answer left nothing on the stream)"}
                  </p>
                }
              >
                <For each={turn.answer}>
                  {(line) => <div class="lieutenant-answer">{line}</div>}
                </For>
              </Show>
            </article>
          )}
        </For>
        <Show when={lieutenantStore.error}>{(error) => <p class="panel-error">{error()}</p>}</Show>
      </div>

      <div class="lieutenant-composer">
        <TextArea
          class="lieutenant-input"
          rows={3}
          value={question()}
          onChange={setQuestion}
          onKeyDown={onKeyDown}
          placeholder="Ask about the harness... (Enter sends)"
          aria-label="Ask Lieutenant"
          disabled={lieutenantStore.jobId !== null}
        />
        <Button
          variant="secondary"
          onClick={ask}
          disabled={lieutenantStore.jobId !== null || question().trim().length === 0}
        >
          {lieutenantStore.jobId ? "Asking..." : "Ask"}
        </Button>
      </div>
    </div>
  );
}
