import { agentVisibilityKey, agentVisible } from "../../state/agentVisibility";
import { forgeStore } from "../../state/forgeStore";
import { Button } from "../../ui/index";
import { setAppState } from "./commands";

export function AgentVisibilityButton(props: { agentKey: string; label: string }) {
  const visible = () => agentVisible(forgeStore.app_state, props.agentKey);

  return (
    <Button
      variant="secondary"
      size="xs"
      aria-label={`${visible() ? "Disable" : "Enable"} ${props.label}`}
      onClick={() =>
        void setAppState(agentVisibilityKey(props.agentKey), String(!visible())).catch(
          () => undefined,
        )
      }
    >
      {visible() ? "Disable" : "Enable"}
    </Button>
  );
}
