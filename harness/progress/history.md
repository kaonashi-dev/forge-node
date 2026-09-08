# Harness log

> Append-only. One entry per closed feature, the most recent one last.
> The lead moves the summary from `current.md` here when marking a feature
> `done` or `blocked`.

---

## Feature 1 — daemon-stats (done · 2026-08-27)

Wire `forge-daemon stats` through `Request::GetStats` / `Response::DaemonStats`
(sessions per state, terminals, clients, uptime). The CLI honours `FORGE_SOCKET`
and does not touch the singleton lock; three E2E cases in `integration.rs`.

- Spec: `harness/specs/1-daemon-stats/`
- Impl: `harness/progress/impl_1.md` (R1–R5)
- Review: `harness/progress/review_1.md` — APPROVED (round 2; C6 resolved by documenting unrelated WIP)
- review_rounds: 1
- Crates: protocol, daemon, client
- The diff stays in the working tree (no harness commit)

---

## Feature 2 — add-support-for-grok-ai (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 3 — test (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 4 — test (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 5 — add-suport-for-grokai (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 6 — test (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 7 — add-support-for-toher-themes (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 8 — add-suport-for-other-themes (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.

---

## Feature 9 — add-support-for-others-theme (blocked · 2026-09-01)

Closed stale harness test data so the checkout slot can be reused.

- Reason: test data
- No implementation or review was run.
