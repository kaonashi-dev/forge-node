import { Show } from "solid-js";
import { Icon } from "../../theme/icons/index";
import { IconButton } from "../../ui/index";
import { handoffJob, setHandoffProgressOpen } from "./handoffJobStore";

export function HandoffStatusButton() {
  return (
    <Show when={handoffJob()}>
      <IconButton label="Open session handoff" onClick={() => setHandoffProgressOpen(true)}>
        <Icon
          name={handoffJob()?.summary || handoffJob()?.error ? "message-square-plus" : "loader"}
          class={
            handoffJob()?.summary || handoffJob()?.error
              ? "forge-icon-muted"
              : "forge-icon-muted forge-icon-spin"
          }
          size={14}
        />
      </IconButton>
    </Show>
  );
}
