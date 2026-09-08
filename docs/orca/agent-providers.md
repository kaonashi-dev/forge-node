# Orca: agent CLI providers

What Orca (MIT, extracted Electron bundle) does for provider detection, accounts,
launch, resume, hooks and token accounting — and what of it Forge should take.

Orca paths below are relative to the extracted bundle root (`out/`). Forge paths
are workspace-relative with line numbers as of `integration/swift-migration`.

Scope: `crates/agents` (all), `crates/domain/src/{agent,external,usage}.rs`,
`crates/daemon/src/{external_agents,opencode_db,usage_stats,jobs}.rs`,
`crates/daemon/src/core.rs` launch/usage paths.

---

## 1. How Orca does it

### 1.1 The provider table

Orca's provider knowledge is one flat data table, `out/shared/tui-agent-config.js`
(`TUI_AGENT_CONFIG`), keyed by agent id. It carries **36 agents** (one of which, `claude-agent-teams`,
is an Orca launch mode over `claude` rather than a separate CLI):

```
claude, claude-agent-teams, openclaude, codex, autohand, opencode, mimo-code,
pi, omp, prime-agent, gemini, antigravity, aider, goose, amp, kilo, kiro, crush,
aug, cline, codebuff, command-code, continue, cursor, droid, kimi, mistral-vibe,
qwen-code, rovo, hermes, openclaw, copilot, grok, devin, ante, trae
```

(`out/shared/agent-kind.js` maps each to a closed telemetry enum;
`out/shared/tui-agent-display-names.js` carries the display names;
`out/shared/tui-agent-selection.js` carries the auto-pick priority order.)

Each entry is small and declarative:

| field | meaning | example |
|---|---|---|
| `detectCmd` | binary name to look for on PATH | `kiro-cli`, not `kiro` |
| `detectCmdAliases` | alternates | `mistral-vibe` → `vibe`, alias `mistral-vibe` |
| `detectRequiredCommands` | co-requisites | `claude-agent-teams` needs `orca` *and* `claude` |
| `detectUnsupportedRuntimes` | skip on these hosts | `['win32','wsl']` |
| `launchCmd` / `launchCmdByPlatform` | the command string | `kiro-cli chat --tui`, `hermes --tui`, `command-code --trust` |
| `expectedProcess` | process-name used to recognise a live agent | `auggie` for id `aug` |
| `promptInjectionMode` | how an initial prompt is delivered | see below |
| `argvPromptSeparator` | `--` before a positional prompt | `trae`, `prime-agent`, `grok` |
| `draftPromptFlag` / `draftPromptEnvVar` | seed the composer without submitting | `--prefill`, `ORCA_PI_PREFILL` |
| `draftPasteReadySignal` / `…TimeoutMs` | when the TUI is ready to accept a paste | `codex-composer-prompt`, 20 s |
| `preflightTrust` | write a trust marker before first launch | `cursor`, `copilot`, `codex` |
| `windowsShiftEnterEncoding` / `ctrlEnterEncoding` | key encoding quirks | `csi-u` |

The per-agent comments are the interesting part — they are a field log of CLI
naming collisions: `continue`'s binary is `cn` because `continue` is a shell
builtin; `aug`'s is `auggie`; `qwen-code`'s is `qwen`; `command-code` is detected
by full name so it does not collide with Windows `cmd.exe`; `trae` is detected on
`traecli` because an unrelated open-source project also ships `trae-cli`.

**Permission ("yolo") flags** are a separate table,
`out/shared/tui-agent-permissions.js` (`YOLO_TUI_AGENT_ARGS`, `YOLO_TUI_AGENT_ENV`):
`claude --dangerously-skip-permissions`, `codex --dangerously-bypass-approvals-and-sandbox`,
`gemini --yolo`, `aider --yes-always`, `amp --dangerously-allow-all`,
`kiro --trust-all-tools`, `cursor --yolo`, `grok --permission-mode bypassPermissions`,
`devin --permission-mode bypass`, `continue --allow "*"`, `goose` via env
`GOOSE_MODE=auto`, and ~15 more. A global three-state summary
(`manual` / `yolo` / `mixed`) is derived by comparing the user's stored per-agent
args against the canonical yolo string, and `applyAgentPermissionMode` flips them
all at once but **only where the user has not customised** — a hand-edited arg
string is left alone rather than clobbered.

### 1.2 Detection

`out/shared/tui-agent-detection-commands.js`. Detection is a **name lookup, not a
probe**: `getTuiAgentDetectionProbeCommands` flattens every agent's `detectCmd`,
aliases and `detectRequiredCommands` into one de-duplicated list; the host
resolves that list once; `resolveDetectedTuiAgentIds` then reports every agent
whose `detectCmd` is present *and* all of whose `detectRequiredCommands` are
present, minus anything filtered by `detectUnsupportedRuntimes`.

There is no version execution, no output verification, no timeout — because
nothing is executed. The cost is O(1) lookups for 36 agents; the price is that a
same-named foreign binary is a false positive, which is why Orca leans on
carefully-chosen unambiguous binary names (`traecli`, `cursor-agent`, `auggie`)
instead of on verification.

`out/shared/agent-process-recognition.js` and `agent-node-entrypoint-identities.js`
do the complementary job at runtime: deciding whether a *running* process is
agent X, from `expectedProcess` plus node-entrypoint identity (the CLIs are Node
scripts, so `argv[0]` is `node`).

### 1.3 Launch and the initial prompt

Orca launches agents as **shell command strings inside a PTY**, not as argv.
`getTuiAgentLaunchCommand` returns `launchCmd` (or the platform override), and
everything downstream splices text into that string
(`out/shared/tui-agent-startup-shell.js` supplies `tokenizeStartupCommand`,
`quoteStartupArg`, `isPosixStartupShell`).

`promptInjectionMode` is the interesting abstraction — five modes:

- **`argv`** — prompt as a positional argument (`claude`, `codex`, `cursor`,
  `grok`, `droid`, `pi`, `omp`, `prime-agent`, `trae`). Optionally preceded by
  `argvPromptSeparator: '--'` so a prompt beginning with `help` or `-` is not
  parsed as a subcommand or flag.
- **`flag-prompt`** — behind a flag (`opencode`, `mimo-code`).
- **`flag-prompt-interactive`** / **`flag-interactive`** — a flag that would
  otherwise run once and exit, paired with `-i` (`gemini`, `antigravity`,
  `copilot`: *"`--prompt` exits on completion (kills the hosted session);
  `-i/--interactive` keeps it interactive"*).
- **`stdin-after-start`** — **type it into the TUI after it boots**. This is the
  fallback for the ~18 agents with no usable prompt flag (`aider`, `goose`,
  `amp`, `crush`, `cline`, `kimi`, `qwen-code`, `devin`, …). `devin` is
  explicitly here because `devin -- <prompt>` auto-submits, which Orca does not
  want.
- **`hermes-query`** — a provider-specific startup-query contract.

`stdin-after-start` needs to know when the TUI is ready. That is
`draftPasteReadySignal`: a per-agent predicate over the PTY output stream —
`codex-composer-prompt`, `grok-composer-prompt`,
`render-cursor-after-bracketed-paste` (wait for the show-cursor that follows
`ESC[?2004h`, because opencode enables bracketed paste before its composer
mounts). With a per-agent timeout (`draftPasteReadyTimeoutMs`, 20 s for codex).
`draftPromptFlag: '--prefill'` (Claude) and `draftPromptEnvVar` (`ORCA_PI_PREFILL`)
are the better path where it exists: seed the composer *without submitting*, so
the paste-after-ready race never happens.

`preflightTrust` writes the CLI's own first-run trust marker before launching
(`.workspace-trusted` for cursor) so the trust menu does not swallow the prompt.

### 1.4 Accounts and auth

Orca has a first-class **managed account** model, not just a config-dir switch.
`out/cli/handlers/account.js` renders `{ accounts: [{ id, email }],
activeAccountId, activeAccountIdsByRuntime: { host, wsl: {…} } }` — accounts are
identified by email, and *which account is active is per execution host*.

Login is not reimplemented: `runAgentLoginInTerminal` spawns the provider's own
CLI login attached to the user's terminal (stdout reserved for the JSON envelope
in `--json` mode, the interactive OAuth prompt on stderr), then Orca captures the
resulting credentials.

The capture is the notable part. `out/main/claude-accounts/keychain.js` exports
`readActiveClaudeKeychainCredentials`, `writeActiveClaudeKeychainCredentials`,
`writeActiveClaudeKeychainCredentialsForRuntime`,
`writeManagedClaudeKeychainCredentials`, `deleteManagedClaudeKeychainCredentials`
(plus `…Strict` variants). On macOS, Claude Code keeps its OAuth token in the
login Keychain under a **single service name**, so switching accounts means
*swapping the Keychain blob*: read the active one, park it under a managed name,
write the target account's blob into the active slot. `CLAUDE_CONFIG_DIR` alone
would not do it. Orca sets `CLAUDE_CONFIG_DIR` too (it appears in the hook
installer chunk in shell/cmd/WSL spellings), but for history and settings.

Concretely (`out/main/index.js` ~241360–242200, ~251780–253030, and
`out/main/chunks/keychain-*.js`):

- Per-account credentials live at
  `<userData>/claude-accounts/<accountId>/auth/.credentials.json` (mode `0600`),
  guarded by an ownership marker `.orca-managed-claude-auth` created with `wx`;
  the resolver refuses symlinks and requires realpath containment.
- On macOS the managed blob is a Keychain item under service
  `"Orca Claude Code Managed Credentials"`, and the *live* one is
  `"Claude Code-credentials"` — or the config-dir-scoped
  `"Claude Code-credentials-<sha256(configDir)[0..8]>"`. Switching accounts
  **copies** (never symlinks): write the target's blob into the live location,
  and on the way out *read back* whatever the CLI refreshed in place, picking the
  freshest of {scoped keychain, legacy keychain, file} and **refusing an
  ambiguous match** rather than guessing.
- OAuth refresh is done by Orca itself against
  `https://platform.claude.com/v1/oauth/token` with a hardcoded client id, a 5 min
  expiry buffer — and is **deferred while a live Claude PTY exists**, so it never
  races the CLI's own refresh.
- At launch, `applyClaudeEnvPatch` **deletes** `ANTHROPIC_API_KEY`,
  `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `AWS_BEARER_TOKEN_BEDROCK`,
  and `ANTHROPIC_CUSTOM_HEADERS` when it matches
  `/authorization|x-api-key|api-key|bearer/i`; a launch that *defines* any of them
  is **refused**: *"This Claude launch defines explicit Anthropic auth environment
  variables. Remove those overrides before using a managed Claude account."*

Codex gets `CODEX_HOME` **and** `ORCA_CODEX_HOME` pointed at
`<userData>/codex-accounts/<id>/home` (marker `.orca-managed-home`), and the
interesting part is what a managed home *inherits*: `skills`, `hooks`, `plugins`,
`plugin-state`, `profile-v2`, `themes`, `prompts` and `AGENTS.md` are
**symlinked** back to `~/.codex/<entry>` (junctions on Windows, `cpSync` plus a
`.orca-copied-<entry>` marker on failure), and `config.toml` is mirrored with
relative paths rewritten. A second account is a second *login*, not a second
blank slate. Codex auth errors have their own classifier
(`out/shared/codex-auth-errors.js`, 11 regexes, ANSI-stripped, capped at 4000
chars).

CLI login (`out/cli/handlers/account.js`) runs in a temp dir: Claude gets
`CLAUDE_CONFIG_DIR=<mktemp>` + `claude auth login --claudeai`; Codex gets
`CODEX_HOME=<mktemp>` + `codex login --device-auth` — device auth specifically
because *"plain OAuth binds a loopback callback the user's browser cannot reach
on a headless/SSH host."* The result is then imported by RPC.

Accounts and resume are coupled: `out/shared/agent-resume-argv-drop.js` exists
solely to *remove* the resume argv when Orca cannot verify which account owns a
session — *"so the pane starts fresh instead of resuming under whichever account
is selected."* A provider session id is scoped to the account that created it.
The same rule governs usage: statusline ingest requires the reported `configDir`
to match the current auth snapshot.

### 1.5 Resume

`out/shared/agent-session-resume.js` is the core. Three pieces:

**A resumable-agent list** (`RESUMABLE_TUI_AGENTS`, 12 of the 36): `claude`,
`codex`, `gemini`, `antigravity`, `opencode`, `pi`, `mimo-code`, `droid`, `grok`,
`devin`, `omp`, `prime-agent`.

**A provider-session extractor** (`extractAgentProviderSession`) that reads the id
out of a **hook payload**, per provider, with per-provider field names:
`session_id` (claude, codex, gemini, droid, kimi, omp), `conversationId`
(antigravity), `sessionID` (opencode, mimo-code), `sessionId`/`session_id`
(grok, devin). `amp`, `cursor`, `command-code`, `copilot`, `hermes` explicitly
return `null` — not resumable.

Critically, for claude/codex it also captures `transcript_path` from the hook,
with this comment:

> *"recent Claude Code names the transcript file with a UUID that differs from
> the hook session_id (so the id-based glob no longer finds it)"*

And for `pi`/`prime-agent` the *file path* **is** the resume locator
(`pi --session <path>`, `prime-agent --resume <path>`), so
`agentProviderSessionsEqual` compares transcript paths for those two and ids for
everyone else.

**An argv builder** (`getAgentResumeArgv`): `claude --resume <id>`,
`codex resume <id>`, `gemini --resume <id>`, `agy --conversation <id>`,
`opencode --session <id>`, `mimo --session <id>`, `droid --resume <id>`,
`grok --resume <id>`, `devin --resume <id>`, `omp --resume <path-or-id>`.

Session ids are validated before use (`normalizeSessionId`): non-empty, ≤512
chars, **must not start with `-`**, no control characters (`hasUnsafeProviderSessionIdChars`).

Because launches are command *strings*, splicing the resume argv in is genuinely
hard, and `out/shared/agent-resume-launch-command.js` is ~150 lines of it:
tokenize the persisted base command with source spans, find the `claude` token in
*command position* only (index 0, after `--`, after PowerShell `&`, or after
`NAME=value` assignments — so `~/.ssh/claude` is never mistaken for the
executable), strip any stale `--resume`/`--continue`/`-r`/`-c` selector by
splicing byte ranges (never re-quoting), insert the authoritative selector before
claude's own `--` terminator if there is one. It bails out to plain appending
whenever any token cannot be modelled for the shell — operators, expansions,
PowerShell `--%`. The header cites a real bug (#12982): a stale selector in a
persisted command competing with the provider session id.

### 1.6 Agent hooks

The mechanism Forge has no equivalent of. Orca runs a **loopback HTTP server**
and installs hook commands into each CLI's own config so the CLI POSTs lifecycle
events to it.

**Coverage.** 14 install targets (`AGENT_HOOK_TARGETS`,
`out/shared/agent-hook-types.js`): claude, openclaude, codex, gemini,
antigravity, amp, cursor, droid, command-code, grok, copilot, hermes, devin,
kimi. 18 *ingest* sources (`out/shared/agent-hook-relay.js`) — the extra four
(opencode, mimo-code, pi, omp, prime-agent) report through a config-dir overlay
or plugin rather than an installed script.

**Events are per-provider, not one set.** Claude:
`SessionStart, UserPromptSubmit, Stop, StopFailure, SubagentStart, SubagentStop,
TeammateIdle, PreToolUse, PostToolUse, PostToolUseFailure, PermissionRequest`
(the last four with `matcher: "*"`). Codex: eight, wire-labelled `session_start`,
`user_prompt_submit`, … Cursor uses camelCase (`beforeSubmitPrompt`, `stop`,
`preToolUse`, `beforeShellExecution`, `beforeMCPExecution`, `afterAgentResponse`).
Gemini has four (`BeforeAgent`, `AfterAgent`, `BeforeTool`, `AfterTool`). Hermes
uses snake_case plugin hooks. Amp is a TypeScript plugin with `session.start`,
`tool.call`, `agent.end`.

**Install targets** are whatever each CLI reads: `~/.claude/settings.json`,
`~/.gemini/settings.json`, `~/.factory/settings.json`,
`~/.cursor/hooks.json`, `~/.gemini/config/hooks.json`,
`<grokHome>/hooks/orca-status.json`, `~/.copilot/hooks/orca.json`,
`~/.config/devin/config.json`, `<kimiHome>/config.toml` (a delimited
`# >>> orca-managed-kimi-hooks` block), `<hermesHome>/config.yaml` + a plugin
directory, `~/.config/amp/plugins/*.ts`, and for Codex a `hooks.json` +
`config.toml` inside a **managed** `CODEX_HOME` — never the user's `~/.codex`.
Writes are atomic (`.<ts>-<uuid>.tmp` → rename) with a rolling `.bak`, and follow
symlinks to the real path.

**The command written into the config is not a path.** It is a
`$HOME`-resolving snippet (`"${HOME-}/.orca/agent-hooks/claude-hook.sh"`, `.cmd`
under msys, a PowerShell `-EncodedCommand` branch when `$HOME` has cmd-unsafe
characters), timeout always 10 s. Ownership is detected by substring match on
`agent-hooks/<stem>.{sh,cmd,ps1}` — including inside base64 `-EncodedCommand`
payloads.

**Transport.** The script POSTs form-encoded to
`http://127.0.0.1:<port>/hook/<source>` with
`--connect-timeout 0.5 --max-time 1.5`, header
`X-Orca-Agent-Hook-Token: <token>`, fields `paneKey`, `tabId`, `launchToken`,
`worktreeId`, `env`, `version`, and `payload@-` (the provider's own stdin JSON).
The server binds `127.0.0.1:0` (ephemeral port), mints `randomUUID()` as the
token, 403s a token mismatch, caps the body at 1 MB, sets a 5 s slowloris
timeout — and **always answers 204, even on a parse error**. A missing script
drains stdin and prints `{}` so the provider is never blocked. Failing open is
the whole design.

**Restart survival.** Port and token are ephemeral, so they are written to
`<userData>/agent-hooks/endpoint.env` (or `endpoint.cmd`, `set `-prefixed) at
mode `0600` in a `0700` dir, atomically, with values constrained to
`/^[A-Za-z0-9._:/-]+$/` or the write is refused. Scripts `. "$ORCA_AGENT_HOOK_ENDPOINT"`
first, so an agent that outlived Orca re-coordinates onto the new port. Stale
`.endpoint-*.tmp` files are swept after 5 min, bounded to 1024 entries.

**Normalization is defensive.** `normalizeHookPayload`
(`out/shared/agent-hook-listener.js:3293`) requires a parseable `paneKey`
(≤200 chars) whose `tabId` matches the posted one, strips a leading BOM
(Cursor/Windows), bounds the JSON at 128 K structural tokens / depth 64, reads
the event name from any of `hook_event_name|hookEventName|hook_type|hookType`,
warns *once* per key (bounded at 32) on a protocol-version or env mismatch, and
runs a per-source normalizer. The status cache is bounded at 500 panes, evicting
`done` or stale panes first. Field caps are explicit
(`out/shared/agent-status-types.js`): prompt 200, tool name 60, tool input 160,
assistant message 8000, interactive prompt 16000, model 120, ≤32 subagents,
payload JSON 4096 structural tokens / depth 16.

**The best idea in the whole subsystem: ranked evidence.**
`out/shared/agent-status-observation.js` declares an ordered list of origins —

```
hook          provider hook event (a relayed SSH hook still counts as 'hook')
osc           OSC 9999 payload parsed out of PTY bytes
title         inferred from the terminal title — "the weakest evidence Orca acts on"
process       the pane's own process/output evidence
launch        seeded when Orca launched the agent itself, before any provider signal
orchestration stamped by dispatch rather than by the agent
```

— with `(authorityId, incarnation, revision)` ordering that is total only *within*
one authority, falling back to timestamps across authorities. Status states are
`working | blocked | waiting | done` (subagents add `idle`), stale after 30 min.

**The OSC 9999 fallback.** `out/shared/agent-status-osc.js` is a stateful parser
for `ESC ] 9999 ; <payload> BEL|ST`, run on every PTY chunk, returning
`cleanData` (the sequence stripped) separately from the parsed payloads, and
buffering a partial prefix across chunk boundaries with a 64 KiB cap. Same
canonical payload as a hook, no provider normalizer, no HTTP.

**Other fallbacks when hooks are silent**: an interrupt keystroke on a pane whose
cached state is `working` (with the baseline unchanged, no live subagents, <30 min
old) synthesizes `{state:'done', interrupted:true}`; a terminal-tail heuristic
derives `blocked`/`waiting` from raw bytes; hydrated status rows are flagged
`restoredUnconfirmed` and never counted as fresh.

**The cost.** Codex hooks are **trust-gated by hash**
(`out/main/codex/codex-app-server-grant-entry.js`): trust lives in
`config.toml` as `[hooks.state."<sourcePath>:<eventLabel>:<groupIndex>:<handlerIndex>"]`
keyed on a signature over `{eventLabel, command, timeoutSec, async, matcher,
statusMessage}`. When a hook's command path moves, Orca drives `hooks/list` over
Codex's app-server RPC, reads `currentHash`/`trustStatus`, and rewrites
`trusted_hash` — with pre- and post-mutation verification, a provenance sidecar,
a grant ledger keyed on home + binary stamp, a 5 min transient-retry interval and
a 30 min capability cache. Installing hooks into somebody else's CLI means owning
its trust model forever.

Remote hosts add a relay that forwards the POST as a JSON-RPC `agent.hook`
notification with `env: 'remote'` and `connectionId: null`; Orca re-normalizes at
the SSH trust boundary — *"so relay skew or a buggy remote process cannot poison
main-process state."* WSL adds a guest-side Node relay with its own endpoint file
and an fs bridge. There is even a detector for antivirus interference
(`HookRequestTruncatedError`, threshold 3, reported once as *"local network
security software is inspecting and blocking loopback HTTP"*).

### 1.7 Environment construction

Orca's child env is an **overlay onto `process.env`**
(`out/main/index.js:35191`): spread `process.env`, then the caller's `env`, then
force `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=Orca`,
`TERM_PROGRAM_VERSION`, `FORCE_HYPERLINK=1`, default `LANG=en_US.UTF-8`.

Because it inherits, it needs explicit *deletion* lists:

- `PANE_IDENTITY_ENV_KEYS` = `ORCA_PANE_KEY`, `ORCA_TAB_ID`, `ORCA_WORKTREE_ID`,
  `ORCA_AGENT_LAUNCH_TOKEN` — deleted unless this launch explicitly provides
  them, and `ORCA_PANE_KEY` only survives after its tabId/leafId are verified
  against the launch.
- `AGENT_HOOK_RUNTIME_ENV_KEYS` — always deleted, then re-added from
  `agentHookServer.buildPtyEnv()` only when hooks are enabled.
- `CLAUDE_CHILD_SESSION_STAMP_ENV_KEYS`, AppImage runtime vars, inherited
  `NO_COLOR`, legacy shim vars, and a caller-supplied `envToDelete` bounded at 32
  entries × 256 chars.

**The git credential guard** (`out/shared/terminal-git-credential-guard.js`) is
applied to the *agent's* environment whenever the launch is unattended or the
command is a recognised agent:

```
GIT_TERMINAL_PROMPT=0
GIT_ASKPASS=""      SSH_ASKPASS=""
GCM_INTERACTIVE=never
GIT_CONFIG_KEY_n/GIT_CONFIG_VALUE_n = credential.interactive=false,
                                      credential.guiPrompt=false
GIT_CONFIG_COUNT=<base + 2>
```

The indexed `GIT_CONFIG_*` protocol is validated before being extended (count must
be a non-negative integer with exactly `2*count` indexed keys and no dangling
indices); if the inherited set is ambiguous, only the scalar guards are applied.

Execution hosts are `local`, `ssh:<encoded target>` and `runtime:<encoded env>`
(`out/shared/execution-host.js`); there is no separate VM kind — an ephemeral VM
is `ssh:runtime-ssh-<id>`. `out/shared/terminal-execution-host.js` derives the
host from the **PTY id**, not the cwd, and an id that starts `ssh:`/`remote:` but
does not parse is `'foreign'` rather than local.

### 1.8 Transcript discovery

Orca reads **17 providers'** on-disk history (`AI_VAULT_AGENT_SOURCES`,
`out/main/chunks/session-scanner-antigravity-history-*.js:2631`). Each source
declares its root dirs, extensions and a file/directory predicate:

`claude` `~/.claude/projects` (dir predicate excludes `subagents`) ·
`codex` `$CODEX_HOME/sessions` else `~/.codex/sessions` ·
`gemini` `~/.gemini/tmp` · `copilot` `$COPILOT_HOME/session-state` ·
`cursor` `~/.cursor/projects` (path must contain an `agent-transcripts`
segment) · `grok` `~/.grok/sessions` (`summary.json`) ·
`devin` `~/.local/share/devin/cli/transcripts` ·
`hermes` `~/.hermes/sessions` (`session_*`) · `rovo` `~/.rovodev/sessions`
(`metadata.json`) · `pi` `~/.pi/agent/sessions` · `omp` `~/.omp/agent/sessions` ·
`prime-agent` `~/.prime/agent/sessions` · `openclaw` `~/.openclaw/agents` ·
`droid` `~/.factory/{sessions,projects}` · `kimi` `~/.kimi-code/sessions`
(`session_*/state.json`) · `antigravity`
`~/.gemini/antigravity-cli/brain/<id>/.system_generated/logs/transcript.jsonl` ·
`opencode` the JSON store *and* `opencode*.db` SQLite.

Two details worth stealing:

**cwd → project dir is a hint, not an answer.** `encodeClaudeProjectPath`
(`:18`) is the same slug Forge computes (`[^a-zA-Z0-9] → -`), but Orca also emits
an NFC-normalised variant (macOS writes NFD), uses the slug only to *narrow* the
readdir, and then verifies by opening the **3 newest** `.jsonl` in that dir and
reading the first `cwd` field (≤200 lines), cached per project dir.
`isClaudeProjectDirInScope` matches `dir === prefix || dir.startsWith(prefix + '-')`,
so descendant directories are included. `getClaudeProjectsDir` honours
`CLAUDE_CONFIG_DIR`.

**Grok groups by `encodeURIComponent(cwd)`** — `<sessions>/<encoded cwd>/<id>/chat_history.jsonl`
(`out/shared/grok-session-paths.js`), with a 255-byte cap, an `lstat` symlink
check at three levels, and a bounded brute-force scan (2048 entries) when the cwd
is unknown.

**An incremental, persisted parse cache.** `parseAgentSessionFileCached`
(`:1854`) keys on path, validates `{mtimeMs, sizeBytes, platform}`, and for the
resumable agents (claude, codex, cursor, copilot, droid, openclaw, pi, omp,
prime-agent, antigravity, gemini-jsonl) resumes from a stored **byte offset**
rather than re-reading the file — guarded by `size >= byteOffset` and an explicit
`endsWithNewlineAt(path, byteOffset)`. Only complete JSONL lines advance the
offset; a trailing partial line is fed to a *clone* for display and discarded.
The cache is persisted across restarts (schema v1, 1.5 s debounce, `0700`/`0600`).

Scanning runs in worker **processes**, not just threads:
`out/main/session-scanner-service-entry.js`, `session-scanner-worker-entry.js`,
`session-scanner-opencode-sqlite-worker-entry.js` (better-sqlite3 isolated, 30 s
list / 15 s parse timeouts, 30 s idle teardown, 3 consecutive deaths and it
stops), `wsl-transcript-fs-process-entry.js`.

### 1.9 Token counting

Orca parses the same files **twice, with two different scanners**: an AI Vault
one (history browser, display totals) in
`out/main/chunks/schema-helpers-*.js` + `session-scanner-antigravity-history-*.js`,
and a usage/cost one in `out/main/index.js` (Claude ~27219–28280, Codex
~28960–29980). They use different field sets and different dedupe.

**Claude, cost scanner** (`parseClaudeUsageSourceRecord`, `index.js:27281`).
Only `type === "assistant"`; requires a session id and timestamp; reads
`input_tokens`, `output_tokens`, `cache_read_input_tokens`,
`cache_creation_input_tokens`; drops the record when the sum is ≤ 0. Reasoning is
never read separately — Anthropic folds thinking into `output_tokens`. Same rule
as Forge.

**Claude dedupe is richer than Forge's** (`buildClaudeUsageDedupeKey`, `:27311`):

```
message.id + record.requestId  →  "<messageId>:<requestId>"
message.id alone               →  "msg:<messageId>"
record.uuid                    →  "uuid:<uuid>"
none of those                  →  never deduped
```

Duplicates are merged by taking **`Math.max` per field**, not by summing.
Cross-file, `scanClaudeUsageFiles` (`:27592`) keeps `turnOwnerByDedupeKey` so a
key belongs to exactly one file; a file whose keys were claimed elsewhere is
flagged `hasDeferredClaims` and re-parsed if the owner disappears. The Claude
usage roots are **`~/.claude/projects` *and* `~/.claude/transcripts`**.

**Codex is cumulative, and Orca's delta handling is more careful than Forge's**
(`resolveCodexUsageDelta`, `index.js:29105–29202`):

- When both `total_token_usage` and `last_token_usage` are present, the delta is
  **`last_token_usage` itself** — the subtraction is only the fallback.
- `rawUsageEquals` (comparing input/cached/output/reasoning, deliberately *not*
  `total`) suppresses repeated events.
- A **non-monotonic total emits a "baseline"**: `previousTotals` is reset and the
  event contributes nothing, rather than producing a bogus delta — unless
  `looksLikeStaleRegression` says it is a stale echo, in which case it is dropped.
- Resuming mid-file sets `totalOnlyBaselinePending`, so the first cumulative
  record after a byte-offset resume becomes a baseline instead of a huge turn.
- After the delta, `cachedInputTokens` is clamped to `min(cached, input)`.
- Event dedupe key = `timestamp | tuple(total) | tuple(last)`, with the same
  per-file ownership scheme; files are additionally deduped by `dev:ino`
  (Codex hardlinks rollouts across managed homes).

**Codex worker sessions are excluded.** `isCodexWorkerSession` rejects a whole
rollout when `payload.thread_source` is not `"user"` or `payload.source.subagent`
is an object.

**The AI Vault scanner is sloppier** and is worth *not* copying: it sums every
`assistant` line's usage with no dedupe at all, and its generic `tokenTotal`
(`schema-helpers:1288`) — used for gemini, droid, opencode's file store and
copilot — sums every alias spelling it knows, `reasoning`/`reasoningOutputTokens`/
`reasoning_output_tokens` **included**, on top of `output_tokens`. The opencode
SQLite path likewise does `tokens_input + tokens_output + tokens_reasoning`
(and selects `tokens_cache_read` and `cost` without using them). Kimi has a
one-off guard — `kimiUsageTotal` returns 0 when `usageScope === "session"` — which
is the same cumulative-double-count problem, patched per provider rather than by
a rule.

No `isSidechain` handling appears anywhere in the bundle.

### 1.10 Cost

Orca **does** have price tables — two hardcoded ones in `out/main/index.js`, with
no remote feed.

**Claude, `MODEL_PRICING$1` (`:27841`)**, USD per 1M tokens, four rates per model
(`input`, `output`, `cacheRead`, `cacheWrite`) rather than multipliers:

```
claude-fable-5   10 / 50 / 1    / 12.5
claude-opus-5     5 / 25 / 0.5  /  6.25
claude-sonnet-5   3 / 15 / 0.3  /  3.75
claude-opus-4-8…-4-5   5 / 25 / 0.5 / 6.25
claude-opus-4-1, -4   15 / 75 / 1.5 / 18.75
claude-sonnet-4-6…-4   3 / 15 / 0.3 / 3.75   (+ long-context tier)
claude-haiku-4-5  1 /  5 / 0.1  /  1.25
claude-haiku-3-5  0.8 / 4 / 0.08 / 1
claude-haiku-3    0.25 / 1.25 / 0.03 / 0.3
```

Plus **long-context tiering**: `SONNET_LONG_CONTEXT_PRICING` (`:27834`) —
`thresholdTokens: 200_000`, above which input 6, output 22.5, cacheRead 0.6,
cacheWrite 7.5. `MODEL_ALIASES` (`:27939`) maps dotted and `-thinking` variants;
`normalizeModelForPricing$1` (`:27960`) strips a leading `anthropic[/:]`,
lowercases, `.`→`-`, then falls back to family catch-alls
(`opus-4 → claude-opus-4-8`, `sonnet-4 → claude-sonnet-4-6`) and **returns `null`
for anything else**.

**OpenAI/Codex, `MODEL_PRICING` (`:29421`)** — `input`, `cachedInput`, `output`,
with optional `{threshold, price}` tier arrays and
`LONG_CONTEXT_THRESHOLD_TOKENS = 272_000`. `gpt-5…5.1-codex-max` 1.25/0.125/10,
`gpt-5.2`/`5.3` family 1.75/0.175/14, `gpt-5.4-mini` 0.75/0.075/4.5,
`gpt-5.4-nano` 0.2/0.02/1.25, `gpt-5.4-pro`/`5.5-pro` 30/30/180 (tiered to
60/60/270), `gpt-5.4` 2.5/0.25/15, `gpt-5.5` 5/0.5/30, `gpt-5.6-sol` 5/0.5/30,
`gpt-5.6-terra` 2.5/0.25/15, `gpt-5.6-luna` 1/0.1/6.
`normalizeModelForPricing` (`:29612`) strips reasoning-effort suffixes
(`minimal|low|medium|high|xhigh|auto|none`, both `model (high)` and `model-high`,
up to 4 iterations) and returns `null` otherwise.
`estimateCostUsd` (`:29660`) clamps `cached = min(cached, input)` and bills
`(input − cached)` at the input rate, `cached` at the discount rate — i.e.
`input_tokens` is treated as **inclusive** of cached, exactly as Forge's
`codex_totals` assumes.

Unknown models cost `null`, are excluded from the sum, and the aggregate reports
`estimatedCostUsd: null` rather than `0` when nothing was priced.
`hasInferredPricing` is OR-folded up through sessions and daily rows and rendered
as `"• inferred pricing"` (`out/renderer/assets/Settings-*.js:22441`) — Forge's
`unpriced_turns` and "the estimate is a floor", reached independently.

Reasoning tokens are carried through the whole Codex pipeline and **never billed**.

### 1.11 Quota and rate limits

Five providers, one uniform window shape
(`{usedPercent, windowMinutes, resetsAt, resetDescription}`), canonical sizes
300 min (5 h) and 10080 min (7 d).

**Claude** — `GET https://api.anthropic.com/api/oauth/usage`, `Bearer` +
`anthropic-beta: oauth-2025-04-20` + `User-Agent: claude-code/2.1.0`, 10 s
timeout; `five_hour` → session, `seven_day` → weekly;
`usedPercent = utilization ?? used_percentage` clamped 0–100. There is a **third
window Forge does not read**: `mapFableWeeklyWindow` looks for a
`data.limits[]` entry with `kind === "weekly_scoped"` and
`scope.model.display_name === "fable"`. Credentials from
`$CLAUDE_CONFIG_DIR/.credentials.json` → `claudeAiOauth.accessToken`, with the
macOS Keychain tried *first* on darwin. Errors are classified: 429 terminal
`rate-limited`, 401 recoverable `stale-token`, 403 `missing-scope` when the body
mentions `user:profile`, ≥500 fallback-only.

**Claude statusline** — the good idea:
> *"Claude Code (>=2.1.80) pipes `rate_limits` to the statusLine command on every
> turn — piggybacked on Messages API responses, so reading it costs no
> usage-endpoint budget (the endpoint 429s under Orca's polling)."*

`CLAUDE_STATUSLINE_PATHNAME = '/statusline/claude'`; the script POSTs a form with
a `payload` JSON field (`rate_limits.five_hour`, `.seven_day`, each with
`used_percentage` or `utilization` and `resets_at`) plus a `configDir` field.
Ingest requires `configDir` to match the current auth snapshot — **usage is
account-scoped**. Client floor 15 s/pane, server dedupe 30 s, and a window whose
percentage moved ≤1 point counts as unchanged.

**Claude PTY fallback** — scrape the TUI after stripping CSI sequences:
`/5h\s+limit[^\d%\r\n]*(\d+)%(?:\s*(used|left))?/i` and the weekly twin,
inverting when the word is `left`.

**Codex** — two paths: the app-server RPC (`initialize` → `initialized` →
`account/rateLimits/read`, `resetsAt` in epoch **seconds**), and
`GET https://chatgpt.com/backend-api/wham/usage` →
`rate_limit.primary_window` / `secondary_window`. Notably Orca **classifies
windows by `limit_window_seconds`** rather than trusting the primary/secondary
naming, falling back to the naming only when the duration is unrecognised. Plus a
reset-credit consume endpoint.

**Gemini** `v1internal:retrieveUserQuota` (`usedPercent = (1 − remainingFraction) × 100`,
worst bucket wins), **Kimi** `/usages`, **Grok** `/billing?format=credits`
(weekly + monthly at 43200 min), **OpenCode Go** cookie-based.

Display helpers worth noting: `out/shared/usage-percentage-display.js` rounds
**before** taking the `100 − x` complement (a fixed bug: `round(100−20.5)=80` vs
`100−round(20.5)=79`), and a non-finite value renders as `0`, never as 100%
remaining. `out/shared/rate-limit-reset-format.js` floors to whole units
(`"47m"`, `"3h 54m"`, `"6d 7h"`, `"now"`) and schedules the next repaint at the
next unit boundary rather than on a timer.

### 1.12 Model discovery and session options

A subsystem Forge has no counterpart to.

**Model list probes** (`out/shared/agent-model-probe-spec.js` and friends), run as
subprocesses, exit-code checked, retried against stderr, falling back to a static
seed on empty:

| agent | command | parser |
|---|---|---|
| claude | `claude -p --input-format stream-json --output-format stream-json --verbose`, with a JSON `control_request`/`list_models` written on **stdin** | `parseClaudeModelList` |
| codex | `codex debug models` | JSON `models[].slug/.display_name/.supported_reasoning_levels` |
| opencode | `opencode models` | one bare id per line |
| cursor | `cursor-agent --list-models` | `^<id>\s+-\s+<label>$` |
| grok | `grok models` | bullet rows after the line `Available models:`, `*` marks the default |
| pi | `pi --list-models` | whitespace table, id = `provider/model` |
| antigravity | `agy models` | one raw line per model |
| amp, kimi, copilot | — | static |

Claude has no `models` subcommand, so Orca writes
`{"type":"control_request","request_id":"orca-model-discovery","request":{"subtype":"list_models"}}`
on stdin and reads the `control_response` back out of the stream — bounded by
`assertJsonTextStructureWithinLimits` (64 K structural tokens, depth 16). An old
CLI answers `{"subtype":"error"}` and exits 0, so the seed survives.

**Session option catalogs** (`out/shared/agent-session-option-catalog*.js`) exist
for 5 agents only — claude, codex, gemini, cursor, grok — and declare, per model,
both the **launch args** and the **mid-session slash command**:

- claude: `--model <v>`; `effort` `low|medium|high[|xhigh|max]` → `--effort <v>`,
  mid-session `/effort`; `fastMode` → `/fast`.
- codex: `-m <v>`; `effort` `minimal…ultra` sliced to a per-model ceiling →
  `-c model_reasoning_effort=<v>`, with a bespoke override detector that
  recognises `--reasoning-effort`, `-c`/`--config`, `=`-joined and clustered
  forms.
- cursor: everything is **composed into the model id** rather than a flag —
  `claude-*` → `<id>[-thinking][-<effort>]`, else `<id>[-<effort>][-fast]`.
- grok: `-m <v>` plus `--reasoning-effort <v>`; discovery is authoritative
  (`discoveredModelsAreAuthoritative`).
- gemini: models only, no options.

`hasFlag` (`out/shared/agent-cli-flag-detection.js`) matches exact tokens,
`flag=value`, and clustered single-dash forms (`-mopus` matches `-m`), always
over the tokens *before* a `--` terminator — so a user's own `--model` in their
args suppresses the catalog's.

---

## 2. Side by side

| Orca mechanism | Forge equivalent | Verdict |
|---|---|---|
| 36-agent flat config table, `out/shared/tui-agent-config.js` | 4 descriptors, `crates/agents/src/builtins.rs:110` | **Gap.** Same shape, 9× the coverage. Adding one is a data edit in both. |
| Detection = one batched PATH name lookup for every agent, no execution (`--version` probing exists but only for single-command runtime checks) | `detection::detect` → `find_executable` then `--version` under a 3 s timeout with `expect_substring` and a 64 KiB output cap (`crates/agents/src/detection.rs:33,78,163,259`) | **Forge better.** Orca's `agent`/`cursor-agent` problem is exactly what `expect_substring` solves (`builtins.rs:160`, `detection.rs:94`). Forge's cost is N subprocesses, and only for candidates actually on PATH. |
| Install-dir fallback when a name is not on PATH: `~/.volta/bin`, `~/.asdf/shims`, `~/.fnm/…`, `~/.local/share/mise/shims`, `~/.local/bin`, `~/.bun/bin`, pnpm/yarn dirs, and every `~/.nvm/versions/node/*/bin` newest-first | Only `ResolvedEnvironment::path_entries` from the login shell (`detection.rs:259`) | **Different tradeoff.** Forge resolves the real login shell, which normally *has* nvm/mise on PATH; Orca's list is a workaround for not doing that. Keep Forge's approach; note it as the fallback if a user reports "installed but not detected". |
| Naming-collision knowledge in comments (`cn`, `auggie`, `qwen`, `traecli`) | `binary_candidates` list per descriptor (`builtins.rs:143,159`) | **Same tradeoff.** Forge's mechanism generalises; Orca has the field data. |
| Launch as a shell command **string**, spliced/tokenized/quoted per shell | `SpawnSpec { program, args, cwd, env }`, argv via `execve` (`crates/agents/src/descriptor.rs:219`) | **Forge much better.** `agent-resume-launch-command.js` (150 lines of span-splicing) and `agent-resume-argv-drop.js` exist only because Orca chose strings. Forge has no quoting surface at all. |
| `promptInjectionMode`: `argv`, `flag-prompt`, `flag-prompt-interactive`, `stdin-after-start`, `hermes-query` | `PromptStyle::{Positional, Flag}`; `None` ⇒ `AgentError::PromptUnsupported` (`crates/domain/src/agent.rs:265`, `descriptor.rs:196`) | **Gap.** Forge refuses an initial prompt for opencode (`builtins.rs:148`) and cursor-with-no-resume; Orca types it into the TUI. Forge owns the PTY, so `stdin-after-start` is available to it. |
| `argvPromptSeparator: '--'` | absent | **Gap (small, real).** `descriptor.rs:196` appends the prompt bare; a prompt starting with `-` is parsed as a flag. |
| `draftPromptFlag: '--prefill'`, `draftPromptEnvVar` | absent | **Gap.** Seeding a composer without submitting is strictly nicer than a positional prompt for interactive use. |
| `draftPasteReadySignal` per agent | absent | Only needed if `stdin-after-start` is adopted. |
| `preflightTrust` (write the CLI's trust marker) | absent | **Different tradeoff.** Writing into another tool's state; see §4. |
| `YOLO_TUI_AGENT_ARGS` + three-state `manual/yolo/mixed` summary | `AgentProfile.args` (free text) (`crates/domain/src/agent.rs:481`) | **Gap.** Forge can express it, but the user must know the flag. A `ProfileFieldEffect` for it would be data-driven. |
| Managed accounts by email, active-per-execution-host; credentials **copied** into the live slot on switch, read back on the way out, ambiguous read-backs refused; Keychain services `"Claude Code-credentials"`, config-dir-scoped `"Claude Code-credentials-<sha8>"`, and managed `"Orca Claude Code Managed Credentials"` | `AgentProfile` with a `CLAUDE_CONFIG_DIR`/`CODEX_HOME`/`XDG_DATA_HOME` env overlay (`builtins.rs:193,215,248`) | **Forge's model is better (no shared-state mutation), but its claim may be false on macOS.** Orca's existence of a *config-dir-scoped* Keychain service says Claude Code does scope by config dir on recent versions — but it also keeps writing the unscoped legacy item, so behaviour depends on the CLI version. Forge's help text says "A separate account: its own login" (`builtins.rs:198`) and `usage/claude.rs:73` reads the **unscoped** service. Verify on a real machine before trusting it. See §3.11. |
| Claude OAuth refresh done by Orca, **deferred while a live Claude PTY exists** | Forge never refreshes; it only reads a token to call the usage endpoint (`usage/claude.rs:57`) | **Forge better** — no token lifecycle to own, no race with the CLI. |
| Login runs the provider's own CLI attached to a terminal | Not modelled; user logs in themselves | **Forge fine.** A terminal app can just tell the user to run `claude /login`. |
| Provider session id from **hook payloads**, per-provider field names, `transcript_path` captured too (`out/shared/agent-session-resume.js`) | Interactive: session id = transcript **filename stem** (`crates/daemon/src/external_agents.rs:272`). Headless: read from the event stream via `HeadlessSpec.session_id_fields` (`crates/daemon/src/jobs.rs:387`, `builtins.rs:57,90`) | **Gap, and a likely correctness bug.** Orca documents that recent Claude Code's transcript filename ≠ hook `session_id`. Forge's `file_stem()` is that exact assumption. The headless path is fine. |
| Session id validated: ≤512, no leading `-`, no control chars | Passed straight through (`descriptor.rs:185`) | **Gap (small).** No injection risk (argv), but a leading `-` becomes a flag. |
| 12 resumable agents; `amp`/`cursor`/`copilot`/`hermes`/`command-code` explicitly not | `ResumeStyle` on 3 of 4; cursor `None` with a reasoned comment (`builtins.rs:166-171`) | **Forge better per provider, narrower in coverage.** Forge's reasoning about cursor's optional-arg `--resume` is more careful than Orca's flat `null`. |
| Resume locator can be a **path** (`pi --session <file>`, `prime-agent --resume <file>`) | `ResumeStyle` always emits `[token, id]` (`crates/domain/src/agent.rs:299`) | **Gap (latent).** Only matters if a path-resuming provider is added, or if Claude moves fully to path-based resume. |
| Account-scoped resume: drop the resume argv rather than resume under the wrong account (`agent-resume-argv-drop.js`) | `resumed_from: HashMap<SessionId, String>` (`crates/daemon/src/core.rs:78`); resume is offered whenever the provider is installed and the workspace exists (`apps/tauri | **Gap.** With profiles, a Claude session recorded under profile A can be resumed under profile B and will silently fail or open empty. |
| Hooks: loopback HTTP + per-run token + endpoint file + OSC 9999 fallback; 14 install targets, 18 ingest sources, per-provider event names (11 for Claude) | **None.** Forge reads OSC 0/2 titles only (`crates/terminal-core/src/alacritty_engine.rs:58`) | **Gap** — the largest one, and the one with the highest install cost. See §3.8. |
| Claude statusline `rate_limits` piped per turn | Poll `api.anthropic.com/api/oauth/usage` every 300 s + jitter (`crates/agents/src/usage/claude.rs:14`, `crates/daemon/src/core.rs:3965`) | **Gap.** Orca explicitly records that endpoint 429ing under polling. Statusline data is live, free and per-account. |
| Codex usage endpoint | `chatgpt.com/backend-api/wham/usage` (`usage/codex.rs:12`), honours `$CODEX_HOME` (`:46`) | **Parity.** |
| Claude credential read | `$HOME/.claude/.credentials.json` then Keychain (`usage/claude.rs:57,73`) | **Bug.** Ignores `CLAUDE_CONFIG_DIR`, unlike `usage/codex.rs:46` (honours `CODEX_HOME`) and `usage/analytics.rs:419` (honours `CLAUDE_CONFIG_DIR`). Inconsistent within one crate. |
| Codex `total_token_usage` cumulative; delta prefers `last_token_usage`, re-**baselines** on a non-monotonic total, clamps `cached ≤ input`, drops stale echoes (`index.js:29105`) | Cumulative, per-field `saturating_sub`, no baseline, no clamp (`crates/agents/src/usage/analytics.rs:321,360`) | **Orca better.** Forge's `saturating_sub` turns a reset total into a silent zero and then under-counts every later turn in that file. See §3.2. |
| Claude total excludes reasoning (`claudeUsageTotal`) | Same, enforced by type and test (`crates/domain/src/usage.rs:49,224`) | **Forge better.** Orca's *other* scanner's generic `tokenTotal` double-counts reasoning, and Kimi needed a bespoke `usageScope === "session"` guard. Forge's rule holds everywhere because there is one `TokenTotals::total`. |
| Codex: `input_tokens` inclusive of cached; bill `(input − cached)` at full rate, `cached` at the discount (`index.js:29660`) | Forge subtracts cached out of input up front (`analytics.rs:397`) then prices | **Parity.** Same arithmetic, different place. |
| Price tables: 4 rates per model (`input/output/cacheRead/cacheWrite`), **long-context tiers** (Sonnet >200 K, OpenAI >272 K), alias + family normalisation, **and OpenAI/Codex models priced** (`index.js:27841, 29421`) | 6 Anthropic families, fixed 0.1×/1.25×/2× cache multipliers, `starts_with("claude")` gate, no tiers (`crates/agents/src/usage/pricing.rs:37,59`) | **Orca better in coverage, Forge better in one place.** Forge prices the 5 m vs 1 h cache-write split, which Orca has no field for. Orca prices Codex, which Forge counts as unpriced. See §3.5. |
| Unknown model → `null` cost, `hasInferredPricing` OR-folded to the UI as *"• inferred pricing"* | `unpriced_turns`, estimate shown as a floor (`crates/domain/src/usage.rs:167`) | **Parity.** Same conclusion, reached independently. |
| Claude turn dedupe: `message.id:requestId` → `msg:<id>` → `uuid:<uuid>`, duplicates merged by **`max` per field**, plus cross-file `turnOwnerByDedupeKey` with deferred re-claim (`index.js:27311,27592`) | `seen: HashSet<u64>` of hashed `message.id` only, first-wins (`analytics.rs:129,160`) | **Orca better.** A retried turn shares a `message.id` with a different `requestId`; Forge drops the second one. See §3.4. |
| Claude usage roots: `~/.claude/projects` **and** `~/.claude/transcripts` | `projects` only (`analytics.rs:419`) | **Gap (small).** |
| Codex worker/subagent rollouts excluded (`isCodexWorkerSession`: `thread_source !== "user"` or `source.subagent`) | Every `.jsonl` under `sessions` is scanned (`analytics.rs:432,470`) | **Gap.** Matters for §3.14's history discoverer more than for token totals. |
| Model discovery: per-provider probes (`codex debug models`, `opencode models`, `cursor-agent --list-models`, `grok models`, and a stdin `control_request`/`list_models` for Claude), merged over a static seed | None. `--model` is a free-text profile field (`builtins.rs:203`) | **Gap.** Forge cannot offer a model picker; the user types a string. |
| Session-option catalogs: per-model reasoning-effort ladders with both launch args and mid-session slash commands (`/model`, `/effort`, `/fast`); cursor composes options *into* the model id | None | **Gap.** |
| Incremental transcript parse: byte-offset resume validated by `{mtime, size}` + `endsWithNewlineAt`, only complete lines advance the offset, cache persisted across restarts (`:1854`) | Whole file re-read on every scan past the TTL (`analytics.rs:229,321`; `external_agents.rs:255`) | **Orca better, and this is the expensive one.** AGENTS.md already records a 201 MB rescan landing on a keystroke. See §3.3. |
| Transcript scanning in worker **processes** (`session-scanner-worker-entry.js`, sqlite worker with 30 s/15 s timeouts and a 3-death circuit breaker) | Off the core lock, 60 s TTL (`usage_stats.rs:22`), 10 s TTL for discovery (`external_agents.rs:76`), `SCAN_LIMIT` 500/60, `MAX_DEPTH` 4, mtime pre-filter (`analytics.rs:53,448,470`) | **Parity, different mechanism.** Forge is bounded and reports `skipped`; a Rust thread is cheaper than a Node process. |
| cwd → project dir: slug is a *hint*, verified by reading `cwd` from the 3 newest transcripts in the dir, cached; NFC and NFD slugs both tried | Slug, then per-transcript `cwd` check (`external_agents.rs:230,266`) | **Parity, Forge slightly more exact** (checks every transcript, not a representative). Forge does not try an NFC variant — a macOS path with combining characters can slug two ways. |
| Ranked status evidence: `hook > osc > title > process > launch > orchestration`, ordered by `(authority, incarnation, revision)` | None — activity is inferred from PTY bytes (`Session::last_activity_at`) | **Gap, and the idea is the valuable part**, independent of hooks. See §3.7. |
| Env is an **overlay onto `process.env`**, with explicit delete-lists (`PANE_IDENTITY_ENV_KEYS`, `AGENT_HOOK_RUNTIME_ENV_KEYS`, `envToDelete` bounded 32×256) | `SpawnSpec.env` is a **complete** environment; PTY launch clears inherited vars first (`descriptor.rs:204`, AGENTS.md) | **Forge much better.** Orca's delete-lists exist only because it inherits. Forge's `RESERVED_PROFILE_VARS` refuses at *save* time (`crates/domain/src/agent.rs:492`) rather than deleting at launch. |
| Refuses to launch when the environment defines `ANTHROPIC_API_KEY`, `ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN`, `AWS_BEARER_TOKEN_BEDROCK`, or auth-like `ANTHROPIC_CUSTOM_HEADERS` | Nothing. A profile may set any of them (`RESERVED_PROFILE_VARS` covers only terminal vars) | **Gap.** `builtins.rs:471` asserts `--bare` is never passed *"so a job is on the same subscription login"* — a profile env var defeats that silently. See §3.6. |
| Git credential guard applied to the **agent's** PTY env (`GIT_TERMINAL_PROMPT=0`, empty `GIT_ASKPASS`/`SSH_ASKPASS`, `GCM_INTERACTIVE=never`, indexed `credential.interactive=false`) | Applied only to Forge's own subprocesses, in `git_service::run_git` / `run_git_network` | **Gap.** An agent running `git push` in a Forge PTY can still block on a credential prompt. See §3.6. |
| Managed `CODEX_HOME` **symlinks** `skills`, `hooks`, `plugins`, `prompts`, `themes`, `AGENTS.md` back to `~/.codex`, and mirrors `config.toml` with paths rewritten | `ensure_profile_dirs` creates an empty `0700` directory (`descriptor.rs:238`) | **Gap.** A Forge Codex profile is a blank slate: no prompts, no skills, no `AGENTS.md`. |
| Execution hosts `local` / `ssh:<id>` / `runtime:<id>`, derived from the **PTY id** not the cwd; unparseable remote ids are `'foreign'`, never local | Local only | **Different scope.** Not a gap today; the "derive from the id, and fail closed" rule is worth remembering if remote hosts ever arrive. |
| opencode SQLite scanner worker | `crates/daemon/src/opencode_db.rs` + legacy JSON tree (`external_agents.rs:466`) | **Parity.** Both read both layouts. |
| History discovery covers claude, codex, opencode, antigravity, grok, … | `external_agents::discover` reads **claude and opencode only** (`external_agents.rs:158`) | **Gap, internally inconsistent.** `usage/analytics.rs:74` scans Codex; `builtins.rs:23` declares `codex resume <id>`; the history panel can never offer it. |
| Provider-specific logic lives in `shared/` (renderer + main both import) | Confined to `crates/agents` by invariant | **Forge better in principle, violated in practice.** `crates/daemon/src/external_agents.rs` hardcodes `~/.claude/projects`, the slug rule, `aiTitle`, `toolUseResult`, `~/.local/share/opencode`; `opencode_db.rs` is 607 lines of one provider's schema. That is provider knowledge outside `crates/agents`. |

---

## 3. Recommendations, ranked

### 3.1 Fix the Claude session id — do not trust the filename

**What.** In `crates/daemon/src/external_agents.rs:272`, `session_id` is
`path.file_stem()`. Orca documents that recent Claude Code writes a transcript
whose filename UUID differs from the session id the CLI resumes by. Read
`sessionId` out of the transcript records (every Claude JSONL line carries it)
and use the filename only as a fallback.

**Files.** `crates/daemon/src/external_agents.rs` — add a `session_id` field to
`Scan`, capture it in `scan_record` (`:350`), prefer it in `parse_transcript`
(`:272`); `claude_subagent_count` (`:441`) also keys the subagent directory on
the id and must keep using whichever name names the directory (verify on a real
`~/.claude` before changing that one).

**Why.** This is the difference between "resume works" and "resume silently opens
an empty conversation". It is the single highest-value item here and costs a few
lines.

**What could break.** If the subagent directory is named by the filename stem and
not by `sessionId`, changing both at once breaks `subagent_count`. Change them
independently. Also, `ExternalAgentSession.session_id` is the GUI's card key
(`crates/domain/src/external.rs:24`) — it will change value for existing
transcripts once, which is cosmetically fine (cards are recomputed per snapshot,
never persisted).

Related, same file: consider recording the transcript path as the authoritative
locator alongside the id — `ExternalAgentSession.transcript_path` already exists
(`external.rs:57`), so it is a matter of letting `ResumeStyle` carry a path
someday, not of finding one.

### 3.2 Re-baseline Codex's cumulative counter instead of clamping to zero

**What.** `scan_codex` (`crates/agents/src/usage/analytics.rs:360`) computes a
turn as `running.field.saturating_sub(previous.field)`, per field, and keeps
`previous = running` unconditionally. When `total_token_usage` goes *down* — a
context reset, a stale echo, a rollout file that carries two threads — every
field saturates to 0, `previous` jumps to the new lower value, and every
subsequent turn in that file is measured from the wrong floor.

Orca handles the same data three ways Forge does not
(`resolveCodexUsageDelta`, `out/main/index.js:29105`):

1. When the event carries **both** `total_token_usage` and `last_token_usage`,
   the delta *is* `last_token_usage`; the subtraction is only the fallback.
   Forge reads only `info.total_token_usage` (`analytics.rs:353`).
2. A non-monotonic total emits a **baseline**: reset `previous`, contribute
   nothing — rather than a bogus (or silently zero) turn.
3. Clamp `cached ≤ input` after the subtraction.

**Files.** `crates/agents/src/usage/analytics.rs:321-393` (`scan_codex`) and
`:397` (`codex_totals`). This is contained: one function, no wire change, no
caller change.

**Why.** It is a silent under-count, in the direction that makes Forge look like
it is working. Point 1 alone is probably the bigger win — `last_token_usage` is
the provider stating the turn directly instead of Forge inferring it.

**What could break.** Existing analytics tests assert the current difference
behaviour; they will need a case per branch. Preferring `last_token_usage`
changes reported numbers for every Codex session, so land it with a test that
pins a real rollout fixture rather than a synthesised one.

### 3.3 Resume transcript scans from a byte offset

**What.** Every scan past the TTL re-reads whole transcripts, line by line, from
byte 0: `scan_claude` (`analytics.rs:229`), `scan_codex` (`:321`),
`parse_transcript` (`crates/daemon/src/external_agents.rs:255`). Orca caches
`{path → (mtimeMs, sizeBytes, byteOffset, partial state)}` and resumes from the
offset when `size >= byteOffset` **and** `endsWithNewlineAt(path, byteOffset)`,
advancing the offset only past complete JSONL lines.

**Files.** `crates/agents/src/usage/analytics.rs` (the two scanners and `Totals`,
which would need to be per-file resumable state rather than per-provider
accumulation); `crates/daemon/src/usage_stats.rs` (which today caches the
*answer* for 60 s, not the *work*); `crates/daemon/src/external_agents.rs`.

**Why.** AGENTS.md already records the defect this prevents: *"that single `Ack`
dragged a 201 MB transcript rescan … onto every Cmd+T."* The TTL caches bound how
*often* the full cost is paid; they do not reduce it. An active conversation
appends a few kilobytes per turn to a file that may be tens of megabytes.

**What could break.** Three things, and they are why Orca's guard is three
conditions rather than one. (a) A truncated or rewritten file must invalidate,
not resume — hence `size >= offset` plus the newline check, and both `mtime` and
`size` in the key. (b) A partial trailing line must not advance the offset, or
the next scan starts mid-record. (c) Forge's Claude dedupe set
(`analytics.rs:129`) and Codex `previous` counter are *cross-file, per-scan*
state; resuming means persisting them per file, which is the real work here.
Consider doing `external_agents` first — its per-file state is a single `Scan`
struct and much easier to snapshot than `Totals`.

### 3.4 Widen the Claude dedupe key and make merging non-destructive

**What.** `Totals::is_new` (`crates/agents/src/usage/analytics.rs:160`) keys on
`message.id` alone and drops any repeat. Orca keys on
`message.id + requestId`, falls back to `msg:<id>` then `uuid:<uuid>`, and merges
collisions by taking `max` per field rather than dropping.

**Why.** A retried assistant turn re-emits the same `message.id` under a new
`requestId` and is a genuinely separate billed turn; Forge silently drops it.
Conversely a partially-written record and its completed rewrite share both, and
`max` is the right merge — Forge's first-wins can keep the truncated one.

**Files.** `crates/agents/src/usage/analytics.rs:129,160,267`.

**What could break.** Widening the key means *more* turns counted, so totals move
up. That is the correct direction but it will look like a regression against any
snapshot test. Also note Forge stores a 64-bit `DefaultHasher` digest rather than
the string — fine at this scale, but a wider key makes collisions marginally more
likely; if that ever matters, store the string.

### 3.5 Extend the price table where Orca has data Forge does not

**What.** Three additions to `crates/agents/src/usage/pricing.rs`:

1. **Codex/OpenAI rates.** Forge's `price_for` (`:74`) returns `None` for
   anything not starting with `claude`, so every Codex turn increments
   `unpriced_turns` and the cost line is a floor of zero for a Codex-heavy user.
   Orca ships `input`/`cachedInput`/`output` for the `gpt-5.x` families.
2. **Long-context tiers.** Anthropic prices Sonnet above 200 K input at 2×
   input / 1.5× output; OpenAI tiers above 272 K. Forge has one flat rate.
3. **Per-model cache rates** instead of global 0.1×/1.25×/2× multipliers.
   Orca stores `cacheRead`/`cacheWrite` per model, which is the same numbers for
   Anthropic today but stops being an assumption.

**Why.** A cost estimate that reads zero for half a workspace's activity is worse
than the "floor" framing suggests, because the floor is not visibly a floor when
only `unpriced_turns` says so.

**What could break.** Rates in a shipped binary go stale, and a *wrong* number is
worse than `None` — which is exactly the reasoning already written into
`pricing.rs:5`. Only add rates that are checkable against a published price page,
date the comment as `pricing.rs:13` already does, and keep the `None` default.
The long-context tier needs the turn's *input* size to pick the rate, which
`Turn` already has. Do **not** copy Orca's suffix-stripping normaliser
(`minimal|low|medium|high|xhigh|auto|none`) wholesale — Forge's substring-on-family
match already survives suffixes.

### 3.6 Environment hardening: three cheap, independent fixes

All three are things Orca does at launch that Forge's launch path does not, and
none of them need a new subsystem.

**(a) Refuse a launch that brings its own Anthropic auth.**
`builtins.rs:471` already asserts that no built-in ever passes `--bare`, on the
grounds that *"a job is the same binary, started the same way, on the same
subscription login."* An `AgentProfile.env` setting `ANTHROPIC_API_KEY`,
`ANTHROPIC_AUTH_TOKEN`, `CLAUDE_CODE_OAUTH_TOKEN` or `AWS_BEARER_TOKEN_BEDROCK`
defeats that silently and bills a metered API account. Orca deletes them and
*refuses* a launch that defines them.
Forge's equivalent is either a second reserved list (provider-scoped, so it
belongs in `crates/agents`, not in `domain::RESERVED_PROFILE_VARS` which is the
terminal contract) or a `SaveAgentProfile` warning. A refusal at save time fits
the existing pattern (`AgentProfile::is_reserved_var`, `crates/domain/src/agent.rs:503`).
*Risk:* a user deliberately running an API-key account would be blocked. Warn
rather than refuse, or scope the check to profiles.

**(b) Put the git credential guard on the agent's PTY, not just on `run_git`.**
Forge's rules already say every Git subprocess goes through `run_git` /
`run_git_network` with prompts disabled — but an *agent* running `git push` in a
Forge PTY inherits none of that and can block on a credential prompt until
someone notices. Orca applies `GIT_TERMINAL_PROMPT=0`, empty `GIT_ASKPASS` and
`SSH_ASKPASS`, `GCM_INTERACTIVE=never`, and indexed
`credential.interactive=false` / `credential.guiPrompt=false` to any unattended
or agent launch.
*Files:* `crates/agents/src/descriptor.rs:204` (where `TERM`/`COLORTERM` are set
today) or the daemon's PTY spawn. *Risk, and it is real:* the indexed
`GIT_CONFIG_KEY_n`/`VALUE_n` protocol must be validated before being extended —
Orca checks that `GIT_CONFIG_COUNT` is a non-negative integer with exactly
`2*count` keys and no gaps, and falls back to the scalar guards when it is not,
because a wrong count makes Git reject the whole environment. Also: a *headless*
job should have this; an *interactive* agent session arguably should not, since a
human is right there and could answer the prompt. Scope it to headless first.

**(c) Let a Codex profile inherit the user's Codex setup.**
`ensure_profile_dirs` (`crates/agents/src/descriptor.rs:238`) creates an empty
`0700` directory. A user who switches `CODEX_HOME` loses their `prompts`,
`skills`, `AGENTS.md` and `themes`. Orca symlinks exactly those back to
`~/.codex` and mirrors `config.toml`.
*Files:* `crates/agents/src/descriptor.rs:238` plus a per-descriptor declaration
of which entries a profile directory inherits (data, so it stays in `builtins.rs`
and nothing branches on a provider id). *Risk:* symlinks into a directory the
user also edits; a `cpSync`-style copy diverges instead. Orca does symlink-then-
copy-with-a-marker, which is more machinery than this is worth — a symlink and a
clear failure is enough.

### 3.7 Rank status evidence — the part of hooks worth having first

**What.** Orca's `AGENT_STATUS_OBSERVATION_ORIGINS` (`hook > osc > title >
process > launch > orchestration`) is a *design*, not a mechanism: a status has a
provenance, a stronger source overrides a weaker one, and comparisons within one
authority are ordered by `(incarnation, revision)` while cross-authority ones fall
back to timestamps. Forge already has two of these origins available — `launch`
(it started the process) and `title` (it parses OSC 0/2,
`crates/terminal-core/src/alacritty_engine.rs:58`) — and no way to say which one
it is currently believing.

Adopting the ranking costs nothing and makes the later work additive: an `osc`
source (§3.8) and eventually a `hook` source slot in above what exists, rather
than replacing it.

**Files.** `crates/domain` for the status type and its origin enum; the daemon's
session state beside `Session::last_activity_at`; `crates/agents` for anything
provider-shaped in the payload.

**What could break.** `Session::last_activity_at` is runtime-only and PTY-owned
with a ≤1/s coalescing clock; a status field must live under the same rules — no
column, no migration, and no per-chunk allocation. Also copy Orca's staleness
rule (30 min) and its `restoredUnconfirmed` idea: a status you restored is not a
status you observed.

### 3.8 Agent hooks: build the OSC channel, not the HTTP server

Orca's hooks buy three things Forge cannot get otherwise:

1. **The provider session id of a live interactive session** (`SessionStart`), so
   a running agent survives a restart and history cards link to live sessions.
   Forge only gets this for headless jobs (`crates/daemon/src/jobs.rs:387`).
2. **The authoritative transcript path**, which makes §3.1 unnecessary rather
   than merely fixed.
3. **Real status** — `UserPromptSubmit` / `Stop` / `PermissionRequest` gives
   "waiting for you" vs "working" without guessing from PTY bytes.

The install cost is the whole story, and the third report makes it larger than it
first looked: 14 install targets across `settings.json`, `hooks.json`,
`config.toml`, `config.yaml` and a TypeScript plugin; a loopback HTTP server with
a per-run token; an endpoint file so an agent that outlived the app can
re-coordinate onto a new ephemeral port; a versioned wire with warn-once
diagnostics; per-source payload normalisers for 18 providers; a bounded status
cache; a detector for antivirus interfering with loopback HTTP; and — see
`out/main/codex/codex-app-server-grant-entry.js` — driving Codex's app-server RPC
to rewrite `hooks.state."<key>".trusted_hash` whenever the hook command's path
changes, with a provenance sidecar, a grant ledger and a 5 min retry interval.

**Recommendation: do not build the HTTP hook server. Build the OSC 9999 channel.**
Orca's own fallback is the cheap 80%:

- Forge owns the VT engine and already parses OSC 0/2. One more OSC number is a
  local, in-process change with no server, no port, no token, no endpoint file,
  no restart re-coordination, and nothing to be blocked by security software.
- The hook a user installs becomes a one-liner that prints an escape sequence to
  the controlling terminal.
- It degrades to nothing: a session with no hook installed behaves as today.

**Files.** `crates/terminal-core/src/alacritty_engine.rs` (read alacritty's
`Handler`/`osc_dispatch` surface first, per the "read the dependency's source"
rule); `crates/domain` for the payload; `crates/agents` for the payload shape and
the per-provider install snippet, because that is provider knowledge (P2).

**What could break.** The OSC must be **stripped from the rendered stream** —
Orca's parser returns `cleanData` separately from `payloads` and buffers a partial
prefix across chunk boundaries with a 64 KiB cap. Getting that wrong puts escape
bytes on screen or grows an unbounded buffer on the delta rung. This is per-chunk
code on the PTY path (`Daemon::pump_terminal`, one core-lock acquisition per
chunk) — `docs/performance.md` rung 2. It must allocate nothing on the common
path where the sequence is absent: scan for `ESC ] 9` before any parsing, never a
`String` concat per chunk like Orca's JS. Copy the payload caps verbatim (prompt
200, tool name 60, assistant message 8000, ≤32 subagents, 4096 structural tokens,
depth 16) — the input is a subprocess writing into a terminal, so it is
untrusted, and AGENTS.md's "clamp before the allocation" rule applies directly.

Scope the first version to *read-only*: recognise the OSC if the user installs
the snippet themselves, and ship the snippet as documentation. Auto-installing
into `~/.claude/settings.json` needs an uninstall path, must not clobber a user's
own hooks, and — copy this from Orca — needs an "the user removed it, do not
reinstall" marker.

### 3.9 Validate the provider session id before it becomes argv

**What.** Reject a resume id that is empty, >512 chars, contains control
characters, or **starts with `-`**, before `descriptor.rs:185` extends `args`.

**Files.** `crates/domain/src/agent.rs` — a `ResumeStyle::args` that returns
`Option<Vec<String>>`, or a validator on `LaunchAgentRequest`;
`crates/agents/src/descriptor.rs:185` maps the failure to
`AgentError::ResumeUnsupported` or a new variant.

**Why.** Cheap, and the ids come off disk — a filename is attacker-influenceable
if a repo ever ships a `.claude` directory. Forge is not exposed to shell
injection (argv, not strings), so this is only about argument-parsing confusion,
but a `--resume --dangerously-skip-permissions` is not a good outcome.

**What could break.** Nothing, if the failure is a refusal rather than a silent
drop. `AgentError` is `#[non_exhaustive]`, so a new variant is additive.

### 3.10 Read Claude's rate limits off the statusline instead of polling

**What.** Instead of (or in addition to) the 300 s poll of
`api.anthropic.com/api/oauth/usage`, install a Forge-owned `statusLine` command
in Claude's settings that reports `rate_limits` to the daemon. Claude Code ≥2.1.80
pipes `used_percentage` / `utilization` and `resets_at` per window to it on every
turn, piggybacked on the Messages API response.

**Files.** `crates/agents/src/usage/claude.rs` (parse — the window shape is
already handled by `usage/mod.rs:63` `percent_from` and `:85` `resets_from`, which
already accept `utilization` and both reset spellings); a new ingest path in
`crates/daemon`; `crates/daemon/src/core.rs:3965` sweeper becomes a fallback.

**Why.** Live rather than 5-minutes-stale, zero network, no 429 risk, and it is
per-launch so it works with profiles. Orca's own comment says the endpoint 429s
under polling — Forge's 300 s + jitter has not hit that yet, but it is one
`RefreshProviderUsage` button away.

**What could break.** This requires **writing into `~/.claude/settings.json`**,
which is the same install cost as hooks (§3.8) and carries the same objections:
clobbering a user's existing statusline, leaving a dangling command after
uninstall, breaking when the Forge binary moves. Orca needs the whole hook server
to receive it, and stamps a `claude-statusline.installed` marker so a user who
deletes it does not get it back. Do **not** do this before §3.8 — it is the
second use of one transport, not a standalone feature. Rate it high in value, low
in urgency.

Two smaller robustness fixes in the same area, with no install cost:

- **A third Claude window exists.** Orca's `mapFableWeeklyWindow` reads
  `data.limits[]` for an entry with `kind === "weekly_scoped"` and
  `scope.model.display_name === "fable"`. Forge maps only `five_hour` and
  `seven_day` (`crates/agents/src/usage/claude.rs:104`), so a per-model weekly cap
  is invisible.
- **Classify Codex windows by duration, not by name.** Forge hardcodes
  `primary_window → "5h"`, `secondary_window → "week"`
  (`crates/agents/src/usage/codex.rs:79`). Orca reads `limit_window_seconds` and
  classifies, using the naming only as a fallback. One field, and it stops a
  renamed or reordered window from being mislabelled.

### 3.11 Honour `CLAUDE_CONFIG_DIR` when reading Claude credentials

**What.** `crates/agents/src/usage/claude.rs:57` reads
`$HOME/.claude/.credentials.json` and never consults `CLAUDE_CONFIG_DIR`.
`usage/codex.rs:46` honours `$CODEX_HOME`; `usage/analytics.rs:419` honours
`CLAUDE_CONFIG_DIR`. Make the three agree.

**Why.** With a profile that sets `CLAUDE_CONFIG_DIR` (the documented account
switch, `builtins.rs:196`), the usage meter reports the *other* account's
allowance. A number that is confidently about the wrong account is worse than no
number — the same reasoning already written into `usage/mod.rs:14`.

**What could break.** `collect_usage` (`core.rs:3876`) reads one resolved
environment, not a per-profile one, so this fix is only half the story: see §3.12.
On its own it changes nothing for users with no profile. Note also that
`keychain_token` (`usage/claude.rs:73`) queries the unscoped service
`"Claude Code-credentials"`; Orca additionally writes and reads a config-dir-scoped
`"Claude Code-credentials-<sha256(configDir)[0..8]>"`, so a config-dir-aware read
should try the scoped name first and fall back.

### 3.12 Decide what usage means when profiles exist

**What.** `Daemon::collect_usage` (`crates/daemon/src/core.rs:3876`) collects one
reading per *provider*, from the daemon's single resolved environment.
`collect_analytics` (`:3948`) likewise scans one `CLAUDE_CONFIG_DIR`. A user with
"Personal" and "Work" profiles sees one meter and one token total, silently
covering whichever directory the login shell happens to name.

Orca's answer is explicit: accounts are first-class, identified by email, with an
active account *per execution host*, and the UI marks which is which.

**What to change.** Either (a) key `ProviderUsage` and `ProviderAnalytics` by
`(provider_id, profile_id)` and collect once per profile, or (b) document that
usage is the default account only, and say so in the UI. (b) is a comment and a
label; (a) touches `crates/domain/src/usage.rs:77,118`, `usage/mod.rs:38`,
`usage/analytics.rs:60`, `core.rs:3876`, the protocol response and the settings
page.

**Why.** Silent wrongness. Pick (b) now, (a) when profiles are actually used by
more than one person.

**What could break.** (a) is a wire change: `ProviderUsage` and `UsageAnalytics`
cross IPC. (b) breaks nothing.

### 3.13 Give initial prompts a fallback for providers with no prompt flag

**What.** Add a third `PromptStyle` (or a sibling field) meaning "type it into the
TUI after startup", and use it for opencode and any future provider whose CLI has
no interactive prompt flag.

**Files.** `crates/domain/src/agent.rs:265` (`PromptStyle`),
`crates/agents/src/builtins.rs:148` (opencode's `None`),
`crates/agents/src/descriptor.rs:196`, and the daemon's PTY startup path, which
would need a "wait until ready, then write" step.

**Why.** "Resolve with AI" (§16.8) and every prompt-carrying launch currently
refuses opencode outright.

**What could break.** A lot, and this is why it is ranked here and not higher.
Orca needed a **per-agent readiness predicate** (`draftPasteReadySignal`) and a
per-agent timeout because a blind sleep loses the prompt: opencode enables
bracketed paste before its composer mounts; grok shimmers a logo so a
quiet-window heuristic never settles. A readiness predicate over PTY output is
provider-specific state living on the hot path. Do this only with the readiness
signal, never with a fixed delay. Prefer §3.14, which is free.

### 3.14 Small, cheap, do them

- **`--` before a positional prompt.** Add an optional separator to
  `PromptStyle::Positional` (`crates/domain/src/agent.rs:265`). Forge's own
  "Resolve with AI" prompts are assembled from templates and diffs; one starting
  with `-` is parsed as a flag. Orca carries this for `trae`, `prime-agent` and
  `grok`. Verify each CLI accepts `--` before adopting it per provider — Claude's
  parser is Commander, Codex's is clap; both do.
- **Discover Codex history.** `external_agents::discover`
  (`crates/daemon/src/external_agents.rs:158`) reads claude and opencode;
  `usage/analytics.rs:432` already knows where Codex rollouts live
  (`$CODEX_HOME/sessions/<y>/<m>/<d>/rollout-*.jsonl`) and `builtins.rs:23`
  already declares `codex resume <id>`. The history panel cannot offer Codex
  today purely because nobody wrote the discoverer. Note the rollout records the
  cwd, so the same "reject a transcript recorded elsewhere" check
  (`external_agents.rs:266`) applies. This is the largest user-visible gap that
  needs no new mechanism.
- **A `ProfileFieldEffect` for permission mode.** Orca's `YOLO_TUI_AGENT_ARGS`
  turns "let it run unattended" into one toggle. Forge can express it in
  `AgentProfile.args` but the user has to know
  `--dangerously-bypass-approvals-and-sandbox`. A declared field
  (`crates/domain/src/agent.rs:456`, `builtins.rs:193`) makes it discoverable and
  keeps the flag in `crates/agents` where it belongs.

### 3.15 More providers and a model picker

Orca's 36 vs Forge's 4. The descriptor mechanism already supports them; each is
one entry in `builtins()` plus a detection test. The ones with the best
value/effort ratio, from Orca's table:

| id | binary | resume | prompt | yolo flag |
|---|---|---|---|---|
| `gemini` | `gemini` | `--resume <id>` | flag + `-i` | `--yolo` |
| `droid` | `droid` | `--resume <id>` | positional | — |
| `grok` | `grok` | `--resume <id>` | positional + `--` | `--permission-mode bypassPermissions` |
| `amp` | `amp` | — | stdin | `--dangerously-allow-all` |
| `copilot` | `copilot` | — | `--prompt` + `-i` | `--yolo` |
| `aider` | `aider` | — | stdin | `--yes-always` |
| `goose` | `goose` | — | stdin | env `GOOSE_MODE=auto` |
| `qwen-code` | `qwen` | — | stdin | `--approval-mode yolo` |

Take `gemini`, `droid` and `grok` first: all three take a positional prompt and a
flag-shaped resume, so they need no new mechanism at all. The stdin-only ones
(`amp`, `aider`, `goose`, `copilot`) should wait for §3.13 or ship with
`prompt: None`, the way opencode does today.

Every one of these needs an `expect_substring` if its binary name is generic
(`grok` in particular — Orca's own comment notes `agent` on PATH is often the
Grok CLI, which is why Forge's cursor descriptor already carries a marker).
Detection cost is linear: `detect_all` (`crates/agents/src/registry.rs:66`) spawns
one 3 s-bounded probe per provider on every refresh. At 4 providers that is
invisible; at 20 it is 20 subprocesses on the startup path. Adopt Orca's
cheap pre-filter — Forge already has it (`find_executable`, `detection.rs:259`,
runs before the probe), so only *installed* providers cost a spawn. That scales
fine; just do not remove that ordering.

**A model picker is the other half.** `--model` is a `ProfileFieldEffect::Flag`
today (`builtins.rs:203`), so the user types a string and finds out at launch
whether it was valid. Orca probes: `codex debug models` (JSON with
`slug`/`display_name`/`supported_reasoning_levels`), `opencode models`,
`cursor-agent --list-models`, `grok models`, `agy models`, `pi --list-models`,
and for Claude — which has no `models` subcommand — a
`{"type":"control_request","request":{"subtype":"list_models"}}` written on stdin
under `-p --input-format stream-json --output-format stream-json --verbose`,
reading the `control_response` back out.

This fits Forge's existing shapes exactly: a `ModelProbe` beside `VersionProbe`
in the descriptor, run by `crates/agents/src/detection.rs:143`
(`run_to_completion` already exists and already bounds a subprocess), results
merged over a static seed so an old CLI keeps working. *Risks:* it is another
subprocess per provider (cache it beside the detection result, do not run it on
every settings render — the `Cmd`+`T` defect in AGENTS.md is exactly this shape),
and the parsers are line-format-fragile, so every one must fall back to the seed
rather than to an empty list. Orca's Claude probe is bounded at 64 K structural
tokens and depth 16; copy that.

---

## 4. Do not copy

- **Shell command strings as the launch representation.** Everything in
  `out/shared/agent-resume-launch-command.js` (span-splicing, `commandPosition`
  tracking, `divergesFromShell` bail-outs, PowerShell `--%` special cases) and
  `agent-resume-argv-drop.js` (suffix-matching to *un*-append an argv) is work
  Forge does not have because `SpawnSpec` is `program + args`
  (`crates/agents/src/descriptor.rs:219`). Orca pays it because it must persist
  and re-edit a user-visible command line. Forge should never gain a "command
  string" field.
- **The AI Vault scanner's generic `tokenTotal`**
  (`out/main/chunks/schema-helpers-*.js:1288`). It sums
  `reasoning`/`reasoningOutputTokens`/`reasoning_output_tokens` on top of
  `output_tokens`, double-counting thinking for any provider reporting both, and
  the opencode SQLite path does the same (`tokens_input + tokens_output +
  tokens_reasoning`). Kimi then needs a bespoke `usageScope === "session"` guard
  and Codex a separate cumulative path — a rule per provider instead of a rule.
  Forge's `TokenTotals::total` (`crates/domain/src/usage.rs:49`) excludes
  reasoning by construction and tests it (`:224`), and there is exactly one of
  it. Keep that. (Orca's *cost* scanner gets this right; the two disagree, which
  is its own warning about parsing the same file twice.)
- **Summing alias spellings.** The same helper adds `input`, `inputTokens` and
  `input_tokens` together. It works only because exactly one is ever present. A
  provider that starts emitting two spellings doubles its own numbers silently.
  Forge's explicit per-provider field reads (`analytics.rs:284`, `:397`) are
  correct and no harder. The one alias Orca reads *defensively* is worth copying
  — `cached_input_tokens ?? cache_read_input_tokens` — because that is a rename,
  not two spellings at once.
- **Parsing the same transcripts twice with two different scanners.** Orca has an
  AI Vault parser and a usage/cost parser over the same `.jsonl` files, with
  different field sets, different dedupe and different answers. Forge is one
  provider away from the same shape: `usage::analytics` and
  `daemon::external_agents` already read the same Claude files for different
  reasons. See §5.
- **Codex hook trust repair** (`out/main/codex/codex-app-server-grant-entry.js`).
  Driving another CLI's app-server RPC to rewrite `hooks.state."<key>".trusted_hash`
  because your hook binary moved is an unbounded maintenance commitment to
  somebody else's security model. If Forge ever installs Codex hooks, install
  them by a **stable path** that never moves (a fixed location under the Forge
  config dir, pointing at a shim) so the trust hash never changes.
- **`preflightTrust`** — pre-writing another tool's first-run trust marker
  (`.workspace-trusted`, and Codex/Copilot equivalents) to skip its consent
  prompt. It is a convenience that defeats a safety gate the CLI author put there
  on purpose, on the user's behalf, without asking. Forge's Git rules already
  take the opposite line (`run_git` never suppresses prompts, it refuses them).
  If a trust prompt is in the way, show it.
- **`YOLO_TUI_AGENT_ARGS` as a global "turn it on everywhere" switch**, and
  especially not as the *shipped default* — `tui-agent-launch-defaults.js`
  re-exports the yolo table as `DEFAULT_TUI_AGENT_ARGS`, so an Orca-launched
  agent skips its own permission prompts unless the user opts out.
  Adopt the *table* (§3.14) — it is provider knowledge that belongs in
  `crates/agents`. Do not adopt `applyAgentPermissionMode`'s one-toggle-flips-every-agent
  behaviour. Per-profile is the right granularity; Forge's `AgentProfile` already
  has it.
- **Keychain blob swapping for account switching**
  (`out/main/chunks/keychain-*.js`). Orca copies the selected account's
  credentials into the *live* slot, reads back whatever the CLI refreshed on the
  way out, picks the freshest of three candidate locations, and refuses when they
  are ambiguous (*"Refusing ambiguous Claude auth read-back"*). All of that
  machinery exists because it mutates a credential store shared with the user's
  own `claude` CLI outside Forge — a crashed switch leaves the wrong account
  active in the user's shell. If §3.11 shows `CLAUDE_CONFIG_DIR` really does not
  isolate the login on this machine's CLI version, the honest fix is to say so in
  the profile help text (`builtins.rs:198`) — not to start writing to the login
  Keychain. Likewise **do not adopt Orca-side OAuth refresh**: it forced a
  "defer while a live PTY exists" rule to avoid racing the CLI's own refresh, and
  Forge has no reason to own a token lifecycle it only reads.
- **The loopback HTTP hook transport** (§3.8). Not because it is badly built — it
  is careful — but because of what it *drags in*: an ephemeral port, a per-run
  token, an endpoint file so a surviving agent can re-coordinate after a restart,
  a temp-file sweeper, a protocol version with warn-once diagnostics, per-source
  normalisers for 18 providers, a bounded status cache, and a detector for
  antivirus blocking loopback HTTP. Every one of those is a real problem the
  design creates. OSC 9999 has none of them.
- **Inferring "the agent stopped" from an interrupt keystroke.** Orca synthesizes
  `{state:'done', interrupted:true}` when the user presses an interrupt key on a
  pane whose cached state is `working`, guarded by five conditions and a 30 min
  window, and disabled outright for two providers. It is a heuristic patching
  around a signal that did not arrive. If the status channel is missing, say
  "unknown", not "done".
- **Worker *processes* for transcript scanning**
  (`out/main/session-scanner-*-entry.js`). That is a Node/Electron answer to
  Node/Electron's single-threaded main loop. Forge's threads plus the two TTL
  caches (`usage_stats.rs:22`, `external_agents.rs:76`) already keep the scan off
  the core lock, and a process is far more expensive than the problem.
- **A closed telemetry enum per agent** (`out/shared/agent-kind.js`). Forge has no
  telemetry and this is not a reason to gain any.
- **Orca's detection model** (name lookup, no verification). Adopt its *data* —
  the binary names and the collision notes — not its method. Forge's version
  probe with `expect_substring` is the reason `agent`-is-actually-grok resolves
  correctly (`crates/agents/src/detection.rs:94`, `builtins.rs:160`), and that
  case is real.

---

## 5. One structural note

AGENTS.md: *"Provider-specific binaries, probes, and launch behavior belong in
`crates/agents`; other crates must not branch on provider IDs."*

`crates/daemon/src/external_agents.rs` (1372 lines) hardcodes
`~/.claude/projects`, the `/`-and-`.`-to-`-` slug rule, `aiTitle`,
`toolUseResult`, `isMeta`, `~/.local/share/opencode/storage`, opencode's
`project/`→`session/`→`message/`→`part/` layout and its `parentID` subagent rule.
`crates/daemon/src/opencode_db.rs` (607 lines) is one provider's SQLite schema.
Meanwhile `crates/agents/src/usage/analytics.rs` holds the *same* providers'
transcript locations for a different purpose, and the two disagree about which
providers exist (analytics: claude + codex; discovery: claude + opencode).

Orca, for all its sprawl, keeps this in one place — `shared/`, imported by both
the renderer and main. Forge has the better rule and is not currently following
it. Moving transcript discovery into `crates/agents` alongside `usage::analytics`
would put the two scans of the same files next to each other, make §3.14's Codex
discoverer a ten-line addition instead of a new subsystem, make §3.1 a one-place
fix, and give §3.3's byte-offset cache one owner instead of two.

That is a larger refactor than anything above and should be its
own feature, but it is the change that makes the rest cheap.
