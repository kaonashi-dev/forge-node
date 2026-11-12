//! Guides served by the binary. Forge does not write them into a checkout.

pub fn guide_text(topic: &str) -> Option<&'static str> {
    Some(match topic {
        "controller" => CONTROLLER,
        "worker" => WORKER,
        "messages" => MESSAGES,
        "board" => BOARD,
        "integration" => INTEGRATION,
        "errors" => ERRORS,
        "sessions" => SESSIONS,
        "spec" => SPEC,
        _ => return None,
    })
}

const SPEC: &str = "\
# forgectl guide spec

A spec run has two agents: a planner owns the spec, the plan and the review; a coder writes the code.
The spec is a Claude artifact link, a path to a Markdown file, the full text, or - for stdin. Forge never fetches a link: the planner reads an artifact with its own artifact tool.
Launch a planner: forgectl --json spec run <spec> [--preset claude|mix] [--coder <provider>] [--coder-profile <name|uuid>] [--planner-profile <name|uuid>] [--worktree <branch>] [--rounds 3]
Profiles pick the account: the brief launches the coder with --profile <uuid>, so a renamed profile still resolves.
- claude: Claude plans at high effort, Claude codes at medium.
- mix: Claude plans at high effort, Cursor codes; --coder opencode picks OpenCode. Cursor applies an effort only with --coder-model.
Be the planner yourself: forgectl spec brief <spec> [same options] prints the brief to follow in this session.
The planner validates every criterion (context and a way to prove it), asks the person all open points at once with its question selector, plans, launches the coder, waits for idle,waiting,exit, reruns the validations, sends numbered findings, and stops after the review rounds.
The power of the simple: the smallest concrete change that meets the spec wins.
Providers, effort levels and models are checked before anything launches; a path that does not exist exits 4.
";

const SESSIONS: &str = "\
# forgectl guide sessions

Find places: forgectl project list; forgectl workspace list --project <name>; forgectl providers
Launch an agent: forgectl --json session spawn --project <name|path|id> --provider <id> --name <title> --prompt-file plan.md
Open a shell: forgectl --json session shell --project <name|path|id> --name <title>
Inside a Forge session, --project defaults to your project and checkout, and spawn's --provider (with your profile) to your own agent.

Hand-offs, as a person asks for them:
- \"Send this plan to Claude with effort medium to implement, then validate it\": write the plan to a file, then
  forgectl --json session spawn --provider claude --effort medium --name impl --prompt-file plan.md
  forgectl --json session wait impl --for idle,waiting,exit --timeout 15m
  A timeout is not proof of work: an agent may never report activity (details.activity shows no evidence). Read the transcript and decide.
  On waiting, read the transcript and answer or ask the person; on idle, validate: session read, session changes, git diff from the session's base_commit, and run the tests yourself.
- \"Create an OpenCode session to implement plan X\": forgectl --json session spawn --provider opencode --name impl --prompt-file plan.md
- \"Open a new session of this agent with what we discussed\": write a self-contained summary (decisions, open questions, file paths) to a file, then
  forgectl --json session spawn --name continuation --prompt 'Continue this work.' --context-file summary.md
  The context follows the prompt under a heading naming you. Prompt and context together are capped at 64 KiB; for more, write a file in the checkout and point to it.
- An existing session takes at most 8 KiB per send; for a longer plan, send a short prompt that names the file to read.
Check what a provider accepts first (forgectl providers): effort levels, model, resume and read-only differ per provider.

Use --worktree <branch> [--base <ref>] for isolation, or --workspace <id|prefix> for an existing checkout.
Use --role <role> and --parent <ref|self> for a child in the same project; without --parent a session is an independent root.
Use --profile <name|uuid>, --model <id>, --effort <level>, --resume <provider id> and --read-only when supported by the provider.
Keep the returned session_id: a title is only as stable as the name you gave, and an agent may retitle an unnamed session.
If a launch fails after its worktree was created, details.worktree_created names it and next removes it.
Resolve a session by UUID, UUID prefix (at least 8 characters), exact case-insensitive title, or project/title. Ambiguity exits 5 and lists candidates.
`self` and `parent` name your own session and the one that launched you.
Report back to the session that launched you: forgectl session send parent --queue --file report.md. It is submitted as soon as the parent is idle.
Follow sessions: forgectl session watch [<ref>...] [--project <name>] prints one JSON line per state or activity change and ends when the named sessions end.
Send with --queue rather than --now: --now types without Enter unless the agent is hook-confirmed idle, and the prompt then sits in its input.
List: forgectl session list [--project <name>] [--kind agent|shell|editor|task] [--active] [--profile <name|id|default>]; each agent row names its profile (account).
Accounts: sessions launched under any profile are listed and driven here. [orchestration] cross_profile keeps a session to its own account (off), lets it read others (read, the default) or also steer them (full); a refusal exits 6. A caller with no FORGE_SESSION_ID is never limited.
Runs Forge did not launch: forgectl session external list [--project <name>] [--provider <id>] [--profile <name|id|default>]; forgectl session external read <provider-session-id|prefix> [--turns N]. live is true while a process still owns the run, null when the provider cannot say.
Take one over: forgectl session adopt <provider-session-id> [--name <title>] resumes it as a Forge agent session in its checkout and account, which then takes send, wait and hooks. It exits 5 while the run is live (two processes would interleave one transcript) unless --force. A Claude session running outside Forge has no input channel: adopt is the supported way to steer it.
Wait by events: forgectl session wait <ref> --for idle,waiting,exit [--timeout 600]; the first condition to hold wins and `matched` names it. Activity states are agent-only; any session can be waited on for exit. Waiting for idle alone hangs until the timeout when the agent stops to ask something.
Read evidence: forgectl session read <ref> [--lines N] [--max-bytes N]; forgectl session changes <ref>
Send to an agent: forgectl session send <ref> --text '...' [--queue|--now] [--no-submit]
Default send requires hook-confirmed idle. Queue has one slot and refuses a second message. Now pastes but never submits in waiting, working, starting or unknown.
Prompts are capped at 8 KiB and controls are stripped. Non-bracketed pastes flatten LF/TAB to spaces.
Run in a shell: forgectl session exec <ref> --text 'cargo test'. One line, no control characters, and only while the shell is at its prompt (exit 5 while a job runs). Delivery is not success: read the transcript, or exec 'exit' and wait --for exit for a code.
Interrupt a job: forgectl session signal <ref> --signal int --foreground. Without --foreground the signal goes to the session's own process group.
Lifecycle: session kill, session restart (an agent's initial prompt is not re-sent), session close (ended sessions only).
Project actions: forgectl action list --project <name>; forgectl action run <label|id> --project <name>
Idle only means the agent stopped its turn: inspect the transcript, diff and tests before declaring completion.
Only an explicit report settles an orchestration attempt. Session dispatch does not choose tasks or integrate branches.
forgectl mcp exposes the same commands as tools over stdio and runs up to 8 calls at once. Forge registers it as the server `forge` in writable Claude and OpenCode launches; elsewhere configure it in the provider's own MCP settings.
";

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

Only an explicit report settles an attempt. Idle, exit, and timeouts raise attention instead.

Over the forge MCP server every command is a tool named with an underscore (task_add, task_start, run_wait, task_review, task_accept, task_integrate, run_close); a flag is an argument without its dashes.
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

Over the forge MCP server the same commands are the tools ask, state_set, inbox, report (outcome, summary) and context; a flag is an argument without its dashes.

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

A mutation timeout or disconnect is exit 3 with details.uncertain true. Retry with the same --request-id (over MCP, the same request_id argument); only ledger mutations take one.
";
