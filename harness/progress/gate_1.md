# Human gate — spec approval

**Feature:** 1 (daemon-stats)
**Title:** Wire `forge-daemon stats`

**Question:** Do you approve this spec for implementation?

**Context:**
- Requirements: 4 (R1–R4)
- Crates: daemon, protocol, client
- Design note: synchronous `GetStats` request/response on the existing client socket; no new daemon.

**Options:** `approve` | `revise` | `block`

**Spec:** `harness/specs/1-daemon-stats/`

---
*Resolved: approved 2026-08-25 via /feature-go 1.*
