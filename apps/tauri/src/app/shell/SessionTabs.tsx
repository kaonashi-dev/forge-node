import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { focusSession } from "../../features/sessions/sessionActions";
import { sessionTabLabel, tabGround, type TabGround } from "../../features/sessions/attention";
import type { Session } from "../../contracts/runtime";
import { SessionGlyph } from "../../features/sessions/SessionGlyph";
import { Icon } from "../../theme/icons/index";
import { AttentionMarker, StateMarker } from "../../features/sessions/markers";
import { ContextMenu, IconButton, type MenuItem } from "../../ui/index";
import { SessionMenu } from "../../features/sessions/SessionMenu";
import { hasQuestion } from "../../features/sessions/waiting";
import { closeSessionFromUi, sessionMenuItems } from "../../features/sessions/sessionMenuItems";
import { centerSplit } from "../../navigation/centerSplitStore";
import {
  clampGapToKind,
  dropFromGap,
  gapAtX,
  moveTabToGap,
  orderFromSessions,
} from "../../navigation/tabOrder";

const TAB_MIN_W = 120;
const CODE_TAB_MIN_W = 92;
const TAB_SCROLL_STEP = 120;
const TAB_DRAG_PX = 4;
const TAB_SCROLL_EDGE_PX = 40;

type SessionTabsProps = {
  sessions: Session[];
  activeId: string | null;
  tabOrder: string[];
  onReorderTabs: (order: string[]) => void;
  /** Code is available whenever a workspace is selected. */
  codeOpen: boolean;
  /** Whether Code, rather than a session, is what the window is showing. */
  codeActive: boolean;
  /** How many files/diffs are parked inside it. */
  codeCount: number;
  onSelectCode: () => void;
  onCloseCode: () => void;
};

export function SessionTabs(props: SessionTabsProps) {
  let scrollEl: HTMLDivElement | undefined;
  const [overflowing, setOverflowing] = createSignal(false);
  /* Which edges of the scrollport hide a tab mid-pill. Drives the mask fade
     so a half-visible current tab dissolves instead of clipping square. */
  const [edgeFade, setEdgeFade] = createSignal<"none" | "left" | "right" | "both">("none");
  const [dragging, setDragging] = createSignal<string | null>(null);
  const [dropTarget, setDropTarget] = createSignal<{
    id: string;
    after: boolean;
  } | null>(null);
  let drag: { id: string; pointerId: number; startX: number; started: boolean } | null = null;
  /* A drop selects the tab from `pointerup`, and the browser still fires the `click`
     that follows it. Without this the drop costs two `SelectSession` round trips. */
  let swallowClick = false;
  const [menu, setMenu] = createSignal<{
    x: number;
    y: number;
    items: MenuItem[];
  } | null>(null);

  const tabs = () => props.sessions.filter((session) => session.terminal_id != null);
  /* While Code is up no session is current. Left at `current`, two tabs in the
     same strip would both claim the window. */
  const groundOf = (session: Session) => {
    const split = centerSplit();
    if (split.kind === "session" && split.extra === session.id && split.focused === "extra") {
      return tabGround(session, session.id);
    }
    return tabGround(session, props.codeActive ? null : props.activeId);
  };

  /* Measured, not counted. Two tabs in a wide window overflow nothing, and a
     count-based guess kept the scroll chevrons on screen with nowhere to
     scroll. */
  const onScroll = () => {
    if (!scrollEl) {
      return;
    }
    const max = scrollEl.scrollWidth - scrollEl.clientWidth;
    const hasOverflow = max > 1;
    setOverflowing(hasOverflow);
    if (!hasOverflow) {
      setEdgeFade("none");
      return;
    }
    const atLeft = scrollEl.scrollLeft <= 1;
    const atRight = scrollEl.scrollLeft >= max - 1;
    setEdgeFade(atLeft ? (atRight ? "none" : "right") : atRight ? "left" : "both");
  };

  onMount(() => {
    onScroll();
    if (!scrollEl || typeof ResizeObserver === "undefined") {
      return;
    }
    const observer = new ResizeObserver(() => onScroll());
    observer.observe(scrollEl);
    onCleanup(() => observer.disconnect());
  });

  // Tabs opening or closing changes scrollWidth without firing scroll or resize.
  createEffect(() => {
    tabs().length;
    props.codeOpen;
    queueMicrotask(onScroll);
  });

  const scrollBy = (delta: number) => {
    if (!scrollEl) {
      return;
    }
    scrollEl.scrollBy({ left: delta, behavior: "smooth" });
    requestAnimationFrame(onScroll);
  };

  function finishDrag(): void {
    drag = null;
    setDragging(null);
    setDropTarget(null);
  }
  onCleanup(finishDrag);

  function tabBoxes(): { left: number; width: number }[] {
    if (!scrollEl) return [];
    const nodes = scrollEl.querySelectorAll<HTMLElement>(".session-tab-wrap:not(.code-tab-wrap)");
    return Array.from(nodes, (node) => {
      const rect = node.getBoundingClientRect();
      return { left: rect.left, width: rect.width };
    });
  }

  function scrollStripToward(x: number): void {
    if (!scrollEl) return;
    const rect = scrollEl.getBoundingClientRect();
    if (x < rect.left + TAB_SCROLL_EDGE_PX) {
      scrollEl.scrollBy({ left: -TAB_SCROLL_STEP / 4 });
    } else if (x > rect.right - TAB_SCROLL_EDGE_PX) {
      scrollEl.scrollBy({ left: TAB_SCROLL_STEP / 4 });
    }
  }

  /* HTML5 `draggable` never starts on a div in WKWebView, and the title bar's
     app-region would steal the gesture if it did. Pointer capture is the same
     path ResizeHandle uses. */
  function onTabPointerDown(sessionId: string, event: PointerEvent): void {
    if (event.button !== 0) return;
    if (event.target instanceof Element && event.target.closest(".session-tab-close")) return;
    const target = event.currentTarget;
    if (!(target instanceof HTMLElement)) return;
    swallowClick = false;
    drag = { id: sessionId, pointerId: event.pointerId, startX: event.clientX, started: false };
    target.setPointerCapture(event.pointerId);
  }

  function onTabPointerMove(sessionId: string, event: PointerEvent): void {
    if (!drag || drag.id !== sessionId || drag.pointerId !== event.pointerId) return;
    if (!drag.started) {
      if (Math.abs(event.clientX - drag.startX) < TAB_DRAG_PX) return;
      drag.started = true;
      setDragging(sessionId);
    }
    event.preventDefault();
    scrollStripToward(event.clientX);
    const list = tabs();
    setDropTarget(
      dropFromGap(
        list.map((session) => session.id),
        clampGapToKind(list, sessionId, gapAtX(tabBoxes(), event.clientX)),
        sessionId,
      ),
    );
  }

  function onTabPointerUp(sessionId: string, event: PointerEvent): void {
    if (!drag || drag.id !== sessionId || drag.pointerId !== event.pointerId) return;
    if (drag.started) {
      swallowClick = true;
      const fromId = drag.id;
      const list = tabs();
      const base = orderFromSessions(list, props.tabOrder);
      props.onReorderTabs(
        moveTabToGap(base, fromId, clampGapToKind(list, fromId, gapAtX(tabBoxes(), event.clientX))),
      );
      focusSession(fromId);
    }
    finishDrag();
  }

  function openSessionMenu(event: MouseEvent, session: Session, label: string): void {
    event.preventDefault();
    event.stopPropagation();
    setMenu({
      x: event.clientX,
      y: event.clientY,
      items: sessionMenuItems(session, label),
    });
  }

  return (
    <>
      <div class="tab-strip-host" data-tauri-drag-region="false">
        <div
          class="tab-strip-scroll"
          classList={{ reordering: dragging() != null }}
          data-fade={edgeFade()}
          data-tauri-drag-region="false"
          ref={scrollEl}
          onScroll={onScroll}
        >
          <div class="tab-strip" role="tablist" aria-label="Sessions">
            {/* Code leads so Option+1 reaches the files. Sessions follow it. */}
            <Show when={props.codeOpen}>
              <div
                class="session-tab-wrap code-tab-wrap"
                classList={{ active: props.codeActive }}
                style={{ "min-width": `${CODE_TAB_MIN_W}px`, "max-width": "200px" }}
              >
                <button
                  type="button"
                  role="tab"
                  class="session-tab"
                  aria-selected={props.codeActive}
                  aria-label={`Code, ${props.codeCount} open`}
                  onClick={props.onSelectCode}
                >
                  <Icon
                    name="file-code"
                    class={props.codeActive ? "forge-icon-accent" : "forge-icon-muted"}
                    size={14}
                  />
                  <span class="session-tab-label">Code</span>
                  <Show when={props.codeCount > 0}>
                    <span class="code-tab-count" aria-hidden="true">
                      {props.codeCount}
                    </span>
                  </Show>
                </button>
                <Show when={props.codeCount > 0}>
                  <IconButton
                    label="Close every open file"
                    hideTooltip
                    size="xs"
                    class="session-tab-close"
                    onClick={(event) => {
                      event.stopPropagation();
                      props.onCloseCode();
                    }}
                  >
                    <Icon name="close" class="forge-icon-faint" size={14} />
                  </IconButton>
                </Show>
              </div>
            </Show>
            <For
              each={tabs()}
              fallback={
                <Show when={!props.codeOpen}>
                  <span class="tab-empty">No sessions</span>
                </Show>
              }
            >
              {(session, index) => {
                const ground = () => groundOf(session);
                const label = () => sessionTabLabel(session, tabs());
                const emphasis = () =>
                  ground() === "current" || ground() === "needs-you" || hasQuestion(session.id)
                    ? "full"
                    : "dim";

                return (
                  <>
                    <Show when={index() === 0 && props.codeOpen}>
                      <span class="tab-divider" aria-hidden="true" />
                    </Show>
                    <SessionTab
                      session={session}
                      ground={ground()}
                      label={label()}
                      emphasis={emphasis()}
                      active={!props.codeActive && session.id === props.activeId}
                      waiting={ground() === "needs-you" || hasQuestion(session.id)}
                      dragging={dragging() === session.id}
                      dropBefore={dropTarget()?.id === session.id && !dropTarget()?.after}
                      dropAfter={dropTarget()?.id === session.id && dropTarget()?.after === true}
                      onSelect={() => {
                        if (swallowClick) {
                          swallowClick = false;
                          return;
                        }
                        // A session tab is a request for that terminal, so it
                        // takes the window back from Code as well as selecting.
                        focusSession(session.id);
                      }}
                      onClose={() => closeSessionFromUi(session, label())}
                      onPointerDown={(event) => onTabPointerDown(session.id, event)}
                      onPointerMove={(event) => onTabPointerMove(session.id, event)}
                      onPointerUp={(event) => onTabPointerUp(session.id, event)}
                      onPointerCancel={finishDrag}
                      onMenu={(event) => openSessionMenu(event, session, label())}
                    />
                  </>
                );
              }}
            </For>
          </div>
        </div>
        <div class="tab-strip-trailing">
          <SessionMenu />
        </div>
        <div class="tab-strip-spacer" data-tauri-drag-region />
        <Show when={overflowing()}>
          <IconButton
            label="Previous tab"
            size="sm"
            class="tab-step"
            onClick={() => scrollBy(-TAB_SCROLL_STEP)}
          >
            <Icon name="chevron-left" class="forge-icon-muted" />
          </IconButton>
          <IconButton
            label="Next tab"
            size="sm"
            class="tab-step"
            onClick={() => scrollBy(TAB_SCROLL_STEP)}
          >
            <Icon name="chevron-right" class="forge-icon-muted" />
          </IconButton>
        </Show>
      </div>
      <Show when={menu()}>
        {(open) => (
          <ContextMenu
            x={open().x}
            y={open().y}
            items={open().items}
            onDismiss={() => setMenu(null)}
          />
        )}
      </Show>
    </>
  );
}

type SessionTabProps = {
  session: Session;
  ground: TabGround;
  label: string;
  emphasis: "full" | "dim";
  active: boolean;
  /** An open question, including one on the tab on screen. */
  waiting: boolean;
  dragging: boolean;
  dropBefore: boolean;
  dropAfter: boolean;
  onSelect: () => void;
  onClose: () => void;
  onPointerDown: (event: PointerEvent) => void;
  onPointerMove: (event: PointerEvent) => void;
  onPointerUp: (event: PointerEvent) => void;
  onPointerCancel: () => void;
  onMenu: (event: MouseEvent) => void;
};

function SessionTab(props: SessionTabProps) {
  const hidesClose = () => props.ground !== "current";

  return (
    <div
      class={`session-tab-wrap ground-${props.ground}`}
      classList={{
        active: props.active,
        waiting: props.waiting,
        "hides-close": hidesClose(),
        dragging: props.dragging,
        "drop-before": props.dropBefore,
        "drop-after": props.dropAfter,
      }}
      style={{ "min-width": `${TAB_MIN_W}px`, "max-width": "200px" }}
      data-tauri-drag-region="false"
      onPointerDown={props.onPointerDown}
      onPointerMove={props.onPointerMove}
      onPointerUp={props.onPointerUp}
      onPointerCancel={props.onPointerCancel}
      onClick={(event) => {
        // Pointer capture retargets `click` onto this wrap, so the selection is
        // handled here; a keyboard press on the tab bubbles here the same way.
        if (event.target instanceof Element && event.target.closest(".session-tab-close")) return;
        props.onSelect();
      }}
      onContextMenu={props.onMenu}
    >
      <button
        type="button"
        role="tab"
        class="session-tab"
        aria-selected={props.active}
        aria-label={props.waiting ? `${props.label}, waiting on you` : props.label}
      >
        <Show when={props.waiting} fallback={<StateMarker session={props.session} />}>
          <AttentionMarker />
        </Show>
        <SessionGlyph
          providerId={props.session.agent_provider_id}
          session={props.session}
          emphasis={props.emphasis}
        />
        <span class="session-tab-label">{props.label}</span>
      </button>
      <IconButton
        label={`Close ${props.label}`}
        hideTooltip
        size="xs"
        class="session-tab-close"
        onClick={(event) => {
          event.stopPropagation();
          props.onClose();
        }}
      >
        <Icon name="close" class="forge-icon-faint" size={14} />
      </IconButton>
    </div>
  );
}
