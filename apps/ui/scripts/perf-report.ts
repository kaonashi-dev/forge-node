#!/usr/bin/env bun
// perf-report.json keeps the bench report's int/float distinction (`125.0` stays `125.0`) and
// key order, which `JSON.parse` loses, so values parse into `Int`/`number` and `Map`.
import { readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

class Int {
  constructor(readonly value: bigint) {}
}
type Json = null | boolean | number | string | Int | Json[] | JsonObject;
type JsonObject = Map<string, Json>;

class JsonDecodeError extends Error {}

function parseJson(text: string): Json {
  let at = 0;
  const NUMBER = /(-?(?:0|[1-9][0-9]*))(\.[0-9]+)?([eE][-+]?[0-9]+)?/y;
  const skip = () => {
    while (at < text.length && " \t\n\r".includes(text[at]!)) at++;
  };
  const fail = (what: string): never => {
    throw new JsonDecodeError(`${what} at ${at}`);
  };
  const LITERALS: [string, Json][] = [
    ["null", null],
    ["true", true],
    ["false", false],
    ["NaN", NaN],
    ["Infinity", Infinity],
    ["-Infinity", -Infinity],
  ];
  const string = (): string => {
    at++;
    let out = "";
    for (;;) {
      const ch = text[at];
      if (ch === undefined) return fail("unterminated string");
      at++;
      if (ch === '"') return out;
      if (ch < " ") return fail("invalid control character");
      if (ch !== "\\") {
        out += ch;
        continue;
      }
      const esc = text[at++];
      const simple: Record<string, string> = {
        '"': '"', "\\": "\\", "/": "/", b: "\b", f: "\f", n: "\n", r: "\r", t: "\t",
      };
      if (esc === "u") {
        const hex = text.slice(at, at + 4);
        if (!/^[0-9a-fA-F]{4}$/.test(hex)) return fail("invalid \\uXXXX escape");
        out += String.fromCharCode(parseInt(hex, 16));
        at += 4;
      } else if (esc !== undefined && esc in simple) {
        out += simple[esc];
      } else {
        return fail("invalid escape");
      }
    }
  };
  const value = (): Json => {
    const ch = text[at];
    if (ch === '"') return string();
    if (ch === "{") {
      at++;
      const object: JsonObject = new Map();
      skip();
      if (text[at] === "}") {
        at++;
        return object;
      }
      for (;;) {
        if (text[at] !== '"') return fail("expecting property name");
        const key = string();
        skip();
        if (text[at++] !== ":") return fail("expecting ':'");
        skip();
        object.set(key, value());
        skip();
        const sep = text[at++];
        if (sep === "}") return object;
        if (sep !== ",") return fail("expecting ',' delimiter");
        skip();
      }
    }
    if (ch === "[") {
      at++;
      const array: Json[] = [];
      skip();
      if (text[at] === "]") {
        at++;
        return array;
      }
      for (;;) {
        array.push(value());
        skip();
        const sep = text[at++];
        if (sep === "]") return array;
        if (sep !== ",") return fail("expecting ',' delimiter");
        skip();
      }
    }
    const word = LITERALS.find(([w]) => text.startsWith(w, at));
    if (word !== undefined) {
      at += word[0].length;
      return word[1];
    }
    NUMBER.lastIndex = at;
    const match = NUMBER.exec(text);
    if (match === null) return fail("expecting value");
    at = NUMBER.lastIndex;
    return match[2] === undefined && match[3] === undefined
      ? new Int(BigInt(match[1]!))
      : Number(match[0]);
  };
  skip();
  const result = value();
  skip();
  if (at !== text.length) fail("extra data");
  return result;
}

/** Python's float `repr`: shortest round-trip digits, exponent outside [1e-4, 1e16). */
function floatRepr(x: number): string {
  if (Number.isNaN(x)) return "nan";
  if (!Number.isFinite(x)) return x > 0 ? "inf" : "-inf";
  if (x === 0) return Object.is(x, -0) ? "-0.0" : "0.0";
  const sign = x < 0 ? "-" : "";
  const [mantissa = "", expText = ""] = Math.abs(x).toExponential().split("e");
  const digits = mantissa.replace(".", "");
  const exp = Number(expText);
  if (exp >= -4 && exp < 16) {
    if (exp < 0) return `${sign}0.${"0".repeat(-exp - 1)}${digits}`;
    const whole = digits.slice(0, exp + 1).padEnd(exp + 1, "0");
    return `${sign}${whole}.${digits.slice(exp + 1) || "0"}`;
  }
  const lead = digits.length > 1 ? `${digits[0]}.${digits.slice(1)}` : digits;
  const power = String(Math.abs(exp)).padStart(2, "0");
  return `${sign}${lead}e${exp < 0 ? "-" : "+"}${power}`;
}

/** Python's `format(x, ".Nf")`: exact binary value, ties to even (`toFixed` rounds ties up). */
function fixedDigits(x: number, places: number): string {
  if (Number.isNaN(x)) return "nan";
  if (!Number.isFinite(x)) return x > 0 ? "inf" : "-inf";
  const view = new DataView(new ArrayBuffer(8));
  view.setFloat64(0, x);
  const bits = view.getBigUint64(0);
  const biased = Number((bits >> 52n) & 0x7ffn);
  const fraction = bits & ((1n << 52n) - 1n);
  const mantissa = biased === 0 ? fraction : fraction | (1n << 52n);
  const exponent = (biased === 0 ? 1 : biased) - 1075;
  let numerator = mantissa * 10n ** BigInt(places);
  let denominator = 1n;
  if (exponent >= 0) numerator <<= BigInt(exponent);
  else denominator <<= BigInt(-exponent);
  let q = numerator / denominator;
  const twice = (numerator % denominator) * 2n;
  if (twice > denominator || (twice === denominator && q % 2n === 1n)) q += 1n;
  const digits = q.toString().padStart(places + 1, "0");
  const body = places === 0 ? digits : `${digits.slice(0, -places)}.${digits.slice(-places)}`;
  return (bits >> 63n === 1n ? "-" : "") + body;
}

function fixed(value: Json, places: number): string {
  if (typeof value === "number") return fixedDigits(value, places);
  if (value instanceof Int) return fixedDigits(Number(value.value), places);
  if (typeof value === "boolean") return fixedDigits(value ? 1 : 0, places);
  throw new TypeError(`unsupported format string passed to ${typeName(value)}.__format__`);
}

function typeName(value: Json): string {
  if (value === null) return "NoneType";
  if (Array.isArray(value)) return "list";
  if (value instanceof Map) return "dict";
  return typeof value;
}

const NON_PRINTABLE = /[\p{Cc}\p{Cf}\p{Cs}\p{Co}\p{Cn}\p{Zl}\p{Zp}\p{Zs}]/u;

function reprString(text: string): string {
  const quote = text.includes("'") && !text.includes('"') ? '"' : "'";
  let out = quote;
  for (const ch of text) {
    const code = ch.codePointAt(0) ?? 0;
    if (ch === "\\" || ch === quote) out += `\\${ch}`;
    else if (ch === "\t") out += "\\t";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch !== " " && NON_PRINTABLE.test(ch)) {
      const hex = code.toString(16);
      if (code < 0x100) out += `\\x${hex.padStart(2, "0")}`;
      else if (code < 0x10000) out += `\\u${hex.padStart(4, "0")}`;
      else out += `\\U${hex.padStart(8, "0")}`;
    } else out += ch;
  }
  return out + quote;
}

function repr(value: Json): string {
  return typeof value === "string" ? reprString(value) : str(value);
}

/** Python's `str`, which is what an f-string field without a format spec prints. */
function str(value: Json): string {
  if (value === null) return "None";
  if (typeof value === "boolean") return value ? "True" : "False";
  if (typeof value === "number") return floatRepr(value);
  if (typeof value === "string") return value;
  if (value instanceof Int) return value.value.toString();
  if (Array.isArray(value)) return `[${value.map(repr).join(", ")}]`;
  const entries = [...value].map(([k, v]) => `${reprString(k)}: ${repr(v)}`);
  return `{${entries.join(", ")}}`;
}

function truthy(value: Json): boolean {
  if (value === null) return false;
  if (typeof value === "boolean") return value;
  if (typeof value === "number") return value !== 0;
  if (typeof value === "string" || Array.isArray(value)) return value.length > 0;
  if (value instanceof Int) return value.value !== 0n;
  return value.size > 0;
}

function asObject(value: Json): JsonObject {
  if (!(value instanceof Map)) throw new TypeError(`'${typeName(value)}' object is not a dict`);
  return value;
}

function get(value: Json, key: string): Json {
  const object = asObject(value);
  const found = object.get(key);
  if (found === undefined) throw new Error(`KeyError: ${reprString(key)}`);
  return found;
}

function opt(value: Json, key: string): Json {
  return asObject(value).get(key) ?? null;
}

function escapeAscii(text: string): string {
  const short: Record<string, string> = {
    '"': '\\"', "\\": "\\\\", "\n": "\\n", "\r": "\\r", "\t": "\\t", "\b": "\\b", "\f": "\\f",
  };
  let out = '"';
  for (let i = 0; i < text.length; i++) {
    const ch = text[i]!;
    const code = text.charCodeAt(i);
    if (ch in short) out += short[ch];
    else if (code < 0x20 || code > 0x7e) out += `\\u${code.toString(16).padStart(4, "0")}`;
    else out += ch;
  }
  return out + '"';
}

/** `json.dump(value, indent=2)` with Python's defaults (`ensure_ascii`, `allow_nan`). */
function dump(value: Json, depth = 0): string {
  if (value === null) return "null";
  if (typeof value === "boolean") return value ? "true" : "false";
  if (typeof value === "string") return escapeAscii(value);
  if (value instanceof Int) return value.value.toString();
  if (typeof value === "number") {
    if (Number.isNaN(value)) return "NaN";
    if (!Number.isFinite(value)) return value > 0 ? "Infinity" : "-Infinity";
    return floatRepr(value);
  }
  const inner = "  ".repeat(depth + 1);
  const outer = "  ".repeat(depth);
  if (Array.isArray(value)) {
    if (value.length === 0) return "[]";
    return `[\n${value.map((v) => inner + dump(v, depth + 1)).join(",\n")}\n${outer}]`;
  }
  if (value.size === 0) return "{}";
  const entries = [...value].map(([k, v]) => `${inner}${escapeAscii(k)}: ${dump(v, depth + 1)}`);
  return `{\n${entries.join(",\n")}\n${outer}}`;
}

const PY_SPACE = "[\\t\\n\\v\\f\\r\\x1c-\\x20\\x85\\xa0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000]";
const STRIP = new RegExp(`^${PY_SPACE}+|${PY_SPACE}+$`, "g");

function strip(text: string): string {
  return text.replace(STRIP, "");
}

function splitlines(text: string): string[] {
  const lines = text.split(/\r\n|[\n\r\v\f\x1c\x1d\x1e\x85\u2028\u2029]/);
  if (lines[lines.length - 1] === "") lines.pop();
  return lines;
}

function pyInt(text: string): Int {
  const trimmed = strip(text);
  if (!/^[+-]?[0-9](_?[0-9])*$/.test(trimmed)) {
    throw new Error(`ValueError: invalid literal for int() with base 10: ${reprString(text)}`);
  }
  return new Int(BigInt(trimmed.replaceAll("_", "")));
}

function num(value: string | undefined): number | null {
  if (value === undefined || value === "") return null;
  const trimmed = strip(value);
  if (/^[+-]?([0-9]+\.?[0-9]*|\.[0-9]+)([eE][+-]?[0-9]+)?$/.test(trimmed)) return Number(trimmed);
  const special = /^([+-]?)(inf|infinity|nan)$/i.exec(trimmed);
  if (special === null) return null;
  if (special[2]!.toLowerCase() === "nan") return NaN;
  return special[1] === "-" ? -Infinity : Infinity;
}

function parseOrRaw(text: string): Json {
  try {
    return parseJson(text);
  } catch (error) {
    if (error instanceof JsonDecodeError) return new Map([["raw", text]]);
    throw error;
  }
}

const args = process.argv.slice(2, 8);
if (args.length !== 6) {
  console.error("usage: perf-report.ts <tmp> <json> <md> <binary> <iterations> <frames>");
  process.exit(2);
}
const [tmp, jsonPath, mdPath, binary, iterations, frames] = args as [
  string, string, string, string, string, string,
];

const utf8 = new TextDecoder("utf-8", { fatal: true, ignoreBOM: true });

function read(name: string): string {
  let bytes: Uint8Array;
  try {
    bytes = readFileSync(join(tmp, name));
  } catch {
    return "";
  }
  return utf8.decode(bytes).replace(/\r\n?/g, "\n");
}

function tail(text: string, limit = 400): string {
  return [...strip(text)].slice(-limit).join("");
}

const report = asObject(parseJson(utf8.decode(readFileSync(join(tmp, "bench.json")))));

const startup: JsonObject = new Map<string, Json>([
  [
    "definition",
    "cold = first launch after the build; warm = the next two, median; launch to the first Drawn frame; RSS 0/1/2 s measured from that first frame, spawn at exec",
  ],
  ["cold", null],
  ["warm", []],
  ["warm_median", null],
  ["headless_proxy", null],
  ["blocked", null],
]);

function runObj(row: string[]): JsonObject {
  if (row.length < 7) throw new Error(`ValueError: not enough values to unpack (expected 7, got ${row.length})`);
  const [label = "", status = "", first, spawn, rss0, rss1, rss2] = row;
  let marker: Json = null;
  const markerText = row[7];
  if (markerText !== undefined && strip(markerText)) marker = parseOrRaw(markerText);
  const obj: JsonObject = new Map<string, Json>([
    ["label", label],
    ["status", status],
    ["launch_to_first_frame_ms", num(first)],
    [
      "rss_mib",
      new Map<string, Json>([
        ["spawn", num(spawn)],
        ["0s", num(rss0)],
        ["1s", num(rss1)],
        ["2s", num(rss2)],
      ]),
    ],
    ["marker", marker],
  ]);
  if (status !== "ok") {
    obj.set("blocked", tail(read(`startup-${label}.err`)) || tail(read(`startup-${label}.out`)));
  }
  return obj;
}

function median(values: (number | null)[]): number | null {
  const sorted = values.filter((v): v is number => v !== null).sort((a, b) => a - b);
  if (sorted.length === 0) return null;
  const middle = Math.floor(sorted.length / 2);
  if (sorted.length % 2) return sorted[middle]!;
  return (sorted[middle - 1]! + sorted[middle]!) / 2;
}

const asFloat = (value: Json): number | null => (typeof value === "number" ? value : null);

const rows = splitlines(read("startup.tsv"))
  .filter((line) => line.includes("\t"))
  .map((line) => line.split("\t"));
if (rows.length > 0) {
  const cold = runObj(rows[0]!);
  const warm = rows.slice(1).map(runObj);
  startup.set("cold", cold);
  startup.set("warm", warm);
  const ok = warm.filter((r) => r.get("status") === "ok");
  if (ok.length > 0) {
    const rss = (r: JsonObject, key: string) => asFloat(get(get(r, "rss_mib"), key));
    startup.set(
      "warm_median",
      new Map<string, Json>([
        ["launch_to_first_frame_ms", median(ok.map((r) => asFloat(get(r, "launch_to_first_frame_ms"))))],
        [
          "rss_mib",
          new Map<string, Json>(
            ["spawn", "0s", "1s", "2s"].map((key) => [key, median(ok.map((r) => rss(r, key)))]),
          ),
        ],
      ]),
    );
  }
  const blocked = [cold, ...warm].filter((r) => r.get("status") !== "ok").map((r) => r.get("blocked") ?? null);
  if (blocked.length > 0) startup.set("blocked", blocked[0]!);
}

const headless = splitlines(read("headless.tsv"));
if (headless.length > 0) {
  const [code = "", elapsed = ""] = [...headless[0]!.split("\t"), "", ""];
  let marker: Json = null;
  if (headless.length > 1) {
    const PREFIX = "rune-first-pixels ";
    const line = headless[1]!;
    marker = parseOrRaw(line.startsWith(PREFIX) ? line.slice(PREFIX.length) : line);
  }
  const digits = (text: string): Json => (/^[0-9]+$/.test(text) ? new Int(BigInt(text)) : null);
  startup.set(
    "headless_proxy",
    new Map<string, Json>([
      ["command", `${binary} --fixture --frames 2 --fps 125 --first-pixels`],
      ["exit_code", digits(code)],
      ["process_ms", digits(elapsed)],
      ["marker", marker],
      ["note", "headless has no window or display; this is a first-frame proxy, not display readiness"],
    ]),
  );
}

report.set("startup", startup);
report.set(
  "harness",
  new Map<string, Json>([
    ["binary", binary],
    ["iterations", pyInt(iterations)],
    ["frames", pyInt(frames)],
    ["wrapper", "apps/ui/scripts/perf.sh"],
    ["note", "terminal and editor samples are CPU clocks on a headless GPU; display vsync is not measured"],
  ]),
);
writeFileSync(jsonPath, `${dump(report)}\n`);

function ms(sample: Json): string {
  if (!truthy(sample)) return "—";
  return `${fixed(get(sample, "p50"), 3)} / ${fixed(get(sample, "p95"), 3)} / ${fixed(get(sample, "max"), 3)}`;
}

function rss(value: Json): string {
  return value === null ? "—" : fixed(value, 2);
}

function slice12(value: Json): string {
  if (typeof value === "string") return [...value].slice(0, 12).join("");
  if (Array.isArray(value)) return str(value.slice(0, 12));
  throw new TypeError(`'${typeName(value)}' object is not subscriptable`);
}

const fixture = get(report, "fixture");
const terminal = get(report, "terminal_125");
const editor = get(report, "editor");
const machine = get(report, "machine");
const t = (key: string) => str(get(terminal, key));
const e = (key: string) => str(get(editor, key));
const lines: string[] = [];
lines.push("# forge-ui perf report");
lines.push("");
lines.push(
  `Generated from \`${binary.slice(binary.lastIndexOf("/") + 1)}\` (${str(get(machine, "profile"))}, Rust ${str(get(machine, "rust"))}, Rune \`${slice12(get(machine, "rune"))}\`) on ${str(get(machine, "os"))}/${str(get(machine, "arch"))}.`,
);
lines.push("");
lines.push("| Fixture | p50 / p95 / max ms |");
lines.push("|---|---:|");
lines.push(`| Session switch (click → painted frame) | ${ms(get(fixture, "switch_ms"))} |`);
lines.push(`| View build | ${ms(get(fixture, "view_ms"))} |`);
lines.push(`| Terminal echo (key → grid → frame) | ${ms(get(fixture, "echo_ms"))} |`);
lines.push(`| Socket ops / on the UI thread | ${str(get(fixture, "socket_ops"))} / ${str(get(fixture, "ui_socket_ops"))} |`);
lines.push("");
lines.push(
  `## Integrated terminal stream (${fixed(get(terminal, "rate_hz"), 0)} Hz, ${t("cols")}×${t("rows")}, ${t("feed")})`,
);
lines.push("");
if (truthy(opt(terminal, "blocked"))) {
  lines.push(`**Blocked:** ${t("blocked")}`);
} else {
  lines.push("| Clock | p50 / p95 / max ms |");
  lines.push("|---|---:|");
  lines.push(`| Delta apply (\`SharedTerminal\`) | ${ms(get(terminal, "delta_apply_ms"))} |`);
  lines.push(`| Rune update (poll+view+layout+paint) | ${ms(get(terminal, "update_ms"))} |`);
  lines.push(`| Terminal build (diff+encode+upload) | ${ms(get(terminal, "build_ms"))} |`);
  lines.push(`| Terminal encode (pass+draw) | ${ms(get(terminal, "encode_ms"))} |`);
  lines.push(`| Frame CPU (update+build+encode) | ${ms(get(terminal, "frame_cpu_ms"))} |`);
  lines.push(`| Host frame (external, update→submit) | ${ms(get(terminal, "host_frame_ms"))} |`);
  lines.push(`| Total CPU/frame (+ delta apply) | ${ms(get(terminal, "total_cpu_ms"))} |`);
  lines.push("");
  lines.push(
    `${t("deltas_applied")} deltas, ${t("view_commits")} shell view commits, ${t("rows_uploaded")} rows uploaded, ${t("instances_per_draw")} instances/draw, ${t("glyph_misses")} glyph misses, ${t("build_allocations")} build allocations (${t("encode_allocations")} in cell encoding); GPU ${t("gpu")} (init ${fixed(get(terminal, "gpu_init_ms"), 1)} ms).`,
  );
}
lines.push("");
lines.push(
  `## Editor (${e("rows")} mounted rows, ${e("folds")} foldable, ${fixed(get(editor, "rate_hz"), 0)} Hz)`,
);
lines.push("");
if (truthy(opt(editor, "blocked"))) {
  lines.push(`**Blocked:** ${e("blocked")}`);
} else {
  lines.push("| Clock | p50 / p95 / max ms |");
  lines.push("|---|---:|");
  lines.push(
    `| Mount (first frame) | ${fixed(get(editor, "mount_ms"), 3)} ms; view ${fixed(get(editor, "mount_view_ms"), 3)} ms |`,
  );
  lines.push(`| Caret-only view build | ${ms(get(editor, "caret_view_ms"))} |`);
  lines.push(`| Caret-only frame CPU | ${ms(get(editor, "caret_frame_ms"))} |`);
  lines.push(`| Key-to-paint CPU proxy | ${ms(get(editor, "key_to_paint_cpu_ms"))} |`);
  lines.push("");
  lines.push(
    `Rows rebuilt: mount ${e("mount_rows_built")}, caret-only ${e("caret_rows_rebuilt")}, key-to-paint ${e("key_to_paint_rows_rebuilt")}. Display/vsync is **not measured**; key-to-paint is the named CPU proxy.`,
  );
}
lines.push("");
lines.push("## Startup / RSS");
lines.push("");
const rssLine = (values: Json) =>
  ["spawn", "0s", "1s", "2s"].map((key) => rss(get(values, key))).join(" / ");
const cold = startup.get("cold") ?? null;
if (truthy(cold)) {
  if (get(cold, "status") === "ok") {
    lines.push(
      `- Cold (first launch after build): ${str(get(cold, "launch_to_first_frame_ms"))} ms to first drawn frame; RSS spawn / 0 / 1 / 2 s ${rssLine(get(cold, "rss_mib"))} MiB`,
    );
  } else {
    lines.push(`- Cold: blocked — ${str(asObject(cold).get("blocked") ?? "no first frame")}`);
  }
}
const warm = startup.get("warm_median") ?? null;
if (truthy(warm)) {
  lines.push(
    `- Warm median (two launches): ${str(get(warm, "launch_to_first_frame_ms"))} ms; RSS spawn / 0 / 1 / 2 s ${rssLine(get(warm, "rss_mib"))} MiB`,
  );
}
const proxy = startup.get("headless_proxy") ?? null;
if (truthy(proxy)) {
  const proxyMarker = opt(proxy, "marker");
  const marker = truthy(proxyMarker) ? proxyMarker : new Map<string, Json>();
  const headlessMs = opt(marker, "headless_ms");
  const first = truthy(headlessMs) ? headlessMs : opt(marker, "frame_cpu_ms");
  lines.push(
    `- Headless proxy: process ${str(get(proxy, "process_ms"))} ms, first-frame marker ${str(first)} ms (no window/display)`,
  );
}
lines.push("");
lines.push("## Measured vs proxy");
lines.push("");
lines.push(
  "- Fixture switch, echo, terminal delta apply, terminal build/encode and the editor view are wall-clock CPU measurements.",
);
lines.push("- Terminal build/encode are the compositor's CPU clocks; GPU time and presentation are not included.");
lines.push("- Editor key-to-paint is a CPU proxy (key handler → one 125 Hz frame); it is not a display measurement.");
lines.push(
  "- Startup windowed runs measure launch → the first `FrameEncoded` reported as `Drawn`; cold means first after the build, not an OS page-cache purge.",
);
writeFileSync(mdPath, `${lines.join("\n")}\n`);
console.log(`perf: wrote ${jsonPath}`);
console.log(`perf: wrote ${mdPath}`);
