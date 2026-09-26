// What a click on one line of output can open.
//
// Pure. URLs and paths are recognised apart and then joined, because a path
// token stops at `=` and `?` and would cut a query string in half.

import { findPathRefs, resolvePath, type PathIndex, type PathRef } from "./pathref";
import { findUrlRefs, type UrlRef } from "./urlref";

export type ScreenLink =
  | { kind: "url"; from: number; to: number; url: string }
  | {
      kind: "path";
      from: number;
      to: number;
      path: string;
      line: number | null;
      checkout: string | null;
    };

/**
 * The links on one line.
 *
 * A URL wins an overlap: `http://localhost:5353/` is an address, not a file
 * with a line number. A path the listing cannot place is dropped, except a
 * slash path it has not seen yet and a path that names another checkout.
 */
export function assembleLinks(
  text: string,
  root: string | null,
  extraRoots: readonly string[],
  index: PathIndex | null,
): ScreenLink[] {
  const urls = findUrlRefs(text);
  const links: ScreenLink[] = urls.map((url) => ({ kind: "url", ...url }));
  for (const ref of findPathRefs(text, root, extraRoots)) {
    if (overlaps(urls, ref.from, ref.to)) continue;
    if (ref.checkout !== null) {
      links.push(pathLink(ref));
      continue;
    }
    const path = resolvePath(ref.path, index);
    if (path === null) continue;
    links.push(pathLink(path === ref.path ? ref : { ...ref, path }));
  }
  return links;
}

/** The link covering a character offset. A URL wins when both cover it. */
export function linkAt(links: readonly ScreenLink[], index: number): ScreenLink | null {
  let path: ScreenLink | null = null;
  for (const link of links) {
    if (index < link.from || index >= link.to) continue;
    if (link.kind === "url") return link;
    path ??= link;
  }
  return path;
}

function pathLink(ref: PathRef): ScreenLink {
  return {
    kind: "path",
    from: ref.from,
    to: ref.to,
    path: ref.path,
    line: ref.line,
    checkout: ref.checkout,
  };
}

function overlaps(urls: readonly UrlRef[], from: number, to: number): boolean {
  return urls.some((url) => from < url.to && url.from < to);
}
