/**
 * The six marks that are logotypes, not UI icons.
 *
 * lucide has no Claude, Codex, opencode, Cursor, Grok or Zed glyph, and it should not
 * — these are brands, drawn from their own SVG. The technique is the mask-tint
 * the whole icon set used before lucide: the SVG is a `mask-image` and the fill
 * is `currentColor`, so a mark takes the colour of the row it sits in and no
 * provider carries a hue of its own. That last part is the invariant the
 * history list, the tab strip and the rail all depend on: a provider is a
 * choice the reader made, and colouring it would rank the providers.
 *
 * `LangIcon` is the one mark that does not do this, and the reason is the same
 * rule read the other way — a file type is not a choice, and its colour is
 * what makes a tree scannable.
 */
export type BrandName = "claude" | "codex" | "opencode" | "cursor" | "grok" | "editor-zed";

const BRAND_SVG: Record<BrandName, string> = {
  claude: "provider-claude.svg",
  codex: "provider-codex.svg",
  opencode: "provider-opencode.svg",
  cursor: "provider-cursor.svg",
  grok: "provider-grok.svg",
  "editor-zed": "editor-zed.svg",
};

/** The brand for a provider id, or `null` for one with no logo of its own. */
export function brandForProvider(providerId: string): BrandName | null {
  switch (providerId) {
    case "claude":
    case "codex":
    case "opencode":
    case "cursor":
    case "grok":
      return providerId;
    default:
      return null;
  }
}

type BrandIconProps = {
  brand: BrandName;
  size?: number;
  class?: string;
  title?: string;
};

export function BrandIcon(props: BrandIconProps) {
  const size = () => props.size ?? 16;
  return (
    <span
      class={`forge-brand-icon ${props.class ?? ""}`}
      role={props.title ? "img" : undefined}
      aria-hidden={props.title ? undefined : true}
      aria-label={props.title}
      style={{
        width: `${size()}px`,
        height: `${size()}px`,
        "--icon-url": `url(/icons/${BRAND_SVG[props.brand]})`,
      }}
    />
  );
}
