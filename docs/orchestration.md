# Orchestration

`forgectl` is a client of the running `forge-daemon`. It does not start a
second daemon and it does not run jobs itself. The daemon keeps the ledger and
is the only enforcer of the rails. A controller — a human, or an ordinary Forge
agent session — decides what happens next.

Source: `crates/domain/src/orchestration` (pure policy), `crates/daemon/src/orchestration`
(effects), `crates/forge-ctl` (the binary).

## Nouns

| Term | Meaning |
|------|---------|
| **Run** | One objective, one controller, one integration workspace, one board. |
| **Task** | A unit of work inside a run, with dependency edges. A task is ready only when every dependency is accepted or integrated. |
| **Attempt** | One assignment of a task to a session. Only `forgectl report` from that attempt's own session settles it. |
| **Board** | Run-scoped versioned JSON. `state set` and `state del` require `--if-version`; `0` creates a missing key. |
| **Controller** | The session allowed to accept, reject, integrate, and cancel. A human caller (no `FORGE_SESSION_ID`) is always allowed. |

Identity inside a worker is only the injected environment: `FORGE_SESSION_ID`,
`FORGE_RUN_ID`, `FORGE_TASK_ID`, `FORGE_ATTEMPT_ID`. There is no hidden run.

## What the daemon will not do

- Choose the next task, retry a failed attempt, or resume workers after a restart.
- Merge a pull request into the project's default branch. `agent_may_integrate`
  defaults to `pr`: an agent controller may open one pull request and may merge
  into the integration branch. Nothing in Forge merges that pull request.
- Settle an attempt because it went idle, missed a heartbeat, timed out, or the
  process exited. Exit without `report` marks the attempt lost and raises
  attention.
- Accept a report from another session or from a superseded attempt (exit 5).

A daemon restart marks live attempts lost (`daemon_restarted`) and an active run
interrupted. `run resume` starts a new controller seeded from the ledger; it
also revives an active run whose controller session has ended, and never
replaces a live one. Run messages, including unanswered questions, outlive the
sessions that sent and received them.

## Placement

A write task defaults to its own managed worktree, branched from the run's
integration head (`forge/task-<slug>-<n>`). Read-only attempts may share a
checkout. At most one writing attempt may be live in a workspace. The other
rails, overridable under `[orchestration]` except the always-on caps, are at
most 4 live attempts per run, 32 tasks per run, run depth 2, and 3 attempts
per task. The feature ships enabled.

## Channels

There is no free-form memory store. Controllers and workers share four things:
stored messages (the body is never written into a PTY), reports (summary capped
at 16 KiB, optional result file capped at 1 MiB), the board, and the integration
branch. At most one one-line pointer is typed, and only while activity is idle
and nobody is blocked in `inbox --wait`, which returns on unread mail only. Waiting, starting, and unknown suppress
the pointer. A separate carriage return follows the line.

`run wait` is level-triggered: if the condition already holds it returns the
triggering items immediately. It, `ask --wait`, `inbox --wait` and `state watch`
block on daemon events rather than polling; `run wait` also re-reads every 5 s
because `waiting` and `stalled` attention appear when a clock crosses a
threshold, which no event announces. Every wait defaults to 600 s, or to the
global `--timeout` when one is given; a malformed `--timeout` exits 2.

## CLI contract

`--json`, or `FORGECTL_JSON=1`, prints one object on stdout.
Success is `{"ok":true,"result":…}`. Failure is
`{"ok":false,"error":{"code","message","details","next"}}` with `next` as argv.
Exit codes: 0 ok, 1 other refusal, 2 usage, 3 unreachable or protocol mismatch,
4 not found, 5 conflict or precondition, 6 policy, 124 wait timeout. A repeated
mutation `--request-id` returns the stored result; one that arrives while the
first is still running is refused with exit 5 rather than run twice. An uncertain timeout exits 3
with `details.uncertain: true`.

`guide` and `schema` are served by the binary. `hook` always exits 0 and does
not write SQLite. A CLI connection is not sent terminal activity, bell,
clipboard, editor-frame, or usage broadcasts.

`pr draft` is outside the run ledger. It is how the background agent behind
the GUI's Open PR hands back its pull request: title, branch, commit subject
(plus optional `--commit-body`) as flags, the body on stdin or `--body-file`.
The daemon checks that `FORGE_SESSION_ID` names a known session and caps each
field at 64 KiB. For that agent it closes the session and commits everything
once, on a new branch when the checkout is on its default (never on the
default itself), pushes, and runs `gh pr create --assignee @me`; any other
session's draft is broadcast as `PullRequestDraftReady` and stored nowhere.

`forge-daemon session` and `forge-daemon context` still run and print a
deprecation hint pointing at `forgectl`.

## Dispatch between sessions and projects

Session dispatch does not create a run or a task. It launches independent graph
roots, including in another project; `FORGE_SESSION_ID` is reported as
`spawned_by`, never used as a cross-project parent.

```sh
forgectl --json project list
forgectl --json session list --project api --kind agent --active
forgectl --json session spawn --project api --provider claude --profile Work \
  --name auth-impl --worktree auth-impl --base main \
  --model opus --effort high --prompt-file plan/auth.md
forgectl session rename api/auth-impl auth-review
forgectl session wait api/auth-review --for idle --timeout 600
forgectl session read api/auth-review --lines 200
forgectl session changes api/auth-review
forgectl session send api/auth-review --queue --text 'Review the failing test.'
forgectl --json session shell --project api --name checks
forgectl session exec api/checks --text 'cargo test -p client'
```

`--project` accepts an exact name (case-insensitive), root path or UUID, the
same way in every command that takes one (`run start` and `run list` included).
Paths resolve canonically, so symlinks and platform aliases name the same project.
`--workspace <uuid|prefix>` uses an existing checkout and conflicts with `--worktree`.
Creating a worktree waits for `WorkspaceCreated` with the ordinary request
timeout; it does not reload a snapshot in a loop. `--base` requires `--worktree`.
Every check that needs no daemon round trip (name, project, workspace, parent,
provider capabilities) runs before the worktree is created. A launch the
daemon still refuses after that reports the checkout in
`details.worktree_created`, and `next` starts with
`forgectl workspace remove <id>`; nothing removes it automatically.

Called from inside a Forge session (`FORGE_SESSION_ID` set), `--project` may
be omitted: the session lands in the caller's project **and checkout**, unless
`--workspace` or `--worktree` says otherwise. `session spawn` may also omit
`--provider`: it then uses the caller's provider and, unless `--profile` is
given, its profile (the same account). A `FORGE_SESSION_ID` this daemon does
not know is a usage error rather than "no caller".

`--prompt`/`--prompt-file` and `--context`/`--context-file` together are
capped at 64 KiB for a launch: the prompt travels as one process argument, not
as a paste, so the 8 KiB `send` budget does not apply. Context follows the
prompt under `Context handed over from Forge session "<title>" (<id>):`; the
caller writes that summary, since a TUI transcript is not a usable record of a
conversation.

`--name` travels in the creating request (`title` on `CreateAgentSession` and
`CreateShellSession`), so a session is never created and then left unnamed by
a failed rename. `--role` tags the session, and `--parent <ref|self>` places it
under a session of the **same** project (the daemon refuses another project's
parent); without `--parent` it stays an independent root. `--resume` passes a
provider-owned conversation id and is refused for a provider that cannot resume.

`session list` filters by `--project`, `--kind agent|shell|editor|task` and
`--active`; each entry adds `active`, `ended`, `exit_code`, `signal`, `role`
and `parent` beside the display strings. `session read --lines N` bounds the
transcript by lines as well as by `--max-bytes`. `session changes` is
`GetSessionChanges`: what the session changed since its base commit.
`session restart`, `session close` and `session signal --signal
int|term|hup|kill [--foreground]` expose the daemon's lifecycle requests;
`--foreground` signals the terminal's foreground process group, as a typed
Ctrl-C would, because a job a shell started is not in the session's own group.
`workspace list`/`workspace remove [--force]` list and remove checkouts, and
`action list`/`action run <label|id>` run an *approved* project action
(`RunAction`) — a repository file is never executed from here.
`--prompt` and `--prompt-file <path|->` are exclusive, as are `--context` and `--context-file`; see the 64 KiB launch budget below.
`--profile` accepts a case-insensitive name or UUID for the selected provider.
`--read-only`, `--model`, and `--effort` require descriptor support
([agents.md](./agents.md)). A restart retains model, effort and read-only mode
for the daemon's lifetime; it never re-sends the initial prompt.

Spawn returns `result: {session_id, terminal_id, project, workspace, title,
provider, spawned_by}` without reloading a snapshot to guess the ids. Names
are trimmed and cannot be empty. Session references in `show`, `read`, `kill`,
`rename`, `send`, and `wait` accept a UUID, at least eight UUID-prefix characters,
an exact case-insensitive title, or `project/title`. Titles need not be unique.
An ambiguous reference exits 5 with candidate ids/prefixes, project, title,
provider, state and activity. Resolving names reads one cached snapshot per
command. Use returned UUIDs for unattended scripts.
`session list` keeps `kind` and `state` as display strings (including exit
details); `session show` returns the structured domain session.

`self` and `parent` are session references relative to `FORGE_SESSION_ID`
and win over a title with the same text. A child reports back with
`session send parent --queue --file report.md`: the report waits in the
parent's one queued slot and is submitted when the parent is hook-confirmed
idle, so a planner can learn that its coder finished without polling.
`session watch [<ref>...] [--project <name>]` follows sessions by events and
prints one JSON line per state or activity change (`snapshot`, `updated`,
`removed`), ending when every named session has ended or at its deadline
(exit 124); unlike other commands it streams lines rather than one object.

`session wait --for idle|waiting|working|exit` takes a comma-separated list
and returns when the first condition holds, with `matched` naming it. A
controller waiting for a worker to finish waits for `idle,waiting,exit`:
`idle` alone sits out the whole timeout when the worker stops to ask a
question. The command checks the initial snapshot and then
blocks on `SessionUpdated`, not polling. Only agents report activity, so these
states are refused with exit 5 for any other kind; `--for exit` waits for any
session to end and returns its state (exit code or signal). The default
deadline is 600 seconds; `--timeout` accepts seconds or a duration such as
`10s`. Timeout exits 124, removal exits 4, and an agent that ends before the
activity it was waited for exits 5. **Idle is a turn boundary, not
proof of success**: inspect the transcript, diff and verification before acting.
Silence alone never changes the daemon's activity state.

### Safe prompt delivery

`session send <ref> --text <text>`, `--file <path|->`, or a positional `-`
accepts at most 8 KiB and removes control characters except LF/TAB. It always
uses paste framing, never arbitrary keys. Without bracketed paste, LF and TAB
become spaces so they cannot approve a permission dialog. `--no-submit` omits
Enter even when the target is idle.

| Policy | Behavior |
|--------|----------|
| Default (`IfIdle`) | Refuse unless the live agent is `idle` with hook evidence. |
| `--queue` | Keep one bounded prompt until hook-confirmed idle; deliver immediately if already idle. A second prompt is `Conflict` (exit 5), never a replacement. |
| `--now` | Paste in any activity state; submit only in hook-confirmed idle. **Never approve permissions or answer questions with this command.** |

The result contains `{context_id, delivered, queued, submitted}`. `delivered`
means the PTY write succeeded, not that the agent accepted or completed the
work. Text entered into an idle prompt changes it to `working`, including a
non-submitted draft, preventing a second automatic submission. A queue belongs
to the exact terminal generation and is discarded on exit, restart or removal.
An uncertain write is consumed and never automatically replayed. The context
envelope audits accepted, sanitized text, not proof of delivery; SQLite writes
use a separate connection outside the core lock. Explicit retry after a
timeout can duplicate input: inspect the terminal first. Unlike ledger
mutations, session dispatch is not deduplicated by `--request-id`.

`forgectl guide sessions` serves this contract to an agent. A parent-folder
planner and a Raycast example live in [multi-repo.md](./multi-repo.md).

### Shell commands

`session exec <ref> --text <line>` (or `-` for stdin, one trailing newline
dropped) sends `SendShellCommand`: one line of at most 8 KiB, typed with paste
framing and followed by Enter. A line containing any control character — LF,
CR, TAB included — is refused, not stripped, because stripping would join two
commands or change a completion. The daemon delivers it only while the shell's
own process group owns the terminal's foreground (`tcgetpgrp` on the master);
while a job runs (an editor, a pager, an agent started by hand) the request
exits 5, since that job would read the line and its Enter as keys. The check
runs again under the PTY writer lock right before writing, the same ordering
`send` uses, and the accepted line is audited as a context envelope. There is
no completion signal for a command: read the transcript, or end the shell with
`exit <code>` and `session wait --for exit`. Text a person already typed at
that prompt is not cleared and would prefix the line.

### Accounts and runs Forge did not launch

The daemon owns every session it launched, whatever profile (account) it runs
under, so `session list`, `send`, `wait` and hooks already work across
accounts. A provider's own registry does not: Claude lists sessions per
`CLAUDE_CONFIG_DIR`, so a work-profile Claude cannot see the personal one, and
neither sees a Forge launch in the other. `session list` names each agent's
`profile` (`{"id": null, "name": "default"}` for the provider's own login,
`null` for a shell or editor) and filters with `--profile <name|uuid|default>`.

`session external list [--project] [--provider] [--profile]` (`ListExternalSessions`)
reports the runs transcript discovery found in the default directory and every
profile's `config_dir`: provider, provider session id, profile, project,
checkout and `cwd`, title, summary, `updated_at` and `live`. `live` is `true`
while a process still owns the run and `false` once none does; Claude answers
it from `<config>/sessions/<pid>.json` (the `sessionId` and a living `pid`,
read in `crates/agents` off the core lock, at most 256 `.json` entries of
16 KiB per account; an account with more is `null`, since an unread entry may
be the live one). OpenCode and Cursor keep no such registry on disk, so theirs
is `null`, and `adopt` does not wait for `--force` on `null`. `session external read <id|prefix> [--turns N] [--max-bytes N]` is
`GetExternalTranscript` with its existing clamps. A run is named by the
provider's session id or a prefix of at least 8 characters; one that matches in
two accounts exits 5 until `--provider` or `--profile` picks one.

`session adopt <id> [--name] [--provider] [--profile] [--force]`
(`AdoptExternalSession`) resumes the run as an ordinary agent session — the
`CreateAgentSession` path with `resume` — in the checkout it was recorded in
(the project's main checkout when that one is gone) and the account it was
found in. `--profile` only chooses between accounts holding the same id: a
transcript lives in its account's config directory, so resuming it under
another login would not find it. A provider whose descriptor cannot resume is
refused, and a run that is `live` exits 5 with `details.reason = "live"`
unless `--force`, because two processes appending to one transcript interleave
it. The adopted session then takes `send`, `wait` and hooks like any other.

`[orchestration] cross_profile` bounds what a *session* may do to another
account; a caller with no `FORGE_SESSION_ID` (a person at a shell, the GUI) is
never limited. `forgectl` names its caller in the connection handshake
(`Hello.caller_session_id`, or a request's own `caller_session_id`) and the
daemon checks these requests against it, and no others:

- **Read:** `GetSessionTranscript`, `GetSessionChanges`, `GetExternalTranscript`,
  and `SendContext`'s source when `include_transcript` is set.
- **Control:** `SendSessionPrompt`, `SendShellCommand`, `SendSignal`,
  `KillSession`, `RestartSession`, `CloseSession`, `RenameSession`,
  `SetSessionRole`, `CreateChildSession` (its parent), `SendContext` (its
  target, and its source when it spawns a child), `AdoptExternalSession` and
  `DeleteExternalSession`.
- **Filtered:** `GetSnapshot` (`sessions`, `external_agents`) and
  `ListExternalSessions`.

Editor sessions belong to no account, so the editor requests are not checked;
neither are orchestration ledger requests, whose rails are their own.

| Mode | Own account | Another account |
|------|-------------|-----------------|
| `off` | everything | filtered out; read and control refused |
| `read` (default) | everything | listed and readable; control refused |
| `full` | everything | everything |

An agent's account is its profile, or the default login; a shell or editor
belongs to none and is never filtered, and a non-agent caller counts as the
default login. A refusal exits 6 with `code: "policy"` and
`details.reason = "policy:cross_profile"`. The caller is self-declared, so this
is a rail against an agent wandering across accounts, not an authentication
boundary: any process that can open the socket can omit it, and an agent with
filesystem access can read the transcripts directly. Session events still reach
every CLI connection.

#### Input to sessions running outside Forge

A Claude session started outside Forge has no stable input API: channels are a
research preview behind `--dangerously-load-development-channels` and a
dialog, and `Stop` or `asyncRewake` hooks are bound to a turn or undocumented.
Adopt it instead.

OpenCode was checked against 2.0.20 (`opencode --version`). `opencode --help`
shows that a plain `opencode` attaches to a shared background service
(`--standalone` runs a private one); `opencode service status` printed its URL
(`http://127.0.0.1:<port>`), and that server answers `/openapi.json` with 401
`UnauthorizedError` unless authenticated with the password kept in the config
directory's `service.json`. `opencode api GET /openapi.json` (which
authenticates itself) lists `GET /api/session`, `GET /api/session/active`
("foreground Session drains currently owned by this OpenCode process"),
`GET /api/session/{id}/inbox` and `POST /api/session/{id}/prompt` with
`{ text, delivery: "steer" | "queue", resume }` ("durably admit one session
input and schedule agent-loop execution"). The `prompt_async` and
`/tui/append-prompt` routes of earlier versions are gone. The sound path to a
live OpenCode session outside Forge is therefore that service's API, not the
Forge plugin, which is loaded only into Forge launches. It is not
implemented: it needs the user's service password, read from their OpenCode
config, behind an explicit opt-in.

### Spec runs

`forgectl spec run <spec>` launches a planner session whose prompt is the brief
`domain::spec_run::compose_planner_prompt` writes; `forgectl spec brief <spec>`
prints the same brief for the calling session to follow as the planner. The
spec is a Claude artifact link (`claude.ai`, `claude.site`), another link, a
Markdown file or the text itself (`-` reads stdin). Forge classifies it and
never fetches it: an artifact needs the planner's own login and artifact tool.
A value that looks like a path but is not a file exits 4 rather than becoming
a one-line spec.

The palette's **Run Spec…** (also in New Session) is the same launch from the
GUI (`apps/ui/src/spec_run.rs`): the client composes the brief with the same
function and starts the planner in the checkout on screen. A relative path
resolves against that checkout and is checked through the daemon's `ReadFile`
(ADR-012), the Markdown file on screen is prefilled, and a refusal (missing
file, unknown coder) stays in the dialog before anything starts. The dialog
offers a planner row for Claude and each visible Claude profile, and a coder row
for every prompt-taking agent and visible profile (`agent_ops::choices`); a
Claude coder, under any profile, codes at `medium`. The rows a run started
with are remembered (`ui.spec_run.planner`, `ui.spec_run.coder`) and offered
first next time.

The planner validates every criterion (context, and a check that proves it),
asks the person every open point at once with its question selector, writes a
plan with no open question, launches the coder with `session spawn --parent
self --role executor`, waits for `idle,waiting,exit`, reruns the validations
itself, sends numbered findings, and hands what remains to the person after
`--rounds` (default 3). It never commits, pushes or merges.

`--preset claude` codes with Claude at `medium`; `--preset mix` codes with
Cursor, or with `--coder opencode`. A preset is a provider and an effort;
`--coder` replaces both. The planner is Claude at `high` unless `--planner`
and `--planner-effort` say otherwise. Provider, effort and model are checked
against the detected descriptors before anything launches; Cursor applies an
effort only with `--coder-model`, since its effort is a model suffix.
`--coder-profile` and `--planner-profile` take a profile name or UUID of that
role's provider (exit 4 when there is none); the planner launches under its
profile, and the brief names the coder's as `session spawn --profile <uuid>`, so
a profile renamed while the run is going still resolves.

### MCP stdio

Agents reach Forge through MCP first. `forgectl mcp` serves every agent-facing
command as a tool: places (`list_projects`, `list_providers`,
`list_workspaces`, `list_actions`, `run_action`, `status`, `guide`), sessions
(`list_sessions`, `show_session`, `read_session`, `session_changes`,
`list_external_sessions`, `read_external_session`, `adopt_session`,
`spawn_session`, `spawn_shell`, `exec_shell`, `send_prompt`, `wait_session`,
`rename_session`, `signal_session`, `kill_session`, `restart_session`,
`close_session`, `run_spec`, `spec_brief`), a worker's side of a run
(`context`, `report`, `report_show`, `ask`, `inbox`, `send_message`,
`state_list`, `state_get`, `state_set`, `state_delete`, `state_watch`,
`pr_draft`), a controller's (`run_start`, `run_list`, `run_show`,
`run_brief`, `run_wait`, `run_close`, `run_resume`, `task_add`, `task_list`,
`task_show`, `task_start`, `task_review`, `task_accept`, `task_reject`,
`task_cancel`, `task_integrate`, `task_cleanup`) and Forge Review
(`review_start`, `review_show`, `review_status`, `review_cancel`; see
[review.md](./review.md)). A tool's
arguments are the command's flags without dashes; `report`'s `outcome` and
`task_integrate`'s `how` name the flag (`done`, `squash`), and list arguments
(`ack`, `after`, `task`) repeat it. `session watch`, `review bench`,
`review rules check` and `hook` stay CLI-only.

It invokes the same CLI handlers in-process, not a child `forgectl`. A
positional `-` is refused, since stdin is the request stream; `pr_draft` and
`run_close` take `body` as text instead. Input lines are capped at 64 KiB;
tool schemas reject extra arguments and key-injection options, an enum-valued
flag is admitted only from its schema's list, and every positional value is
passed after `--`. Up to 8 `tools/call` requests run at once, each on its own
daemon connection (a shared one would let concurrent waits consume each
other's events); a ninth waits for a free slot, so a long `wait_session` or
`ask` with `wait` (maximum 600 s) does not stall the calls behind it.
Responses can therefore arrive out of request order; match them by `id`.

Every ledger mutation tool (`report`, `ask`, `send_message`, `state_set`,
`state_delete`, `run_start`, `run_brief`, `run_close`, `run_resume`, and the
`task_*` mutations) takes an optional `request_id`, the `--request-id` of the
CLI: a retry after a timeout returns the stored result rather than acting
twice. Session dispatch offers no key, because the daemon keeps no receipt
for it. A call whose `_meta` carries a `progressToken` receives
`notifications/progress` every 10 s until it answers, so a client that
resets its timeout on progress does not cut a long `wait_session`,
`run_wait`, `ask` or `state_watch`.

`tools/list` follows the caller's identity: a worker (`FORGE_ATTEMPT_ID`) sees
its own side and no controller, review, spec or `pr_draft` tools; a
controller (`FORGE_RUN_ID` alone) sees no `context`, `report`, `ask` or
`pr_draft`; a session outside a run sees no `context`, `report` or `ask`, but
keeps the board and `report_show` because `run_start` with `controller: self`
makes it a controller without giving it `FORGE_RUN_ID`. Only the advertised
list narrows. A call is never refused for the role, since the daemon enforces
the rails; a worker allowed nested runs starts one with `forgectl run start`.
A call queued behind the eight running ones reports progress while it waits
for a slot.

The daemon registers the server as `forge` in every **writable** Claude and
OpenCode launch when `[orchestration] mcp` is on (the default) and a
`forgectl` sits beside `forge-daemon`. The server's environment carries the
session's identity (`FORGE_SOCKET`, `FORGE_SESSION_ID`, `FORGE_WORKSPACE` and,
for orchestration, `FORGE_RUN_ID`, `FORGE_TASK_ID`, `FORGE_ATTEMPT_ID`), since
a provider need not pass its own environment to an MCP child. Claude gets a
trailing `--mcp-config <inline JSON>`; it adds to a profile's own
`--mcp-config`, and goes after the prompt because the option is variadic.
OpenCode gets `OPENCODE_CONFIG_CONTENT` with
`{"mcp":{"forge":{"type":"local","command":[…],"environment":{…}}}}`, unless
the launch already sets that variable. Cursor takes no launch-local MCP
configuration, so it keeps the CLI. Its only sources are the checkout's
`.cursor/mcp.json` and the user's `~/.cursor/mcp.json`; it hands an MCP child
only `PATH` and `HOME`, so the session identity would have to travel through
`${env:…}` interpolation, and its approval covers the interpolated values,
so every session would need approving again or `--approve-mcps`, which would
also approve whatever server the repository declares. Writing either file, or
passing that flag, would break the rule that a repository can offer but never
run.

A read-only launch (a pull-request reviewer, a read-only task) never gets the
server: dispatching a writing agent is itself a mutation, and `exec_shell`
runs a command line in any shell session at its prompt. A writable agent
already has a shell, and so `forgectl`, so the server grants it nothing new.

Launch prompts follow what the agent has (`domain::orchestration::ToolSurface`):
worker and controller prompts name MCP tools when the daemon registers the
server and the CLI otherwise, and the MCP wording keeps one line pointing at
`forgectl guide` in case the provider did not load it. The pull-request draft
task names `pr_draft` first and `forgectl pr draft` as its fallback, because
the GUI composes it without knowing the provider's surface.

To configure the server by hand elsewhere, use the absolute installed
`forgectl` path and pass `--socket <path>` before `mcp` if the host does not
inherit `FORGE_SOCKET`. For Claude:

```json
{"mcpServers":{"forge":{"command":"/absolute/path/forgectl","args":["mcp"]}}}
```

`--mcp-config` is variadic, like Claude's `--add-dir`: do not leave it as the
last option in a profile before Forge's positional prompt. Follow it with a
separate option, or use Claude's ordinary project MCP configuration instead.
