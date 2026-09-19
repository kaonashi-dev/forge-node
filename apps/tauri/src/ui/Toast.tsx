import { Toast as Kobalte, toaster } from "@kobalte/core/toast";
import { Portal } from "solid-js/web";
import { Show } from "solid-js";
import { Icon } from "../theme/icons/index";
import type { ForgeIconName } from "../theme/icons/index";

export type ToastTone = "info" | "success" | "danger";

export type ToastRequest = {
  title: string;
  /** One line of detail. A toast is a receipt, not a report. */
  detail?: string;
  tone?: ToastTone;
  /** Milliseconds on screen. Ignored for `danger`, which waits to be read. */
  duration?: number;
};

const TONE_ICON: Record<ToastTone, ForgeIconName> = {
  info: "agent",
  success: "check",
  danger: "close",
};

/**
 * How long a toast stays.
 *
 * Long enough to read a short sentence at a glance while doing something else,
 * short enough that four in a row do not stack into a wall. A failure gets no
 * timer at all: something did not happen, and the notice has to survive the
 * moment someone was looking elsewhere.
 */
const DEFAULT_MS = 4_000;

/** Background outcomes only; requests requiring an answer belong in a dialog. */
export function toast(request: ToastRequest): void {
  const tone = request.tone ?? "info";
  toaster.show((props) => (
    <Kobalte
      toastId={props.toastId}
      class={`forge-toast forge-toast-${tone}`}
      duration={tone === "danger" ? Number.POSITIVE_INFINITY : (request.duration ?? DEFAULT_MS)}
    >
      <Icon name={TONE_ICON[tone]} size={14} class="forge-toast-icon" />
      <div class="forge-toast-text">
        <Kobalte.Title class="forge-toast-title">{request.title}</Kobalte.Title>
        <Show when={request.detail}>
          {(text) => <Kobalte.Description class="forge-toast-detail">{text()}</Kobalte.Description>}
        </Show>
      </div>
      <Kobalte.CloseButton class="forge-toast-close" aria-label="Dismiss">
        <Icon name="close" size={12} />
      </Kobalte.CloseButton>
    </Kobalte>
  ));
}

/**
 * The region toasts appear in. Mounted once, by the shell.
 *
 * Bottom-right: top-centre would sit over the title bar and the tab strip,
 * which is where the pointer is most of the time.
 */
export function ToastRegion() {
  return (
    <Portal>
      <Kobalte.Region aria-label="Notifications">
        <Kobalte.List class="forge-toast-list" />
      </Kobalte.Region>
    </Portal>
  );
}
