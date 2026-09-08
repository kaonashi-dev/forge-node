import { Show } from "solid-js";
import { sessionAttention } from "../../runtime/attention";
import { now } from "../../runtime/clock";
import type { Session } from "../../runtime/types";
import { sessionWork } from "../../runtime/work";
import { runtimeStore } from "../../store/runtimeStore";
import { BrandIcon, brandForProvider } from "./BrandIcon";
import type { IconEmphasis } from "./Icon";
import { Icon } from "./Icon";
import { sessionGlyphFace } from "./glyphFace";

type SessionGlyphProps = {
  providerId: string | null;
  /**
   * When set, a busy session is a spinner and a finished one the user is not
   * in is a check. Launch menus omit this and always paint identity.
   */
  session?: Session;
  emphasis?: IconEmphasis;
  size?: number;
};

/**
 * Provider mark for an agent, or the terminal glyph for a shell.
 *
 * A shared robot for every agent would make a workspace with four providers
 * unreadable, so a known provider draws its own `BrandIcon`; one without a logo
 * falls back to the lucide bot. Shells take the accent so they stay apart from
 * those marks.
 *
 * No mark carries a fill of its own: each is tinted by whatever the row it sits
 * in is already using, which is what keeps four providers in one list reading
 * as four shapes rather than as four colours.
 */
export function SessionGlyph(props: SessionGlyphProps) {
  const emphasis = () => props.emphasis ?? "full";
  const work = () => {
    const session = props.session;
    if (!session) return undefined;
    return sessionWork(session, sessionAttention(session.id), now());
  };
  const face = () => {
    const session = props.session;
    if (!session) return "identity" as const;
    const attention = sessionAttention(session.id);
    return sessionGlyphFace(work(), {
      active: session.id === runtimeStore.activeSession,
      isAgent: session.agent_provider_id != null,
      unread: attention.unread,
    });
  };

  return (
    <Show
      when={face() === "working"}
      fallback={
        <Show
          when={face() === "done"}
          fallback={
            <IdentityGlyph providerId={props.providerId} emphasis={emphasis()} size={props.size} />
          }
        >
          <Icon
            name="check"
            class={work() === "idle" ? "forge-icon-green" : "forge-icon-muted"}
            size={props.size}
            title={work() === "idle" ? "Finished" : "Exited"}
          />
        </Show>
      }
    >
      <Icon
        name="loader"
        class={`forge-icon-spin ${work() === "starting" ? "forge-icon-amber" : "forge-icon-blue"}`}
        size={props.size}
        title={work() === "starting" ? "Starting" : "Working"}
      />
    </Show>
  );
}

function IdentityGlyph(props: {
  providerId: string | null;
  emphasis: IconEmphasis;
  size?: number;
}) {
  if (props.providerId) {
    const brand = brandForProvider(props.providerId);
    if (brand) {
      return (
        <BrandIcon
          brand={brand}
          size={props.size}
          class={props.emphasis === "dim" ? "forge-icon-dim" : undefined}
        />
      );
    }
    return <Icon name="agent" emphasis={props.emphasis} size={props.size} />;
  }

  return (
    <Icon
      name="square-terminal"
      class={props.emphasis === "full" ? "forge-icon-accent" : "forge-icon-faint"}
      size={props.size}
    />
  );
}
