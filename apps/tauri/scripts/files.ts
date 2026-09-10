export async function readBytes(path: string | URL, cap: number): Promise<Uint8Array<ArrayBuffer>> {
  const file = Bun.file(path);
  if (file.size > cap) throw new Error(`File exceeds ${cap} bytes: ${path}`);
  // Slicing also bounds a file that grows between the size check and the read.
  const bytes = await file.slice(0, cap + 1).bytes();
  if (bytes.length > cap) throw new Error(`File exceeds ${cap} bytes: ${path}`);
  return bytes;
}

export async function readJson(path: string | URL, cap: number): Promise<unknown> {
  return JSON.parse(new TextDecoder().decode(await readBytes(path, cap)));
}
