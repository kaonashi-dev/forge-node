import { describe, expect, it } from "vitest";
import {
  attach,
  bytesToBase64,
  detach,
  insertMarkers,
  markerFor,
  MAX_SPEC_IMAGES,
  MAX_SPEC_IMAGE_BYTES,
  MAX_SPEC_IMAGES_TOTAL_BYTES,
  specImageMime,
  type SpecImage,
} from "./specImages";

function image(bytes: number, mime = "image/png"): SpecImage {
  return { mime, data: "AAAA", bytes };
}

describe("specImageMime", () => {
  it("takes what the daemon writes, and leaves everything else to the browser", () => {
    expect(specImageMime("image/PNG")).toBe("image/png");
    expect(specImageMime("image/jpeg")).toBe("image/jpeg");
    expect(specImageMime("image/tiff")).toBeNull();
    expect(specImageMime("text/plain")).toBeNull();
  });
});

describe("attach", () => {
  it("keeps the images that fit and reports the first that did not", () => {
    const full = Array.from({ length: MAX_SPEC_IMAGES }, () => image(1024));
    const { images, refused } = attach(full, [image(1024)]);
    expect(images).toHaveLength(MAX_SPEC_IMAGES);
    expect(refused).toContain(`at most ${MAX_SPEC_IMAGES}`);
  });

  it("refuses one image over the per-image limit without dropping the paste", () => {
    const { images, refused } = attach([], [image(MAX_SPEC_IMAGE_BYTES + 1), image(64)]);
    expect(images).toEqual([image(64)]);
    expect(refused).toContain("for one image");
  });

  it("counts what is already attached towards the total", () => {
    const third = MAX_SPEC_IMAGES_TOTAL_BYTES / 3;
    const attached = [image(third), image(third), image(third)];
    const { images, refused } = attach(attached, [image(1024)]);
    expect(images).toHaveLength(3);
    expect(refused).toContain("together");
  });

  it("refuses a type the daemon would refuse", () => {
    const { images, refused } = attach([], [image(64, "image/tiff")]);
    expect(images).toEqual([]);
    expect(refused).toContain("image/tiff");
  });
});

describe("insertMarkers", () => {
  it("writes the marker at the caret, spaced off the words around it", () => {
    const { text, caret } = insertMarkers("make it like this", 12, 12, 0, 1);
    expect(text).toBe("make it like [Image #1] this");
    expect(text.slice(caret)).toBe(" this");
  });

  it("replaces the selection and numbers a multi-image paste in order", () => {
    const { text } = insertMarkers("before HERE after", 7, 11, 1, 2);
    expect(text).toBe("before [Image #2] [Image #3] after");
  });

  it("leaves the caret after a trailing space at the end of the text", () => {
    const { text, caret } = insertMarkers("look at", 7, 7, 0, 1);
    expect(text).toBe("look at [Image #1] ");
    expect(caret).toBe(text.length);
  });
});

describe("detach", () => {
  // The marker is an index into the upload, so a removal in the middle is a
  // renumbering: the third image becomes the file the second marker names.
  it("drops the marker it removed and renumbers the ones after it", () => {
    const images = [image(1), image(2), image(3)];
    const text = `${markerFor(0)} first ${markerFor(1)} second ${markerFor(2)} third`;
    const dropped = detach(images, text, 1);
    expect(dropped.images).toEqual([image(1), image(3)]);
    expect(dropped.text).toBe("[Image #1] first second [Image #2] third");
  });

  it("leaves text with no markers alone", () => {
    const dropped = detach([image(1)], "no marker here", 0);
    expect(dropped.text).toBe("no marker here");
    expect(dropped.images).toEqual([]);
  });
});

describe("bytesToBase64", () => {
  it("encodes bytes above 0x7f without mangling them", () => {
    expect(bytesToBase64(new Uint8Array([0x89, 0x50, 0x4e, 0x47]))).toBe("iVBORw==");
  });

  it("encodes more bytes than fit in one argument list", () => {
    const big = new Uint8Array(0x8000 * 2 + 7).fill(0xff);
    expect(bytesToBase64(big)).toBe(Buffer.from(big).toString("base64"));
  });
});
