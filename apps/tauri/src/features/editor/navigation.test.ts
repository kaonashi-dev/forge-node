import { afterEach, describe, expect, it } from "vitest";
import { invokeAction, resetActions } from "../../actions/dispatch";
import { bindEditorNavigation } from "./navigation";

describe("editor navigation actions", () => {
  afterEach(() => resetActions());

  it("answer only while bound, and the latest focus wins", () => {
    const sent: string[] = [];
    expect(invokeAction("go_to_definition")).toBe(false);
    const first = bindEditorNavigation((which) => sent.push(`first:${which}`));
    const second = bindEditorNavigation((which) => sent.push(`second:${which}`));
    invokeAction("go_to_definition");
    second();
    invokeAction("go_back");
    first();
    expect(invokeAction("go_back")).toBe(false);
    expect(sent).toEqual(["second:definition", "first:back"]);
  });
});
