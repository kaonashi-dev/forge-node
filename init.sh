#!/usr/bin/env bash
# init.sh — repository gate.
#
# Usage:
#   ./init.sh          # toolchain pin, then `scripts/dev check`
#   ./init.sh --fast   # the same, minus the cargo gate

set -u
cd "$(dirname "$0")"

FAST=0
for arg in "$@"; do
  case "$arg" in
    --fast) FAST=1 ;;
    -h|--help) sed -n '2,6p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) printf 'init.sh: unknown argument: %s\n' "$arg" >&2; exit 2 ;;
  esac
done

RED='\033[0;31m'; GREEN='\033[0;32m'; YELLOW='\033[0;33m'; NC='\033[0m'
ok()   { printf "${GREEN}[OK]${NC}    %s\n" "$1"; }
warn() { printf "${YELLOW}[WARN]${NC}  %s\n" "$1"; }
fail() { printf "${RED}[FAIL]${NC}  %s\n" "$1"; }

EXIT_CODE=0

echo "── 1. Environment ──────────────────────────────────────"

for bin in git cargo rustc; do
  if ! command -v "$bin" >/dev/null 2>&1; then
    fail "$bin is not installed"
    exit 1
  fi
done
ok "git, cargo, rustc present"

PINNED=$(sed -n 's/^channel *= *"\(.*\)"/\1/p' rust-toolchain.toml)
ACTUAL=$(rustc --version | awk '{print $2}')
if [ -n "$PINNED" ] && [ "$PINNED" != "$ACTUAL" ]; then
  fail "rustc $ACTUAL does not match rust-toolchain.toml ($PINNED)"
  EXIT_CODE=1
else
  ok "rustc $ACTUAL == rust-toolchain.toml"
fi

echo ""
if [ "$FAST" -eq 1 ]; then
  echo "── 2. Cargo gate ───────────────────────────────────────"
  warn "skipped (--fast)."
else
  echo "── 2. scripts/dev check (fmt → clippy → test) ──────────"
  if scripts/dev check; then
    ok "fmt + clippy + test green"
  else
    fail "scripts/dev check is red"
    EXIT_CODE=1
  fi
fi

echo ""
echo "── 3. Summary ──────────────────────────────────────────"
if [ $EXIT_CODE -eq 0 ]; then
  ok "ready."
else
  fail "not ready. Resolve the above before moving forward."
fi
exit $EXIT_CODE
