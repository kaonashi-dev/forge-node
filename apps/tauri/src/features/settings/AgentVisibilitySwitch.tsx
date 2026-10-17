import { agentVisibilityKey, agentVisible } from "../../state/agentVisibility";
import { forgeStore } from "../../state/forgeStore";
import { Switch } from "../../ui/index";
import { setAppState } from "./commands";

/** Off hides the agent from every picker; the provider stays installed. */
export function AgentVisibilitySwitch(props: { agentKey: string; label: string }) {
  const visible = () => agentVisible(forgeStore.app_state, props.agentKey);

  return (
    <Switch
      label={`${props.label} enabled`}
      hideLabel
      checked={visible()}
      onChange={(next) =>
        void setAppState(agentVisibilityKey(props.agentKey), String(next)).catch(() => undefined)
      }
    />
  );
}
