import { afterEach, expect, it, vi } from "vitest";
import { editorInputBatch } from "./editorInputBatch";

afterEach(() => vi.useRealTimers());

it("flushes accepted input to its original session when the surface is replaced", () => {
  vi.useFakeTimers();
  const send = vi.fn().mockResolvedValue(undefined);
  const first = editorInputBatch("A", send);
  first.push({ Text: "draft A" });
  const second = editorInputBatch("B", send);
  first.flush();
  second.push({ Text: "draft B" });
  vi.runAllTimers();
  expect(send.mock.calls).toEqual([
    ["A", [{ Text: "draft A" }]],
    ["B", [{ Text: "draft B" }]],
  ]);
  first.flush();
  second.flush();
  expect(send).toHaveBeenCalledTimes(2);
});
