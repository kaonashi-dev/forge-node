import { afterEach, describe, expect, it, vi } from "vitest";
import { createImageReader, imageSource } from "./previewImages";

describe("imageSource", () => {
  it("resolves a relative path against the document's directory", () => {
    expect(imageSource("./shots/a.png", "docs/plan.md")).toEqual({
      kind: "workspace",
      path: "docs/shots/a.png",
    });
    expect(imageSource("../assets/logo.svg", "docs/plan.md")).toEqual({
      kind: "workspace",
      path: "assets/logo.svg",
    });
    expect(imageSource("my%20shot.png?raw=true#top", "README.md")).toEqual({
      kind: "workspace",
      path: "my shot.png",
    });
  });

  it("reads a leading slash from the checkout root", () => {
    expect(imageSource("/assets/logo.png", "docs/deep/plan.md")).toEqual({
      kind: "workspace",
      path: "assets/logo.png",
    });
  });

  it("draws a web address or an inline image as it is", () => {
    expect(imageSource("https://example.com/a.png", "README.md")).toEqual({
      kind: "url",
      url: "https://example.com/a.png",
    });
    expect(imageSource("data:image/png;base64,iVBORw==", "README.md")).toEqual({
      kind: "url",
      url: "data:image/png;base64,iVBORw==",
    });
  });

  it("refuses other schemes and paths that leave the checkout", () => {
    expect(imageSource("javascript:alert(1)", "README.md")).toBeNull();
    expect(imageSource("file:///etc/passwd", "README.md")).toBeNull();
    expect(imageSource("//evil.example/a.png", "README.md")).toBeNull();
    expect(imageSource("data:text/html;base64,PHNjcmlwdD4=", "README.md")).toBeNull();
    expect(imageSource("../../outside.png", "docs/plan.md")).toBeNull();
    expect(imageSource("%E0%A4%A.png", "README.md")).toBeNull();
  });
});

describe("createImageReader", () => {
  afterEach(() => {
    vi.useRealTimers();
  });

  it("shares one read between two asks for the same path", async () => {
    const send = vi.fn(async () => undefined);
    const reader = createImageReader(send);
    const first = reader.read("w", "a.png");
    const second = reader.read("w", "a.png");
    reader.settle("w", "a.png", { url: "data:image/png;base64,AA==" });
    await expect(first).resolves.toBe("data:image/png;base64,AA==");
    await expect(second).resolves.toBe("data:image/png;base64,AA==");
    expect(send).toHaveBeenCalledTimes(1);
  });

  it("holds reads past the limit until one is answered", () => {
    const send = vi.fn(async () => undefined);
    const reader = createImageReader(send, { inFlight: 2, timeoutMs: 1_000 });
    for (const path of ["a.png", "b.png", "c.png"]) {
      reader.read("w", path).catch(() => undefined);
    }
    expect(send.mock.calls.map((call) => (call as unknown[])[1])).toEqual(["a.png", "b.png"]);

    // An answer for a read that was never sent is not a slot freed.
    reader.settle("w", "c.png", { url: "early" });
    expect(send).toHaveBeenCalledTimes(2);

    reader.settle("w", "a.png", { error: "gone" });
    expect(send.mock.calls.map((call) => (call as unknown[])[1])).toEqual([
      "a.png",
      "b.png",
      "c.png",
    ]);
  });

  it("gives up on a read the host never answers", async () => {
    vi.useFakeTimers();
    const reader = createImageReader(async () => undefined, { inFlight: 1, timeoutMs: 50 });
    const read = reader.read("w", "a.png");
    vi.advanceTimersByTime(50);
    await expect(read).rejects.toThrow("Timed out");
  });
});
