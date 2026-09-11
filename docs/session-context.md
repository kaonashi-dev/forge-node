# Session context and cross-provider mediation

How one Forge session cites another, hands work to a different provider, or
spawns a child — without providers talking to each other.

Source: `crates/daemon/src/context_xfer.rs`, `session_cli.rs`, `core.rs`
(`SendContext` / `CreateChildSession`); GUI: `SpawnChildDialog`,
`SendContextDialog`, `HandoffDialog`. Domain: §8.2–8.3. Protocol: v18+.

## The rule

**Forge mediates. Providers do not peer.**

Claude cannot call Codex, and Codex cannot read Claude's transcript. Both talk
only to the daemon (protocol or `forge-daemon` CLI). ACP is Client↔one Agent,
not agent↔agent. Cross-provider work always looks like:

```
session A  →  SendContext / CreateChildSession / handoff  →  daemon  →  session B
```

`B` may use any installed provider. The graph edge (`parent_session_id`) and
an optional `ContextEnvelope` are Forge state.

## Three human gestures

| Gesture | What it does | Graph | Envelope |
|---------|--------------|-------|----------|
| **Continue in a New Session…** | Capture this terminal's transcript; start a *new* agent (any provider) with that text as `initial_prompt` | Parent set when the source is a Forge session | No (prompt only) |
| **Spawn Child…** | Start a nested agent under this session with an optional typed prompt | Always a child | No (unless you use Send Context → spawn) |
| **Send Context…** | Persist an auditable envelope; either paste it into a live peer or spawn a child that starts with it | Child when spawning | Always |

Handoff leaves the original session running. Spawn/Send Context are the
orchestration primitives; handoff is “continue this conversation elsewhere.”

## What agents call

Every Forge-launched PTY gets `FORGE_SESSION_ID`, `FORGE_WORKSPACE`, and
`forge-daemon` prepended on `PATH`. From inside an agent:

```bash
# Who is running
forge-daemon session list

# Cite another session (plain text off its terminal)
forge-daemon session read <session-id>

# Nested agent, any provider
forge-daemon session spawn-child --provider codex --prompt "Review the plan"

# Hand work to an existing session (PTY paste when live)
forge-daemon context send --to <session-id> \
  --summary "Plan ready" \
  --instructions "Implement tasks 1–3" \
  --include-transcript

# Or spawn the receiver with the same envelope as initial_prompt
forge-daemon context send --spawn-provider claude \
  --summary "Need a second opinion" \
  --include-transcript

# Inbox / outbox for this session (FORGE_SESSION_ID when --session omitted)
forge-daemon context list
```

`--from` defaults to `FORGE_SESSION_ID`. Without either, the CLI refuses.

### Framed delivery

`SendContext` always stores a `ContextEnvelope`. When the target has a live
terminal, the daemon also writes a paste:

```
--- forge context ---
From session <uuid> (<title>)
Summary: …
Instructions:
…
Cited transcript:
```
…
```
--- end forge context ---
```

A spawn path uses the same text as `initial_prompt` (providers that support
launch prompts). `summary` / `instructions` are clamped to 8 KiB each before
store; transcript bytes follow the same budgets as `GetSessionTranscript`.

## Protocol surface

| Request | Answer |
|---------|--------|
| `GetSessionTranscript { session_id, max_lines?, max_bytes? }` | `SessionTranscript` — cite / handoff capture |
| `CreateChildSession { parent, kind, provider, profile?, role, workspace_policy, initial_prompt? }` | `SessionCreated` |
| `CreateContextEnvelope { envelope }` | `Ack` (persist only) |
| `SendContext { source, target? \| spawn?, summary?, instructions?, include_transcript, … }` | `Ack` (deliver) or `SessionCreated` (spawn) |
| `ListContextEnvelopes { session_id }` | `ContextEnvelopes` — source or target |

Exactly one of `target_session_id` or `spawn` on `SendContext`. At least one of
`summary`, `instructions`, or `include_transcript` must be set.

`ChildWorkspacePolicy`: `SameWorkspace`, `NewManagedWorktree`, or
`ExistingWorkspace` (same project; depth ≤ 8, ADR-010).

## Domain objects

- **Session graph** — `parent_session_id` / `root_session_id`; logical, not the
  OS process tree. See [domain.md](./domain.md).
- **ContextEnvelope** — append-only row (`context_envelopes`): source, optional
  target, summary, instructions, artifacts (e.g. `TerminalExcerpt`), optional
  git context. Survives a closed target (`SET NULL`); deleting the source
  cascades.

Discovered (external) agent runs are **not** graph nodes: handoff from History
starts a Forge session with a prompt, but there is no `parent_session_id`.

## What this is not

- Not Claude's rename/send or provider-native “subagent” APIs — those stay
  inside one CLI.
- Not MCP yet — the CLI is the agent-facing surface; MCP can wrap the same
  requests later.
- Not ACP peer routing — ACP (when wired) remains one client session per
  agent process.
- Not the harness Spec→Implement→Review bus — that stays headless jobs on
  `harness/`; see [harness.md](./harness.md).

## Related

- [domain.md](./domain.md) — session graph, `ContextEnvelope`, roles
- [protocol.md](./protocol.md) — request table
- [agents.md](./agents.md) — `FORGE_*` env, launch prompts, profiles
- [architecture.md](./architecture.md) — P5 session graph
