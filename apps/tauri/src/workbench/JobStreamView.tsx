import { Index, Show, createEffect, createMemo, type Accessor } from "solid-js";
import * as harness from "../harness/api";
import { Badge, Button } from "../ui";
import { harnessStore } from "../store/harnessStore";
import { jobStateTone, jobStepLabel } from "../harness/steps";
import {
  groupStreamLines,
  streamLineBody,
  streamResultFailed,
  turnHeadline,
  type StreamBlock,
  type StreamItem,
  type StreamTurn,
} from "../harness/stream";
import { jobIsRunning, type Job } from "../runtime/types";

/**
 * One headless run's output, as it arrives.
 *
 * Not a terminal: a job has no PTY, no cursor and nothing to type into. The
 * daemon already summarised each line; this view groups those lines into a
 * conversation and a command log rather than painting them through the cell grid.
 */
export function JobStreamView(props: { job: Job }) {
  const running = () => jobIsRunning(props.job.state);

  return (
    <section class="job-stream">
      <header class="job-stream-head">
        <span class="job-role">{jobStepLabel(props.job.role)}</span>
        <Badge tone={jobStateTone(props.job.state)}>{jobStateLabel(props.job)}</Badge>
        <span class="history-spacer" />
        <Show when={running()}>
          <Button
            variant="danger-ghost"
            size="xs"
            onClick={() => void harness.cancelJob(props.job.id).catch(() => undefined)}
          >
            Stop
          </Button>
        </Show>
      </header>

      <PromptBlock job={props.job} />

      <StreamBody
        job={props.job.id}
        running={running()}
        empty={running() ? "Waiting for output…" : "This run wrote nothing."}
      />

      <footer class="job-stream-foot">
        <span class="panel-note">{props.job.log_path}</span>
      </footer>
    </section>
  );
}

/**
 * A step this daemon never ran.
 *
 * Same stream, no prompt and no Stop: the job row died with the daemon that
 * held it, so what the agent was asked is gone while what it did is still on
 * disk — and whatever it was doing, it is not doing it now.
 */
export function RecordedStreamView(props: { job: string; label: string }) {
  return (
    <section class="job-stream">
      <header class="job-stream-head">
        <span class="job-role">{props.label}</span>
        <Badge tone="neutral">earlier run</Badge>
      </header>
      <StreamBody job={props.job} running={false} empty="This step left no transcript on disk." />
    </section>
  );
}

/**
 * The task the step was given, verbatim.
 *
 * Scrolls rather than truncates: a harness prompt names the files the agent is
 * expected to write, and a person checking why a step did the wrong thing is
 * usually reading exactly that part. No provider echoes its input, so without
 * this the stream begins at the agent's first sentence.
 */
function PromptBlock(props: { job: Job }) {
  const text = () => (props.job.prompt.trim() === "" ? props.job.summary : props.job.prompt);

  return (
    <details class="job-prompt">
      <summary class="job-prompt-head">
        <span class="panel-note">Prompt</span>
        <span class="panel-note">
          {props.job.provider_id} · {jobStepLabel(props.job.role)}
        </span>
        <Show when={props.job.provider_session_id}>
          {(id) => <span class="panel-note">session {id()}</span>}
        </Show>
      </summary>
      <pre class="job-prompt-body">{text()}</pre>
    </details>
  );
}

/** The blocks themselves, following the tail. */
function StreamBody(props: { job: string; empty: string; running: boolean }) {
  let scroller!: HTMLDivElement;

  const lines = createMemo(() => harnessStore.output[props.job]?.lines ?? []);
  const blocks = createMemo(() => groupStreamLines(lines()));
  const latestTurn = createMemo(() => {
    const list = blocks();
    for (let index = list.length - 1; index >= 0; index -= 1) {
      if (list[index]?.kind === "turn") return index;
    }
    return -1;
  });

  // Seed from disk once: a job opened after it started has output nobody was
  // listening for, and the stream only carries what happens next.
  createEffect(() => {
    const id = props.job;
    if (harnessStore.output[id] !== undefined) return;
    void harness.readJobLog(id).catch(() => undefined);
  });

  /**
   * Whether the view is following the tail.
   *
   * Answered from the scroll event rather than from the batch effect. Reading
   * `scrollHeight` forces the layout the pending mutations invalidated, and a
   * run at 1 000 lines/s asks that question ~125 times a second for an answer
   * that only changes when someone actually scrolls.
   */
  let pinned = true;
  let seen = -1;

  function trackPin(): void {
    pinned = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight < 64;
  }

  createEffect(() => {
    // Line count, not block count: a message can grow without adding a block.
    const count = lines().length;
    if (count === seen) return;
    seen = count;
    if (!pinned || !scroller) return;
    scroller.scrollTop = scroller.scrollHeight;
  });

  return (
    <div ref={scroller} class="job-stream-body" aria-live="polite" role="log" onScroll={trackPin}>
      {/* `Index` and not `For`: the list is keyed by position, it only ever
          appends or slides, and two blank lines are the same value — `For`
          would collide on them and rebuild rows that never moved. */}
      <Index each={blocks()} fallback={<p class="panel-note">{props.empty}</p>}>
        {(block, index) => (
          <StreamBlockView
            block={block}
            latest={() => latestTurn() === index}
            live={() => props.running}
          />
        )}
      </Index>
    </div>
  );
}

function StreamBlockView(props: {
  block: Accessor<StreamBlock>;
  latest: Accessor<boolean>;
  live: Accessor<boolean>;
}) {
  return (
    <>
      <Show when={props.block().kind === "turn"}>
        <TurnView
          turn={() => props.block() as StreamTurn}
          latest={props.latest}
          live={props.live}
        />
      </Show>
      <Show when={props.block().kind !== "turn"}>
        <StreamItemView item={() => props.block() as StreamItem} />
      </Show>
    </>
  );
}

function TurnView(props: {
  turn: Accessor<StreamTurn>;
  latest: Accessor<boolean>;
  live: Accessor<boolean>;
}) {
  return (
    <details
      class="job-turn"
      classList={{ live: props.live() && props.latest() }}
      open={props.latest()}
    >
      <summary class="job-turn-head">
        <span>{turnHeadline(props.turn().completed)}</span>
      </summary>
      <div class="job-turn-body">
        <Index each={props.turn().items}>{(item) => <StreamItemView item={item} />}</Index>
      </div>
    </details>
  );
}

function StreamItemView(props: { item: Accessor<StreamItem> }) {
  const kind = () => props.item().kind;
  const lines = () => itemLines(props.item());

  return (
    <>
      <Show when={kind() === "message"}>
        <div class="job-message">
          <Index each={lines()}>{(line) => <p>{streamLineBody(line())}</p>}</Index>
        </div>
      </Show>
      <Show when={kind() === "thinking"}>
        <details class="job-think">
          <summary class="job-think-head">Thinking</summary>
          <div class="job-think-body">
            <Index each={lines()}>{(line) => <p>{streamLineBody(line())}</p>}</Index>
          </div>
        </details>
      </Show>
      <Show when={kind() === "command"}>
        <CommandView item={() => props.item() as Extract<StreamItem, { kind: "command" }>} />
      </Show>
      <Show when={kind() === "edit"}>
        <div class="job-edit">
          <Index each={lines()}>{(line) => <p>{line()}</p>}</Index>
        </div>
      </Show>
      <Show when={kind() === "error"}>
        <div class="job-error">
          <Index each={lines()}>{(line) => <p>{line()}</p>}</Index>
        </div>
      </Show>
      <Show when={kind() === "meta"}>
        <div class="job-meta">
          <Index each={lines()}>{(line) => <p>{line()}</p>}</Index>
        </div>
      </Show>
      <Show when={kind() === "gap"}>
        <div class="job-gap" aria-hidden="true" />
      </Show>
    </>
  );
}

function CommandView(props: { item: Accessor<Extract<StreamItem, { kind: "command" }>> }) {
  const result = () => props.item().result;
  const failed = () => {
    const line = result();
    return line !== undefined && streamResultFailed(line);
  };

  return (
    <div class="job-cmd" classList={{ "is-fail": failed() }}>
      <div class="job-cmd-line">{props.item().command}</div>
      <Show when={result()}>
        {(line) => (
          <div class="job-cmd-result" classList={{ "is-fail": failed(), "is-ok": !failed() }}>
            {line()}
          </div>
        )}
      </Show>
    </div>
  );
}

function itemLines(item: StreamItem): string[] {
  switch (item.kind) {
    case "message":
    case "thinking":
    case "edit":
    case "error":
    case "meta":
      return item.lines;
    default:
      return [];
  }
}

export function jobStateLabel(job: Job): string {
  if (job.state === "Succeeded") return "succeeded";
  if (job.state === "Cancelled") return "cancelled";
  if (job.state === "Failed") {
    return job.exit_code === null ? "failed" : `failed (${job.exit_code})`;
  }
  if (job.state === "Queued") return "queued";
  if (job.state === "Running") return "running";
  return String(job.state);
}
