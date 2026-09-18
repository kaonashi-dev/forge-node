import {
  For,
  Match,
  Show,
  Switch,
  createContext,
  createEffect,
  createMemo,
  createSignal,
  onCleanup,
  onMount,
  untrack,
  useContext,
  type JSX,
} from "solid-js";
import { Dynamic } from "solid-js/web";
import { Icon } from "../../theme/icons/index";
import { themeBase } from "../../theme/ThemeProvider";
import { isDiagram, renderDiagram } from "./diagram";
import {
  parseMarkdown,
  safeHref,
  toggleTaskMarker,
  type MdBlock,
  type MdImage,
  type MdListItem,
  type MdSpan,
} from "./markdownBlocks";

type MdText = Extract<MdSpan, { kind: "text" }>;

type Draft = { start: number; end: number; text: string };

type EditApi = {
  draft: () => Draft | null;
  begin: (start: number, end: number, event: MouseEvent) => void;
  setText: (text: string) => void;
  commit: () => void;
  cancel: () => void;
  toggle: (start: number, end: number) => void;
};

const Edit = createContext<EditApi | null>(null);
const Nested = createContext(false);

/**
 * Where an image the document names is drawn from: a URL to put in `src`, or
 * `null` when it must not be shown.
 */
export type MdImageLoader = (src: string) => Promise<string> | null;

/**
 * Absent, images are offered as links: a pull request body is remote text,
 * and drawing it would fetch whatever address it names. An accessor, because
 * a Provider reads its value once and a new loader must reach every image.
 */
const ImageLoader = createContext<() => MdImageLoader | undefined>(() => undefined);

/** Caller-supplied because shared rendering may not reach the host on its own. */
const OpenUrl = createContext<(href: string) => void>(() => undefined);

/** Path activation is caller policy: a pull request body and a preview differ. */
const PathRenderer = createContext<(text: string) => JSX.Element>((text) => text);

/**
 * A heading in the body is a heading *under* the view's own title, so the
 * outline stays true; how big it looks is `data-level`'s job, not the tag's.
 */
const HEADING_TAG = ["h3", "h4", "h5", "h6", "h6", "h6"] as const;

/**
 * Rendered Markdown, from text the app did not write.
 *
 * The blocks arrive as data and are drawn with JSX, so a document carrying
 * markup shows the markup. The one `innerHTML` is a diagram's SVG, which
 * `diagram.ts` has mermaid sanitise. A link opens through the host rather than
 * as a navigation — this is a webview, and letting it follow an `href` would
 * replace the app with the page.
 */
export function Markdown(props: {
  text: string;
  class?: string;
  images?: MdImageLoader;
  /** A link opens through the host; shared rendering cannot reach it itself. */
  onOpenUrl: (href: string) => void;
  /** Inline code that names a file; the caller decides what an open does. */
  renderPath: (text: string) => JSX.Element;
  /**
   * When set, a click on a block edits its source and a click on a task box
   * flips the marker. The next string is that range's new markdown. A pull
   * request body does not pass this.
   */
  onEdit?: (start: number, end: number, next: string) => void;
  /** The in-progress block, so a capture-phase save can write it without a blur. */
  onDraft?: (draft: Draft | null) => void;
}) {
  const blocks = createMemo(() => parseMarkdown(props.text));
  const [draft, setDraft] = createSignal<Draft | null>(null);
  let committing = false;
  let justClosed = false;

  function closeDraft(): void {
    setDraft(null);
    props.onDraft?.(null);
    justClosed = true;
    queueMicrotask(() => {
      justClosed = false;
    });
  }

  function commit(): void {
    const current = draft();
    if (!current || !props.onEdit) {
      closeDraft();
      return;
    }
    if (current.text === props.text.slice(current.start, current.end)) {
      closeDraft();
      return;
    }
    committing = true;
    props.onEdit(current.start, current.end, current.text);
    closeDraft();
    queueMicrotask(() => {
      committing = false;
    });
  }

  const api: EditApi = {
    draft,
    begin: (start, end, event) => {
      if (!props.onEdit || justClosed || ignoreEditClick(event)) return;
      const current = draft();
      if (current) {
        if (current.start === start && current.end === end) return;
        commit();
        return;
      }
      const next = { start, end, text: props.text.slice(start, end) };
      setDraft(next);
      props.onDraft?.(next);
    },
    setText: (text) => {
      const current = draft();
      if (!current) return;
      const next = { ...current, text };
      setDraft(next);
      props.onDraft?.(next);
    },
    commit,
    cancel: closeDraft,
    toggle: (start, end) => {
      if (!props.onEdit || justClosed || draft()) return;
      const next = toggleTaskMarker(props.text.slice(start, end));
      if (next === null) return;
      committing = true;
      props.onEdit(start, end, next);
      queueMicrotask(() => {
        committing = false;
      });
    },
  };

  createEffect(() => {
    props.text;
    // A reload or take-disk rewrites the source; a commit sets `committing`
    // so this does not drop the block we just wrote.
    if (committing) return;
    if (untrack(draft)) untrack(closeDraft);
  });

  return (
    <ImageLoader.Provider value={() => props.images}>
      <OpenUrl.Provider value={props.onOpenUrl}>
        <PathRenderer.Provider value={props.renderPath}>
          <Edit.Provider value={props.onEdit ? api : null}>
            <div
              class={`forge-md ${props.class ?? ""}`}
              data-editable={props.onEdit ? "" : undefined}
            >
              <Blocks blocks={blocks()} nested={!props.onEdit} />
              <Show when={props.onEdit && blocks().length === 0}>
                <p
                  class="forge-md-p forge-md-empty"
                  onClick={(event) => api.begin(0, props.text.length, event)}
                >
                  Write…
                </p>
              </Show>
            </div>
          </Edit.Provider>
        </PathRenderer.Provider>
      </OpenUrl.Provider>
    </ImageLoader.Provider>
  );
}

function ignoreEditClick(event: MouseEvent): boolean {
  const target = event.target;
  if (target instanceof Element && target.closest("a, button, input, textarea")) return true;
  const sel = window.getSelection();
  if (!sel || sel.isCollapsed || !(event.currentTarget instanceof Node)) return false;
  return event.currentTarget.contains(sel.anchorNode);
}

function Blocks(props: { blocks: MdBlock[]; nested?: boolean }) {
  return (
    <Nested.Provider value={!!props.nested}>
      <For each={props.blocks}>
        {(block) => (
          <Show when={!props.nested} fallback={<BlockView block={block} />}>
            <Editable block={block}>
              <BlockView block={block} />
            </Editable>
          </Show>
        )}
      </For>
    </Nested.Provider>
  );
}

function Editable(props: { block: MdBlock; children: JSX.Element }) {
  const edit = useContext(Edit);
  return (
    <Show when={edit} fallback={props.children}>
      {(api) => (
        <Show
          when={editing(api(), props.block)}
          fallback={
            <div
              class="forge-md-hit"
              onClick={(event) => api().begin(props.block.start, props.block.end, event)}
            >
              {props.children}
            </div>
          }
        >
          <BlockEditor
            kind={props.block.kind}
            level={props.block.kind === "heading" ? props.block.level : undefined}
          />
        </Show>
      )}
    </Show>
  );
}

function editing(api: EditApi, block: MdBlock): boolean {
  const current = api.draft();
  return current !== null && current.start === block.start && current.end === block.end;
}

function BlockEditor(props: { kind: MdBlock["kind"]; level?: number }) {
  const edit = useContext(Edit);
  let area!: HTMLTextAreaElement;

  const fit = (): void => {
    area.style.height = "0px";
    area.style.height = `${area.scrollHeight}px`;
  };

  onMount(() => {
    area.value = edit?.draft()?.text ?? "";
    fit();
    area.focus();
    const n = area.value.length;
    area.setSelectionRange(n, n);
  });

  return (
    <textarea
      ref={area}
      class="forge-md-edit"
      data-kind={props.kind}
      data-level={props.level}
      spellcheck={props.kind !== "code" && props.kind !== "table"}
      rows={1}
      aria-label="Edit markdown"
      onInput={(event) => {
        edit?.setText(event.currentTarget.value);
        fit();
      }}
      onBlur={() => edit?.commit()}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          edit?.cancel();
        } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
          event.preventDefault();
          edit?.commit();
        }
      }}
    />
  );
}

function asKind<K extends MdBlock["kind"]>(
  block: MdBlock,
  kind: K,
): Extract<MdBlock, { kind: K }> | undefined {
  return block.kind === kind ? (block as Extract<MdBlock, { kind: K }>) : undefined;
}

function BlockView(props: { block: MdBlock }) {
  return (
    <Switch>
      <Match when={asKind(props.block, "heading")}>
        {(heading) => (
          <Dynamic
            component={HEADING_TAG[heading().level - 1] ?? "h6"}
            class="forge-md-heading"
            data-level={heading().level}
          >
            <Spans spans={heading().spans} />
          </Dynamic>
        )}
      </Match>
      <Match when={asKind(props.block, "paragraph")}>
        {(paragraph) => (
          <p class="forge-md-p">
            <Spans spans={paragraph().spans} />
          </p>
        )}
      </Match>
      <Match when={asKind(props.block, "list")}>
        {(list) => (
          <Dynamic component={list().ordered ? "ol" : "ul"} class="forge-md-list">
            {/* Depth is an attribute rather than a nested list: the parser
                reads indent, and one flat pass cannot know where a level
                ends. It indents the same and needs no closing. */}
            <For each={list().items}>
              {(item) => (
                <li
                  class="forge-md-item"
                  data-depth={item.depth}
                  data-task={item.checked === null ? undefined : ""}
                >
                  <Show when={item.checked !== null}>
                    <TaskMark item={item} />
                  </Show>
                  <span class="forge-md-item-text">
                    <Spans spans={item.spans} />
                  </span>
                </li>
              )}
            </For>
          </Dynamic>
        )}
      </Match>
      <Match when={asKind(props.block, "code")}>
        {(code) => (
          <Show
            when={isDiagram(code().lang)}
            fallback={<CodeBlock lang={code().lang} text={code().text} />}
          >
            <Diagram text={code().text} />
          </Show>
        )}
      </Match>
      <Match when={asKind(props.block, "quote")}>
        {(quote) => (
          <blockquote class="forge-md-quote">
            <Blocks blocks={quote().blocks} nested />
          </blockquote>
        )}
      </Match>
      <Match when={asKind(props.block, "table")}>
        {(table) => (
          <div class="forge-md-table-scroll">
            <table class="forge-md-table">
              <thead>
                <tr>
                  <For each={table().head}>
                    {(cell) => (
                      <th>
                        <Spans spans={cell} />
                      </th>
                    )}
                  </For>
                </tr>
              </thead>
              <tbody>
                <For each={table().rows}>
                  {(row) => (
                    <tr>
                      <For each={row}>
                        {(cell) => (
                          <td>
                            <Spans spans={cell} />
                          </td>
                        )}
                      </For>
                    </tr>
                  )}
                </For>
              </tbody>
            </table>
          </div>
        )}
      </Match>
      <Match when={asKind(props.block, "rule")}>
        <hr class="forge-md-rule" />
      </Match>
    </Switch>
  );
}

function TaskMark(props: { item: MdListItem }) {
  const edit = useContext(Edit);
  const nested = useContext(Nested);
  const checked = () => props.item.checked === true;
  const mark = () => (
    <Show when={checked()}>
      <Icon name="check" size={10} />
    </Show>
  );
  return (
    <Show
      when={edit && !nested}
      fallback={
        <span
          class="forge-md-box"
          data-checked={checked() ? "" : undefined}
          role="img"
          aria-label={checked() ? "Done" : "Not done"}
        >
          {mark()}
        </span>
      }
    >
      <button
        type="button"
        class="forge-md-box"
        data-checked={checked() ? "" : undefined}
        aria-pressed={checked()}
        aria-label={checked() ? "Mark as not done" : "Mark as done"}
        onClick={(event) => {
          event.stopPropagation();
          edit?.toggle(props.item.start, props.item.end);
        }}
      >
        {mark()}
      </button>
    </Show>
  );
}

function CodeBlock(props: { lang: string | null; text: string }) {
  return (
    <pre class="forge-md-pre" data-lang={props.lang ?? undefined}>
      <code>{props.text}</code>
    </pre>
  );
}

/**
 * A mermaid block, drawn. Until the engine answers the source is what is
 * shown, and it stays shown — with mermaid's complaint under it — when the
 * diagram does not parse: a half-written chart is still worth reading.
 */
function Diagram(props: { text: string }) {
  let host!: HTMLDivElement;
  const [drawn, setDrawn] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  createEffect(() => {
    const source = props.text;
    const base = themeBase();
    let live = true;
    onCleanup(() => {
      live = false;
    });
    setError(null);
    renderDiagram(source, base).then(
      (svg) => {
        if (!live) return;
        host.innerHTML = svg;
        setDrawn(true);
      },
      (reason: unknown) => {
        if (!live) return;
        setDrawn(false);
        setError(reason instanceof Error ? reason.message : String(reason));
      },
    );
  });

  return (
    <figure class="forge-md-diagram">
      <div ref={host} class="forge-md-diagram-svg" classList={{ hidden: !drawn() }} />
      <Show when={!drawn()}>
        <CodeBlock lang="mermaid" text={props.text} />
      </Show>
      <Show when={error()}>
        {(message) => <figcaption class="forge-md-diagram-error">{message()}</figcaption>}
      </Show>
    </figure>
  );
}

function Spans(props: { spans: MdSpan[] }) {
  return (
    <For each={props.spans}>
      {(span) => (
        <Switch>
          <Match when={span.kind === "code" && span}>
            {(code) => <code class="forge-md-inline-code">{code().text}</code>}
          </Match>
          <Match when={span.kind === "link" && span}>
            {(link) => <ExternalLink href={link().href}>{link().text}</ExternalLink>}
          </Match>
          <Match when={span.kind === "image" && span}>{(image) => <Image span={image()} />}</Match>
          <Match when={span.kind === "text" && span}>{(text) => <Emphasis span={text()} />}</Match>
        </Switch>
      )}
    </For>
  );
}

function ExternalLink(props: { href: string; children: JSX.Element }) {
  const open = useContext(OpenUrl);
  return (
    <a
      class="forge-md-link"
      href={props.href}
      rel="noreferrer"
      onClick={(event) => {
        event.preventDefault();
        open(props.href);
      }}
    >
      {props.children}
    </a>
  );
}

type ImageState =
  | { kind: "loading" }
  | { kind: "ready"; url: string }
  | { kind: "failed"; reason: string };

function Image(props: { span: MdImage }) {
  const loader = useContext(ImageLoader);
  const [state, setState] = createSignal<ImageState>({ kind: "loading" });

  createEffect(() => {
    const load = loader();
    if (!load) return;
    const read = load(props.span.src);
    if (read === null) {
      setState({ kind: "failed", reason: "This image cannot be shown here." });
      return;
    }
    let live = true;
    onCleanup(() => {
      live = false;
    });
    setState({ kind: "loading" });
    read.then(
      (url) => live && setState({ kind: "ready", url }),
      (reason: unknown) =>
        live &&
        setState({
          kind: "failed",
          reason: reason instanceof Error ? reason.message : String(reason),
        }),
    );
  });

  const label = () => props.span.alt || props.span.src;
  const picture = () => {
    const current = state();
    if (current.kind === "ready") {
      return (
        <img
          class="forge-md-img"
          src={current.url}
          alt={props.span.alt}
          style={props.span.width ? { width: props.span.width } : undefined}
          onError={() => setState({ kind: "failed", reason: "The image did not load." })}
        />
      );
    }
    return (
      <span
        class="forge-md-img-placeholder"
        data-state={current.kind}
        title={current.kind === "failed" ? current.reason : undefined}
      >
        <Icon name={current.kind === "failed" ? "image-off" : "image"} size={12} />
        {label()}
      </span>
    );
  };

  return (
    <Show
      when={loader()}
      fallback={
        <Show when={safeHref(props.span.src)} fallback={label()}>
          {(href) => <ExternalLink href={href()}>{label()}</ExternalLink>}
        </Show>
      }
    >
      <Show when={props.span.href} fallback={picture()}>
        {(href) => <ExternalLink href={href()}>{picture()}</ExternalLink>}
      </Show>
    </Show>
  );
}

function Emphasis(props: { span: MdText }) {
  // Only plain prose is scanned for paths: a fenced block is quoted verbatim
  // and an `[a](b)` link already says where it goes.
  const paths = useContext(PathRenderer);
  const inner = () => (props.span.em ? <em>{paths(props.span.text)}</em> : paths(props.span.text));
  return (
    <Show when={props.span.strong} fallback={inner()}>
      {<strong>{inner()}</strong>}
    </Show>
  );
}
