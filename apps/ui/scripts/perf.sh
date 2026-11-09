#!/usr/bin/env bash
# W7 perf harness: one command, one machine-readable report.
#
#   apps/ui/scripts/perf.sh
#
# Writes perf-report.json and perf-report.md beside this script. Sections:
# fixture switch/echo, integrated 125 Hz terminal, 88-row editor, startup/RSS,
# and the Tauri key-to-delta baseline. The Tauri section uses prebuilt
# `forge-daemon` and `forge-tauri-latency` binaries when they exist; point
# PERF_TAURI_ROOT at a checkout that has them, or leave it to record the
# blocker. It never points the latency tool at the user's live daemon: a
# temporary socket and data directory are used.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="$(cd "$UI_DIR/../.." && pwd)"
REPORT_JSON="$SCRIPT_DIR/perf-report.json"
REPORT_MD="$SCRIPT_DIR/perf-report.md"

export RUSTUP_TOOLCHAIN="$(sed -n 's/^channel = "\(.*\)"/\1/p' "$UI_DIR/rust-toolchain.toml")"
export CARGO_INCREMENTAL="${CARGO_INCREMENTAL:-0}"
if [ -z "${SDKROOT:-}" ] && [ -x "$ROOT/scripts/macos-sdk" ]; then
    SDKROOT="$("$ROOT/scripts/macos-sdk")"
    export SDKROOT
fi

cd "$UI_DIR" || exit 2

TMP="$(mktemp -d "${TMPDIR:-/tmp}/forge-ui-perf.XXXXXX")" || exit 2
DAEMON_PID=""
LATENCY_PID=""
WATCHDOG=""
cleanup() {
    [ -n "$WATCHDOG" ] && kill "$WATCHDOG" 2>/dev/null
    [ -n "$LATENCY_PID" ] && kill "$LATENCY_PID" 2>/dev/null
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    # Only the directory this script made.
    case "$TMP" in
        */forge-ui-perf.*) rm -rf "$TMP" ;;
    esac
}
trap cleanup EXIT

now_ms() {
    python3 -c 'import time; print(int(time.time() * 1000))'
}

rss_mib() {
    local kb
    kb="$(ps -o rss= -p "$1" 2>/dev/null | tr -d ' ')"
    if [ -n "$kb" ]; then
        python3 -c "print(round($kb / 1024.0, 2))"
    else
        printf ''
    fi
}

wait_until() {
    local launch="$1" target="$2" now rem
    now="$(now_ms)"
    rem=$((target - (now - launch)))
    if [ "$rem" -gt 0 ]; then
        sleep "$(python3 -c "print($rem / 1000.0)")"
    fi
}

if [ "${PERF_SKIP_BUILD:-0}" != "1" ]; then
    echo "perf: building release forge-ui"
    cargo build --release --quiet || exit 2
fi

TARGET="${CARGO_TARGET_DIR:-$UI_DIR/target}"
case "$TARGET" in
    /*) ;;
    *) TARGET="$UI_DIR/$TARGET" ;;
esac
BIN="$TARGET/release/forge-ui"
if [ ! -x "$BIN" ]; then
    echo "perf: missing $BIN (run without PERF_SKIP_BUILD=1)" >&2
    exit 2
fi

ITERS="${PERF_BENCH_ITERS:-500}"
FRAMES="${PERF_BENCH_FRAMES:-300}"
echo "perf: forge-ui --bench all ($ITERS switch iterations, $FRAMES frames at 125 Hz)"
"$BIN" --fixture --bench all --bench-iterations "$ITERS" --bench-frames "$FRAMES" \
    --bench-report "$TMP/bench.json" >"$TMP/bench.out" 2>"$TMP/bench.err"
BENCH_CODE=$?
if [ "$BENCH_CODE" -ne 0 ] || [ ! -s "$TMP/bench.json" ]; then
    echo "perf: forge-ui --bench all failed (exit $BENCH_CODE)" >&2
    tail -30 "$TMP/bench.err" >&2
    exit 2
fi

# One launch: marker on the first Drawn frame, RSS at 0/1/2 s from spawn.
run_startup() {
    local label="$1"
    local out="$TMP/startup-$label.out"
    local err="$TMP/startup-$label.err"
    local launch marker status spawn rss0 rss1 rss2 pid first line
    : >"$err"
    launch="$(now_ms)"
    "$BIN" --fixture --bench-startup >"$out" 2>"$err" &
    pid=$!
    spawn=""
    for _ in 1 2 3 4 5; do
        spawn="$(rss_mib "$pid")"
        [ -n "$spawn" ] && break
        sleep 0.01
    done
    for _ in $(seq 1 400); do
        grep -q '^forge-ui-first-frame ' "$err" 2>/dev/null && break
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    marker="$(now_ms)"
    if grep -q '^forge-ui-first-frame ' "$err" 2>/dev/null; then
        status=ok
    else
        status=blocked
    fi
    rss0=""
    rss1=""
    rss2=""
    if [ "$status" = ok ] && kill -0 "$pid" 2>/dev/null; then
        rss0="$(rss_mib "$pid")"
        wait_until "$marker" 1000
        rss1="$(rss_mib "$pid")"
        wait_until "$marker" 2000
        rss2="$(rss_mib "$pid")"
    fi
    { kill "$pid" 2>/dev/null; wait "$pid"; } 2>/dev/null
    for _ in $(seq 1 40); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    { kill -9 "$pid"; } 2>/dev/null
    wait "$pid" 2>/dev/null
    first=""
    [ "$status" = ok ] && first=$((marker - launch))
    line="$(grep -m1 '^forge-ui-first-frame ' "$err" 2>/dev/null | sed 's/^forge-ui-first-frame //')"
    printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n' \
        "$label" "$status" "$first" "$spawn" "$rss0" "$rss1" "$rss2" "$line" >>"$TMP/startup.tsv"
}

: >"$TMP/startup.tsv"
if [ "${PERF_SKIP_STARTUP:-0}" != "1" ]; then
    echo "perf: startup/RSS (windowed fixture app)"
    run_startup cold
    run_startup warm1
    run_startup warm2
    if ! awk -F '\t' '$2 == "ok" { found = 1 } END { exit !found }' "$TMP/startup.tsv"; then
        echo "perf: no windowed first frame; trying the headless proxy"
        headless_start="$(now_ms)"
        "$BIN" --fixture --frames 2 --fps 125 --first-pixels \
            >"$TMP/headless.out" 2>"$TMP/headless.err"
        headless_code=$?
        headless_end="$(now_ms)"
        printf '%s\t%s\n' "$headless_code" "$((headless_end - headless_start))" >"$TMP/headless.tsv"
        cat "$TMP/headless.out" "$TMP/headless.err" \
            | grep -m1 '^rune-first-pixels ' >>"$TMP/headless.tsv" || true
    fi
fi

# The Tauri baseline: same machine, isolated daemon, the prebuilt route.
: >"$TMP/tauri.tsv"
if [ "${PERF_SKIP_TAURI:-0}" != "1" ]; then
    echo "perf: Tauri latency baseline"
    TAURI_ROOT="${PERF_TAURI_ROOT:-$ROOT}"
    DAEMON="${PERF_DAEMON_BIN:-}"
    LATENCY="${PERF_TAURI_BIN:-}"
    if [ -z "$DAEMON" ]; then
        for profile in debug release; do
            if [ -x "$TAURI_ROOT/target/$profile/forge-daemon" ]; then
                DAEMON="$TAURI_ROOT/target/$profile/forge-daemon"
                break
            fi
        done
    fi
    if [ -z "$LATENCY" ]; then
        for profile in debug release; do
            if [ -x "$TAURI_ROOT/target/$profile/forge-tauri-latency" ]; then
                LATENCY="$TAURI_ROOT/target/$profile/forge-tauri-latency"
                break
            fi
        done
    fi
    if [ -z "$DAEMON" ] || [ -z "$LATENCY" ]; then
        printf 'blocked\t%s\n' \
            "no prebuilt forge-daemon/forge-tauri-latency under $TAURI_ROOT/target; \`make latency-tauri\` builds the root Tauri workspace (Rust 1.89). Set PERF_TAURI_ROOT to a checkout that has them." \
            >"$TMP/tauri.tsv"
    else
        HOME_DIR="$TMP/tauri-home"
        SOCK="$TMP/tauri.sock"
        mkdir -p "$HOME_DIR"
        FORGE_SOCKET="$SOCK" FORGE_HOME="$HOME_DIR" "$DAEMON" run \
            >"$TMP/tauri-daemon.log" 2>&1 &
        DAEMON_PID=$!
        for _ in $(seq 1 200); do
            [ -S "$SOCK" ] && break
            sleep 0.05
        done
        if [ ! -S "$SOCK" ]; then
            printf 'blocked\t%s\n' "the isolated forge-daemon did not open $SOCK" >"$TMP/tauri.tsv"
        else
            FORGE_SOCKET="$SOCK" "$BIN" --probe --socket "$SOCK" \
                >"$TMP/tauri-probe.json" 2>"$TMP/tauri-probe.err"
            probe_code=$?
            if [ "$probe_code" -ne 0 ]; then
                printf 'blocked\t%s\n' \
                    "forge-ui --probe could not seed the isolated daemon: $(tail -c 400 "$TMP/tauri-probe.err")" \
                    >"$TMP/tauri.tsv"
            else
                FORGE_SOCKET="$SOCK" FORGE_HOME="$HOME_DIR" "$LATENCY" \
                    >"$TMP/tauri.out" 2>"$TMP/tauri.err" &
                LATENCY_PID=$!
                (
                    deadline=$(( $(now_ms) + ${PERF_TAURI_TIMEOUT:-180} * 1000 ))
                    while kill -0 "$LATENCY_PID" 2>/dev/null; do
                        if [ "$(now_ms)" -ge "$deadline" ]; then
                            kill -9 "$LATENCY_PID" 2>/dev/null
                            break
                        fi
                        sleep 0.5
                    done
                ) &
                WATCHDOG=$!
                wait "$LATENCY_PID"
                latency_code=$?
                kill "$WATCHDOG" 2>/dev/null
                WATCHDOG=""
                LATENCY_PID=""
                printf '%s\n' "$latency_code" >"$TMP/tauri.code"
                if [ "$latency_code" -eq 0 ]; then
                    printf 'measured\t%s\t%s\t%s\n' \
                        "isolated daemon (protocol-matched), same machine" "$LATENCY" "$DAEMON" \
                        >"$TMP/tauri.tsv"
                else
                    printf 'blocked\t%s\t%s\t%s\n' \
                        "forge-tauri-latency exited $latency_code: $(tail -c 400 "$TMP/tauri.err")" \
                        "$LATENCY" "$DAEMON" >"$TMP/tauri.tsv"
                fi
            fi
        fi
        kill "$DAEMON_PID" 2>/dev/null
        wait "$DAEMON_PID" 2>/dev/null
        DAEMON_PID=""
    fi
else
    printf 'skipped\tPERF_SKIP_TAURI=1\n' >"$TMP/tauri.tsv"
fi

python3 - "$TMP" "$REPORT_JSON" "$REPORT_MD" "$BIN" "$ITERS" "$FRAMES" <<'PY'
import json
import os
import re
import sys

tmp, json_path, md_path, binary, iterations, frames = sys.argv[1:7]
report = json.load(open(os.path.join(tmp, "bench.json")))

def read(name, default=""):
    try:
        return open(os.path.join(tmp, name), encoding="utf-8").read()
    except OSError:
        return default

def tail(text, limit=400):
    text = text.strip()
    return text[-limit:] if text else ""

def num(value):
    if value is None or value == "":
        return None
    try:
        return float(value)
    except ValueError:
        return None

startup = {
    "definition": "cold = first launch after the build; warm = the next two, median; launch to the first Drawn frame; RSS 0/1/2 s measured from that first frame, spawn at exec",
    "cold": None,
    "warm": [],
    "warm_median": None,
    "headless_proxy": None,
    "blocked": None,
}
rows = [line.split("\t") for line in read("startup.tsv").splitlines() if "\t" in line]
def run_obj(row):
    label, status, first, spawn, rss0, rss1, rss2 = row[:7]
    marker = None
    if len(row) > 7 and row[7].strip():
        try:
            marker = json.loads(row[7])
        except json.JSONDecodeError:
            marker = {"raw": row[7]}
    obj = {
        "label": label,
        "status": status,
        "launch_to_first_frame_ms": num(first),
        "rss_mib": {
            "spawn": num(spawn),
            "0s": num(rss0),
            "1s": num(rss1),
            "2s": num(rss2),
        },
        "marker": marker,
    }
    if status != "ok":
        err = read(f"startup-{label}.err")
        obj["blocked"] = tail(err) or tail(read(f"startup-{label}.out"))
    return obj

if rows:
    startup["cold"] = run_obj(rows[0])
    startup["warm"] = [run_obj(row) for row in rows[1:]]
    ok = [row for row in startup["warm"] if row["status"] == "ok"]
    if ok:
        def median(values):
            values = sorted(v for v in values if v is not None)
            if not values:
                return None
            middle = len(values) // 2
            if len(values) % 2:
                return values[middle]
            return (values[middle - 1] + values[middle]) / 2
        startup["warm_median"] = {
            "launch_to_first_frame_ms": median([r["launch_to_first_frame_ms"] for r in ok]),
            "rss_mib": {
                key: median([r["rss_mib"][key] for r in ok])
                for key in ("spawn", "0s", "1s", "2s")
            },
        }
    blocked = [r["blocked"] for r in [startup["cold"], *startup["warm"]] if r and r["status"] != "ok"]
    if blocked:
        startup["blocked"] = blocked[0]

headless = read("headless.tsv").splitlines()
if headless:
    code, elapsed = (headless[0].split("\t") + ["", ""])[:2]
    marker = None
    if len(headless) > 1:
        line = headless[1]
        line = line[len("rune-first-pixels "):] if line.startswith("rune-first-pixels ") else line
        try:
            marker = json.loads(line)
        except json.JSONDecodeError:
            marker = {"raw": line}
    startup["headless_proxy"] = {
        "command": f"{binary} --fixture --frames 2 --fps 125 --first-pixels",
        "exit_code": int(code) if code.isdigit() else None,
        "process_ms": int(elapsed) if elapsed.isdigit() else None,
        "marker": marker,
        "note": "headless has no window or display; this is a first-frame proxy, not display readiness",
    }

tauri_rows = [line.split("\t") for line in read("tauri.tsv").splitlines() if "\t" in line]
tauri = {
    "canonical_command": "make latency-tauri",
    "status": "blocked",
    "blocker": "the harness did not run",
    "command": None,
    "p50_ms": None,
    "p95_ms": None,
    "p99_ms": None,
    "samples": None,
    "lost": None,
    "frame_bytes": None,
    "latency_binary": None,
    "daemon_binary": None,
    "note": "the latency tool creates and kills a shell session; this harness only points it at a temporary daemon",
}
if tauri_rows:
    status, note = tauri_rows[0][0], tauri_rows[0][1]
    if len(tauri_rows[0]) > 2 and tauri_rows[0][2]:
        tauri["latency_binary"] = tauri_rows[0][2]
    if len(tauri_rows[0]) > 3 and tauri_rows[0][3]:
        tauri["daemon_binary"] = tauri_rows[0][3]
    tauri["status"] = status
    tauri["blocker"] = None if status == "measured" else note
    if status == "measured":
        tauri["command"] = "forge-tauri-latency with an isolated forge-daemon (prebuilt binaries)"
        tauri["note"] = note
        stdout = read("tauri.out")
        match = re.search(r"key-to-delta: p50 ([\d.]+) ms .*? p95 ([\d.]+) ms .*? p99 ([\d.]+) ms", stdout)
        if match:
            tauri["p50_ms"], tauri["p95_ms"], tauri["p99_ms"] = (float(v) for v in match.groups())
        samples = re.search(r"samples: (\d+) \(lost (\d+)\)", stdout)
        if samples:
            tauri["samples"], tauri["lost"] = int(samples.group(1)), int(samples.group(2))
        wire = re.search(r"frame on the wire: mean (\d+) B . max (\d+) B", stdout)
        if wire:
            tauri["frame_bytes"] = {"mean": int(wire.group(1)), "max": int(wire.group(2))}
        tauri["stdout"] = stdout.strip()
    else:
        tauri["blocker"] = note

report["startup"] = startup
report["tauri"] = tauri
report["harness"] = {
    "binary": binary,
    "iterations": int(iterations),
    "frames": int(frames),
    "wrapper": "apps/ui/scripts/perf.sh",
    "note": "terminal and editor samples are CPU clocks on a headless GPU; display vsync is not measured",
}
json.dump(report, open(json_path, "w"), indent=2)
open(json_path, "a").write("\n")

def ms(sample):
    if not sample:
        return "—"
    return f"{sample['p50']:.3f} / {sample['p95']:.3f} / {sample['max']:.3f}"

def rss(value):
    return "—" if value is None else f"{value:.2f}"

fixture = report["fixture"]
terminal = report["terminal_125"]
editor = report["editor"]
machine = report["machine"]
lines = []
lines.append("# forge-ui perf report")
lines.append("")
lines.append(f"Generated from `{os.path.basename(binary)}` ({machine['profile']}, Rust {machine['rust']}, Rune `{machine['rune'][:12]}`) on {machine['os']}/{machine['arch']}.")
lines.append("")
lines.append("| Fixture | p50 / p95 / max ms |")
lines.append("|---|---:|")
lines.append(f"| Session switch (click → painted frame) | {ms(fixture['switch_ms'])} |")
lines.append(f"| View build | {ms(fixture['view_ms'])} |")
lines.append(f"| Terminal echo (key → grid → frame) | {ms(fixture['echo_ms'])} |")
lines.append(f"| Socket ops / on the UI thread | {fixture['socket_ops']} / {fixture['ui_socket_ops']} |")
lines.append("")
lines.append(f"## Integrated terminal stream ({terminal['rate_hz']:.0f} Hz, {terminal['cols']}×{terminal['rows']}, {terminal['feed']})")
lines.append("")
if terminal.get("blocked"):
    lines.append(f"**Blocked:** {terminal['blocked']}")
else:
    lines.append("| Clock | p50 / p95 / max ms |")
    lines.append("|---|---:|")
    lines.append(f"| Delta apply (`SharedTerminal`) | {ms(terminal['delta_apply_ms'])} |")
    lines.append(f"| Rune update (poll+view+layout+paint) | {ms(terminal['update_ms'])} |")
    lines.append(f"| Terminal build (diff+encode+upload) | {ms(terminal['build_ms'])} |")
    lines.append(f"| Terminal encode (pass+draw) | {ms(terminal['encode_ms'])} |")
    lines.append(f"| Frame CPU (update+build+encode) | {ms(terminal['frame_cpu_ms'])} |")
    lines.append(f"| Host frame (external, update→submit) | {ms(terminal['host_frame_ms'])} |")
    lines.append(f"| Total CPU/frame (+ delta apply) | {ms(terminal['total_cpu_ms'])} |")
    lines.append("")
    lines.append(f"{terminal['deltas_applied']} deltas, {terminal['view_commits']} shell view commits, {terminal['rows_uploaded']} rows uploaded, {terminal['instances_per_draw']} instances/draw, {terminal['glyph_misses']} glyph misses, {terminal['build_allocations']} build allocations ({terminal['encode_allocations']} in cell encoding); GPU {terminal['gpu']} (init {terminal['gpu_init_ms']:.1f} ms).")
lines.append("")
lines.append(f"## Editor ({editor['rows']} mounted rows, {editor['folds']} foldable, {editor['rate_hz']:.0f} Hz)")
lines.append("")
if editor.get("blocked"):
    lines.append(f"**Blocked:** {editor['blocked']}")
else:
    lines.append("| Clock | p50 / p95 / max ms |")
    lines.append("|---|---:|")
    lines.append(f"| Mount (first frame) | {editor['mount_ms']:.3f} ms; view {editor['mount_view_ms']:.3f} ms |")
    lines.append(f"| Caret-only view build | {ms(editor['caret_view_ms'])} |")
    lines.append(f"| Caret-only frame CPU | {ms(editor['caret_frame_ms'])} |")
    lines.append(f"| Key-to-paint CPU proxy | {ms(editor['key_to_paint_cpu_ms'])} |")
    lines.append("")
    lines.append(f"Rows rebuilt: mount {editor['mount_rows_built']}, caret-only {editor['caret_rows_rebuilt']}, key-to-paint {editor['key_to_paint_rows_rebuilt']}. Display/vsync is **not measured**; key-to-paint is the named CPU proxy.")
lines.append("")
lines.append("## Startup / RSS")
lines.append("")
if startup.get("cold"):
    cold = startup["cold"]
    if cold["status"] == "ok":
        rss_values = cold["rss_mib"]
        lines.append(f"- Cold (first launch after build): {cold['launch_to_first_frame_ms']} ms to first drawn frame; RSS spawn / 0 / 1 / 2 s {rss(rss_values['spawn'])} / {rss(rss_values['0s'])} / {rss(rss_values['1s'])} / {rss(rss_values['2s'])} MiB")
    else:
        lines.append(f"- Cold: blocked — {cold.get('blocked', 'no first frame')}")
if startup.get("warm_median"):
    warm = startup["warm_median"]
    rss_values = warm["rss_mib"]
    lines.append(f"- Warm median (two launches): {warm['launch_to_first_frame_ms']} ms; RSS spawn / 0 / 1 / 2 s {rss(rss_values['spawn'])} / {rss(rss_values['0s'])} / {rss(rss_values['1s'])} / {rss(rss_values['2s'])} MiB")
if startup.get("headless_proxy"):
    proxy = startup["headless_proxy"]
    marker = proxy.get("marker") or {}
    first = marker.get("headless_ms") or marker.get("frame_cpu_ms")
    lines.append(f"- Headless proxy: process {proxy['process_ms']} ms, first-frame marker {first} ms (no window/display)")
lines.append("")
lines.append("## Tauri baseline")
lines.append("")
if tauri["status"] == "measured":
    lines.append(f"- Command: {tauri['command']} (canonical: `{tauri['canonical_command']}`)")
    if tauri.get("latency_binary"):
        lines.append(f"- Binary: `{tauri['latency_binary']}` against `{tauri['daemon_binary']}`")
    lines.append(f"- key-to-delta p50 / p95 / p99: **{tauri['p50_ms']} / {tauri['p95_ms']} / {tauri['p99_ms']} ms** over {tauri['samples']} samples, {tauri['lost']} lost")
    if tauri.get("frame_bytes"):
        lines.append(f"- Frame on the wire: mean {tauri['frame_bytes']['mean']} B, max {tauri['frame_bytes']['max']} B")
    lines.append(f"- {tauri['note']}")
else:
    lines.append(f"- Canonical command: `{tauri['canonical_command']}`")
    lines.append(f"- **Blocked:** {tauri['blocker']}")
lines.append("")
lines.append("## Measured vs proxy")
lines.append("")
lines.append("- Fixture switch, echo, terminal delta apply, terminal build/encode and the editor view are wall-clock CPU measurements.")
lines.append("- Terminal build/encode are the compositor's CPU clocks; GPU time and presentation are not included.")
lines.append("- Editor key-to-paint is a CPU proxy (key handler → one 125 Hz frame); it is not a display measurement.")
lines.append("- Startup windowed runs measure launch → the first `FrameEncoded` reported as `Drawn`; cold means first after the build, not an OS page-cache purge.")
open(md_path, "w").write("\n".join(lines) + "\n")
print(f"perf: wrote {json_path}")
print(f"perf: wrote {md_path}")
PY

echo "perf: report $REPORT_JSON"
echo "perf: summary $REPORT_MD"
exit 0
