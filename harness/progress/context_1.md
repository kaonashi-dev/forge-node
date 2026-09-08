# Context bundle — feature 1 (daemon-stats)

## spec_raw (verbatim)

`forge-daemon stats` today prints "not wired yet" to stderr and exits with code 0, so a script cannot tell "no data" apart from "it worked". I want it to query the running daemon and show real runtime statistics: live sessions per state, open terminals, connected clients and daemon uptime. If no daemon is running, it has to say so and exit with a code != 0.

## Crates

daemon, protocol, client

## Explore artefacts

- (none — precedent was obvious in daemon CLI and integration tests)

## Invariants to respect

- `client::Client` stays blocking std sockets; stats is a synchronous read like other one-shot requests.
- No new PTY, no `terminal-core` outside the daemon.
- `protocol` message types stay `#[non_exhaustive]`-safe on the client side.

## Implementer checklist

1. Follow `harness/specs/1-daemon-stats/tasks.md` in order.
2. Map every `R<n>` to a test in `impl_1.md`.
3. Run `scripts/dev check` before review.
