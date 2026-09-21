import { Show } from "solid-js";
import { Icon, LangIcon } from "../../../theme/icons/index";
import { viewLabel, type WorkbenchView } from "../../../navigation/views";

/** The mark on a Code tab — by name for a file, by kind for the rest. */
export function ViewGlyph(props: { view: WorkbenchView; size?: number }) {
  const size = () => props.size ?? 13;
  const glyph = () => {
    const view = props.view;
    switch (view.kind) {
      case "diff":
        return { icon: "git-branch", tone: "forge-icon-amber" } as const;
      case "review":
        return { icon: "list-checks", tone: "forge-icon-accent" } as const;
      case "pr_detail":
      case "pr_compose":
        return { icon: "git-pull-request", tone: "forge-icon-accent" } as const;
      case "pr_review":
        return { icon: "list-checks", tone: "forge-icon-amber" } as const;
      case "editor-terminal":
        return { icon: "file-code", tone: "forge-icon-accent" } as const;
      case "search":
        return { icon: "search", tone: "forge-icon-amber" } as const;
      default:
        return { icon: "square-terminal", tone: "forge-icon-faint" } as const;
    }
  };

  return (
    <Show
      when={props.view.kind === "editor-terminal"}
      fallback={<Icon name={glyph().icon} class={glyph().tone} size={size()} />}
    >
      <LangIcon path={viewLabel(props.view)} size={size()} />
    </Show>
  );
}
