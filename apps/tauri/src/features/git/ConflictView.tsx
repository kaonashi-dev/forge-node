import { For, Index, Show, createMemo } from "solid-js";
import { activeWorkspace } from "../../state/workspace";
import { Button } from "../../ui/index";
import { openEditorAt } from "../editor/open";
import { openFile } from "../files/commands";
import { conflictLabel, conflictSides } from "./conflictPrompt";
import { conflictBlocksOf } from "./conflictBlocks";
import { PatchView } from "./diff/PatchView";
import { ConflictMark } from "./GitMarks";
import { stageConflict } from "./rebaseActions";
import { gitStore, setGitStore } from "./state";

/**
 * One conflicted path at a time, read three ways.
 *
 * The regions come from the working-tree patch, so what shows is what is on
 * disk now. Once no markers are left, whoever resolved the file — the person or
 * an agent — has only proposed a resolution: staging it here is what accepts it.
 */
export function ConflictView() {
  const conflicts = () => gitStore.rebase?.conflicts ?? [];
  const index = createMemo(() =>
    Math.max(
      0,
      conflicts().findIndex((conflict) => conflict.path === gitStore.conflictFocus),
    ),
  );
  const conflict = () => conflicts()[index()] ?? null;
  const file = createMemo(() => {
    const path = conflict()?.path;
    return gitStore.diff?.files.find((item) => item.path === path) ?? null;
  });
  const readable = () => {
    const current = file();
    return current !== null && !current.binary && !current.truncated;
  };
  const blocks = createMemo(() => {
    const current = file();
    return current && readable() ? conflictBlocksOf(current.patch) : [];
  });
  const sides = createMemo(() => {
    const state = gitStore.rebase;
    return state ? conflictSides(state) : { ours: "HEAD", theirs: "the incoming side" };
  });

  const regions: HTMLElement[] = [];
  let cursor = -1;

  function nextRegion(): void {
    if (blocks().length === 0) return;
    cursor = (cursor + 1) % blocks().length;
    regions[cursor]?.scrollIntoView({ block: "start" });
  }

  function step(offset: number): void {
    const list = conflicts();
    if (list.length === 0) return;
    const next = list[(index() + offset + list.length) % list.length];
    cursor = -1;
    setGitStore("conflictFocus", next?.path ?? null);
  }

  function openAt(line: number | null): void {
    const path = conflict()?.path;
    const workspace = activeWorkspace();
    if (!path || !workspace) return;
    openEditorAt(path, line ?? 1);
    void openFile(workspace, path).catch(() => undefined);
  }

  return (
    <Show when={conflict()}>
      {(current) => (
        <section class="git-conflict-view" aria-label="Conflict resolution">
          <header class="git-conflict-view-head">
            <span class="git-conflict-view-path">{current().path}</span>
            <Show
              when={blocks().length > 0}
              fallback={
                <span class="git-conflict-view-state" data-state="unmarked">
                  {readable() ? "no markers left" : conflictLabel(current().code)}
                </span>
              }
            >
              <span class="git-conflict-view-state" data-state="conflicted">
                <ConflictMark size={12} />
                {blocks().length} conflict{blocks().length === 1 ? "" : "s"}
              </span>
            </Show>
            <span class="history-spacer" />
            <Show when={conflicts().length > 1}>
              <Button variant="ghost" size="xs" onClick={() => step(-1)}>
                Previous file
              </Button>
              <Button variant="ghost" size="xs" onClick={() => step(1)}>
                Next file
              </Button>
            </Show>
            <Show when={blocks().length > 1}>
              <Button variant="secondary" size="sm" onClick={nextRegion}>
                Next conflict
              </Button>
            </Show>
          </header>

          <Show when={blocks().length > 0}>
            <div class="git-three-way-heads" aria-hidden="true">
              <span class="git-side" data-side="ours">
                <span class="git-side-swatch" />
                Ours <span class="git-side-ref">{sides().ours}</span>
              </span>
              <span class="git-side" data-side="base">
                <span class="git-side-swatch" />
                Base
              </span>
              <span class="git-side" data-side="theirs">
                <span class="git-side-swatch" />
                Theirs <span class="git-side-ref">{sides().theirs}</span>
              </span>
            </div>
            <Index each={blocks()}>
              {(block, position) => (
                <article
                  ref={(element) => (regions[position] = element)}
                  class="git-conflict-block"
                  aria-label={`Conflict ${position + 1} of ${blocks().length}`}
                >
                  <header class="git-conflict-block-head">
                    <span class="forge-attention-dot" aria-hidden="true" />
                    <span class="git-conflict-block-title">
                      Conflict {position + 1} of {blocks().length}
                    </span>
                    <Show when={block().line}>
                      {(line) => <span class="git-conflict-block-line">line {line()}</span>}
                    </Show>
                    <span class="history-spacer" />
                    <Button variant="ghost" size="xs" onClick={() => openAt(block().line)}>
                      Resolve in editor
                    </Button>
                  </header>
                  <div class="git-three-way">
                    <Side label={`Ours, ${sides().ours}`} side="ours" lines={block().ours} />
                    <Show
                      when={block().base}
                      fallback={
                        <div class="git-three-way-side" data-side="base">
                          <p class="git-three-way-empty">
                            No base recorded. Git wrote this conflict without diff3 markers.
                          </p>
                        </div>
                      }
                    >
                      {(base) => <Side label="Base" side="base" lines={base()} />}
                    </Show>
                    <Side
                      label={`Theirs, ${sides().theirs}`}
                      side="theirs"
                      lines={block().theirs}
                    />
                  </div>
                  <Show when={!block().complete}>
                    <p class="panel-note">
                      Part of this region is outside the patch's context. Open it in the editor to
                      read all of it.
                    </p>
                  </Show>
                </article>
              )}
            </Index>
          </Show>

          <Show when={blocks().length === 0 && readable()}>
            <div class="git-proposal">
              <header class="git-proposal-head">
                <span class="git-proposal-title">Proposed resolution</span>
                <span class="history-spacer" />
                <span class="git-proposal-note">not staged · you stage it</span>
              </header>
              <p class="git-proposal-body">
                No conflict markers are left in this file. Read the change below: staging it is what
                accepts it, whether you or an agent wrote it.
              </p>
              <div class="git-actions">
                <Button
                  variant="primary"
                  size="sm"
                  onClick={() => stageConflict(current().path, 0)}
                >
                  Stage file
                </Button>
                <Button variant="secondary" size="sm" onClick={() => openAt(null)}>
                  Open in editor
                </Button>
              </div>
              <div class="diff-body">
                <PatchView patch={file()?.patch ?? ""} onOpenLine={(line) => openAt(line)} />
              </div>
            </div>
          </Show>

          <Show when={!readable()}>
            <p class="panel-note">
              There is no patch to read for this path: it is binary, over budget, or gone from one
              side. Resolve it in the editor or a terminal, then stage it.
            </p>
          </Show>

          <footer class="git-conflict-view-foot">
            <span>
              Conflict file <strong>{index() + 1}</strong> of {conflicts().length} left in the
              replay
            </span>
            <span class="history-spacer" />
            <Show when={blocks().length > 0 || !readable()}>
              <Button
                variant="secondary"
                size="sm"
                onClick={() => stageConflict(current().path, blocks().length)}
              >
                Stage file
              </Button>
            </Show>
          </footer>
        </section>
      )}
    </Show>
  );
}

function Side(props: { label: string; side: "ours" | "base" | "theirs"; lines: string[] }) {
  return (
    <div class="git-three-way-side" data-side={props.side} aria-label={props.label} role="group">
      <For each={props.lines} fallback={<p class="git-three-way-empty">Empty on this side.</p>}>
        {(line) => <div class="git-three-way-line">{line === "" ? " " : line}</div>}
      </For>
    </div>
  );
}
