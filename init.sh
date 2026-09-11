#!/usr/bin/env bash
# init.sh — gate of the subagent harness.
#
# An agent runs it when starting a `/feature` session, when the implementer
# finishes, and before closing. If it fails, the session does not move forward.
# Human interface (status/list/next): `scripts/harness`. Recipe: docs/harness.md.
#
# Usage:
#   ./init.sh          # environment + base files + state + `scripts/dev check`
#   ./init.sh --fast   # all of the above MINUS the cargo gate (seconds)
#   scripts/harness gate [--fast]   # the same gate through the harness CLI
#
# Block 4 is expensive on purpose: fmt + clippy + test over the Rust workspace
# takes minutes, not seconds. The cheap hooks use --fast; closing a feature
# uses the full version

set -u
cd "$(dirname "$0")"

FAST=0
for arg in "$@"; do
  case "$arg" in
    --fast) FAST=1 ;;
    -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) printf 'init.sh: unknown argument: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
ok()   { printf "${GREEN}[OK]${NC}    %s\n" "$1"; }
warn() { printf "${YELLOW}[WARN]${NC}  %s\n" "$1"; }
fail() { printf "${RED}[FAIL]${NC}  %s\n" "$1"; }

EXIT_CODE=0

echo "── 1. Environment ──────────────────────────────────────"

for bin in git cargo rustc bun; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    fail "$bin is not installed"
    exit 1
  fi
done
ok "git, cargo, rustc, bun present"

PINNED=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
ACTUAL=$(rustc --version | awk '{print $2}')
if [ -n "$PINNED" ] && [ "$PINNED" != "$ACTUAL" ]; then
  fail "rustc $ACTUAL does not match rust-toolchain.toml ($PINNED)"
  EXIT_CODE=1
else
  ok "rustc $ACTUAL == rust-toolchain.toml"
fi

echo ""
echo "── 2. Harness base files ───────────────────────────────"

# Tooling only: features.json and progress/ are local state that a fresh clone
# does not have until its first registration.
for f in AGENTS.md harness/CHECKPOINTS.md scripts/harness; do
  if [ ! -e "$f" ]; then
    fail "missing base file: $f"
    EXIT_CODE=1
  else
    ok "$f exists"
  fi
done

echo ""
echo "── 3. Coherence of harness/features.json ───────────────"

bun harness/src/validate.ts || EXIT_CODE=1

echo ""
if [ "$FAST" -eq 1 ]; then
  echo "── 4. Cargo gate ───────────────────────────────────────"
  warn "skipped (--fast). Closing a feature requires the full ./init.sh."
else
  echo "── 4. scripts/dev check (fmt → clippy → test) ──────────"
  if scripts/dev check; then
    ok "fmt + clippy + test green"
  else
    fail "scripts/dev check is red"
    EXIT_CODE=1
  fi
fi

echo ""
echo "── 5. Summary ──────────────────────────────────────────"
if [ $EXIT_CODE -eq 0 ]; then
  ok "harness ready."
else
  fail "harness is NOT ready. Resolve the above before moving forward."
fi
exit $EXIT_CODE
