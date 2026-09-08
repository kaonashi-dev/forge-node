# Implementation — harness UI (phases 1–5)

## R<n> → evidence

| Requirement | Evidence |
|-------------|----------|
| R1 List harness features (panel) | `apps/tauri manual: open right panel → Features |
| R2 Feature tab draft/start | `apps/tauri `cargo check -p forge-tauri` |
| R3 Orchestrator launch + link | `RuntimeCommand::NewHarnessAgent` in `runtime.rs`; `link_harness_session` in handler |
| R4 Gate approve + implement child | `approve_harness_spec` + `NewHarnessChild` Executor in `app_shell.rs` |
| R5 Reviewer child on in_review | `pending_harness_reviewer` + `spawn_harness_reviewer` |
| R6 Session tree in sidebar | `session_tree.rs` + `sidebar.rs` indent |
| R7 Role in tab titles | `session_tabs.rs` uses `session_display_title` |
| R8 Status bar gate line | `status_bar.rs` + `harness_gate_feature()` |
| R9 From-issue registration | `RegisterHarnessFromIssue` command + feature tab issue field |
| R10 Validate before commit UX | `ValidateHarness` command + output in feature tab |

Verification: `cargo check -p forge-tauri` (2026-08-28, exit 0)

## Files by crate

### ui (new)
- `src/features_panel.rs` — right panel list/filter/refresh
- `src/feature_view.rs` — main pane feature tab
- `src/session_tree.rs` — tree ordering + role title prefix

### ui (modified)
- `src/runtime.rs` — harness RuntimeCommand/Update + handlers
- `src/app_shell.rs` — state, wiring, actions, panes
- `src/session_menu.rs` — “New feature…”
- `src/sidebar.rs` — indented session tree
- `src/session_tabs.rs` — role-prefixed agent tab labels
- `src/status_bar.rs` — spec_ready notice
- `src/lib.rs` — mod declarations

### docs
- `docs/plan-harness-ui.md` — phase checkboxes 1–5 (phase 4 worktree banner left open)

## Outside declared crates

- `docs/plan-harness-ui.md` — checkbox updates per user request
- `harness/progress/*` — implementer protocol

## Left out

- **Worktree isolation banner** (plan phase 4): no “implementing in worktree …” line in feature tab yet
- **Executor/reviewer prompts** read `context_<id>.md` / `impl_<id>.md` via static instructions; runtime does not pre-fetch artifact text into the child prompt (agents read harness/ on disk)
- **Auto-poll**: refresh is explicit (panel refresh, feature refresh, tab open) — no timer
- **Planner/Researcher** child auto-spawn not wired (orchestrator PTY is expected to create those)
