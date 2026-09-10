import { afterEach, expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { readBytes, readJson } from "./files";

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(
    directories.splice(0).map((path) => rm(path, { recursive: true, force: true })),
  );
});

async function fixture(contents: string): Promise<string> {
  const dir = await mkdtemp(join(tmpdir(), "forge-tooling-"));
  directories.push(dir);
  const path = join(dir, "fixture.json");
  await Bun.write(path, contents);
  return path;
}

test("the limit counts bytes rather than Unicode characters", async () => {
  const path = await fixture("éé");
  expect((await readBytes(path, 4)).length).toBe(4);
  await expect(readBytes(path, 3)).rejects.toThrow("exceeds 3 bytes");
});

test("JSON reads reject invalid and oversized input", async () => {
  const path = await fixture('{"ok":true}');
  expect(await readJson(path, 11)).toEqual({ ok: true });
  await expect(readJson(path, 10)).rejects.toThrow("exceeds");
  await Bun.write(path, "{");
  await expect(readJson(path, 11)).rejects.toThrow();
});
