#!/usr/bin/env bash
# PostToolUse (Edit|Write) — formats and checks only the package that was touched.
#
# The full gate (`scripts/dev check`) takes minutes in this workspace because of
# Rust clippy/tests and the Tauri TypeScript gate. Running it after every edit
# would make the cycle unusable, so this catches local errors while they are cheap.
#
# stdin: the hook's JSON. stderr is shown to Claude; the exit code never blocks
# in PostToolUse, so we always exit 0.
set -u
cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/../..}" || exit 0

FILE=$(jq -r '.tool_input.file_path // .tool_input.path // empty' 2>/dev/null)
[ -n "$FILE" ] || exit 0

REL=${FILE#"$PWD/"}
case "$REL" in
  crates/*.rs)
    # Relative path → crates/<dir>/… → package name from that crate's Cargo.toml.
    DIR=${REL#crates/}
    DIR=${DIR%%/*}
    MANIFEST="crates/$DIR/Cargo.toml"
    [ -f "$MANIFEST" ] || exit 0
    PKG=$(sed -n 's/^name *= *"\(.*\)"/\1/p' "$MANIFEST" | head -1)
    [ -n "$PKG" ] || exit 0

    cargo fmt -p "$PKG" >/dev/null 2>&1

    if ! OUT=$(cargo check -p "$PKG" --all-targets --message-format short 2>&1); then
      {
        echo "[harness] cargo check -p $PKG failed:"
        printf '%s\n' "$OUT" | grep -E '^(error|warning)' | head -15
        echo "[harness] the full gate is ./init.sh"
      } >&2
    fi
    ;;
  apps/tauri/*)
    case "$REL" in
      *.ts|*.tsx) ;;
      *) exit 0 ;;
    esac
    if ! command -v bun >/dev/null 2>&1; then
      echo "[harness] bun not found; skipping Tauri TypeScript check" >&2
      exit 0
    fi
    TAURI_REL=${REL#apps/tauri/}
    bun run --cwd apps/tauri fmt:write "$TAURI_REL" >/dev/null 2>&1
    if ! OUT=$(bun run --cwd apps/tauri lint "$TAURI_REL" 2>&1); then
      {
        echo "[harness] oxlint $REL failed:"
        printf '%s\n' "$OUT" | grep -E '^(error|warning|  ×|  !)' | head -20
        echo "[harness] the full Tauri gate is bun run --cwd apps/tauri lint && bun run --cwd apps/tauri fmt:check && bun run --cwd apps/tauri typecheck && bun run --cwd apps/tauri test"
      } >&2
    fi
    if ! OUT=$(bun run --cwd apps/tauri typecheck 2>&1); then
      {
        echo "[harness] tsc --noEmit failed:"
        printf '%s\n' "$OUT" | grep -E '(^|:)(error TS[0-9]+|error|warning)' | head -20
        echo "[harness] the full gate is ./init.sh"
      } >&2
    fi
    ;;
  *) exit 0 ;;
esac
exit 0
