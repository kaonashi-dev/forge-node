import { expect, test } from "vitest";
import { assertBun } from "../scripts/runtime";

test("the Vitest worker runs the pinned Bun runtime", () => {
  expect(() => assertBun()).not.toThrow();
});
