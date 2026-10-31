import {
  For,
  Index,
  Match,
  Show,
  Switch,
  createEffect,
  createMemo,
  createResource,
  on,
  onCleanup,
} from "solid-js";
import { Markdown } from "../../../shared/markdown/Markdown";
import { openUrl } from "../../../runtime/host";
import { PathText } from "../references/PathText";
import { previewKindFor } from "./previewRoute";
import { SourceSwitch } from "./SourceSwitch";
import { EditorBreadcrumbs } from "../../editor/EditorChromeBars";
import { svgDataUrl } from "./previewImages";
import { csvDelimiter, parseCsv } from "./csvTable";
import {
  beginWorkbenchRequest,
  failWorkbenchRequest,
  openFile,
  previewImageReader,
} from "../commands";
import { filesStore } from "../state";

/**
 * A file the terminal editor cannot draw, drawn.
 *
 * Markdown as a document, a CSV as a table, an SVG as a picture, a raster
 * image as itself. Text
 * never reaches here: `previewKindFor` sends it to the editor, which is what
 * every other file opens in.
 *
 * Reads go through the daemon like every other file read (ADR-012) — the
 * WebView opens nothing.
 */
export function PreviewView(props: { workspace: string; path: string }) {
  const kind = createMemo(() => previewKindFor(props.path));

  /* Markdown, CSV and SVG are text, so they come through the same `ReadFile` the
     editor's conflict view uses; only a raster needs `ReadImage`. */
  createEffect(
    on(
      () => [props.workspace, props.path, kind()] as const,
      ([workspace, path, previewKind]) => {
        if (previewKind === "image") return;
        let active = true;
        onCleanup(() => {
          active = false;
        });
        beginWorkbenchRequest("file");
        void openFile(workspace, path).catch((error) => {
          if (active) failWorkbenchRequest("file", error);
        });
      },
    ),
  );

  const text = createMemo(() =>
    filesStore.file?.path === props.path ? (filesStore.file?.text ?? null) : null,
  );

  /*
   * An SVG is shown through an `<img>` and never as `innerHTML`. SVG can carry
   * script and this is a file from the checkout, which an agent may have
   * written: an `<img>` refuses to run it, a div would not.
   */
  const svgUrl = createMemo(() => {
    if (kind() !== "svg") return null;
    const body = text();
    if (body === null) return null;
    return svgDataUrl(body);
  });

  const table = createMemo(() => {
    if (kind() !== "csv") return null;
    const body = text();
    return body === null ? null : parseCsv(body, csvDelimiter(props.path));
  });

  const [raster] = createResource(
    () => (kind() === "image" ? ([props.workspace, props.path] as const) : null),
    ([workspace, path]) => previewImageReader.read(workspace, path),
  );

  const loader = (src: string) =>
    /^[a-z]+:/i.test(src)
      ? null
      : previewImageReader.read(props.workspace, resolve(props.path, src));

  return (
    <div class="preview-view">
      <EditorBreadcrumbs>
        <SourceSwitch path={props.path} surface="preview" />
      </EditorBreadcrumbs>
      <div class="preview-body" classList={{ "preview-document": kind() === "markdown" }}>
        <Show when={filesStore.fileError}>
          {(error) => <p class="panel-error">Could not read the file: {error()}</p>}
        </Show>
        <Switch>
          <Match when={kind() === "markdown"}>
            <Show when={text() !== null} fallback={<p class="panel-note">Reading…</p>}>
              <Markdown
                class="forge-md-doc"
                text={text() ?? ""}
                images={loader}
                onOpenUrl={(href) => void openUrl(href).catch(() => undefined)}
                renderPath={(text) => <PathText text={text} />}
              />
            </Show>
          </Match>
          <Match when={kind() === "csv"}>
            <Show when={table()} fallback={<p class="panel-note">Reading…</p>}>
              {(csv) => (
                <CsvPreview rows={csv().rows} width={csv().width} truncated={csv().truncated} />
              )}
            </Show>
          </Match>
          <Match when={kind() === "svg"}>
            <Show when={svgUrl()} fallback={<p class="panel-note">Reading…</p>}>
              {(url) => <img class="preview-image" src={url()} alt={props.path} />}
            </Show>
          </Match>
          <Match when={kind() === "image"}>
            <Show
              when={raster()}
              fallback={
                <p class={raster.error ? "panel-error" : "panel-note"}>
                  {raster.error ? String(raster.error) : "Reading…"}
                </p>
              }
            >
              {(url) => <img class="preview-image" src={url()} alt={props.path} />}
            </Show>
          </Match>
        </Switch>
      </div>
    </div>
  );
}

/** The first row is the header; a short row is padded to the widest one. */
function CsvPreview(props: { rows: string[][]; width: number; truncated: boolean }) {
  const columns = createMemo(() => Array.from({ length: props.width }, (_, column) => column));
  const cells = (row: string[]) => (
    <Index each={columns()}>{(column) => <td>{row[column()] ?? ""}</td>}</Index>
  );
  return (
    <Show when={props.rows.length > 0} fallback={<p class="panel-note">Empty file.</p>}>
      <table class="forge-md-table preview-table">
        <thead>
          <tr>
            <Index each={columns()}>{(column) => <th>{props.rows[0][column()] ?? ""}</th>}</Index>
          </tr>
        </thead>
        <tbody>
          <For each={props.rows.slice(1)}>{(row) => <tr>{cells(row)}</tr>}</For>
        </tbody>
      </table>
      <Show when={props.truncated}>
        <p class="panel-note">
          Showing the first rows and columns only. Edit opens the whole file.
        </p>
      </Show>
    </Show>
  );
}

/**
 * A relative `src` in a document, against the document's own directory.
 *
 * Kept here rather than in the loader so `previewRoute` stays free of paths:
 * the daemon canonicalises inside the checkout either way and refuses anything
 * that leaves it, so this only has to be the *intended* path.
 */
function resolve(document: string, src: string): string {
  if (src.startsWith("/")) return src.slice(1);
  const base = document.slice(0, document.lastIndexOf("/") + 1);
  const parts: string[] = [];
  for (const piece of `${base}${src}`.split("/")) {
    if (piece === "" || piece === ".") continue;
    if (piece === "..") parts.pop();
    else parts.push(piece);
  }
  return parts.join("/");
}
