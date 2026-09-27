//! Guides served by the binary. Forge does not write them into a checkout.

pub fn guide_text(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "controller" => CONTROLLER,
        "worker" => WORKER,
        "messages" => MESSAGES,
        "board" => BOARD,
        "integration" => INTEGRATION,
        "errors" => ERRORS,
        _ => return None,
    })
}

const CONTROLLER: &str = "\
# forgectl guide controller

You are the controller. The daemon keeps the ledger and the rails. It does not choose the next task, retry, or merge into the default branch.

1. Start small. One task unless the work is truly parallel. Use --after when one task needs another's output.
2. The loop is: task add, task start, run wait --for attention,reported,question, act. Never poll with sleep.
3. A report is evidence, not proof. Run task review before accept.
4. When a worker is waiting, a human must answer the prompt in that terminal. Never type into it.
5. When a worker is stalled, send one message. After that, task reject --retry or task cancel. Absence is not a verdict.
6. Integrate after each accept so dependents branch from the integration head.
7. Finish with run close --pr --cleanup when agent_may_integrate allows the pull request. An agent may open that pull request and may not merge it.
8. run resume starts a new controller from the ledger. It does not revive workers.

Only an explicit forgectl report settles an attempt. Idle, exit, and timeouts raise attention instead.
";

const WORKER: &str = "\
# forgectl guide worker

Carry out the task in this checkout.

- Commit on this branch. Do not push, open pull requests, or create worktrees.
- Ask the controller with: forgectl ask --wait \"<question>\"
- Record a decision: forgectl state set <key> '<json>' --if-version <n>
- If you see \"Forge: N new message(s)\", run: forgectl inbox
- When finished, run exactly once: forgectl report --done|--failed|--blocked --summary \"...\"
- The full current brief is: forgectl context

Do not edit AGENTS.md or CLAUDE.md. Forge will not either.
";

const MESSAGES: &str = "\
# forgectl guide messages

Messages are stored. They are not pasted into a terminal.

- send --to controller|task:<id>|session:<id>|run:<id>
- inbox, or inbox --wait
- ask --wait returns the answer without typing into the PTY
- A pointer line is typed only while the session is idle, and never while it is waiting, starting, or unknown. A blocked inbox --wait suppresses the pointer.
- A run address stores one envelope per live attempt.
";

const BOARD: &str = "\
# forgectl guide board

The board is run-scoped, versioned JSON. set and del require --if-version. Version 0 creates a key. A mismatch exits 5 and includes the current version.

Keys match [a-z0-9._/-]{1,128}. Conventions, not enforced: decision/<topic>, claim/<path>, checkpoint/<task>, vote/<reviewer>.
A claim does not lock git.
";

const INTEGRATION: &str = "\
# forgectl guide integration

Accepted task branches merge into the run's integration worktree with task integrate --merge (--no-ff or --squash).

A conflict is attention. Resolve it in the integration worktree, then task integrate --continue or --abort.
run close --pr opens one pull request from that workspace. Cleanup reports anything it could not remove and does not delete branches.
";

const ERRORS: &str = "\
# forgectl guide errors

JSON errors are on stdout: {\"ok\":false,\"error\":{\"code\",\"message\",\"details\",\"next\"}}. next is literal argv.

Exit codes: 0 ok or wait met, 1 other refusal, 2 usage, 3 unreachable or protocol mismatch (sandbox_denied on EPERM/EACCES), 4 not found, 5 conflict or precondition, 6 policy, 124 wait timeout.

A mutation timeout or disconnect is exit 3 with details.uncertain true. Retry with the same --request-id.
";
