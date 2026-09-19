import { describe, expect, it } from "vitest";
import { sessionGlyphFace, type SessionGlyphContext } from "./glyphFace";

function ctx(partial: Partial<SessionGlyphContext> = {}): SessionGlyphContext {
  return { active: false, isAgent: true, unread: false, ...partial };
}

describe("sessionGlyphFace", () => {
  it("spins only for background work the user has not seen", () => {
    expect(sessionGlyphFace("starting", ctx())).toBe("working");
    expect(sessionGlyphFace("working", ctx({ unread: true }))).toBe("working");
  });

  it("does not spin for the session the user is in, even if the clock is fresh", () => {
    expect(sessionGlyphFace("working", ctx({ active: true, unread: true }))).toBe("identity");
    expect(sessionGlyphFace("starting", ctx({ active: true }))).toBe("identity");
    expect(sessionGlyphFace("idle", ctx({ active: true }))).toBe("identity");
  });

  it("does not spin for leftover activity after the user already looked", () => {
    expect(sessionGlyphFace("working", ctx({ unread: false }))).toBe("identity");
  });

  it("checks a finished agent the user is not in", () => {
    expect(sessionGlyphFace("idle", ctx())).toBe("done");
    expect(sessionGlyphFace("exited", ctx())).toBe("done");
  });

  it("never treats a shell as finished work", () => {
    expect(sessionGlyphFace("running", ctx({ isAgent: false }))).toBe("identity");
    expect(sessionGlyphFace("exited", ctx({ isAgent: false }))).toBe("identity");
    expect(sessionGlyphFace("running", ctx({ active: true, isAgent: false }))).toBe("identity");
  });

  it("leaves needs-you and failed on identity so the marker can speak", () => {
    expect(sessionGlyphFace("needs-you", ctx({ unread: true }))).toBe("identity");
    expect(sessionGlyphFace("failed", ctx())).toBe("identity");
  });

  it("stays identity when there is no work to show", () => {
    expect(sessionGlyphFace(undefined, ctx())).toBe("identity");
  });
});
