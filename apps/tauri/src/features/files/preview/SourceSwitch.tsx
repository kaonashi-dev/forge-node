import { Show } from "solid-js";
import { open } from "../../../navigation/viewsStore";
import { Button } from "../../../ui/index";
import { openEditorSource } from "../../editor/open";
import { hasEditableSource } from "./previewRoute";

/**
 * The other surface for a file that is both a document and text.
 *
 * Hidden for a raster: there is no source the editor will open.
 */
export function SourceSwitch(props: { path: string; surface: "preview" | "editor" }) {
  return (
    <Show when={hasEditableSource(props.path)}>
      <Button
        size="xs"
        variant="secondary"
        onClick={() => {
          if (props.surface === "preview") openEditorSource(props.path);
          else open({ kind: "preview", path: props.path });
        }}
      >
        {props.surface === "preview" ? "Edit" : "Preview"}
      </Button>
    </Show>
  );
}
