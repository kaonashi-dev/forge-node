# Implementation plan — Per-provider usage in the status bar (§16.2)

**Status:** ready to implement
**Date:** 2026-08-23
**Scope:** fill the §16.2 plumbing (empty today) with real usage sources through local OAuth credentials, polling in the daemon, and multi-window segments in the status bar.

---

## 0. Research findings

The §16.2 plumbing exists end to end and is verified in the code:

| Piece | Location | Status |
|-------|----------|--------|
| `UsageProbe` CLI contract + `collect` | `crates/agents/src/usage.rs` | Complete, no sources |
| `ProviderUsage` model (flat) | `crates/domain/src/agent.rs:120-145` | Flat: `used_percent/window/resets_at/collected_at` |
| `ListProviderUsage` request | `crates/protocol/src/request.rs:289` | Complete |
| `ProviderUsage(Vec<…>)` response | `crates/protocol/src/response.rs:68` | Complete |
| `ProviderUsageChanged` event | `crates/protocol/src/event.rs:107` | Complete |
| `Store.usage` + wholesale apply | `crates/client/src/store.rs:85,200` | Complete + test |
| `collect_usage()` in the daemon | `crates/daemon/src/core.rs:2460` | Complete on demand; **no polling** |
| Meter in the status bar | `apps/tauri | A single flat meter per provider |
| `usage_probe: None` in the 4 builtins | `crates/agents/src/builtins.rs:57` | No builtin declares a source |

**New decisions that come out of the code (they were not in the overview):**

1. **Idempotent broadcast.** `ProviderUsage.collected_at = Timestamp::now()` changes on every sweep, so comparing the whole struct would always say "it changed". The comparison that decides the broadcast must ignore `collected_at` and look only at `provider_id + windows`. → introduce a semantic equality helper.
2. **CLI and OAuth gating diverge.** `collect_usage()` today filters on `usage_probe.is_some()` **and** requires `DetectionStatus::Installed` (it needs the `executable`). The OAuth sources **do not need** the binary installed (they read local credentials), only the `ResolvedEnvironment` for `$HOME`/`$CODEX_HOME`. The gating must branch on the `UsageSource` variant.
3. **A new HTTP dep.** There is no `ureq`/`reqwest` in the workspace. `ureq` is added to `[workspace.dependencies]` and only to the `agents` crate.
4. **Startup order.** The CLI usage depends on detection (a separate thread, `core.rs:122-129`) having populated `inner.detections`. The sweeper must tolerate a still-empty detection (it self-corrects on the next tick); the OAuth part works from the first tick.
5. **No jitter with `rand`.** The workspace has no `rand`/`fastrand`. The jitter is derived from the nanos of `Timestamp::now()` (cheap, enough to desynchronise startups) — no dep is added.
6. **`run_to_completion` already serves the Keychain fallback** (`crates/agents/src/detection.rs:143`): it is the "existing subprocess pattern" the overview mentions; it is reused for `security find-generic-password`.

---

## 1. Domain — multi-window model

**File:** `crates/domain/src/agent.rs`

Replace the flat fields of `ProviderUsage` with `windows: Vec<UsageWindow>`:

```rust
/// One allowance window reported by a provider.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageWindow {
    pub used_percent: u8,          // 0..=100; stays Eq (no floats on the wire)
    pub window: String,            // "5h", "week"
    pub resets_at: Option<Timestamp>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderUsage {
    pub provider_id: AgentProviderId,
    pub windows: Vec<UsageWindow>,
    pub collected_at: Timestamp,
}

impl ProviderUsage {
    /// Semantic equality for deciding the broadcast: ignores `collected_at`.
    #[must_use]
    pub fn same_reading(&self, other: &Self) -> bool {
        self.provider_id == other.provider_id && self.windows == other.windows
    }
}
```

Export `UsageWindow` in `crates/domain/src/lib.rs` (next to `ProviderUsage`).
Persistence does not store usage → **no migration**.

**Forced ripple (compiles-or-breaks):**
- `crates/agents/src/usage.rs` — the CLI path now builds `windows: vec![UsageWindow{…}]`.
- `crates/client/src/store.rs:507` — the `sample_usage` helper and the two wholesale replacement tests.
- `apps/tauri — `meter()` (see §5).
- `crates/daemon/src/core.rs:2488` — the fold into `inner.usage` does not change shape (still keyed by `provider_id`).

---

## 2. Agents — real sources per provider

### 2.1 Descriptor: `usage_probe` → `usage_source`

**File:** `crates/domain/src/agent.rs`

```rust
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum UsageSource {
    Cli(UsageProbe),   // the existing JSON contract, untouched
    CodexOAuth,
    ClaudeOauth,
}
```

In `AgentDescriptor`, replace `pub usage_probe: Option<UsageProbe>` with
`pub usage_source: Option<UsageSource>`. `UsageProbe` is kept unchanged.

**Fixture ripple (they all set `usage_probe: None` → `usage_source: None`):**
- `crates/agents/src/builtins.rs:57` (see §3)
- `apps/tauri/src/palette/`
- `crates/protocol/src/lib.rs:203`

### 2.2 New module `crates/agents/src/usage/`

Turn `usage.rs` into a directory. All the provider-specific logic lives here (P2).

```
usage/
├── mod.rs      # dispatch by UsageSource; public API `collect()` (current signature untouched)
├── cli.rs      # the current UsageProbe path, moved as-is
├── http.rs     # HttpClient trait + UreqClient (prod) + fixture structure (tests)
├── codex.rs    # ~/.codex/auth.json → wham/usage → windows
└── claude.rs   # ~/.claude/.credentials.json (+ Keychain fallback) → oauth/usage → windows
```

**`mod.rs` — dispatch:**

```rust
pub fn collect(descriptor, executable, env) -> Option<ProviderUsage> {
    match descriptor.usage_source.as_ref()? {
        UsageSource::Cli(probe)   => cli::collect(descriptor, executable, probe, env),
        UsageSource::CodexOAuth   => codex::collect(descriptor, env, &UreqClient::default()),
        UsageSource::ClaudeOauth  => claude::collect(descriptor, env, &UreqClient::default()),
    }
}
```

**`http.rs` — fakeable HTTP (no network in CI):**

```rust
pub trait HttpClient {
    /// GET with headers; None on any failure (timeout, non-2xx, network).
    fn get(&self, url: &str, headers: &[(&str, &str)]) -> Option<Vec<u8>>;
}

pub struct UreqClient { /* agent with a short timeout (~5s) */ }
// tests: FakeHttp { responses: HashMap<String, Vec<u8>> }
```

Never log tokens nor Authorization. `ureq` with short
`timeout_connect`/`timeout_read`.

**`codex.rs`:**
- Read `~/.codex/auth.json` honouring `$CODEX_HOME` (through `env.get("CODEX_HOME")`, falling back to `$HOME/.codex`).
- Extract the access token (Bearer).
- `GET https://chatgpt.com/backend-api/wham/usage` with `Authorization: Bearer <token>`.
- Map `primary_window` → the `"5h"` (session) window and `secondary_window` → `"week"`, each with `used_percent` (rounded from the fraction) and `resets_at`.
- Return `ProviderUsage{ provider_id, windows, collected_at: now }`.

**`claude.rs`:**
- Token from `~/.claude/.credentials.json`.
- **macOS Keychain fallback** through a subprocess: `security find-generic-password -s "Claude Code-credentials" -w` using `detection::run_to_completion` (an existing pattern, no new crate).
- `GET https://api.anthropic.com/api/oauth/usage` with `Authorization: Bearer <token>` + the `anthropic-beta: oauth-2025-04-20` header.
- Map `five_hour` → `"5h"` and `seven_day` → `"week"`.

**Error handling (the current invariant):** "not signed in", a failed parse, the network down, a non-2xx → `None`. The provider is simply absent from the result, as today. No error ever turns into a number on screen.

### 2.3 Cargo

- Root `Cargo.toml`: add `ureq = { version = "2", features = ["json"] }` (or without `json`, parsing with the `serde_json` already present) to `[workspace.dependencies]`.
- `crates/agents/Cargo.toml`: `ureq.workspace = true`.

---

## 3. Builtins — declare the sources

**File:** `crates/agents/src/builtins.rs`

- `claude` → `usage_source: Some(UsageSource::ClaudeOauth)`
- `codex` → `usage_source: Some(UsageSource::CodexOAuth)`
- `opencode`, `cursor` → `usage_source: None`

Adjust the `descriptor(...)` function to take `usage_source` as a parameter (or
set it by id). Update the §16.2 comment and the builtin tests if they assert on
the absence of a source.

---

## 4. Daemon — periodic polling

**File:** `crates/daemon/src/core.rs`

### 4.1 Sweeper thread

In `Daemon::start`, next to the detection thread (`core.rs:122-129`), launch a
std thread:

```rust
{
    let d = daemon.clone();
    std::thread::spawn(move || d.run_usage_sweeper());
}
```

`run_usage_sweeper`:
1. An initial tick shortly after startup (the OAuth part works already; the CLI one self-corrects once detection populates `inner.detections`).
2. Loop: `collect_usage()` (it already releases the lock during the probes — invariant satisfied), compare with the last batch **semantically** (`same_reading`), and only if it changed → `registry.broadcast_domain(DaemonEvent::ProviderUsageChanged { usage })`.
3. Sleep ~5 min ± jitter derived from the nanos of `Timestamp::now()`.
4. Leave the loop if `self.is_shutting_down()` (using the existing `AtomicBool`, `core.rs:266`). Sleep in short stretches (e.g. 30×10s) so it reacts to the shutdown without waiting 5 min.

### 4.2 `collect_usage()` — branch the gating

It currently requires `DetectionStatus::Installed` for everyone. Change the
filter:
- `UsageSource::Cli(_)` → requires `Installed { executable }` (as today).
- `UsageSource::CodexOAuth | ClaudeOauth` → does **not** require a binary; pass a dummy/`None` `executable` and only the `env`. Adjust `agents::collect_usage` so `executable` is an `Option<&Path>` or only relevant in the CLI branch.

`ListProviderUsage` on demand (`core.rs:582`) stays the same; it now returns real
data.

### 4.3 Test

A sweeper broadcast test with an injected `FakeHttp` (or testing `collect_usage`
+ `same_reading` directly): two identical batches → a single broadcast; a
different batch → a second broadcast.

---

## 5. UI — multi-window segments

**File:** `apps/tauri/src/shell/StatusBar.tsx`
- `meter()` starts iterating `usage.windows`: **one segment per window**.
- Each segment: the existing bar (`METER_W`, `METER_WARN`/`METER_FULL` colours) + the text `{p}% used · resets in {h}h {m}m`.
- **Countdown** from `window.resets_at` — `resets_at - Timestamp::now()` → hours/min; if it already passed or is `None`, drop the "resets in" part.
- **Extended tooltip**: `{display_name} · {window}: {p}% used`.
- **Staleness**: if `collected_at` is older than 2× the cadence (~10 min), dim (faint colour) the whole provider block.
- **Repaint per minute** so the countdown advances: a light the shell timer (`cx.spawn` + an interval) or a re-render on events. A single timer at the status-bar level, not one per segment.

Ripple: the iterator in `status_bar()` (`:91-97`) still filters by
`prefs.agent_visible`; now each `usage` can expand into several segments.

---

## 6. Implementation order

1. **Domain** — `UsageWindow` + `ProviderUsage` refactor + `same_reading` + fixing the tests in client/store and the protocol fixtures. *(the workspace compiles broken→green)*
2. **Agents** — `UsageSource`, split `usage/` (move the CLI), `http.rs` with a fakeable `HttpClient`, `codex.rs`/`claude.rs` + parsing tests with JSON fixtures. Add `ureq`.
3. **Builtins** — declare `CodexOAuth`/`ClaudeOauth`.
4. **Daemon** — sweeper thread + per-variant gating + broadcast test.
5. **UI** — multi-window segments + countdown + staleness + repaint timer.
6. **Gate** — `scripts/dev check`.

---

## 7. Risks / notes

- **No network in CI:** everything HTTP sits behind `HttpClient`; the tests use `FakeHttp` with fixture bodies. No test does real networking.
- **Endpoint formats:** the exact shapes of `wham/usage` and `oauth/usage` are encoded as `#[derive(Deserialize)]` structs with `#[serde(default)]` where there is doubt; an unexpected shape → `None`, never a panic.
- **Tokens:** never in logs nor in error messages; `tracing` only with names/states.
- **Keychain (macOS):** the `security` subprocess may ask for permission the first time; that is OS behaviour, we do not force it. Silent fallback to `None` if it fails.
- **Empty multi-window:** a signed-in provider with no windows returned → an empty `windows`; the UI draws no segments (the same "unknown ≠ 0" criterion as today).
