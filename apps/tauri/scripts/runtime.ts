import { engines } from "../package.json";

export function assertBun(): void {
  if (!Bun.semver.satisfies(process.versions.bun, engines.bun)) {
    throw new Error(`Forge tooling requires Bun ${engines.bun}; run bun upgrade`);
  }
}

if (import.meta.main) assertBun();
