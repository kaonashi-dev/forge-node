#!/bin/sh
cat >/dev/null
[ -n "$FORGE_SESSION_ID" ] || exit 0
ctl=__FORGECTL__
[ -x "$ctl" ] || ctl=$(command -v forgectl 2>/dev/null)
if [ -n "$ctl" ]; then
    "$ctl" hook --state "$1" --source "${2:-claude}" >/dev/null 2>&1
fi
if [ "$1" = waiting ] && [ -w /dev/tty ]; then
    printf '\007' >/dev/tty 2>/dev/null || :
fi
exit 0
