/** What a paste gesture should do with what is on the clipboard. */
type Paste = { kind: "text"; text: string } | { kind: "agent" } | { kind: "empty" };

/**
 * Where the paste is going. The destination decides, not only the payload:
 * Notes, Excel, Numbers and Slack put text *and* an image representation on
 * one clipboard, and reading the image first made every pane take the wrong
 * branch — the editor pasted nothing and a shell pasted `ctrl-v`
 * (`quoted-insert` in readline) instead of the text.
 */
export type PasteTarget =
  /** The Code pane, which holds text and nothing else. */
  | "editor"
  /** A shell or anything else that reads typed input. */
  | "shell"
  /** An agent's own terminal: pressing `ctrl-v` lets it read the image itself. */
  | "agent";

export function clipboardPaste(
  data: (Pick<DataTransfer, "getData" | "types"> & { items?: DataTransfer["items"] }) | null,
  target: PasteTarget,
): Paste {
  if (!data) return { kind: "empty" };
  // Only an agent can do anything with an image, and only by reading the
  // system clipboard itself; text travels as a paste event for everyone.
  if (target === "agent" && holdsImage(data)) return { kind: "agent" };
  const text = data.getData("text/plain");
  return text ? { kind: "text", text } : { kind: "empty" };
}

/**
 * Whether the clipboard holds an image an agent should read itself.
 *
 * WebKit, and so the app's WKWebView, lists only DOM-safe types in `types`:
 * a screenshot arrives as `Files` with an `image/*` file item, never as an
 * `image/png` type, which only Chromium exposes. A file copied in the Finder
 * is also a file item, but it carries its `file:` URL, and that path is what
 * the paste should be.
 */
function holdsImage(
  data: Pick<DataTransfer, "getData" | "types"> & { items?: DataTransfer["items"] },
): boolean {
  if (data.types.some((type) => type.startsWith("image/"))) return true;
  const imageFile = Array.from(data.items ?? []).some(
    (item) => item.kind === "file" && item.type.startsWith("image/"),
  );
  if (!imageFile) return false;
  const finderCopy =
    data.types.includes("text/uri-list") && data.getData("text/uri-list").startsWith("file:");
  return !finderCopy;
}
