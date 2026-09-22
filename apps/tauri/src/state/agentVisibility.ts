export function agentVisibilityKey(key: string): string {
  return `ui.agent_visible.${key}`;
}

export function agentVisible(appState: Record<string, string>, key: string): boolean {
  return appState[agentVisibilityKey(key)] !== "false";
}
