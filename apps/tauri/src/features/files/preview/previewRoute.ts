// Which files open as something rendered rather than as text.
//
// The terminal editor owns every file a cell grid can show, which is almost
// all of them. What it cannot show is a *rendered* document: a Markdown page
// with its headings and images, an SVG as a picture rather than as its source,
// a PNG at all. Those keep a DOM pane, and this decides which is which.
//
// Extension only, and the same rule the daemon's `ReadImage` uses — routing a
// file to a preview the daemon will refuse bytes for is worse than opening it
// as text.

import { imageMime } from "./previewImages";

/** What surface a path belongs on. */
export type PreviewKind = "markdown" | "svg" | "image";

/** Extensions rendered as a Markdown document. */
const MARKDOWN = new Set(["md", "markdown", "mdx"]);

/**
 * The rendered surface this path belongs on, or `null` for the editor.
 *
 * `null` is the common answer and the right default: a kind this does not
 * recognise is text, and text is what the editor is for.
 */
export function previewKindFor(path: string): PreviewKind | null {
  const name = path.replace(/[?#].*$/, "");
  const dot = name.lastIndexOf(".");
  const slash = name.lastIndexOf("/");
  if (dot <= slash + 1) return null;
  const extension = name.slice(dot + 1).toLowerCase();
  if (MARKDOWN.has(extension)) return "markdown";
  // An SVG is both a picture and a text file. It opens as a picture; Edit
  // opens the source, because a rendering is not the file.
  if (extension === "svg") return "svg";
  return imageMime(name) === null ? null : "image";
}

/**
 * Whether a path opens as text in the terminal editor.
 *
 * The inverse of [`previewKindFor`], named so a caller reads as the question it
 * is asking rather than as a negation.
 */
export function opensInEditor(path: string): boolean {
  return previewKindFor(path) === null;
}

/**
 * Whether a preview is text the editor can open.
 *
 * A raster is not: the editor refuses a binary file. Markdown and SVG are,
 * so the preview is a rendering of a file that still has a source.
 */
export function hasEditableSource(path: string): boolean {
  const kind = previewKindFor(path);
  return kind === "markdown" || kind === "svg";
}

/** Where an open lands. A line into editable source is a caret request. */
export function openingSurface(path: string, line?: number): "editor" | "preview" {
  if (opensInEditor(path) || (line !== undefined && hasEditableSource(path))) return "editor";
  return "preview";
}
