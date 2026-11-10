#!/usr/bin/env bash
# Perf harness: one command, one machine-readable report.
#
#   apps/ui/scripts/perf.sh
#
# Writes perf-report.json and perf-report.md beside this script. Sections:
# fixture switch/echo, integrated 125 Hz terminal, 88-row editor and
# startup/RSS. It never points anything at the user's live daemon: a temporary
# socket and data directory are used.
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
WATCHDOG=""
cleanup() {
    [ -n "$WATCHDOG" ] && kill "$WATCHDOG" 2>/dev/null
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null
    # Only the directory this script made.
    case "$TMP" in
        */forge-ui-perf.*) rm -rf "$TMP" ;;
    esac
}
trap cleanup EXIT

now_ms() {
    bun -e 'console.log(Date.now())'
}

rss_mib() {
    local kb
    kb="$(ps -o rss= -p "$1" 2>/dev/null | tr -d ' ')"
    if [ -n "$kb" ]; then
        # KiB to MiB at two places, ties to even: kb * 100 / 1024 = kb * 25 / 256.
        local hundredths=$((kb * 25 / 256)) rem=$((kb * 25 % 256)) frac
        if [ $((rem * 2)) -gt 256 ] || { [ $((rem * 2)) -eq 256 ] && [ $((hundredths % 2)) -eq 1 ]; }; then
            hundredths=$((hundredths + 1))
        fi
        frac="$(printf '%02d' $((hundredths % 100)))"
        printf '%d.%s\n' $((hundredths / 100)) "${frac%0}"
    else
        printf ''
    fi
}

wait_until() {
    local launch="$1" target="$2" now rem
    now="$(now_ms)"
    rem=$((target - (now - launch)))
    if [ "$rem" -gt 0 ]; then
        sleep "$(printf '%d.%03d' $((rem / 1000)) $((rem % 1000)))"
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

bun "$SCRIPT_DIR/perf-report.ts" "$TMP" "$REPORT_JSON" "$REPORT_MD" "$BIN" "$ITERS" "$FRAMES"

echo "perf: report $REPORT_JSON"
echo "perf: summary $REPORT_MD"
exit 0
