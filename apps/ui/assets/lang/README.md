# Language marks

The file-type icons on file tree rows, editor tabs and quick-open rows. Which name a
file gets is decided by `src/file_icons.rs`, whose tests fail if a name there has no file
here or a file here has no name.

## Where they came from

Vendored from [ziishaned/zed-jetbrains-icons](https://github.com/ziishaned/zed-jetbrains-icons)
(Apache-2.0), which packages the IntelliJ platform icons as a Zed icon theme.
`LICENSE` beside this file is that project's licence, kept because Apache-2.0
§4 requires it to travel with the artwork.

Most of the SVGs carry `Copyright 2000-2024 JetBrains s.r.o. and contributors.
Use of this source code is governed by the Apache 2.0 license.` in a comment at
the top. **Do not strip those comments** — retaining them is the other half of
§4. About a third of the files carry no such header: they are third-party
project logos the upstream author added (Go, Rust, Swift, Vue, Next.js, PHP,
Dart, Deno, Elixir, GraphQL, Prisma, Sass, Astro, Haskell, Nix, HCL, C, C++).
Those marks belong to their respective projects; Apache-2.0 §6 does not license
trademarks, and using them as file-type marks is the same customary use every
editor makes of them.

Nothing here is edited. The upstream `crystal` icon is missing from that
repository and is therefore absent; `zig` points at a file that does not exist
there either, so Zig falls back to the plain sheet.

## Two variants

`dark/` and `light/` differ only in the fill values — the upstream set is drawn
twice, once for each IntelliJ ground. The client picks the set from whether the
theme's kit is dark, so a new light theme needs no work here.

## Adding a mark

1. Copy `<name>.svg` into **both** `dark/` and `light/`, header intact.
2. Add `"<name>"` to `lang_icons!` and the extension or file name to one of the
   tables in `src/file_icons.rs`.
3. `scripts/dev ui check` — the vendored-set test is what proves the two agree.
