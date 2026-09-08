import { For, Show, createEffect, createSignal, onCleanup, onMount } from "solid-js";
import { focusSession } from "./sessionActions";
import { sessionTabLabel, tabGround, type TabGround } from "../runtime/attention";
import type { Session } from "../runtime/types";
import { AttentionMarker, Icon, SessionGlyph, StateMarker } from "../theme/icons";
import { ContextMenu, IconButton, type MenuItem } from "../ui";
import { SessionMenu } from "./SessionMenu";
import { closeSessionFromUi, sessionMenuItems } from "./sessionMenuItems";
import { dropFromGap, gapAtX, moveTabToGap, orderFromSessions } from "./tabOrder";
import type { TextInputRequest } from "./TextInputDialog";

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
  /** Whether the Code tab exists — it does once something is open in it. */
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
  const [scrolledLeft, setScrolledLeft] = createSignal(false);
  const [scrolledRight, setScrolledRight] = createSignal(false);
  const [overflowing, setOverflowing] = createSignal(false);
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
  const groundOf = (session: Session) =>
    tabGround(session, props.codeActive ? null : props.activeId);

  /* Measured, not counted. Two tabs in a wide window overflow nothing, and a
     count-based guess left the edge mask permanently dimming the last tab and
     kept the scroll chevrons on screen with nowhere to scroll. */
  const onScroll = () => {
    if (!scrollEl) {
      return;
    }
    const slack = scrollEl.scrollWidth - scrollEl.clientWidth;
    const over = slack > 1;
    setOverflowing(over);
    setScrolledLeft(over && scrollEl.scrollLeft > 1);
    setScrolledRight(over && scrollEl.scrollLeft < slack - 1);
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
    setDropTarget(
      dropFromGap(
        tabs().map((session) => session.id),
        gapAtX(tabBoxes(), event.clientX),
        sessionId,
      ),
    );
  }

  function onTabPointerUp(sessionId: string, event: PointerEvent): void {
    if (!drag || drag.id !== sessionId || drag.pointerId !== event.pointerId) return;
    if (drag.started) {
      swallowClick = true;
      const fromId = drag.id;
      const base = orderFromSessions(tabs(), props.tabOrder);
      props.onReorderTabs(moveTabToGap(base, fromId, gapAtX(tabBoxes(), event.clientX)));
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
          classList={{
            "fade-right": scrolledRight(),
            "fade-left": scrolledLeft(),
            reordering: dragging() != null,
          }}
          data-tauri-drag-region="false"
          ref={scrollEl}
          onScroll={onScroll}
        >
          <div class="tab-strip" role="tablist" aria-label="Sessions">
            <For each={tabs()} fallback={<span class="tab-empty">No sessions</span>}>
              {(session, index) => {
                const ground = () => groundOf(session);
                const prev = () => {
                  const list = tabs();
                  const i = index();
                  return i > 0 ? groundOf(list[i - 1]) : null;
                };
                const showDivider = () => prev() === "quiet" && ground() === "quiet" && index() > 0;
                const label = () => sessionTabLabel(session, tabs());
                const emphasis = () =>
                  ground() === "current" || ground() === "needs-you" ? "full" : "dim";

                return (
                  <>
                    <Show when={showDivider()}>
                      <span class="tab-divider" aria-hidden="true" />
                    </Show>
                    <SessionTab
                      session={session}
                      ground={ground()}
                      label={label()}
                      emphasis={emphasis()}
                      active={!props.codeActive && session.id === props.activeId}
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
            {/* Files and diffs get one window tab of their own rather than a
                strip stacked on top of a terminal: the terminal keeps its own
                tab, and opening a file no longer covers whichever session the
                person was watching. */}
            <Show when={props.codeOpen}>
              <div class="session-tab-wrap code-tab-wrap">
                <div
                  role="tab"
                  class={`session-tab ${props.codeActive ? "ground-current" : "ground-quiet"}`}
                  aria-selected={props.codeActive}
                  aria-label={`Code — ${props.codeCount} open`}
                  style={{ "min-width": `${CODE_TAB_MIN_W}px`, "max-width": "200px" }}
                  onClick={props.onSelectCode}
                  onKeyDown={(event) => {
                    if (event.key === "Enter" || event.key === " ") {
                      event.preventDefault();
                      props.onSelectCode();
                    }
                  }}
                >
                  <Icon
                    name="folder-open"
                    class={props.codeActive ? "forge-icon-accent" : "forge-icon-muted"}
                    size={14}
                  />
                  <span class="session-tab-label">Code</span>
                  <span class="code-tab-count">{props.codeCount}</span>
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
                </div>
              </div>
            </Show>
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
      class="session-tab-wrap"
      classList={{
        "hides-close": hidesClose(),
        dragging: props.dragging,
        "drop-before": props.dropBefore,
        "drop-after": props.dropAfter,
      }}
      data-tauri-drag-region="false"
      onPointerDown={props.onPointerDown}
      onPointerMove={props.onPointerMove}
      onPointerUp={props.onPointerUp}
      onPointerCancel={props.onPointerCancel}
      onClick={(event) => {
        // The only click handler of the tab: pointer capture retargets `click`
        // onto this wrap, and a click that is not retargeted bubbles here from
        // the inner tab anyway. The close button stops the bubble itself.
        if (event.target instanceof Element && event.target.closest(".session-tab-close")) return;
        props.onSelect();
      }}
      onContextMenu={props.onMenu}
    >
      <div
        role="tab"
        class={`session-tab ground-${props.ground}`}
        aria-selected={props.active}
        style={{
          "min-width": `${TAB_MIN_W}px`,
          "max-width": "200px",
        }}
        onKeyDown={(event) => {
          if (event.key === "Enter" || event.key === " ") {
            event.preventDefault();
            props.onSelect();
          }
        }}
      >
        <Show
          when={props.ground === "needs-you"}
          fallback={<StateMarker session={props.session} />}
        >
          <AttentionMarker />
        </Show>
        <SessionGlyph
          providerId={props.session.agent_provider_id}
          session={props.session}
          emphasis={props.emphasis}
        />
        <span class="session-tab-label">{props.label}</span>
        <IconButton
          label="Close session"
          hideTooltip
          class="session-tab-close"
          onClick={(event) => {
            event.stopPropagation();
            props.onClose();
          }}
        >
          <Icon name="close" class="forge-icon-faint" size={14} />
        </IconButton>
      </div>
    </div>
  );
}
