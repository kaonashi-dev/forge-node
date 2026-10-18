import { Match, Switch, createEffect, createMemo, createSignal, on } from "solid-js";
import { sessionAttention } from "./attention";
import { now } from "../../runtime/clock";
import type { Session } from "../../contracts/runtime";
import { sessionWork } from "./work";
import { connectionStore } from "../../state/connection";
import { BrandIcon, brandForProvider } from "../../theme/icons/BrandIcon";
import type { IconEmphasis } from "../../theme/icons/Icon";
import { Icon } from "../../theme/icons/Icon";
import { sessionGlyphFace } from "./glyphFace";

type SessionGlyphProps = {
  providerId: string | null;
  /**
   * When set, a busy session is a spinner, one that stopped to ask is a bell,
   * and a finished one the user is not in is a check. Launch menus omit this
   * and always paint identity.
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
 * falls back to the Forge agent glyph. Shells take the accent so they stay apart
 * from those marks.
 *
 * No mark carries a fill of its own: each is tinted by whatever the row it sits
 * in is already using, which is what keeps four providers in one list reading
 * as four shapes rather than as four colours. The bell is the exception — an
 * agent waiting on a person owns the attention colour wherever it is drawn.
 */
export function SessionGlyph(props: SessionGlyphProps) {
  const emphasis = () => props.emphasis ?? "full";
  const work = createMemo(() => {
    const session = props.session;
    if (!session) return undefined;
    return sessionWork(session, sessionAttention(session.id), now());
  });
  /*
   * The bell swings and the check draws when the session *becomes* waiting or
   * finished, not whenever the row mounts: switching checkouts or unfolding a
   * card remounts glyphs whose state did not change.
   */
  const [arrived, setArrived] = createSignal<"needs-you" | "done" | null>(null);
  createEffect(
    on(
      work,
      (next, previous) => {
        if (next === previous) return;
        if (next === "needs-you") setArrived("needs-you");
        else if (
          (next === "idle" || next === "exited") &&
          (previous === "working" || previous === "starting")
        )
          setArrived("done");
        else setArrived(null);
      },
      { defer: true },
    ),
  );
  const face = () => {
    const session = props.session;
    if (!session) return "identity" as const;
    const attention = sessionAttention(session.id);
    return sessionGlyphFace(work(), {
      active: session.id === connectionStore.activeSession,
      isAgent: session.agent_provider_id != null,
      unread: attention.unread,
    });
  };

  return (
    <Switch
      fallback={
        <IdentityGlyph
          providerId={props.providerId}
          kind={props.session?.kind}
          emphasis={emphasis()}
          size={props.size}
        />
      }
    >
      <Match when={face() === "needs-you"}>
        <Icon
          name="bell"
          class={`forge-icon-needs-you ${arrived() === "needs-you" ? "glyph-ring-once" : ""}`}
          size={props.size}
          title="Needs you"
        />
      </Match>
      <Match when={face() === "working"}>
        <Icon
          name="loader"
          class={`forge-icon-spin ${work() === "starting" ? "forge-icon-amber" : "forge-icon-accent"}`}
          size={props.size}
          title={work() === "starting" ? "Starting" : "Working"}
        />
      </Match>
      <Match when={face() === "done"}>
        <Icon
          name="check"
          class={`${arrived() === "done" ? "glyph-draw" : ""} ${work() === "idle" ? "forge-icon-green" : "forge-icon-muted"}`}
          size={props.size}
          title={work() === "idle" ? "Finished" : "Exited"}
        />
      </Match>
    </Switch>
  );
}

function IdentityGlyph(props: {
  providerId: string | null;
  kind?: string;
  emphasis: IconEmphasis;
  size?: number;
}) {
  if (props.kind === "Editor") {
    return <Icon name="file-code" emphasis={props.emphasis} size={props.size} />;
  }
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
