import { describe, expect, it } from "vitest";
import { buildPathIndex } from "./pathref";
import { assembleLinks, linkAt } from "./screenLinks";

const ROOT = "/Users/kaonashi/hellopay/repos/hellopay-backend";
const WORKTREE = "/Users/kaonashi/hellopay/repos/hellopay-backend-portal-balance";

const paths = (text: string, root: string | null = ROOT, extras: string[] = []) =>
  assembleLinks(text, root, extras, null).flatMap((link) =>
    link.kind === "path" ? [link.path] : [],
  );

describe("assembleLinks", () => {
  it("opens the address Vite printed, and does not read the port as a line", () => {
    const text = "  ➜  Local:   http://localhost:5353/";
    const links = assembleLinks(text, ROOT, [], null);
    expect(links).toEqual([
      {
        kind: "url",
        from: text.indexOf("http"),
        to: text.length,
        url: "http://localhost:5353/",
      },
    ]);
    expect(linkAt(links, text.indexOf("http") + 4)?.kind).toBe("url");
    expect(linkAt(links, 0)).toBeNull();
  });

  it("opens a path named in prose, and not a type or a function beside it", () => {
    expect(paths("Agrupación (src/ledger/balance-by-method.ts): suma los buckets")).toEqual([
      "src/ledger/balance-by-method.ts",
    ]);
    expect(paths("Docs: actualicé docs/provider-balances.md con el nuevo contrato.")).toEqual([
      "docs/provider-balances.md",
    ]);
    expect(paths("Portal: un DTO nuevo, PortalBalanceResponseDto, agrega el campo.")).toEqual([]);
    expect(paths("1. DAO (src/ledger/daos/wallet-dao.service.ts)")).toEqual([
      "src/ledger/daos/wallet-dao.service.ts",
    ]);
    expect(paths("2. Agrupación (src/ledger/ledger.service.ts)")).toEqual([
      "src/ledger/ledger.service.ts",
    ]);
    expect(paths("Agregar findBucketsByOrganization(organizationId, tx?).")).toEqual([]);
    expect(paths("Una función pura exportada, groupBalanceByMethod(wallet,")).toEqual([]);
  });

  it("opens a ~/ path in the worktree it names, not the checkout beside it", () => {
    const text =
      "Update(~/hellopay/repos/hellopay-backend-portal-balance/test/provider-balances.postgres-spec.ts)";
    const links = assembleLinks(text, ROOT, [WORKTREE], null);
    expect(links).toEqual([
      {
        kind: "path",
        from: text.indexOf("~/"),
        to: text.length - 1,
        path: "test/provider-balances.postgres-spec.ts",
        line: null,
        checkout: WORKTREE,
      },
    ]);
  });

  it("keeps a slash path the listing has not seen yet", () => {
    const index = buildPathIndex([], true);
    const links = assembleLinks("wrote src/ledger/balance-by-method.ts", ROOT, [], index);
    expect(links.map((link) => (link.kind === "path" ? link.path : link.url))).toEqual([
      "src/ledger/balance-by-method.ts",
    ]);
  });

  it("still refuses to guess between two files with the same name", () => {
    const index = buildPathIndex([
      { path: "crates/daemon/src/core.rs", kind: "File", ignored: false },
      { path: "crates/client/src/core.rs", kind: "File", ignored: false },
    ]);
    expect(assembleLinks("see src/core.rs", ROOT, [], index)).toEqual([]);
  });
});
