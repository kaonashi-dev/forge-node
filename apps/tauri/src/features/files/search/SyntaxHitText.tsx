import { For } from "solid-js";
import type { SyntaxHit, SyntaxToken } from "./searchSyntax";

function Tokens(props: { tokens: SyntaxToken[] }) {
  return (
    <For each={props.tokens}>
      {(token) =>
        token.scope ? (
          <span style={{ color: `var(--search-${token.scope})` }}>{token.text}</span>
        ) : (
          token.text
        )
      }
    </For>
  );
}

export function SyntaxHitText(props: { segments: SyntaxHit[] }) {
  return (
    <For each={props.segments}>
      {(segment) =>
        segment.hit ? (
          <mark class="search-hit">
            <Tokens tokens={segment.tokens} />
          </mark>
        ) : (
          <Tokens tokens={segment.tokens} />
        )
      }
    </For>
  );
}
