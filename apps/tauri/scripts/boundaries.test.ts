import { expect, test } from "bun:test";
import { cycles, graphOf, violationsOf, type Source } from "./boundaries";

const files = (entries: Record<string, string>): Source[] =>
  Object.entries(entries).map(([path, source]) => ({ path, source }));

test("a foundation cannot import a feature", () => {
  const violations = violationsOf(
    files({
      "contracts/runtime.ts": `import { x } from "../features/files/state";`,
      "features/files/state.ts": "export const x = 1;",
    }),
  );
  expect(violations).toHaveLength(1);
  expect(violations[0]).toContain("contracts/ may not import features/files");
});

test("a re-export is an edge like any import", () => {
  const violations = violationsOf(
    files({
      "navigation/views.ts": `export { open } from "../features/editor/open";`,
      "features/editor/open.ts": "export const open = 1;",
    }),
  );
  expect(violations).toHaveLength(1);
  expect(violations[0]).toContain("navigation/ may not import features/editor");
});

test("a side-effect import is checked too", () => {
  const violations = violationsOf(
    files({
      "state/workspace.ts": `import "../features/files/state";`,
      "features/files/state.ts": "export const x = 1;",
    }),
  );
  expect(violations).toHaveLength(1);
});

test("a features-to-app import fails while app reaches everything", () => {
  const upward = violationsOf(
    files({
      "features/files/state.ts": `import { app } from "../../app/integrations/workspaceFocus";`,
      "app/integrations/workspaceFocus.ts": "export const app = 1;",
    }),
  );
  expect(upward).toHaveLength(1);
  expect(upward[0]).toContain("files may not import app");
  expect(
    violationsOf(
      files({
        "app/shell/AppShell.tsx": `import { x } from "../../features/files/state";`,
        "features/files/state.ts": "export const x = 1;",
      }),
    ),
  ).toEqual([]);
});

test("a listed peer feature edge is allowed and an unlisted one is not", () => {
  expect(
    violationsOf(
      files({
        "features/git/GitPanel.tsx": `import { PrComposeView } from "../pull-requests/PrComposeView";`,
        "features/pull-requests/PrComposeView.tsx": "export const PrComposeView = 1;",
      }),
    ),
  ).toEqual([]);
  const violations = violationsOf(
    files({
      "features/git/GitPanel.tsx": `import { x } from "../terminal/TerminalPane";`,
      "features/terminal/TerminalPane.tsx": "export const x = 1;",
    }),
  );
  expect(violations).toHaveLength(1);
  expect(violations[0]).toContain("git may not import features/terminal");
});

test("a literal dynamic import resolves and is checked", () => {
  const violations = violationsOf(
    files({
      "theme/ThemeProvider.tsx": `const mod = await import("../features/sessions/markers");`,
      "features/sessions/markers.tsx": "export const Marker = 1;",
    }),
  );
  expect(violations).toHaveLength(1);
  expect(violations[0]).toContain("theme/ may not import features/sessions");
});

test("a type-only edge does not make a fake runtime cycle", () => {
  const sources = files({
    "contracts/runtime.ts": `import type { Session } from "./session";\nexport type { Session };`,
    "contracts/session.ts": `import type { Editor } from "./runtime";\nexport type { Editor };`,
  });
  expect(cycles(sources)).toEqual([]);
  expect(graphOf(sources).edges.every((edge) => edge.kind === "type")).toBe(true);
});

test("a runtime cycle is reported", () => {
  const components = cycles(
    files({
      "features/files/state.ts": `import { g } from "../git/state";\nexport const f = 1;`,
      "features/git/state.ts": `import { f } from "../files/state";\nexport const g = 1;`,
    }),
  );
  expect(components).toHaveLength(1);
  expect(components[0].sort()).toEqual(["features/files/state.ts", "features/git/state.ts"]);
});

test("tests and generated fixtures are outside the production graph", () => {
  const violations = violationsOf(
    files({
      "features/files/state.test.ts": `import { x } from "../../app/shell/AppShell";`,
      "contracts/generated/fixtures.ts": `import { x } from "../../features/files/state";`,
      "app/shell/AppShell.tsx": "export const x = 1;",
      "features/files/state.ts": "export const x = 1;",
    }),
  );
  expect(violations).toEqual([]);
});

test("an unresolved specifier is not guessed at", () => {
  expect(
    violationsOf(
      files({
        "theme/ThemeProvider.tsx": `const mod = import("../features/nope/nothing");`,
      }),
    ),
  ).toEqual([]);
});
