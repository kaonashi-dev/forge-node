#!/usr/bin/env bash
# Stop — refuses to close the session with the harness in an incoherent state.
#
# It deliberately does NOT run the suite: `init.sh --fast` validates the
# environment, the base files and the coherence of features.json in seconds. The
# expensive gate is run by the implementer and the reviewer, not by the close.
#
# Blocks with exit 2 (stderr = the reason Claude sees). It blocks ONLY ONCE per
# session: if the agent stops again after the warning, it is allowed to close —
# a guard that blocks in a loop is worse than one that warns once.
set -u
cd "${CLAUDE_PROJECT_DIR:-$(dirname "$0")/../..}" || exit 0

INPUT=$(cat)
SESSION=$(printf '%s' "$INPUT" | jq -r '.session_id // "no-session"' 2>/dev/null)
STAMP="${TMPDIR:-/tmp}/forge-harness-guard-$SESSION"

# With no initialised harness, this repo has nothing to watch over.
[ -f harness/features.json ] || exit 0

REASON=""

if ! STATE=$(bun harness/src/validate.ts 2>&1); then
  REASON="The harness state is incoherent:
$STATE"
else
  # One line per active feature — a project checked out into several worktrees
  # runs one in each — and this session is closing in exactly one of them, so
  # the guard only judges the feature of the checkout it is standing in.
  HERE=$(pwd -P)
  ACTIVE=$(bun harness/src/cli.ts active 2>/dev/null | awk -F'|' -v here="$HERE" \
    '$4 == "" || $4 == here { print; exit }')
  if [ -n "$ACTIVE" ]; then
    IFS='|' read -r ID SLUG STATUS _CHECKOUT <<<"$ACTIVE"
    # The note lives with the rest of the state, in the checkout that owns it —
    # this session may be closing in a worktree, whose harness/ is a frozen copy
    # from its commit. `cli.ts active` above already answered from that root.
    ROOT=$(bun harness/src/cli.ts root 2>/dev/null) || ROOT=""
    [ -n "$ROOT" ] || ROOT=$(pwd -P)
    NOTE="$ROOT/harness/progress/current_$ID.md"
    [ -f "$NOTE" ] || NOTE="$ROOT/harness/progress/current.md"
    # -i because the template the harness writes titles it "feature <id>" in
    # lower case: a guard that blocks over the case of one letter warns about an
    # incoherent state that does not exist.
    if ! grep -qi "feature $ID\b" "$NOTE" 2>/dev/null; then
      REASON="Feature $ID ($SLUG) is in '$STATUS' but $NOTE
does not describe it. Document the state there before closing, or mark
the feature as 'blocked' with its reason. See harness/CHECKPOINTS.md C2."
    fi
  fi
fi

[ -z "$REASON" ] && exit 0

if [ -f "$STAMP" ]; then
  echo "[harness] repeated warning, not blocking again: $REASON" >&2
  exit 0
fi

: > "$STAMP"
echo "[harness] $REASON" >&2
exit 2
