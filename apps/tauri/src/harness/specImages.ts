/**
 * Images pasted into the spec compose box.
 *
 * The daemon takes bytes and writes them beside the spec; the box keeps a
 * marker in the text for each one, because a CLI agent reads a path and never
 * a clipboard. Everything with a right and a wrong answer lives here so a test
 * can import it without Solid's server build.
 *
 * The limits mirror `domain::MAX_SPEC_IMAGE*`. The daemon refuses the same
 * paste again — this is only what keeps the refusal out of a round trip.
 */

export const MAX_SPEC_IMAGES = 4;
export const MAX_SPEC_IMAGE_BYTES = 5 * 1024 * 1024;
export const MAX_SPEC_IMAGES_TOTAL_BYTES = 12 * 1024 * 1024;

/** One attached image, as the box holds it and as it crosses the bridge. */
export type SpecImage = {
  mime: string;
  /** Standard base64: what the host decodes, and what the thumbnail reads. */
  data: string;
  /** Decoded size, for the chip and for the limits. */
  bytes: number;
};

/** The MIME the daemon accepts for `type`, or `null` to leave the paste alone. */
export function specImageMime(type: string): string | null {
  const mime = type.toLowerCase();
  return mime === "image/png" ||
    mime === "image/jpeg" ||
    mime === "image/webp" ||
    mime === "image/gif"
    ? mime
    : null;
}

/** What the spec text calls the image at `index` — `harness_runner` resolves it. */
export function markerFor(index: number): string {
  return `[Image #${index + 1}]`;
}

/** A `src` for the chip, without a second copy of the bytes to revoke. */
export function thumbnailUrl(image: SpecImage): string {
  return `data:${image.mime};base64,${image.data}`;
}

export function readableSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * Take what fits, and say what did not.
 *
 * Partial rather than all-or-nothing: a fifth screenshot is a mistake about
 * one image, and throwing away the four that were fine helps with neither.
 */
export function attach(
  current: readonly SpecImage[],
  incoming: readonly SpecImage[],
): { images: SpecImage[]; refused: string | null } {
  const images = [...current];
  let total = images.reduce((sum, image) => sum + image.bytes, 0);
  let refused: string | null = null;
  const refuse = (reason: string) => {
    refused ??= reason;
  };
  for (const image of incoming) {
    if (specImageMime(image.mime) === null) {
      refuse(`${image.mime} is not an image the spec takes: PNG, JPEG, WebP or GIF.`);
    } else if (images.length >= MAX_SPEC_IMAGES) {
      refuse(`A spec carries at most ${MAX_SPEC_IMAGES} images.`);
    } else if (image.bytes > MAX_SPEC_IMAGE_BYTES) {
      refuse(
        `${readableSize(image.bytes)} is over the ${readableSize(MAX_SPEC_IMAGE_BYTES)} limit for one image.`,
      );
    } else if (total + image.bytes > MAX_SPEC_IMAGES_TOTAL_BYTES) {
      refuse(
        `The images together are over the ${readableSize(MAX_SPEC_IMAGES_TOTAL_BYTES)} limit.`,
      );
    } else {
      images.push(image);
      total += image.bytes;
    }
  }
  return { images, refused };
}

/** A marker and the single space a paste leaves after it. */
const MARKER = /\[Image #(\d+)\]( ?)/g;

/**
 * Drop one image and renumber the markers around it.
 *
 * The marker is an index, so removing the first image moves every later one:
 * left alone, `[Image #3]` would point at the file of the image below it.
 */
export function detach(
  images: readonly SpecImage[],
  text: string,
  index: number,
): { images: SpecImage[]; text: string } {
  const rewritten = text.replace(MARKER, (_match, digits: string, space: string) => {
    const marked = Number(digits) - 1;
    if (marked === index) return "";
    return `${markerFor(marked > index ? marked - 1 : marked)}${space}`;
  });
  return { images: images.filter((_image, at) => at !== index), text: rewritten };
}

/**
 * Write the markers for `count` images at the caret.
 *
 * The paste lands where the pointer was — a screenshot pasted mid-sentence is
 * being talked about there — with a space on either side so it never welds
 * itself to a word.
 */
export function insertMarkers(
  text: string,
  start: number,
  end: number,
  from: number,
  count: number,
): { text: string; caret: number } {
  const markers = Array.from({ length: count }, (_unused, at) => markerFor(from + at)).join(" ");
  const before = text.slice(0, start);
  const after = text.slice(end);
  const lead = before === "" || /\s$/.test(before) ? "" : " ";
  const tail = after === "" || !/^\s/.test(after) ? " " : "";
  return {
    text: `${before}${lead}${markers}${tail}${after}`,
    caret: start + lead.length + markers.length + tail.length,
  };
}

/** Chunked because a megabyte of arguments overflows the call stack. */
const CHUNK = 0x8000;

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let at = 0; at < bytes.length; at += CHUNK) {
    binary += String.fromCharCode(...bytes.subarray(at, at + CHUNK));
  }
  return btoa(binary);
}
