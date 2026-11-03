import { describe, expect, it } from "vitest";
import type { ReviewSettings } from "../../contracts/workbench";
import { initialPreset, presetKey } from "./reviewPreset";

const settings: ReviewSettings = {
  default_preset: "negocio",
  presets: [
    { id: "estandar", name: "Estándar", tier: null },
    { id: "negocio", name: "Reglas de negocio", tier: "plus" },
  ],
  rules: [],
  errors: [],
};

describe("reviewPreset", () => {
  it("keys the remembered preset per project", () => {
    expect(presetKey("p1")).toBe("ui.review.preset.p1");
  });

  it("starts from the remembered preset when it is still listed", () => {
    expect(initialPreset(settings, "estandar")).toBe("estandar");
  });

  it("falls back to the default for an unlisted or missing remembered preset", () => {
    expect(initialPreset(settings, "gone")).toBe("negocio");
    expect(initialPreset(settings, undefined)).toBe("negocio");
  });
});
