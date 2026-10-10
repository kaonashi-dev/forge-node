import { For, createMemo } from "solid-js";
import { editorPalette } from "../../../theme/editorTheme";
import { themeBase } from "../../../theme/ThemeProvider";
import type { SyntaxToken } from "../../../shared/syntax/highlight";
import type { Segment } from "./patchDocument";
import { syntaxSegments } from "./patchSyntax";

export function patchSyntaxColors() {
  return createMemo(() =>
    Object.fromEntries(
      Object.entries(editorPalette(themeBase()).scopes).map(([scope, color]) => [
        `--diff-${scope}`,
        color,
      ]),
    ),
  );
}

function Tokens(props: { tokens: SyntaxToken[] }) {
  return (
    <For each={props.tokens}>
      {(token) =>
        token.scope ? (
          <span data-scope={token.scope} style={{ color: `var(--diff-${token.scope})` }}>
            {token.text}
          </span>
        ) : (
          token.text
        )
      }
    </For>
  );
}

export function PatchText(props: { tokens: SyntaxToken[]; mark?: Segment }) {
  const segments = createMemo(() => syntaxSegments(props.tokens, props.mark));
  return (
    <For each={segments()}>
      {(segment) =>
        segment.changed ? (
          <mark class="forge-diff-intra">
            <Tokens tokens={segment.tokens} />
          </mark>
        ) : (
          <Tokens tokens={segment.tokens} />
        )
      }
    </For>
  );
}
