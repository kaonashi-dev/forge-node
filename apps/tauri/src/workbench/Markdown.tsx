import { For, Match, Show, Switch, createMemo } from "solid-js";
import { Dynamic } from "solid-js/web";
import { openUrl } from "../runtime/api";
import { Icon } from "../theme/icons";
import { parseMarkdown, type MdBlock, type MdSpan } from "./markdownBlocks";

type MdText = Extract<MdSpan, { kind: "text" }>;

/**
 * A heading in the body is a heading *under* the view's own title, so the
 * outline stays true; how big it looks is `data-level`'s job, not the tag's.
 */
const HEADING_TAG = ["h3", "h4", "h5", "h6", "h6", "h6"] as const;

/**
 * Rendered Markdown, from text the app did not write.
 *
 * The blocks arrive as data and are drawn with JSX: nothing here reaches
 * `innerHTML`, so a description carrying markup shows the markup. A link opens
 * through the host rather than as a navigation — this is a webview, and letting
 * it follow an `href` would replace the app with the page.
 */
export function Markdown(props: { text: string; class?: string }) {
  const blocks = createMemo(() => parseMarkdown(props.text));
  return (
    <div class={`forge-md ${props.class ?? ""}`}>
      <Blocks blocks={blocks()} />
    </div>
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
              <pre class="forge-md-pre" data-lang={code().lang ?? undefined}>
                <code>{code().text}</code>
              </pre>
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

function Spans(props: { spans: MdSpan[] }) {
  return (
    <For each={props.spans}>
      {(span) => (
        <Switch>
          <Match when={span.kind === "code" && span}>
            {(code) => <code class="forge-md-inline-code">{code().text}</code>}
          </Match>
          <Match when={span.kind === "link" && span}>
            {(link) => (
              <a
                class="forge-md-link"
                href={link().href}
                rel="noreferrer"
                onClick={(event) => {
                  event.preventDefault();
                  void openUrl(link().href).catch(() => undefined);
                }}
              >
                {link().text}
              </a>
            )}
          </Match>
          <Match when={span.kind === "text" && span}>{(text) => <Emphasis span={text()} />}</Match>
        </Switch>
      )}
    </For>
  );
}

function Emphasis(props: { span: MdText }) {
  const inner = () => (props.span.em ? <em>{props.span.text}</em> : props.span.text);
  return (
    <Show when={props.span.strong} fallback={inner()}>
      {<strong>{inner()}</strong>}
    </Show>
  );
}
