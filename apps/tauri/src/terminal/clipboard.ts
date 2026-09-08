type Paste = { kind: "text"; text: string } | { kind: "agent" } | { kind: "empty" };

export function clipboardPaste(data: Pick<DataTransfer, "getData" | "types"> | null): Paste {
  if (!data) return { kind: "empty" };
  // Image clipboards can also carry a textual source URL.
  if (data.types.some((type) => type === "Files" || type.startsWith("image/"))) {
    return { kind: "agent" };
  }
  const text = data.getData("text/plain");
  return text ? { kind: "text", text } : { kind: "empty" };
}
