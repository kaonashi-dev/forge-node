import { For, Show, createEffect, createMemo, createSignal, onCleanup } from "solid-js";
import { buildExcerpts, shouldSearchContent } from "../panels/fileContentSearch";
import { clearFindInFiles, findInFilesPending, openEditorAt } from "../store/viewsStore";
import { workbenchStore } from "../store/workbenchStore";
import { Icon, LangIcon } from "../theme/icons";
import { EmptyState, IconButton, SearchField, Skeleton } from "../ui";
import { editorPalette } from "../theme/editorTheme";
import { themeBase } from "../theme/ThemeProvider";
import { SyntaxHitText } from "./SyntaxHitText";
import { highlightExcerpt, syntaxHits } from "./searchSyntax";
import {
  askedNeedle,
  contentQuery,
  contentResults,
  runContentSearch,
  scheduleContentSearch,
  setContentQuery,
} from "./projectSearch";

export function ProjectSearchView() {
  let field: HTMLInputElement | undefined;
  const [folded, setFolded] = createSignal<ReadonlySet<string>>(new Set());
  const files = createMemo(() => buildExcerpts(contentResults()?.matches ?? []));
  const syntaxColors = createMemo(() =>
    Object.fromEntries(
      Object.entries(editorPalette(themeBase()).scopes).map(([scope, color]) => [
        `--search-${scope}`,
        color,
      ]),
    ),
  );
  const allFolded = () => files().length > 0 && files().every((file) => folded().has(file.path));

  createEffect(() => scheduleContentSearch(workbenchStore.workspace, contentQuery()));
  createEffect(() => {
    const request = findInFilesPending();
    if (request) {
      clearFindInFiles();
      if (request.query !== null) setContentQuery(request.query);
    }
    const frame = requestAnimationFrame(() => {
      field?.focus({ preventScroll: true });
      field?.select();
    });
    onCleanup(() => cancelAnimationFrame(frame));
  });

  function toggle(path: string): void {
    const next = new Set(folded());
    if (!next.delete(path)) next.add(path);
    setFolded(next);
  }

  function searchNow(): void {
    const workspace = workbenchStore.workspace;
    const needle = contentQuery().trim();
    if (workspace && shouldSearchContent(needle)) runContentSearch(workspace, needle);
  }

  const summary = () => {
    const results = contentResults();
    if (!results) return "";
    const hits = results.matches.length;
    const count = files().length;
    return `${hits} match${hits === 1 ? "" : "es"} in ${count} file${count === 1 ? "" : "s"}${
      results.truncated ? " — stopped at the hit budget" : ""
    }`;
  };

  return (
    <div class="project-search" style={syntaxColors()}>
      <header class="project-search-bar">
        <SearchField
          class="project-search-field"
          aria-label="Search in files"
          placeholder="Search in files…"
          value={contentQuery()}
          ref={(element) => (field = element)}
          onChange={setContentQuery}
          onClear={() => setContentQuery("")}
          onKeyDown={(event) => {
            if (event.key === "Enter") searchNow();
          }}
        />
        <span class="project-search-summary">{summary()}</span>
        <IconButton
          label={allFolded() ? "Unfold every file" : "Fold every file"}
          disabled={files().length === 0}
          onClick={() =>
            setFolded(allFolded() ? new Set() : new Set(files().map((file) => file.path)))
          }
        >
          <Icon
            name={allFolded() ? "chevron-right" : "chevron-down"}
            class="forge-icon-muted"
            size={13}
          />
        </IconButton>
        <IconButton
          label="Search again"
          disabled={!shouldSearchContent(contentQuery()) || workbenchStore.workspace === null}
          onClick={searchNow}
        >
          <Icon name="refresh" class="forge-icon-muted" size={13} />
        </IconButton>
      </header>

      <Show when={workbenchStore.searchError}>
        {(error) => <p class="panel-error">{error()}</p>}
      </Show>
      <Show
        when={workbenchStore.workspace !== null}
        fallback={<EmptyState message="No checkout selected." />}
      >
        <Show
          when={shouldSearchContent(contentQuery())}
          fallback={<EmptyState message="Type at least two characters to search the checkout." />}
        >
          <Show
            when={contentResults()}
            fallback={
              <Show when={workbenchStore.loading.search}>
                <Skeleton label="Searching the checkout" rows={8} />
              </Show>
            }
          >
            <Show
              when={files().length > 0}
              fallback={<EmptyState message="Nothing matches that." />}
            >
              <div class="project-search-files">
                <For each={files()}>
                  {(file) => {
                    const cut = file.path.lastIndexOf("/");
                    return (
                      <section class="project-search-file">
                        <button
                          type="button"
                          class="project-search-file-head"
                          aria-expanded={!folded().has(file.path)}
                          onClick={() => toggle(file.path)}
                        >
                          <Icon
                            name={folded().has(file.path) ? "chevron-right" : "chevron-down"}
                            class="forge-icon-faint"
                            size={12}
                          />
                          <LangIcon path={file.path} size={13} />
                          <span class="project-search-file-name">{file.path.slice(cut + 1)}</span>
                          <span class="project-search-file-dir">{file.path.slice(0, cut + 1)}</span>
                          <span class="project-search-file-hits">{file.hits}</span>
                        </button>
                        <Show when={!folded().has(file.path)}>
                          <For each={file.excerpts}>
                            {(excerpt) => {
                              const tokens = highlightExcerpt(
                                file.path,
                                excerpt.map((line) => line.text),
                              );
                              const lines = createMemo(() =>
                                excerpt.map((line, index) => ({
                                  ...line,
                                  segments: syntaxHits(
                                    tokens[index] ?? [{ text: line.text, scope: null }],
                                    askedNeedle() ?? "",
                                  ),
                                })),
                              );
                              return (
                                <div class="project-search-excerpt">
                                  <div class="project-search-lines">
                                    <For each={lines()}>
                                      {(line) => (
                                        <button
                                          type="button"
                                          class="project-search-line"
                                          classList={{ hit: line.hit }}
                                          onClick={() => openEditorAt(file.path, line.line)}
                                        >
                                          <span class="project-search-number">{line.line}</span>
                                          <span class="project-search-code">
                                            <SyntaxHitText segments={line.segments} />
                                          </span>
                                        </button>
                                      )}
                                    </For>
                                  </div>
                                </div>
                              );
                            }}
                          </For>
                        </Show>
                      </section>
                    );
                  }}
                </For>
              </div>
            </Show>
          </Show>
        </Show>
      </Show>
    </div>
  );
}
