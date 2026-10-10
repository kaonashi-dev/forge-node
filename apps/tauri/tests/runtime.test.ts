import { expect, test } from "vitest";
import { assertBun } from "../scripts/runtime";

test("the Vitest worker runs a supported Bun runtime", () => {
  expect(() => assertBun()).not.toThrow();
});
