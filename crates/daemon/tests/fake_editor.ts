// A stand-in `forge-editor` for `scenario_editor.rs`: it speaks the control
// protocol over the daemon's socket and acts out one scripted mode. The test
// writes it with an absolute Bun shebang, because the harness PATH holds only
// its own bin directory.
import { existsSync, writeFileSync } from "node:fs";
import { connect, type Socket } from "node:net";
import { basename } from "node:path";

type Value = null | boolean | number | string | Value[] | { [key: string]: Value };
type Fields = { [key: string]: Value };

const CONTROL_VERSION = Number("__CONTROL_VERSION__");

function packInt(n: number): Buffer {
  if (n >= 0 && n <= 0x7f) return Buffer.from([n]);
  if (n <= 0xff) return Buffer.from([0xcc, n]);
  if (n <= 0xffff) {
    const out = Buffer.alloc(3);
    out[0] = 0xcd;
    out.writeUInt16BE(n, 1);
    return out;
  }
  if (n <= 0xffffffff) {
    const out = Buffer.alloc(5);
    out[0] = 0xce;
    out.writeUInt32BE(n, 1);
    return out;
  }
  const out = Buffer.alloc(9);
  out[0] = 0xcf;
  out.writeBigUInt64BE(BigInt(n), 1);
  return out;
}

function packStr(s: string): Buffer {
  const bytes = Buffer.from(s, "utf8");
  const n = bytes.length;
  if (n < 32) return Buffer.concat([Buffer.from([0xa0 | n]), bytes]);
  if (n < 256) return Buffer.concat([Buffer.from([0xd9, n]), bytes]);
  const header = Buffer.alloc(3);
  header[0] = 0xda;
  header.writeUInt16BE(n, 1);
  return Buffer.concat([header, bytes]);
}

const packBool = (v: boolean): Buffer => Buffer.from([v ? 0xc3 : 0xc2]);
const packNil = (): Buffer => Buffer.from([0xc0]);

function sized(small: number, wide: number, n: number): Buffer {
  if (n < 16) return Buffer.from([small | n]);
  const header = Buffer.alloc(3);
  header[0] = wide;
  header.writeUInt16BE(n, 1);
  return header;
}

const packArray = (items: Buffer[]): Buffer =>
  Buffer.concat([sized(0x90, 0xdc, items.length), ...items]);

const packMap = (items: [string, Buffer][]): Buffer =>
  Buffer.concat([
    sized(0x80, 0xde, items.length),
    ...items.flatMap(([key, value]) => [packStr(key), value]),
  ]);

function frame(payload: Buffer): Buffer {
  const header = Buffer.alloc(4);
  header.writeUInt32BE(payload.length, 0);
  return Buffer.concat([header, payload]);
}

const message = (name: string, fields: [string, Buffer][]): Buffer =>
  frame(packMap([[name, packMap(fields)]]));

const hello = (sessionId: string, pid: number): Buffer =>
  message("Hello", [
    ["version", packInt(CONTROL_VERSION)],
    ["session_id", packStr(sessionId)],
    ["pid", packInt(pid)],
  ]);

const opened = (requestId: number): Buffer =>
  message("Opened", [
    ["request_id", packInt(requestId)],
    ["document_version", packInt(1)],
  ]);

const state = (path: string, line = 1): Buffer =>
  message("State", [
    ["request_id", packNil()],
    [
      "state",
      packMap([
        ["path", packStr(path)],
        ["line", packInt(line)],
        ["column", packInt(1)],
        ["dirty", packBool(false)],
        ["read_only", packBool(true)],
        ["document_version", packInt(1)],
      ]),
    ],
  ]);

function viewFrame(
  rows: [number, [string, string][]][],
  totalLines: number,
  docVersion: number,
  caret: [number, number] = [0, 0],
): Buffer {
  const packed = rows.map(([line, spans]) =>
    packMap([
      ["line", packInt(line)],
      ["truncated", packBool(false)],
      [
        "spans",
        packArray(
          spans.map(([text, scope]) =>
            packMap([
              ["text", packStr(text)],
              ["scope", packStr(scope)],
            ]),
          ),
        ),
      ],
    ]),
  );
  const body = packMap([
    ["buffer_id", packInt(1)],
    ["doc_version", packInt(docVersion)],
    ["first_line", packInt(0)],
    ["total_lines", packInt(totalLines)],
    ["rows", packArray(packed)],
    ["clipped", packBool(false)],
    ["folded", packArray([])],
    [
      "caret",
      packMap([
        ["line", packInt(caret[0])],
        ["column", packInt(caret[1])],
      ]),
    ],
    ["selection", packArray([])],
    ["extra_carets", packArray([])],
    ["decorations", packArray([])],
  ]);
  return message("ViewFrame", [["frame", body]]);
}

const findDefinition = (requestId: number, symbol: string): Buffer =>
  message("FindDefinition", [
    ["request_id", packInt(requestId)],
    ["symbol", packStr(symbol)],
  ]);

const saveRequest = (requestId: number, text: string, documentVersion: number): Buffer =>
  message("SaveRequest", [
    ["request_id", packInt(requestId)],
    ["text", packStr(text)],
    ["document_version", packInt(documentVersion)],
  ]);

function unpack(buf: Buffer, at: number): [Value, number] {
  const b = buf[at] as number;
  const str = (start: number, n: number): [Value, number] => [
    buf.subarray(start, start + n).toString("utf8"),
    start + n,
  ];
  const list = (n: number, start: number): [Value, number] => {
    const out: Value[] = [];
    let i = start;
    for (let k = 0; k < n; k++) {
      const [value, next] = unpack(buf, i);
      out.push(value);
      i = next;
    }
    return [out, i];
  };
  const map = (n: number, start: number): [Value, number] => {
    const out: Fields = {};
    let i = start;
    for (let k = 0; k < n; k++) {
      const [key, afterKey] = unpack(buf, i);
      const [value, next] = unpack(buf, afterKey);
      out[String(key)] = value;
      i = next;
    }
    return [out, i];
  };
  if (b <= 0x7f) return [b, at + 1];
  if (b >= 0xa0 && b <= 0xbf) return str(at + 1, b & 0x1f);
  if (b >= 0x80 && b <= 0x8f) return map(b & 0x0f, at + 1);
  if (b >= 0x90 && b <= 0x9f) return list(b & 0x0f, at + 1);
  switch (b) {
    case 0xdc:
      return list(buf.readUInt16BE(at + 1), at + 3);
    case 0xdd:
      return list(buf.readUInt32BE(at + 1), at + 5);
    case 0xc0:
      return [null, at + 1];
    case 0xc2:
      return [false, at + 1];
    case 0xc3:
      return [true, at + 1];
    case 0xcc:
      return [buf[at + 1] as number, at + 2];
    case 0xcd:
      return [buf.readUInt16BE(at + 1), at + 3];
    case 0xce:
      return [buf.readUInt32BE(at + 1), at + 5];
    case 0xcf:
      return [Number(buf.readBigUInt64BE(at + 1)), at + 9];
    case 0xd9:
      return str(at + 2, buf[at + 1] as number);
    case 0xda:
      return str(at + 3, buf.readUInt16BE(at + 1));
    case 0xde:
      return map(buf.readUInt16BE(at + 1), at + 3);
    default:
      throw new Error(`unsupported msgpack 0x${b.toString(16).padStart(2, "0")}`);
  }
}

/** Length-prefixed frames off the control socket, one at a time. */
class Frames {
  private buffer = Buffer.alloc(0);
  private waiting: (() => void)[] = [];
  private ended = false;

  constructor(socket: Socket) {
    socket.on("data", (chunk: Buffer) => {
      this.buffer = Buffer.concat([this.buffer, chunk]);
      this.wake();
    });
    const end = (): void => {
      this.ended = true;
      this.wake();
    };
    socket.on("end", end);
    socket.on("error", end);
  }

  private wake(): void {
    for (const resolve of this.waiting.splice(0)) resolve();
  }

  async next(): Promise<Fields> {
    for (;;) {
      if (this.buffer.length >= 4) {
        const n = this.buffer.readUInt32BE(0);
        if (this.buffer.length >= 4 + n) {
          const payload = this.buffer.subarray(4, 4 + n);
          this.buffer = this.buffer.subarray(4 + n);
          return unpack(payload, 0)[0] as Fields;
        }
      }
      if (this.ended) throw new Error("control eof");
      await new Promise<void>((resolve) => this.waiting.push(resolve));
    }
  }
}

const field = (value: Value | undefined, key: string): Value | undefined =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
const text = (value: Value | undefined): string => (typeof value === "string" ? value : "");
const int = (value: Value | undefined, fallback: number): number =>
  typeof value === "number" && value !== 0 ? value : fallback;

async function waitForGo(): Promise<void> {
  for (let i = 0; i < 600 && !existsSync(".forge-go"); i++) await Bun.sleep(50);
}

const env = process.env;
const sockPath = env.FORGE_EDITOR_CONTROL ?? "";
const sessionId = env.FORGE_SESSION_ID ?? "";
writeFileSync(
  ".forge-editor-env",
  ["FORGE_EDITOR_CONTROL", "FORGE_SESSION_ID", "FORGE_WORKSPACE"]
    .map((key) => `${key}=${env[key] ?? ""}\n`)
    .join(""),
);

const mode: string = process.env.FORGE_TEST_EDITOR_MODE ?? "serve";
if (mode === "hang") {
  await Bun.sleep(30_000);
  process.exit(0);
}

const sock = connect(sockPath);
await new Promise<void>((resolve, reject) => {
  sock.once("connect", resolve);
  sock.once("error", reject);
});
const frames = new Frames(sock);
const send = (bytes: Buffer): void => {
  sock.write(bytes);
};

send(hello(sessionId, process.pid));
await frames.next();
const openBody = field(await frames.next(), "Open");
const opening = text(field(openBody, "text"));
let path = text(field(openBody, "path")) || "buffer";
// Opened first: painting the PTY can block if its reader has not started,
// and the handshake cannot wait on that.
send(opened(int(field(openBody, "request_id"), 1)));
send(state(path, int(field(openBody, "line"), 1)));
process.stdout.write(opening);

if (mode === "crash") {
  await Bun.sleep(200);
  process.exit(1);
}

/** The next message named one of `names`; the daemon also sends GitMarks unprompted. */
async function awaitMessage(...names: string[]): Promise<[string, Value]> {
  const deadline = Date.now() + 20_000;
  while (Date.now() < deadline) {
    const msg = await frames.next();
    const retarget = field(msg, "Retarget");
    if (retarget !== undefined) path = text(field(retarget, "path"));
    for (const name of names) {
      if (name in msg) return [name, msg[name] as Value];
    }
  }
  console.error(`no ${names.join("/")} arrived`);
  process.exit(1);
}

const places = (body: Value): Value[] => {
  const found = field(body, "places");
  return Array.isArray(found) ? found : [];
};

const openPath = (place: Value, extra: [string, Buffer][] = []): Buffer =>
  message("OpenPath", [
    ["request_id", packInt(1501)],
    ["path", packStr(text(field(place, "path")))],
    ["line", packInt(int(field(place, "line"), 0))],
    ...extra,
  ]);

if (mode === "marks") {
  const [, body] = await awaitMessage("GitMarks");
  const marks = field(body, "marks");
  const lines = (Array.isArray(marks) ? marks : []).map(
    (mark) => `${String(field(mark, "line"))} ${String(field(mark, "kind"))}\n`,
  );
  writeFileSync(".forge-editor-marks", `${lines.join("")}end\n`);
}

if (mode === "save_stale") {
  // One save, after the test has made the file stale: the conflict stands
  // so it can be read back.
  await waitForGo();
  send(saveRequest(1001, "from the editor\n", 2));
  const [name] = await awaitMessage("Saved", "SaveRefused");
  writeFileSync(".forge-editor-save", `${name}\n`);
}

if (mode === "save_twice") {
  // The test makes the file stale on disk, then drops the sentinel: the
  // first save must be refused and the second must go through.
  await waitForGo();
  const answers: string[] = [];
  for (const attempt of [1, 2]) {
    send(saveRequest(1000 + attempt, "from the editor\n", 2));
    const [name] = await awaitMessage("Saved", "SaveRefused");
    answers.push(name);
  }
  writeFileSync(".forge-editor-save", `${answers.join(" ")}\n`);
}

if (mode === "definition" || (mode === "definition_nestjs" && path.endsWith(".service.ts"))) {
  const symbol = mode === "definition_nestjs" ? "upsertJobScheduler" : "answer";
  send(findDefinition(1500, symbol));
  const [, body] = await awaitMessage("Definitions");
  const found = places(body);
  const lines = found.map(
    (place) =>
      `${text(field(place, "path"))}:${String(field(place, "line"))} ${text(field(place, "text")).trim()}\n`,
  );
  writeFileSync(".forge-editor-definitions", `${lines.join("")}end\n`);
  const first = found[0];
  if (mode === "definition_nestjs" && first !== undefined) send(openPath(first));
}

if (mode === "definition_follow") {
  if (path === "a.rs") {
    send(findDefinition(1500, "answer"));
    const [, body] = await awaitMessage("Definitions");
    const place = places(body)[0] ?? null;
    send(
      openPath(place, [
        ["column", packInt(int(field(place, "column"), 0))],
        [
          "from",
          packMap([
            ["path", packStr("a.rs")],
            ["line", packInt(1)],
            ["column", packInt(9)],
          ]),
        ],
      ]),
    );
  } else {
    const [, body] = await awaitMessage("Reveal");
    const origin = field(body, "from") ?? {};
    writeFileSync(
      ".forge-editor-reveal",
      `${String(field(body, "line"))} ${String(field(body, "column"))} ${String(field(body, "utf16"))} ${String(field(origin, "path"))}:${String(field(origin, "column"))}\n`,
    );
  }
}

if (mode === "copy") {
  // Written only once the test has attached: the escape is a single burst,
  // and a client that subscribed after it would wait for a store that has
  // already been broadcast.
  await waitForGo();
  // What `Ctrl-C` leaves on the wire: the clipboard *store*, base64 of
  // "picked up". The daemon's VT engine is the only thing that reads it.
  process.stdout.write("\x1b]52;c;cGlja2VkIHVw\x07");
}

if (mode === "headless") {
  // The DOM surface: no raw mode, no ANSI, and the window is what the
  // daemon is expected to broadcast. `--headless` must have reached argv,
  // or the surface flag never made it to the spawn.
  writeFileSync(".forge-editor-argv", `${process.argv.slice(2).join(" ")}\n`);
  send(
    viewFrame(
      [
        [
          0,
          [
            ["fn", "Keyword"],
            [" main", "Function"],
          ],
        ],
      ],
      2,
      1,
    ),
  );
  const seen: string[] = [];
  while (seen.length < 2) {
    const msg = await frames.next();
    const input = field(msg, "Input");
    const view = field(field(msg, "SetView"), "view");
    if (input !== undefined) {
      const events = field(input, "events");
      const keys: string[] = [];
      for (const event of Array.isArray(events) ? events : []) {
        const key = field(event, "Key");
        const typed = field(event, "Text");
        if (key !== undefined) {
          const which = field(key, "key");
          const name = typeof which === "string" ? which : String(field(which, "Char"));
          keys.push(`${name}+${String(field(key, "modifiers"))}`);
        } else if (typed !== undefined) {
          keys.push(`text:${String(typed)}`);
        }
      }
      seen.push(`input ${keys.join(",")}`);
    } else if (view !== undefined) {
      seen.push(`view ${String(field(view, "first_line"))} ${String(field(view, "line_count"))}`);
    }
  }
  writeFileSync(".forge-editor-surface", `${seen.join("\n")}\nend\n`);
  // A second window, so the test can tell a frame that answered the input
  // from the one that was already on screen.
  send(viewFrame([[0, [["Xfn main", "Plain"]]]], 2, 2, [0, 1]));
}

if (mode === "save") {
  const body = process.env.FORGE_TEST_SAVE_TEXT ?? "saved by the editor\n";
  send(saveRequest(1000, body, 2));
  const [name, payload] = await awaitMessage("Saved", "SaveRefused");
  const detail = text(field(payload, "revision")) || text(field(payload, "reason"));
  writeFileSync(".forge-editor-save", `${name} ${detail}\n`);
}

if (mode === "move") {
  const original = path;
  const receipt = `.forge-move-${basename(original)}`;
  let count = 0;
  for (;;) {
    const msg = await frames.next();
    const retarget = field(msg, "Retarget");
    if (retarget !== undefined) {
      path = text(field(retarget, "path"));
    } else if ("GetState" in msg) {
      // An old queued state must never redirect the daemon's save destination.
      send(state(original, 7));
    } else if ("Save" in msg) {
      count += 1;
      send(saveRequest(2000 + count, `draft ${original}\n`, count + 1));
      const [name] = await awaitMessage("Saved", "SaveRefused");
      writeFileSync(receipt, `${name} ${path} ${count}\n`);
    }
  }
}

// Alive until the daemon stops it, even once the control socket is gone.
setInterval(() => {}, 1_000);
