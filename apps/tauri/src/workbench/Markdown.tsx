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
  useContext,
  type JSX,
} from "solid-js";
import { Dynamic } from "solid-js/web";
import { openUrl } from "../runtime/api";
import { Icon } from "../theme/icons";
import { themeBase } from "../theme/ThemeProvider";
import { isDiagram, renderDiagram } from "./diagram";
import { parseMarkdown, safeHref, type MdBlock, type MdImage, type MdSpan } from "./markdownBlocks";
import { PathText } from "./PathText";

type MdText = Extract<MdSpan, { kind: "text" }>;

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
export function Markdown(props: { text: string; class?: string; images?: MdImageLoader }) {
  const blocks = createMemo(() => parseMarkdown(props.text));
  return (
    <ImageLoader.Provider value={() => props.images}>
      <div class={`forge-md ${props.class ?? ""}`}>
        <Blocks blocks={blocks()} />
      </div>
    </ImageLoader.Provider>
  );
}

function Blocks(props: { blocks: MdBlock[] }) {
  return (
    <For each={props.blocks}>
      {(block) => (
        <Switch>
          <Match when={block.kind === "heading" && block}>
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
          <Match when={block.kind === "paragraph" && block}>
            {(paragraph) => (
              <p class="forge-md-p">
                <Spans spans={paragraph().spans} />
              </p>
            )}
          </Match>
          <Match when={block.kind === "list" && block}>
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
                        <span
                          class="forge-md-box"
                          data-checked={item.checked === true ? "" : undefined}
                          role="img"
                          aria-label={item.checked === true ? "Done" : "Not done"}
                        >
                          <Show when={item.checked === true}>
                            <Icon name="check" size={10} />
                          </Show>
                        </span>
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
          <Match when={block.kind === "code" && block}>
            {(code) => (
              <Show when={isDiagram(code().lang)} fallback={<CodeBlock {...code()} />}>
                <Diagram text={code().text} />
              </Show>
            )}
          </Match>
          <Match when={block.kind === "quote" && block}>
            {(quote) => (
              <blockquote class="forge-md-quote">
                <Blocks blocks={quote().blocks} />
              </blockquote>
            )}
          </Match>
          <Match when={block.kind === "table" && block}>
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
          <Match when={block.kind === "rule"}>
            <hr class="forge-md-rule" />
          </Match>
        </Switch>
      )}
    </For>
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
  return (
    <a
      class="forge-md-link"
      href={props.href}
      rel="noreferrer"
      onClick={(event) => {
        event.preventDefault();
        void openUrl(props.href).catch(() => undefined);
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
  const inner = () =>
    props.span.em ? (
      <em>
        <PathText text={props.span.text} />
      </em>
    ) : (
      <PathText text={props.span.text} />
    );
  return (
    <Show when={props.span.strong} fallback={inner()}>
      {<strong>{inner()}</strong>}
    </Show>
  );
}
