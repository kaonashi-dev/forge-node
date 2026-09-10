import { engines } from "../package.json";

export function assertBun(): void {
  if (process.versions.bun !== engines.bun) {
    throw new Error(
      `Forge tooling requires Bun ${engines.bun}; run bun upgrade --version ${engines.bun}`,
    );
  }
}

if (import.meta.main) assertBun();
