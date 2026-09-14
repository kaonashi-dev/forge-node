# UI marks as URLs

`folder.svg` and `folder-open.svg`, traced from lucide `folder` / `folder-open`
(`lucide-solid`, ISC — the same set `src/theme/icons/forgeIcons.ts` renders as
Solid components). They exist as files because
`packages/file-workbench` may not import Solid: the explorer places a row mark
by URL, so a glyph it has to draw needs an address rather than a component.

They are chrome, not artwork, so they are drawn as a `mask-image` over
`currentColor` — `BrandIcon` and `.forge-brand-icon` use the same technique.
Stroke width is 1.5 to match `Icon`, which pins lucide's 2 down to the weight
of the rest of the shell.

`LICENSE` beside this file is lucide's, kept because ISC requires the notice to
travel with the artwork.
