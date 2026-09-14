// On-demand change preview; the host supplies saved patch rows, never filesystem access.

export type GitChange = {
  /** Inclusive 1-based range in the saved document; deletions use the surviving line. */
  from: number;
  to: number;
  kind: "added" | "modified" | "deleted";
  rows: {
    kind: "added" | "removed" | "meta";
    text: string;
    before: number | null;
    after: number | null;
    emphasis?: { from: number; to: number };
  }[];
};

const PREVIEW_ROWS = 200;

export function createChangeDetails(
  host: HTMLElement,
  options: {
    onClose: () => void;
    onStep: (direction: number) => void;
    onOpenDiff?: () => void;
  },
) {
  const panel = document.createElement("section");
  panel.className = "fw-change";
  panel.setAttribute("role", "dialog");
  panel.setAttribute("aria-label", "Change details");
  panel.tabIndex = -1;
  const header = document.createElement("header");
  header.className = "fw-change-head";
  const title = document.createElement("span");
  title.className = "fw-change-title";
  title.setAttribute("aria-live", "polite");
  header.append(title);

  function action(label: string, text: string, run: () => void): HTMLButtonElement {
    const button = document.createElement("button");
    button.type = "button";
    button.title = label;
    button.setAttribute("aria-label", label);
    button.textContent = text;
    button.addEventListener("click", run);
    header.append(button);
    return button;
  }

  if (options.onOpenDiff) action("Open full diff", "Diff ↗", options.onOpenDiff);
  const previous = action("Previous change", "↑", () => options.onStep(-1));
  const next = action("Next change", "↓", () => options.onStep(1));
  action("Close change details", "×", options.onClose);
  const content = document.createElement("div");
  content.className = "fw-change-content";
  content.tabIndex = 0;
  content.setAttribute("role", "region");
  content.setAttribute("aria-label", "Changed lines");
  const footer = document.createElement("div");
  footer.className = "fw-change-foot";
  panel.append(header, content, footer);
  panel.addEventListener("keydown", (event) => {
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    options.onClose();
  });
  host.append(panel);

  return {
    element: panel,
    show(change: GitChange, index: number, total: number): void {
      const label = { added: "Added", modified: "Modified", deleted: "Deleted" }[change.kind];
      const added = change.rows.filter((row) => row.kind === "added").length;
      const removed = change.rows.filter((row) => row.kind === "removed").length;
      title.textContent = `${label} · ${index + 1}/${total} · +${added} −${removed}`;
      previous.disabled = index === 0;
      next.disabled = index === total - 1;
      panel.dataset.kind = change.kind;
      const fragment = document.createDocumentFragment();
      for (const row of change.rows.slice(0, PREVIEW_ROWS)) {
        const line = document.createElement("div");
        line.className = `fw-change-row fw-change-${row.kind}`;
        const number = document.createElement("span");
        number.className = "fw-change-number";
        number.textContent = String(row.before ?? row.after ?? "");
        const sign = document.createElement("span");
        sign.className = "fw-change-sign";
        sign.textContent = row.kind === "added" ? "+" : row.kind === "removed" ? "−" : "";
        const text = document.createElement("span");
        text.className = "fw-change-text";
        const span = row.emphasis;
        if (span && span.to > span.from) {
          const mark = document.createElement("mark");
          mark.textContent = row.text.slice(span.from, span.to);
          text.append(row.text.slice(0, span.from), mark, row.text.slice(span.to));
        } else {
          text.textContent = row.text || " ";
        }
        line.append(number, sign, text);
        fragment.append(line);
      }
      content.replaceChildren(fragment);
      content.scrollTop = 0;
      content.scrollLeft = 0;
      const omitted = change.rows.length - PREVIEW_ROWS;
      footer.textContent =
        omitted > 0
          ? `${omitted} more lines omitted from preview${options.onOpenDiff ? " · Open full diff to see all" : ""}`
          : "Saved changes · Escape to close";
    },
    destroy: () => panel.remove(),
  };
}
