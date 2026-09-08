import { runtimeStore, setRuntimeStore, type ConfirmRequest } from "../store/runtimeStore";
import { AlertDialog, Button } from "../ui";

/**
 * The one confirmation the shell asks any destructive question through.
 *
 * Mounted once by `AppShell` and driven from `runtimeStore.confirm`, so a call
 * site anywhere — a menu item, a keyboard action, a panel button — asks with
 * `requestConfirm({...})` and never has to own a dialog, a signal or a prop
 * path to reach one.
 */
export function ConfirmDialog(props: { request: ConfirmRequest }) {
  function dismiss(): void {
    setRuntimeStore("confirm", null);
  }

  function go(): void {
    // Read before dismissing: `AppShell` renders this from a non-keyed `Show`,
    // whose accessor throws a stale read the moment the condition goes falsy —
    // the same trap `RemoveWorktreeDialog` documents.
    const run = props.request.onConfirm;
    dismiss();
    run();
  }

  return (
    <AlertDialog
      title={props.request.title}
      description={props.request.description}
      size="sm"
      open={runtimeStore.confirm !== null}
      onDismiss={dismiss}
      footer={
        <>
          <Button variant="secondary" onClick={dismiss}>
            Cancel
          </Button>
          <Button variant={props.request.destructive ? "danger" : "primary"} onClick={go}>
            {props.request.confirmLabel}
          </Button>
        </>
      }
    />
  );
}
