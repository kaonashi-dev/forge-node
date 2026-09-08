import { AlertDialog as KobalteAlert } from "@kobalte/core/alert-dialog";
import { Dialog as Kobalte } from "@kobalte/core/dialog";
import { Show, splitProps, type JSX } from "solid-js";

/** Three widths, tokenised, instead of a `width: min(…)` per dialog. */
export type DialogSize = "sm" | "md" | "lg";

export type DialogProps = {
  /** Announced title. Rendered unless `hideTitle` is set. */
  title: string;
  hideTitle?: boolean;
  description?: string;
  children: JSX.Element;
  /** The row of buttons along the bottom, if there is one. */
  footer?: JSX.Element;
  size?: DialogSize;
  /**
   * Drops the body's padding and gap.
   *
   * The palette and the branch picker are full-bleed: their query row draws a
   * border across the whole card and their list runs to its edges. Padded, the
   * seam stops 24px short on either side and the card grows three 16px gaps it
   * was never designed around.
   */
  flush?: boolean;
  /** Extra class on the card, for the rare bespoke case. */
  class?: string;
  /** Where the card sits: `top` for pickers, `center` for confirmations. */
  align?: "top" | "center";
  open?: boolean;
  onDismiss: () => void;
};

const cardClass = (own: { size?: DialogSize; class?: string }) =>
  `forge-dialog-card forge-dialog forge-dialog-${own.size ?? "md"} ${own.class ?? ""}`;

/**
 * Every ordinary modal in the shell.
 *
 * The five dialogs this replaces each re-implemented the scrim, the Escape key
 * and the click-outside, invented their own width, and let the whole card grow
 * with their content. Kobalte traps focus and gives it back; the card here caps
 * its height and scrolls the body, sizes from a token, and animates in and out
 * off the `data-expanded`/`data-closed` Kobalte already emits.
 *
 * A destructive choice wants `AlertDialog` below, not this — it carries the
 * `alertdialog` role and does not dismiss on a click outside.
 */
export function Dialog(props: DialogProps) {
  const [own] = splitProps(props, [
    "title",
    "hideTitle",
    "description",
    "children",
    "footer",
    "size",
    "flush",
    "class",
    "align",
    "open",
    "onDismiss",
  ]);
  return (
    <Kobalte
      open={own.open ?? true}
      modal
      onOpenChange={(open) => {
        if (!open) own.onDismiss();
      }}
    >
      <Kobalte.Portal>
        <Kobalte.Overlay class="forge-scrim" />
        <div class="forge-dialog-positioner" data-align={own.align ?? "top"}>
          <Kobalte.Content class={cardClass(own)}>
            <Show
              when={!own.hideTitle}
              fallback={<Kobalte.Title class="forge-visually-hidden">{own.title}</Kobalte.Title>}
            >
              <div class="forge-dialog-header">
                <Kobalte.Title class="forge-dialog-title">{own.title}</Kobalte.Title>
              </div>
            </Show>
            <div class="forge-dialog-body" classList={{ "forge-dialog-body-flush": own.flush }}>
              <Show when={own.description}>
                {(text) => (
                  <Kobalte.Description class="forge-dialog-copy">{text()}</Kobalte.Description>
                )}
              </Show>
              {own.children}
            </div>
            <Show when={own.footer}>
              {(footer) => <div class="forge-dialog-footer">{footer()}</div>}
            </Show>
          </Kobalte.Content>
        </div>
      </Kobalte.Portal>
    </Kobalte>
  );
}

export type AlertDialogProps = {
  title: string;
  description?: string;
  children?: JSX.Element;
  footer: JSX.Element;
  size?: DialogSize;
  class?: string;
  open?: boolean;
  onDismiss: () => void;
};

/**
 * The destructive confirmation — removing a worktree, discarding a session.
 *
 * Kobalte's `AlertDialog` gives it the `alertdialog` role and holds it open on
 * a click outside, so a delete is confirmed or cancelled on purpose rather than
 * dismissed by a stray click. The card is the same one `Dialog` draws.
 */
export function AlertDialog(props: AlertDialogProps) {
  const [own] = splitProps(props, [
    "title",
    "description",
    "children",
    "footer",
    "size",
    "class",
    "open",
    "onDismiss",
  ]);
  return (
    <KobalteAlert
      open={own.open ?? true}
      modal
      onOpenChange={(open) => {
        if (!open) own.onDismiss();
      }}
    >
      <KobalteAlert.Portal>
        <KobalteAlert.Overlay class="forge-scrim" />
        <div class="forge-dialog-positioner" data-align="center">
          <KobalteAlert.Content class={cardClass({ size: own.size ?? "sm", class: own.class })}>
            <div class="forge-dialog-header">
              <KobalteAlert.Title class="forge-dialog-title">{own.title}</KobalteAlert.Title>
            </div>
            <Show when={own.description || own.children}>
              <div
                class="forge-dialog-body"
                classList={{
                  "forge-dialog-body-compact": Boolean(own.description) && !own.children,
                }}
              >
                <Show when={own.description}>
                  {(text) => (
                    <KobalteAlert.Description class="forge-dialog-copy">
                      {text()}
                    </KobalteAlert.Description>
                  )}
                </Show>
                <Show when={own.children}>{(body) => body()}</Show>
              </div>
            </Show>
            <div class="forge-dialog-footer">{own.footer}</div>
          </KobalteAlert.Content>
        </div>
      </KobalteAlert.Portal>
    </KobalteAlert>
  );
}
