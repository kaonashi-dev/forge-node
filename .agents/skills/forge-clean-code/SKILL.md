---
name: forge-clean-code
description: >
  Deep architecture, documentation, and code-quality review of Forge Node that
  implements justified fixes (not a report-only). Use when the user runs
  /forge-clean-code, asks for a clean-code pass, architecture audit,
  agent-navigability review, or to re-run the quality review of this repository.
---

# Forge Node clean-code review

You are executing a review of **this** repository. Implement justified fixes
and verify them. Do not stop at a plan or a findings list.

## Ground truth (read, do not copy)

| Topic | Source |
|-------|--------|
| Invariants, crate seams, comments policy | `AGENTS.md` |
| Vocabulary and doc map | `CONTEXT.md` |
| System map | `docs/architecture.md` |
| Cost rungs | `docs/performance.md` — **required** before delta, render, core lock, or process work |
| ADR numbers in code | `docs/decisions.md` |
| Commands / "where to change X" | `docs/development.md`, `scripts/dev` |
| Reviewer checkboxes if an invariant changes | `.agents/skills/invariant-c4/SKILL.md`, `.agents/skills/invariant-c5/SKILL.md`, `.agents/skills/invariant-c7/SKILL.md` |
| GUI | Tauri 2 + **Solid** (not React). Bun 1.4.1. Tokens: `apps/tauri/src/theme/tokens.ts` |

`Cargo.toml`, crate `//!` docs, `scripts/dev`, and current source beat `plan/`.
Historical `apps/tauri/PARITY.md` / `PROGRESS.md` are not the gate.

## Hard limits

- Preserve the user's uncommitted diff. Inspect `git status` first.
- No commits, pushes, new dependencies, or edits to existing migrations.
- No destructive operations. Public behaviour stays unless a defect is demonstrated.
- No massive refactors, speculative layers, or aesthetic-only edits.
- Do not `unwrap`/`expect` on recoverable daemon paths. Keep required lock/`#[allow]` comments.
- Bun APIs stay in `apps/tauri/scripts`, never in WebView `src/`.

## Procedure

### 1. Snapshot git

Record `git status --short` and `git diff --stat`. Do not revert or restyle the user's files unless they contain a defect this review proves.

### 2. Map, then split

Read `CONTEXT.md` and `docs/architecture.md`. Split work by **owned paths** (no two writers on the same file):

| Slice | Owns |
|-------|------|
| Docs / AGENTS / agent-nav | `AGENTS.md`, `CONTEXT.md`, `docs/**`, `.agents/**`, crate `lib.rs` headers, `scripts/dev`, `Makefile` |
| Daemon runtime | `crates/daemon/src/**`, `crates/terminal-core/**` |
| Crate seams | `domain`, `protocol`, `client`, `git-service`, `fs-service`, `persistence`, `terminal-input`, `apps/tauri/src-tauri/Cargo.toml` |
| Solid frontend | `apps/tauri/src/**` (Solid: `createSignal` / `createMemo` / `createEffect` / `onCleanup`) |
| Comments | module `//!` / file headers only — no repo-wide comment sweep |

Launch **explore** subagents for independent slices. Each prompt must include: owned paths, hard limits, this hunt list, and: write findings to an absolute path under `plan/architecture-review/` and **return only that path**.

Parent keeps integration, cross-cutting decisions, and the final gate.

### 3. Filter, then implement

Every finding needs evidence (`path:line`), impact, and a proportionate fix. Priority:

1. Behaviour bugs, seam violations, concurrency (locks, Drop flags, process groups)
2. Cost on the real rung (cell / delta / frame / request) — `docs/performance.md`
3. Docs that would make an agent do the wrong thing (`scripts/dev` vs AGENTS.md, version pins, crate map, `PROTOCOL_VERSION`)
4. Module headers that cite `plan/` `§N`, invent types (`AgentService`, `WorkspaceService`, a `ui` crate, "tokio client"), or retell `docs/architecture.md`

Skip: size-only splits of `core.rs`, collapsing `Client` request wrappers, unifying service/domain DTOs for neatness, "fixing" open rows in `docs/performance.md` unless this diff is already on that path.

### 4. Hunt (classes this repo actually grows)

Do not treat this as a closed checklist. It is where previous passes found real defects.

**Runtime**

- Coalescing flags (`fetching`, `pr_opening`, `pr_refreshing`, `provisioning`, `drafting`) cleared only on the happy path — need a `Drop` guard like `ProvisioningGuard`.
- `Mutex::lock().unwrap()` on the PTY/delta path when a sibling method already uses `PoisonError::into_inner`.
- `#[non_exhaustive]` wildcards that **do the wrong work** (unknown `SearchKind` → `Name`) instead of `InvalidRequest`.
- Process spawn without `.process_group(0)` + kill **negative** pgid; unbounded `wait()`; `read_to_end` / `into_reader` without `.take(cap)`.
- Path jail: lexical `starts_with` on a **missing** nested path under a symlink prefix (`fs-service::resolve_inside`).

**Frontend (Solid)**

- `createEffect` load loops: empty success (`items.length > 0`) or uncleared error treated as "not loaded".
- Per-frame style/layout work that is derivable from store/metrics.
- Host `kind: String` (and similar) that silently defaults.

**Docs / agent-nav**

- `scripts/dev daemon` is **isolated** (`FORGE_DEV_DIR`). A sentence that says it uses daily per-user paths is false.
- `forge-daemon dump` is unwired; `stats` is live `GetStats`. Do not lump them.
- Built-ins are whatever `crates/agents/src/builtins.rs` lists (currently five, including `grok`). Count from code.
- `PROTOCOL_VERSION` in `docs/protocol.md` must match `protocol::PROTOCOL_VERSION`.
- Workspace members in `Cargo.toml` must appear in the crate map (`docs/architecture.md`, `README.md`).
- Toolchain leftovers after a runtime change (`pnpm` vs Bun in `tauri.conf.json`, docs, CI).
- Broken links to unpublished `plan/` files from normative `docs/`.
- Crate `//!` longer than a 2–6 line map, or citing `§7.5` as current truth.

### 5. Verify

Targeted tests per batch. Then:

- `scripts/dev check` (fmt, clippy `-D warnings`, `cargo test --workspace`)
- If `apps/tauri` changed: `bun run --cwd apps/tauri check`
- If an invariant in `AGENTS.md` changed: the matching `invariant-c4`, `invariant-c5` or `invariant-c7` skill

Do not claim a test ran if you only read the code. Tests passing do **not** prove cost rungs.

### 6. Report

Write `plan/architecture-review/REPORT.md` (local; `plan/` is not published): coverage by slice, findings with evidence, what you implemented, what you deferred and why, commands run and outcomes.

Final chat: short. User's language. What broke and was fixed, architecture/docs/agent-nav changes, gates, leftovers.

## Subagents

Explore is read-only. Parent implements.

If explore cannot write `plan/`, it pastes the report in its return and the parent saves it.
