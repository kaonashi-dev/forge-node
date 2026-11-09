#!/usr/bin/env bash
# Head-to-head: the shipping Tauri bundle against the Rune bundle, measured the
# same external way against one isolated daemon.
#
#   apps/ui/scripts/compare-tauri.sh            # measure the existing bundles
#   COMPARE_BUILD=1 apps/ui/scripts/compare-tauri.sh   # rebuild stale bundles first
#
# Writes compare-report.md and compare-report.json beside this script. Knobs:
#   COMPARE_TAURI_APP / COMPARE_RUNE_APP  bundles (dist/macos/Forge Node.app, dist/rune/Forge Node Rune.app)
#   COMPARE_DAEMON_BIN    the one daemon both apps share (default: the Tauri bundle's)
#   COMPARE_WARM=5        warm launches per app (median)
#   COMPARE_IDLE_SECS=60  idle CPU window, starting 10 s after launch
#   COMPARE_STREAM_SECS=30
#   COMPARE_ORDER="tauri rune"
#   COMPARE_LABEL         free text recorded in the report (e.g. "quiet machine")
#
# Both apps run from copies under a temp directory with a distinct bundle id, so
# neither shares WebKit storage or LaunchServices identity with an installed
# Forge Node. The daemon gets its own FORGE_HOME and socket; the user's daemon,
# ~/.forge-node and /Applications are never touched.
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
UI_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
ROOT="$(cd "$UI_DIR/../.." && pwd)"
REPORT_JSON="$SCRIPT_DIR/compare-report.json"
REPORT_MD="$SCRIPT_DIR/compare-report.md"

TAURI_APP="${COMPARE_TAURI_APP:-$ROOT/dist/macos/Forge Node.app}"
RUNE_APP="${COMPARE_RUNE_APP:-$ROOT/dist/rune/Forge Node Rune.app}"
WARM="${COMPARE_WARM:-5}"
IDLE_SECS="${COMPARE_IDLE_SECS:-60}"
STREAM_SECS="${COMPARE_STREAM_SECS:-30}"
ORDER="${COMPARE_ORDER:-tauri rune}"
LAUNCH_TIMEOUT_MS="${COMPARE_LAUNCH_TIMEOUT_MS:-30000}"

die() { printf 'compare: %s\n' "$*" >&2; exit 2; }
note() { printf 'compare: %s\n' "$*"; }

[ "$(uname -s)" = Darwin ] || die "macOS only"
for tool in swiftc python3 footprint git du shasum plutil; do
    command -v "$tool" >/dev/null 2>&1 || die "missing $tool"
done

SDKROOT="${SDKROOT:-$("$ROOT/scripts/macos-sdk" 2>/dev/null)}"
export SDKROOT

# A bundle is stale when a commit after its build touched what it is built from.
stale_commits() {
    local app="$1"
    shift
    local built
    built="$(stat -f %m "$app/Contents/Info.plist" 2>/dev/null)" || { echo missing; return; }
    git -C "$ROOT" log --since="@$built" --format=%h -- "$@" | wc -l | tr -d ' '
}
TAURI_STALE="$(stale_commits "$TAURI_APP" apps/tauri crates Cargo.toml Cargo.lock)"
RUNE_STALE="$(stale_commits "$RUNE_APP" apps/ui crates Cargo.toml Cargo.lock)"
if [ "${COMPARE_BUILD:-0}" = 1 ]; then
    if [ "$TAURI_STALE" != 0 ]; then
        note "building the Tauri bundle (scripts/package-macos)"
        FORGE_SKIP_FRONTEND_INSTALL="${FORGE_SKIP_FRONTEND_INSTALL:-1}" "$ROOT/scripts/package-macos" || die "package-macos failed"
        TAURI_STALE=0
    fi
    if [ "$RUNE_STALE" != 0 ]; then
        note "building the Rune bundle (scripts/rune-app)"
        CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$(dirname "$ROOT")/forge-node-rune-target}" "$ROOT/scripts/rune-app" || die "rune-app failed"
        RUNE_STALE=0
    fi
fi
[ -x "$TAURI_APP/Contents/MacOS/Forge" ] || die "no Tauri bundle at $TAURI_APP (scripts/package-macos)"
[ -x "$RUNE_APP/Contents/MacOS/ForgeNodeRune" ] || die "no Rune bundle at $RUNE_APP (scripts/rune-app)"
DAEMON_SRC="${COMPARE_DAEMON_BIN:-$TAURI_APP/Contents/MacOS/forge-daemon}"
FORGECTL_SRC="$TAURI_APP/Contents/MacOS/forgectl"
[ -x "$DAEMON_SRC" ] || die "no daemon at $DAEMON_SRC"
[ -x "$FORGECTL_SRC" ] || die "no forgectl at $FORGECTL_SRC"

# Short: the socket path must fit sun_path (104 bytes).
TMP="$(mktemp -d /tmp/forge-compare.XXXXXX)" || die "mktemp failed"
FORGE_HOME_DIR="$TMP/home"
SOCK="$FORGE_HOME_DIR/daemon.sock"
RESULTS="$TMP/results.tsv"
DAEMON_PID=""
APP_PIDS=""
REGISTERED_ARR=()
: >"$RESULTS"

# Every process this script started, and only those: app pids it spawned, the
# processes that name one of them as responsible, and the daemon tree.
cleanup() {
    local pid
    for pid in $APP_PIDS; do
        stop_app "$pid"
    done
    if [ -n "$DAEMON_PID" ]; then
        kill -TERM "$DAEMON_PID" 2>/dev/null
        for _ in $(seq 1 50); do kill -0 "$DAEMON_PID" 2>/dev/null || break; sleep 0.1; done
        kill -KILL "$DAEMON_PID" 2>/dev/null
        # Shells it spawned have their own sessions; anything still holding the
        # temp tree belongs to this run.
        pkill -KILL -f "$TMP/" 2>/dev/null
    fi
    for app in ${REGISTERED_ARR[@]+"${REGISTERED_ARR[@]}"}; do
        /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -u "$app" >/dev/null 2>&1
    done
    case "$TMP" in
        /tmp/forge-compare.*) rm -rf "$TMP" ;;
    esac
}
trap cleanup EXIT
trap 'exit 130' INT TERM

record() { printf '%s\t%s\n' "$1" "$2" >>"$RESULTS"; }

note "compiling the probe helper"
HELPER="$TMP/helper"
swiftc ${SDKROOT:+-sdk "$SDKROOT"} -O "$SCRIPT_DIR/compare-helper.swift" -o "$HELPER" 2>"$TMP/swiftc.err" \
    || die "swiftc failed: $(tail -5 "$TMP/swiftc.err")"

app_set() {
    local app="$1"
    kill -0 "$app" 2>/dev/null || return 0
    printf '%s\n' "$app"
    "$HELPER" resp "$app" | grep -vx "${DAEMON_PID:-x}"
}

stop_app() {
    local app="$1" set pid
    set="$(app_set "$app")"
    [ -z "$set" ] && return 0
    kill -TERM "$app" 2>/dev/null
    for _ in $(seq 1 50); do kill -0 "$app" 2>/dev/null || break; sleep 0.1; done
    kill -KILL "$app" 2>/dev/null
    # WebKit helpers outlive a killed host for a moment; they are this app's.
    for pid in $set; do
        [ "$pid" = "$app" ] && continue
        kill -0 "$pid" 2>/dev/null && kill -KILL "$pid" 2>/dev/null
    done
    APP_PIDS="$(printf '%s\n' $APP_PIDS | grep -vx "$app" | tr '\n' ' ')"
}

cpu_snapshot() { "$HELPER" cpu "$@"; }

# Bytes of phys_footprint summed over a set, and the per-process listing.
footprint_sum() {
    local out="$1"
    shift
    footprint --noCategories -f bytes -j "$out" "$@" >/dev/null 2>&1
    python3 - "$out" <<'PY'
import json, sys
try:
    data = json.load(open(sys.argv[1]))
except Exception:
    print("")
    sys.exit(0)
procs = data.get("processes", [])
print(sum(p.get("footprint", 0) for p in procs), ";".join(f"{p.get('name')}:{p.get('pid')}:{p.get('footprint', 0)}" for p in procs))
PY
}

# Seconds of CPU the set used between two snapshots, over wall seconds.
cpu_percent() {
    python3 - "$1" "$2" "$3" <<'PY'
import sys
def load(path):
    out = {}
    for line in open(path):
        parts = line.split()
        if len(parts) == 2:
            out[parts[0]] = int(parts[1])
    return out
before, after, wall = load(sys.argv[1]), load(sys.argv[2]), float(sys.argv[3])
used = sum(max(ns - before.get(pid, 0), 0) for pid, ns in after.items())
print(round(used / 1e9 / wall * 100, 2))
PY
}

# time.time, not time.monotonic: the system Python 3.9 counts monotonic from
# its own process start, so two invocations cannot be subtracted.
now_s() { python3 -c 'import time; print(f"{time.time():.3f}")'; }
sleep_until() {
    local start="$1" offset="$2"
    python3 -c "import time; d = $start + $offset - time.time(); time.sleep(d) if d > 0 else None"
}

# ---------------------------------------------------------------- bundles ---

note "bundle sizes"
for which in tauri rune; do
    if [ "$which" = tauri ]; then app="$TAURI_APP"; gui="Forge"; else app="$RUNE_APP"; gui="forge-ui"; fi
    record "$which.bundle.path" "$app"
    record "$which.bundle.total_kib" "$(du -sk "$app" | cut -f1)"
    for file in "$app"/Contents/MacOS/*; do
        record "$which.bundle.file.$(basename "$file")" "$(du -sk "$file" | cut -f1)"
    done
    record "$which.bundle.resources_kib" "$(du -sk "$app/Contents/Resources" 2>/dev/null | cut -f1)"
    record "$which.bundle.gui" "$gui"
    record "$which.bundle.version" "$(plutil -extract CFBundleShortVersionString raw -o - "$app/Contents/Info.plist" 2>/dev/null)"
    record "$which.bundle.built" "$(date -r "$(stat -f %m "$app/Contents/Info.plist")" '+%Y-%m-%dT%H:%M:%S%z')"
    record "$which.bundle.gui_sha256" "$(shasum -a 256 "$app/Contents/MacOS/$gui" | cut -c1-16)"
    record "$which.bundle.daemon_sha256" "$(shasum -a 256 "$app/Contents/MacOS/forge-daemon" | cut -c1-16)"
done
record tauri.bundle.stale_commits "$TAURI_STALE"
record rune.bundle.stale_commits "$RUNE_STALE"
[ -d "$ROOT/apps/tauri/dist" ] && record tauri.frontend_dist_kib "$(du -sk "$ROOT/apps/tauri/dist" | cut -f1)"

# Copies with their own bundle ids, so WebKit storage and LaunchServices
# identity are this run's alone.
install_copy() {
    local src="$1" dest="$2" id="$3"
    ditto "$src" "$dest" || die "copy $src failed"
    REGISTERED_ARR+=("$dest")
    plutil -replace CFBundleIdentifier -string "$id" "$dest/Contents/Info.plist" || die "plist $dest"
}
mkdir -p "$TMP/apps"
install_copy "$TAURI_APP" "$TMP/apps/Forge Node.app" "dev.forgenode.compare.tauri.$$"
install_copy "$RUNE_APP" "$TMP/apps/Forge Node Rune.app" "dev.forgenode.compare.rune.$$"
TAURI_EXE="$TMP/apps/Forge Node.app/Contents/MacOS/Forge"
RUNE_EXE="$TMP/apps/Forge Node Rune.app/Contents/MacOS/ForgeNodeRune"
cp "$DAEMON_SRC" "$TMP/forge-daemon"
cp "$FORGECTL_SRC" "$TMP/forgectl"
DAEMON="$TMP/forge-daemon"
FORGECTL="$TMP/forgectl"

# ----------------------------------------------------------------- daemon ---

note "isolated daemon at $SOCK"
mkdir -p "$FORGE_HOME_DIR"
printf '[editor]\nexecutable = "%s"\n' "$TMP/apps/Forge Node.app/Contents/MacOS/forge-editor" >"$FORGE_HOME_DIR/config.toml"
export FORGE_HOME="$FORGE_HOME_DIR" FORGE_SOCKET="$SOCK" FORGE_RUNE_HOME="$FORGE_HOME_DIR"
unset FORGE_DAEMON_BIN FORGE_SESSION_ID FORGE_WORKSPACE FORGE_RUN_ID FORGE_TASK_ID FORGE_ATTEMPT_ID FORGECTL_JSON
"$DAEMON" run >"$TMP/daemon.log" 2>&1 &
DAEMON_PID=$!
for _ in $(seq 1 200); do [ -S "$SOCK" ] && break; sleep 0.05; done
[ -S "$SOCK" ] || die "the isolated daemon did not open $SOCK: $(tail -5 "$TMP/daemon.log")"

REPO="$TMP/repo"
mkdir -p "$REPO"
git -C "$REPO" init -q -b main
printf '# compare\n' >"$REPO/README.md"
git -C "$REPO" -c user.name=compare -c user.email=compare@localhost -c commit.gpgsign=false add README.md
git -C "$REPO" -c user.name=compare -c user.email=compare@localhost -c commit.gpgsign=false commit -qm init

PROTOCOL="$("$FORGECTL" --json status 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["result"]["protocol_version"])' 2>/dev/null)"
[ -n "$PROTOCOL" ] || die "forgectl status failed against $SOCK (protocol mismatch between the daemon and forgectl?)"
record daemon.protocol "$PROTOCOL"

# forgectl has no add-project, so AddProject goes over the wire: a 4-byte
# big-endian length and a MessagePack body (serde, struct-as-map).
python3 - "$SOCK" "$REPO" "$PROTOCOL" <<'PY' || die "AddProject failed"
import socket, struct, sys

def pack(v):
    if v is None: return b"\xc0"
    if v is True: return b"\xc3"
    if v is False: return b"\xc2"
    if isinstance(v, int):
        if 0 <= v < 128: return bytes([v])
        return b"\xcf" + struct.pack(">Q", v)
    if isinstance(v, str):
        b = v.encode()
        if len(b) < 32: return bytes([0xa0 | len(b)]) + b
        if len(b) < 256: return b"\xd9" + bytes([len(b)]) + b
        return b"\xda" + struct.pack(">H", len(b)) + b
    if isinstance(v, dict):
        out = bytes([0x80 | len(v)]) if len(v) < 16 else b"\xde" + struct.pack(">H", len(v))
        for k, x in v.items(): out += pack(k) + pack(x)
        return out
    raise TypeError(v)

class Reader:
    def __init__(self, b): self.b, self.i = b, 0
    def take(self, n):
        s = self.b[self.i:self.i + n]; self.i += n; return s
    def u(self, fmt, n): return struct.unpack(fmt, self.take(n))[0]
    def read(self):
        t = self.take(1)[0]
        if t < 0x80: return t
        if t >= 0xe0: return t - 0x100
        if 0x80 <= t <= 0x8f: return self.map(t & 0x0f)
        if 0x90 <= t <= 0x9f: return [self.read() for _ in range(t & 0x0f)]
        if 0xa0 <= t <= 0xbf: return self.take(t & 0x1f).decode("utf-8", "replace")
        simple = {0xc0: None, 0xc2: False, 0xc3: True}
        if t in simple: return simple[t]
        sizes = {0xc4: ">B", 0xc5: ">H", 0xc6: ">I"}
        if t in sizes: return self.take(self.u(sizes[t], struct.calcsize(sizes[t])))
        exts = {0xc7: ">B", 0xc8: ">H", 0xc9: ">I"}
        if t in exts:
            n = self.u(exts[t], struct.calcsize(exts[t])); self.take(1); return self.take(n)
        fixext = {0xd4: 1, 0xd5: 2, 0xd6: 4, 0xd7: 8, 0xd8: 16}
        if t in fixext: self.take(1); return self.take(fixext[t])
        nums = {0xca: ">f", 0xcb: ">d", 0xcc: ">B", 0xcd: ">H", 0xce: ">I", 0xcf: ">Q",
                0xd0: ">b", 0xd1: ">h", 0xd2: ">i", 0xd3: ">q"}
        if t in nums: return self.u(nums[t], struct.calcsize(nums[t]))
        strs = {0xd9: ">B", 0xda: ">H", 0xdb: ">I"}
        if t in strs: return self.take(self.u(strs[t], struct.calcsize(strs[t]))).decode("utf-8", "replace")
        if t == 0xdc: return [self.read() for _ in range(self.u(">H", 2))]
        if t == 0xdd: return [self.read() for _ in range(self.u(">I", 4))]
        if t == 0xde: return self.map(self.u(">H", 2))
        if t == 0xdf: return self.map(self.u(">I", 4))
        raise ValueError(hex(t))
    def map(self, n):
        out = {}
        for _ in range(n):
            k = self.read()
            out[k if isinstance(k, (str, int)) else repr(k)] = self.read()
        return out

sock_path, repo = sys.argv[1], sys.argv[2]
s = socket.socket(socket.AF_UNIX); s.settimeout(30); s.connect(sock_path)
def send(msg):
    body = pack(msg); s.sendall(struct.pack(">I", len(body)) + body)
def recv():
    head = b""
    while len(head) < 4: head += s.recv(4 - len(head))
    n = struct.unpack(">I", head)[0]; body = b""
    while len(body) < n: body += s.recv(n - len(body))
    return Reader(body).read()

send({"Hello": {"protocol_version": int(sys.argv[3]),
                "client_version": "compare", "client_kind": "Cli"}})
ack = recv()
if "HelloAck" not in ack:
    sys.exit(f"hello: {ack}")
send({"Request": {"request_id": 1, "body": {"AddProject": {"path": repo}}}})
while True:
    msg = recv()
    if isinstance(msg, dict) and "Response" in msg and msg["Response"].get("request_id") == 1:
        body = msg["Response"]["body"]
        if isinstance(body, dict) and "Err" in body: sys.exit(f"AddProject: {body['Err']}")
        break
PY

SESSION_JSON="$("$FORGECTL" --json session shell --project "$REPO" --name compare 2>&1)" \
    || die "forgectl session shell: $SESSION_JSON"
SESSION_ID="$(printf '%s' "$SESSION_JSON" | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r.get("session_id") or r.get("id") or r.get("session",{}).get("id",""))')"
[ -n "$SESSION_ID" ] || die "no session id in: $SESSION_JSON"
record daemon.session "$SESSION_ID"

# The streaming load: colored lines as fast as /bin/sh can print them, then a
# marker file. sh, because the session runs the user's login shell.
cat >"$TMP/stream.sh" <<'SH'
end=$(( $(date +%s) + $1 ))
i=0
while [ "$(date +%s)" -lt "$end" ]; do
    j=0
    while [ $j -lt 200 ]; do
        printf '\033[3%dm%08d \033[1mforge\033[22m compare stream \033[4m%s\033[24m the quick brown fox jumps over the lazy dog\033[0m\n' $((j % 8)) $i "$1"
        j=$((j + 1)); i=$((i + 1))
    done
done
printf '%s\n' "$i" >"$2"
SH

shell_exec() {
    local tries=0 out
    while :; do
        out="$("$FORGECTL" --json session exec "$SESSION_ID" --text "$1" 2>&1)" && return 0
        tries=$((tries + 1))
        # The shell may not be back at its prompt yet.
        [ "$tries" -ge 40 ] && { printf '%s\n' "$out" >&2; return 1; }
        sleep 0.25
    done
}

wait_file() {
    for _ in $(seq 1 "$2"); do [ -s "$1" ] && return 0; sleep 0.25; done
    return 1
}

# --------------------------------------------------------------- per app ---

launch() {
    local which="$1" exe out
    if [ "$which" = tauri ]; then exe="$TAURI_EXE"; else exe="$RUNE_EXE"; fi
    out="$("$HELPER" launch "$LAUNCH_TIMEOUT_MS" "$TMP/$which-app.log" "$exe")"
    LAUNCH_PID="$(printf '%s\n' "$out" | awk '$1 == "pid" { print $2 }')"
    LAUNCH_MS="$(printf '%s\n' "$out" | awk '$1 == "window_ms" { print $2 }')"
    LAUNCH_DISCLAIMED="$(printf '%s\n' "$out" | awk '$1 == "disclaimed" { print $2 }')"
    [ -n "$LAUNCH_PID" ] && APP_PIDS="$APP_PIDS $LAUNCH_PID"
}

measure_app() {
    local which="$1" start fp set n
    note "$which: cold launch"
    launch "$which"
    local app="$LAUNCH_PID"
    record "$which.startup.cold_ms" "$LAUNCH_MS"
    record "$which.disclaimed" "$LAUNCH_DISCLAIMED"
    [ -n "$app" ] && [ "$LAUNCH_MS" != -1 ] || { record "$which.blocked" "no window within ${LAUNCH_TIMEOUT_MS} ms: $(tail -c 300 "$TMP/$which-app.log")"; stop_app "$app"; return; }
    start="$(now_s)"

    sleep_until "$start" 10
    set="$(app_set "$app")"
    fp="$(footprint_sum "$TMP/$which-fp10.json" $set)"
    record "$which.footprint10.bytes" "${fp%% *}"
    record "$which.footprint10.procs" "${fp#* }"
    record "$which.window" "$("$HELPER" windows "$app")"
    cpu_snapshot $set "$DAEMON_PID" >"$TMP/$which-idle0.cpu"
    local idle_t0
    idle_t0="$(now_s)"

    sleep_until "$start" 60
    fp="$(footprint_sum "$TMP/$which-fp60.json" $(app_set "$app"))"
    record "$which.footprint60.bytes" "${fp%% *}"
    record "$which.footprint60.procs" "${fp#* }"

    sleep_until "$idle_t0" "$IDLE_SECS"
    set="$(app_set "$app")"
    cpu_snapshot $set >"$TMP/$which-idle1.cpu"
    cpu_snapshot "$DAEMON_PID" >"$TMP/$which-idle1.daemon"
    local wall
    wall="$(python3 -c "print($(now_s) - $idle_t0)")"
    grep -v "^$DAEMON_PID " "$TMP/$which-idle0.cpu" >"$TMP/$which-idle0.app"
    grep "^$DAEMON_PID " "$TMP/$which-idle0.cpu" >"$TMP/$which-idle0.daemon"
    record "$which.idle.cpu_pct" "$(cpu_percent "$TMP/$which-idle0.app" "$TMP/$which-idle1.cpu" "$wall")"
    record "$which.idle.daemon_cpu_pct" "$(cpu_percent "$TMP/$which-idle0.daemon" "$TMP/$which-idle1.daemon" "$wall")"
    record "$which.idle.wall_s" "$wall"
    record "$which.idle.procs" "$(printf '%s ' $set)"

    # The PTY size proves the app attached: it is the app's window, not the default.
    rm -f "$TMP/$which-size"
    if shell_exec "stty size > $TMP/$which-size"; then
        wait_file "$TMP/$which-size" 40 && record "$which.pty_size" "$(cat "$TMP/$which-size")"
    fi

    note "$which: streaming for ${STREAM_SECS}s"
    rm -f "$TMP/$which-stream.done"
    set="$(app_set "$app")"
    cpu_snapshot $set >"$TMP/$which-stream0.app"
    cpu_snapshot "$DAEMON_PID" >"$TMP/$which-stream0.daemon"
    local s0
    s0="$(now_s)"
    if shell_exec "sh $TMP/stream.sh $STREAM_SECS $TMP/$which-stream.done"; then
        wait_file "$TMP/$which-stream.done" $(( (STREAM_SECS + 30) * 4 )) || record "$which.stream.blocked" "the stream did not finish"
        wall="$(python3 -c "print($(now_s) - $s0)")"
        cpu_snapshot $(app_set "$app") >"$TMP/$which-stream1.app"
        cpu_snapshot "$DAEMON_PID" >"$TMP/$which-stream1.daemon"
        record "$which.stream.cpu_pct" "$(cpu_percent "$TMP/$which-stream0.app" "$TMP/$which-stream1.app" "$wall")"
        record "$which.stream.daemon_cpu_pct" "$(cpu_percent "$TMP/$which-stream0.daemon" "$TMP/$which-stream1.daemon" "$wall")"
        record "$which.stream.wall_s" "$wall"
        record "$which.stream.lines" "$(cat "$TMP/$which-stream.done" 2>/dev/null)"
        fp="$(footprint_sum "$TMP/$which-fpstream.json" $(app_set "$app"))"
        record "$which.footprint_after_stream.bytes" "${fp%% *}"
    else
        record "$which.stream.blocked" "SendShellCommand was refused"
    fi
    stop_app "$app"
    sleep 2

    n=1
    while [ "$n" -le "$WARM" ]; do
        note "$which: warm launch $n/$WARM"
        launch "$which"
        record "$which.startup.warm$n" "$LAUNCH_MS"
        sleep 3
        stop_app "$LAUNCH_PID"
        sleep 2
        n=$((n + 1))
    done
}

for which in $ORDER; do
    measure_app "$which"
done

# Key-to-screen cannot be measured identically: there is no screen capture
# permission here and no shared key-injection path. Record each client's own
# method instead, clearly labelled.
note "per-client latency (different methods)"
RUNE_BIN="$TMP/apps/Forge Node Rune.app/Contents/MacOS/forge-ui"
if "$RUNE_BIN" --fixture --bench 300 --bench-report "$TMP/rune-bench.json" >"$TMP/rune-bench.out" 2>"$TMP/rune-bench.err"; then
    record rune.bench.json "$TMP/rune-bench.json"
else
    record rune.bench.blocked "forge-ui --fixture --bench exited $?: $(tail -c 300 "$TMP/rune-bench.err")"
fi
LATENCY="${COMPARE_TAURI_LATENCY_BIN:-}"
if [ -z "$LATENCY" ]; then
    for profile in release debug; do
        [ -x "$ROOT/target/$profile/forge-tauri-latency" ] && { LATENCY="$ROOT/target/$profile/forge-tauri-latency"; break; }
    done
fi
if [ -n "$LATENCY" ]; then
    if "$LATENCY" >"$TMP/tauri-latency.out" 2>"$TMP/tauri-latency.err"; then
        record tauri.latency.out "$TMP/tauri-latency.out"
        record tauri.latency.bin "$LATENCY"
    else
        record tauri.latency.blocked "$(basename "$LATENCY") exited $?: $(tail -c 300 "$TMP/tauri-latency.err" | tr '\n' ' ')"
    fi
else
    record tauri.latency.blocked "no prebuilt forge-tauri-latency (make latency-tauri)"
fi

python3 - "$RESULTS" "$REPORT_JSON" "$REPORT_MD" "$ROOT" "$IDLE_SECS" "$STREAM_SECS" "$WARM" "$DAEMON_SRC" "${COMPARE_LABEL:-}" <<'PY'
import json, os, platform, re, statistics, subprocess, sys

results, json_path, md_path, root, idle_secs, stream_secs, warm, daemon_src, label = sys.argv[1:10]
kv = {}
for line in open(results):
    if "\t" in line:
        k, v = line.rstrip("\n").split("\t", 1)
        kv[k] = v

def sh(*cmd):
    try:
        return subprocess.run(cmd, capture_output=True, text=True, cwd=root).stdout.strip()
    except OSError:
        return ""

def num(v):
    try:
        return float(v)
    except (TypeError, ValueError):
        return None

def mib(b):
    v = num(b)
    return None if v is None else round(v / 1048576, 1)

def kib_mb(k):
    v = num(k)
    return None if v is None else round(v / 1024, 1)

def app(which):
    files = {k.split(".", 3)[3]: kib_mb(v) for k, v in kv.items() if k.startswith(f"{which}.bundle.file.")}
    gui = kv.get(f"{which}.bundle.gui")
    shared = {n: files[n] for n in ("forge-daemon", "forge-editor", "forgectl") if n in files}
    gui_files = {n: s for n, s in files.items() if n not in shared}
    warm_runs = [num(v) for k, v in kv.items() if re.fullmatch(fr"{which}\.startup\.warm\d+", k)]
    warm_ok = [v for v in warm_runs if v is not None and v >= 0]
    return {
        "bundle": {
            "path": kv.get(f"{which}.bundle.path"),
            "version": kv.get(f"{which}.bundle.version"),
            "built": kv.get(f"{which}.bundle.built"),
            "commits_since_build": kv.get(f"{which}.bundle.stale_commits"),
            "gui_sha256_16": kv.get(f"{which}.bundle.gui_sha256"),
            "daemon_sha256_16": kv.get(f"{which}.bundle.daemon_sha256"),
            "total_mb": kib_mb(kv.get(f"{which}.bundle.total_kib")),
            "gui_mb": round(sum(v for v in gui_files.values() if v), 1),
            "gui_files_mb": gui_files,
            "shared_mb": round(sum(v for v in shared.values() if v), 1),
            "shared_files_mb": shared,
            "resources_mb": kib_mb(kv.get(f"{which}.bundle.resources_kib")),
        },
        "startup_ms": {
            "cold": num(kv.get(f"{which}.startup.cold_ms")),
            "warm_runs": warm_runs,
            "warm_median": statistics.median(warm_ok) if warm_ok else None,
        },
        "footprint_mib": {
            "10s": mib(kv.get(f"{which}.footprint10.bytes")),
            "60s": mib(kv.get(f"{which}.footprint60.bytes")),
            "after_stream": mib(kv.get(f"{which}.footprint_after_stream.bytes")),
            "processes_10s": kv.get(f"{which}.footprint10.procs"),
            "processes_60s": kv.get(f"{which}.footprint60.procs"),
        },
        "idle": {
            "cpu_pct": num(kv.get(f"{which}.idle.cpu_pct")),
            "daemon_cpu_pct": num(kv.get(f"{which}.idle.daemon_cpu_pct")),
            "wall_s": num(kv.get(f"{which}.idle.wall_s")),
            "pids": kv.get(f"{which}.idle.procs"),
        },
        "stream": {
            "cpu_pct": num(kv.get(f"{which}.stream.cpu_pct")),
            "daemon_cpu_pct": num(kv.get(f"{which}.stream.daemon_cpu_pct")),
            "wall_s": num(kv.get(f"{which}.stream.wall_s")),
            "lines": num(kv.get(f"{which}.stream.lines")),
            "blocked": kv.get(f"{which}.stream.blocked"),
        },
        "pty_size_rows_cols": kv.get(f"{which}.pty_size"),
        "windows_on_screen": kv.get(f"{which}.window"),
        "responsibility_disclaimed": kv.get(f"{which}.disclaimed") == "1",
        "blocked": kv.get(f"{which}.blocked"),
    }

tauri, rune = app("tauri"), app("rune")

latency = {"tauri": {"method": "forge-tauri-latency: key written to the daemon socket -> the TerminalDelta carrying it back (no WebView, no paint)"},
           "rune": {"method": "forge-ui --fixture --bench: recorded fixture, key -> replica -> one painted frame on a headless GPU (no daemon, no display)"}}
if "tauri.latency.out" in kv:
    out = open(kv["tauri.latency.out"]).read()
    m = re.search(r"key-to-delta: p50 ([\d.]+) ms .*? p95 ([\d.]+) ms .*? p99 ([\d.]+) ms", out)
    if m:
        latency["tauri"].update(p50_ms=float(m.group(1)), p95_ms=float(m.group(2)), p99_ms=float(m.group(3)))
    latency["tauri"]["binary"] = kv.get("tauri.latency.bin")
else:
    latency["tauri"]["blocked"] = kv.get("tauri.latency.blocked")
if "rune.bench.json" in kv:
    bench = json.load(open(kv["rune.bench.json"]))
    echo = (bench.get("fixture") or {}).get("echo_ms") or bench.get("echo_ms") or {}
    latency["rune"].update(p50_ms=echo.get("p50"), p95_ms=echo.get("p95"), max_ms=echo.get("max"))
else:
    latency["rune"]["blocked"] = kv.get("rune.bench.blocked")

report = {
    "label": label or None,
    "machine": {
        "cpu": sh("sysctl", "-n", "machdep.cpu.brand_string"),
        "memory_gib": round(int(sh("sysctl", "-n", "hw.memsize") or 0) / 2**30),
        "os": f"macOS {platform.mac_ver()[0]} ({sh('sw_vers', '-buildVersion')})",
        "load_avg_at_report": sh("sysctl", "-n", "vm.loadavg"),
    },
    "commit": sh("git", "rev-parse", "HEAD"),
    "dirty": bool(sh("git", "status", "--porcelain", "--untracked-files=no")),
    "daemon": {"binary": daemon_src, "protocol": kv.get("daemon.protocol")},
    "settings": {"idle_secs": int(idle_secs), "stream_secs": int(stream_secs), "warm_launches": int(warm)},
    "tauri": tauri,
    "rune": rune,
    "latency_different_methods": latency,
    "method": [
        "Both bundles run from copies in a temp directory with distinct bundle ids; the GUI executable is spawned directly (posix_spawn with responsibility disclaimed, as a Finder launch would) with FORGE_SOCKET/FORGE_HOME (Rune: FORGE_RUNE_HOME) naming one isolated daemon.",
        "One daemon, seeded with one project (a one-commit temp git repo) and one shell session; both apps attach to that same session.",
        "Startup: posix_spawn -> first on-screen layer-0 window >= 200x200 owned by the app pid (CGWindowListCopyWindowInfo, polled every 2 ms). This is window-on-screen, not content painted: Tauri maps its window before the WebView paints, and screen capture is not available to measure pixels.",
        "Cold is the first launch of a freshly copied bundle; the OS page cache is not purged, so it is first-launch-after-install, not a cold boot.",
        "Memory: phys_footprint from footprint(1), summed over the app pid plus every process whose responsible pid is the app (WebKit WebContent/Networking/GPU helpers for Tauri), excluding the daemon.",
        f"Idle CPU: user+system time (proc_pid_rusage) of that set over {idle_secs} s starting 10 s after launch, divided by wall time; 100% = one core. The Tauri host schedules an update check 30 s after launch, inside this window.",
        f"Streaming CPU: /bin/sh printing SGR-colored lines as fast as it can for {stream_secs} s in the attached session (SendShellCommand); app set and daemon process measured separately. The daemon caps emission at 125 deltas/s per attached terminal, so the app load is frame-bound.",
        "PTY size is read with `stty size` in the session after attaching; it reflects each app's window, so the grids may differ.",
        "Key-to-screen is not measured identically (no screen capture permission, no shared key-injection path); the two numbers under latency_different_methods are each client's own harness and are not comparable.",
    ],
}
json.dump(report, open(json_path, "w"), indent=2)
open(json_path, "a").write("\n")

def fmt(v, unit=""):
    return "—" if v is None else f"{v:g}{unit}"

def delta(t, r, lower_better=True):
    if t is None or r is None:
        return "—"
    d = r - t
    pct = f" ({d / t * 100:+.0f}%)" if t else ""
    return f"{d:+.1f}{pct}"

rows = [
    ("Bundle on disk (MB)", tauri["bundle"]["total_mb"], rune["bundle"]["total_mb"]),
    ("  GUI binary (+launcher) (MB)", tauri["bundle"]["gui_mb"], rune["bundle"]["gui_mb"]),
    ("  Shared daemon/editor/forgectl (MB)", tauri["bundle"]["shared_mb"], rune["bundle"]["shared_mb"]),
    ("Startup cold, spawn→window (ms)", tauri["startup_ms"]["cold"], rune["startup_ms"]["cold"]),
    (f"Startup warm median of {warm} (ms)", tauri["startup_ms"]["warm_median"], rune["startup_ms"]["warm_median"]),
    ("Footprint at 10 s (MiB)", tauri["footprint_mib"]["10s"], rune["footprint_mib"]["10s"]),
    ("Footprint at 60 s (MiB)", tauri["footprint_mib"]["60s"], rune["footprint_mib"]["60s"]),
    ("Footprint after streaming (MiB)", tauri["footprint_mib"]["after_stream"], rune["footprint_mib"]["after_stream"]),
    (f"Idle CPU, {idle_secs} s avg (% of a core)", tauri["idle"]["cpu_pct"], rune["idle"]["cpu_pct"]),
    (f"Streaming CPU, app set, {stream_secs} s (% of a core)", tauri["stream"]["cpu_pct"], rune["stream"]["cpu_pct"]),
    ("Streaming CPU, daemon (% of a core)", tauri["stream"]["daemon_cpu_pct"], rune["stream"]["daemon_cpu_pct"]),
]
lines = ["# Tauri vs Rune", ""]
if label:
    lines += [f"**Run:** {label}", ""]
m = report["machine"]
lines += [
    f"{m['cpu']}, {m['memory_gib']} GiB, {m['os']}. Commit `{report['commit'][:12]}`{' (dirty)' if report['dirty'] else ''}; daemon protocol {report['daemon']['protocol']} (`{os.path.relpath(daemon_src, root)}`), load {m['load_avg_at_report']} at report time.",
    "",
    f"Tauri bundle {tauri['bundle']['version']} built {tauri['bundle']['built']} ({tauri['bundle']['commits_since_build']} relevant commits since); Rune bundle {rune['bundle']['version']} built {rune['bundle']['built']} ({rune['bundle']['commits_since_build']} since).",
    "",
    "| Metric | Tauri | Rune | Δ (Rune − Tauri) |",
    "|---|---:|---:|---:|",
]
for name, t, r in rows:
    lines.append(f"| {name} | {fmt(t)} | {fmt(r)} | {delta(t, r)} |")
lines += [
    "",
    f"PTY rows×cols while attached: Tauri {tauri['pty_size_rows_cols'] or '—'}, Rune {rune['pty_size_rows_cols'] or '—'}. Lines printed in the stream: Tauri {fmt(tauri['stream']['lines'])}, Rune {fmt(rune['stream']['lines'])}. Idle daemon CPU: Tauri {fmt(tauri['idle']['daemon_cpu_pct'])}%, Rune {fmt(rune['idle']['daemon_cpu_pct'])}%.",
    "",
]
if tauri["pty_size_rows_cols"] and rune["pty_size_rows_cols"] and tauri["pty_size_rows_cols"] != rune["pty_size_rows_cols"]:
    lines += [
        f"**Unequal grids:** the two apps left the shared PTY at different sizes ({tauri['pty_size_rows_cols']} vs {rune['pty_size_rows_cols']} rows×cols), so the streaming rows compare different cell counts per frame.",
        "",
    ]
lines += [
    "Processes counted (name:pid:bytes at 60 s):",
    "",
    f"- Tauri: {tauri['footprint_mib']['processes_60s'] or '—'}",
    f"- Rune: {rune['footprint_mib']['processes_60s'] or '—'}",
    "",
]
for which, data in (("Tauri", tauri), ("Rune", rune)):
    for key in ("blocked",):
        if data.get(key):
            lines.append(f"**{which} blocked:** {data[key]}")
    if data["stream"].get("blocked"):
        lines.append(f"**{which} stream blocked:** {data['stream']['blocked']}")
lines += ["## Key-to-screen: not measured identically", ""]
for which in ("tauri", "rune"):
    l = latency[which]
    if l.get("p95_ms") is not None:
        lines.append(f"- {which.title()}: p50 {fmt(l.get('p50_ms'))} / p95 {fmt(l.get('p95_ms'))} ms — {l['method']}")
    else:
        lines.append(f"- {which.title()}: blocked ({l.get('blocked')}) — {l['method']}")
lines += ["", "These are different routes and are not a Δ.", "", "## Method", ""]
lines += [f"- {m}" for m in report["method"]]
open(md_path, "w").write("\n".join(lines) + "\n")
print(f"compare: wrote {json_path}")
print(f"compare: wrote {md_path}")
PY
