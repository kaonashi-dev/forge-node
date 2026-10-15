import { For, createMemo } from "solid-js";
import type { SyntaxToken } from "../../../shared/syntax/highlight";
import type { Segment } from "./patchDocument";
import { syntaxSegments } from "./patchSyntax";

function Tokens(props: { tokens: SyntaxToken[] }) {
  return (
    <For each={props.tokens}>
      {(token) => (token.scope ? <span data-syntax={token.scope}>{token.text}</span> : token.text)}
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
