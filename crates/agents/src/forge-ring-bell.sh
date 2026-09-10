#!/bin/sh
# Forge attention: ring ASCII BEL when a Forge-launched agent asks the user.
# Providers feed hook JSON on stdin; drain it so nothing blocks. See docs/agents.md.
{ command -p cat >/dev/null 2>&1 || cat >/dev/null 2>&1 || true; }
[ -n "$FORGE_SESSION_ID" ] || exit 0
[ -w /dev/tty ] || exit 0
printf '\007' >/dev/tty 2>/dev/null || true
exit 0
