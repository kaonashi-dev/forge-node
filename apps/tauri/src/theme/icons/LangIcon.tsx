import { themeBase } from "../ThemeProvider";
import { baseIsLight } from "../tokens";
import { langIconFor } from "./langIcons";

type LangIconProps = {
  /** A file name or a whole path — the mark is read off the last segment. */
  path: string;
  size?: number;
  class?: string;
  title?: string;
};

/**
 * The language mark on a file row.
 *
 * The one mark in the shell that keeps colours of its own. Everything else —
 * `Icon`, `BrandIcon` — is drawn in `currentColor` so it takes the colour of
 * the row it sits in; these are JetBrains' file-type icons, and their colour
 * *is* the information. That is why this is a `background-image` and not the
 * mask `BrandIcon` uses: a mask would paint them all one flat colour.
 *
 * They are vendored in two variants because the artwork carries its own fills,
 * so a light theme needs the set that was drawn for a pale ground rather than
 * a filter over the dark one.
 */
export function LangIcon(props: LangIconProps) {
  const size = () => props.size ?? 14;
  const variant = () => (baseIsLight(themeBase()) ? "light" : "dark");
  return (
    <span
      class={`forge-lang-icon ${props.class ?? ""}`}
      role={props.title ? "img" : undefined}
      aria-hidden={props.title ? undefined : true}
      aria-label={props.title}
      style={{
        width: `${size()}px`,
        height: `${size()}px`,
        "--icon-url": `url(/icons/lang/${variant()}/${langIconFor(props.path)}.svg)`,
      }}
    />
  );
}
