// Type-check only: OpenCode provides this module at runtime, and the shape
// below is what `forge-attention.ts` relies on, not the full API.
declare module "@opencode/plugin/tui" {
  export type Route = { type: "session"; sessionID: string } | { type: "home" | (string & {}) };

  export interface SessionEvent {
    data: { sessionID: string };
  }

  export interface FormEvent {
    data: { form: { sessionID: string } };
  }

  export interface PluginContext {
    options: Record<string, unknown>;
    ui: { router: { current(): Route } };
    data: {
      session: { root(sessionID: string): string | undefined };
      on(event: "form.created", listener: (event: FormEvent) => void): () => void;
      on(event: string, listener: (event: SessionEvent) => void): () => void;
    };
  }

  export interface PluginDefinition {
    id: string;
    setup(context: PluginContext): void | (() => void);
  }

  export const Plugin: {
    define(definition: PluginDefinition): PluginDefinition;
  };
}
