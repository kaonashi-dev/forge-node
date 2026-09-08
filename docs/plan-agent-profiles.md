# Implementation plan — Agent profiles (§13.4 extended)

**Status:** implemented (2026-08-23). This document stands as the record of the
decisions; the description of the current behaviour lives in
[`docs/agents.md`](./agents.md#launch-profiles-134),
[`docs/ui.md`](./ui.md#launch-profiles-134),
[`docs/protocol.md`](./protocol.md) and
[`docs/persistence.md`](./persistence.md).
**Date:** 2026-08-23
**Scope:** complete the **Agents** section of Settings (today two groups: default + visibility) with the real provider catalogue and, on top of it, **profiles**: named variations of a provider that define their own command, arguments and environment. A profile is launched from the same surfaces as an agent (rail, the tab strip's `+`, the palette, `⌘⇧A`).

---

## 0. Research findings

### 0.1 What already exists

| Piece | Location | Status |
|-------|----------|--------|
| `AgentDescriptor` (static data, travels over IPC) | `crates/domain/src/agent.rs` | Complete; `default_args: Vec<String>` already exists and is empty in all four builtins |
| 4 builtins (`claude`, `codex`, `opencode`, `cursor`) | `crates/agents/src/builtins.rs` | Complete |
| `AgentRegistry` + per-provider executable override | `crates/agents/src/registry.rs` | Complete, with a `provider_overrides` table |
| `build_launch(descriptor, req, env)` | `crates/agents/src/descriptor.rs` | Complete; it already concatenates `default_args + req.extra_args` |
| `LaunchAgentRequest { extra_args, executable_override }` | `crates/domain/src/agent.rs` | Complete; the daemon calls it with `extra_args: vec![]` |
| Agent spawn | `crates/daemon/src/core.rs:2065-2092` | Complete; the **only** place where the profile has to be injected |
| `CreateAgentSession { workspace_id, provider_id, parent, role }` | `crates/protocol/src/request.rs` | Missing `profile_id` |
| Launch surfaces (4 copies of the same loop over `store.providers`) | `apps/tauri` sidebar, session menu, palette, settings chips | Duplicated; they have to be unified before adding profiles |
| Settings' Agents page | `apps/tauri | Two groups: *Default agent* and *Agent visibility* |
| `app_state` as the client preferences store | `crates/persistence` + `SetAppState` | Complete (`ui.default_agent`, `ui.agent_visible.<id>`) |

**The plan already reserved the slot:** §13.4 "Custom agents (post-MVP)" defines new agents declared in `config.toml`. This is different and complementary: a **profile** is not a new provider, it is a way to start a known one. When §13.4 lands, its providers get profiles for free, because profiles are indexed by `provider_id`.

### 0.2 Layout bug found (fixed)

The page looked **empty** in the app: `settings.rs::nav()` did `.w(px(NAV_W))` and then `.size_full()`, and `size_full()` writes `size.width = 100%` over the fixed width (the shell's `style_helpers!` macro: the `size` prefix → `size.width` + `size.height`). With `flex_shrink_0()`, the rail took the whole window and the content panel collapsed to 0 px. The same in the content `div`. Fixed by changing both `.size_full()` to `.h_full()`. **Without this fix there is nothing to design: the section is not visible.**

### 0.3 The fish function behind the requirement

```fish
function claude-work --description "Run Claude Code with Work account config"
    set -l dir "$HOME/.claude-work"
    mkdir -p "$dir"
    set -lx CLAUDE_CONFIG_DIR "$dir"
    command claude $argv
end
```

Translated into the model: a **name** (`Work`) + an **environment**
(`CLAUDE_CONFIG_DIR=~/.claude-work`) + a **prior mkdir** + the **same binary** +
optional **args**. Nothing else. Note that `claude-work` is a *fish function*,
not an executable on `PATH`: a Forge profile cannot point at it as a binary, it
**replaces** it.

### 0.4 What the Claude Code documentation says

| Mechanism | What it does | Source |
|-----------|--------------|--------|
| `CLAUDE_CONFIG_DIR` | Moves settings, session history and plugins out of `~/.claude` → **it is the account switch** | [settings](https://code.claude.com/docs/en/settings), [env-vars](https://code.claude.com/docs/en/env-vars) |
| `ANTHROPIC_PROFILE` | The Anthropic profile name to authenticate with; it wins over the `/login` credential | env-vars |
| `ANTHROPIC_MODEL` / `ANTHROPIC_DEFAULT_MODEL` | The session's model / the initial model | env-vars |
| `--model` | This session's model; it wins over settings and over `ANTHROPIC_MODEL` | cli-reference |
| `--settings <path\|json>` | A settings file that overrides `settings.json` for this session | cli-reference |
| `--permission-mode`, `--add-dir`, `--effort`, `--bare` | Other common startup flags | cli-reference |
| `CODEX_HOME` | Codex's equivalent of `CLAUDE_CONFIG_DIR` (already read by `crates/agents/src/usage/codex.rs:47`) | — |

**Design conclusion:** to *run* a profile no provider-specific knowledge is
needed — a binary + args + env is enough. What is provider-specific is only
**what to offer in the form**, and that is declared as data in `crates/agents`
(principle P2: no other crate branches on `provider_id`).

---

## 1. Concept

> An **agent profile** is a named way to start a known provider: its own
> command, arguments and environment.

- It is **not** a new provider (that is §13.4).
- It is **not** a session: it is the template sessions are born from.
- It inherits the icon, the detection, the binary candidates and the
  `usage_source` from the provider.
- The "bare" provider is still launchable: it is the implicit profile
  (`profile_id: None`).

Vocabulary in the UI: **Profile**. It is shown with the name the user gives it
(`Personal`, `Work`), not as `Claude Code (Personal)` — the user literally asked
to be able to launch "claude personal" and "claude work" as if they were entries
of their own.

---

## 2. Domain — `crates/domain`

`ids.rs`: one more UUID v7 newtype, like all the others.

```rust
uuid_id!(
    /// Identifies a saved launch profile for an agent provider (§13.4).
    AgentProfileId
);
```

`agent.rs`:

```rust
/// A named way to launch a provider: its own command, arguments and
/// environment (§13.4).
///
/// A profile is not a provider. It borrows the provider's descriptor —
/// icon, detection, binary candidates — and only overrides how the process
/// is started. `env` is an *overlay* on the resolved environment, unlike
/// `SpawnSpec.env`, which is complete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentProfile {
    pub id: AgentProfileId,
    /// The provider this profile launches.
    pub provider_id: AgentProviderId,
    /// What the user calls it — "Personal", "Work".
    pub name: String,
    /// Program to run instead of the detected binary. `None` inherits it.
    pub executable: Option<PathBuf>,
    /// Appended after the descriptor's `default_args`.
    pub args: Vec<String>,
    /// Applied over the resolved environment, in order.
    pub env: Vec<(String, String)>,
    pub created_at: Timestamp,
}
```

And, in the descriptor, **what each provider knows how to suggest**:

```rust
/// A field the profile editor offers for this provider (§13.4).
///
/// Pure data: the UI renders it, the daemon reads `Env { is_directory }`
/// to create the directory before spawning. No crate branches on the
/// provider id to get this.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileField {
    /// "Config directory".
    pub label: String,
    /// One line of help: "Its own login, settings and history.".
    pub help: String,
    pub effect: ProfileFieldEffect,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum ProfileFieldEffect {
    /// Set an environment variable. A directory-valued one is created
    /// before launch, which is what the hand-written shell wrappers did.
    Env { name: String, is_directory: bool },
    /// Append `flag` and the value the user types to the arguments.
    Flag { flag: String },
}
```

`AgentDescriptor` gains one field: `pub profile_fields: Vec<ProfileField>`.

`Session` gains `pub agent_profile_id: Option<AgentProfileId>` so the tab and the
history can say which profile it ran with.

---

## 3. `crates/agents`

**`builtins.rs`** — the only provider-specific data this plan adds:

| Provider | Fields offered |
|----------|----------------|
| `claude` | `Env { CLAUDE_CONFIG_DIR, is_directory: true }` "Config directory"; `Flag { --model }` "Default model" |
| `codex`  | `Env { CODEX_HOME, is_directory: true }`; `Flag { --model }` |
| `opencode`, `cursor` | none (the generic args/env editor is still available) |

**`descriptor.rs::build_launch`** already concatenates `default_args +
req.extra_args`; the profile travels as `extra_args` and as
`executable_override`. Only the environment overlay is missing, and it is
applied **after** the terminal hints and **before** the daemon injects
`FORGE_*`:

```rust
pub fn build_launch_with_env_overlay(
    descriptor: &AgentDescriptor,
    req: &LaunchAgentRequest,
    env: &ResolvedEnvironment,
    overlay: &[(String, String)],
) -> Result<SpawnSpec, AgentError>
```

A note on order: `TERM`/`COLORTERM` are set by Forge and are **not** negotiable
from a profile; if an overlay carries them, they are ignored with a
`tracing::warn!`. A profile must not be able to break the emulator.

**Directories**: a helper `ensure_profile_dirs(descriptor, overlay) ->
io::Result<()>`, which creates with `0700` the directory of every variable
declared `is_directory` (the same permission as the worktrees root, §14.2).

---

## 4. Persistence — migration 4

It is **appended** (never edited nor reordered) in
`crates/persistence/src/migrations.rs`:

```sql
CREATE TABLE agent_profiles (
    id              TEXT PRIMARY KEY,
    provider_id     TEXT NOT NULL,
    name            TEXT NOT NULL,
    executable_path TEXT,
    args_json       TEXT NOT NULL,
    env_json        TEXT NOT NULL,
    created_at      TEXT NOT NULL
);
CREATE INDEX idx_agent_profiles_provider ON agent_profiles(provider_id);
CREATE UNIQUE INDEX idx_agent_profiles_name ON agent_profiles(provider_id, name COLLATE NOCASE);

ALTER TABLE sessions ADD COLUMN agent_profile_id TEXT;
```

- `provider_id` is not an FK: providers are not rows.
- `sessions.agent_profile_id` is not an FK either and is **not** cascade-deleted:
  a session in the history must survive the profile that launched it. If the
  profile no longer exists, the UI falls back to the provider's name.
- Profiles **do** persist across restarts (unlike sessions, which
  `purge_sessions` deletes on startup).

---

## 5. Protocol

```rust
// request.rs, Agents section
/// Create or replace a launch profile (§13.4) → `Ack`; `AgentProfilesChanged`.
SaveAgentProfile { profile: AgentProfile },
/// Delete a launch profile → `Ack`; `AgentProfilesChanged`.
RemoveAgentProfile { profile_id: AgentProfileId },
```

An upsert in a single request, following the `CreateContextEnvelope` precedent
(the client builds the whole domain object, id included).

- `Response::Snapshot` gains `agent_profiles: Vec<AgentProfile>`.
- `DaemonEvent::AgentProfilesChanged { profiles: Vec<AgentProfile> }` — the
  complete set, like `ProviderUsageChanged` and `AgentDetectionChanged`: a client
  that misses an event stays coherent.
- `CreateAgentSession` and `CreateChildSession` gain
  `profile_id: Option<AgentProfileId>`.

Errors: a duplicate name → `Conflict`; an unknown provider → `NotFound`; an empty
name or an environment variable with an invalid name → `InvalidRequest`; a custom
executable that does not pass the version probe → `ProviderNotInstalled` (with
the reason in `details`).

---

## 6. Daemon

1. **Startup:** `Inner.profiles: Vec<AgentProfile>` loaded from the table; it
   goes into the snapshot.
2. **`SaveAgentProfile`:** validates the name and the variables; if it carries an
   `executable`, it runs the descriptor's version probe over that binary (3 s,
   the same as §13.1) and rejects it if it does not accept it — that way the form
   gives the error on save, not on launch. It persists and broadcasts.
3. **Spawn** (`core.rs`, the `SessionKind::Agent` arm, today `extra_args:
   vec![]`):

```text
resolve the profile (if profile_id)
  ↓
program   = profile.executable  (exists and is executable)  ||  binary verified by detection
extra_args= profile.args
  ↓
ensure_profile_dirs(descriptor, profile.env)     ← the mkdir -p from the fish function
  ↓
build_launch_with_env_overlay(...)
  ↓
upsert FORGE_SESSION_ID / FORGE_WORKSPACE        ← unchanged, always last
```

The §13.1 step 4 invariant holds: with no custom `executable` **only** the binary
that passed the probe may run. With a custom `executable`, it was already
accepted on save; at launch, existence and the execute permission are checked
(the file may have disappeared).

4. **`RestartSession`** re-reads the session's `agent_profile_id`, so a restart
   respects the profile.
5. **`RemoveAgentProfile`** does not touch live sessions: the process is already
   running with its environment.

---

## 7. Client

`Store` gains `pub agent_profiles: Vec<AgentProfile>`, populated by the snapshot
and replaced wholesale by `AgentProfilesChanged`. `RuntimeCommand::NewAgent`
gains `profile: Option<AgentProfileId>`.

---

## 8. UI

### 8.1 A single catalogue of launchables (prerequisite)

Today the "one terminal + one provider per row" loop is copied in four places.
With profiles it would be four places ×2 rules. First of all, a new module
`apps/tauri/src/shell/` (launchables):
```rust
pub(crate) enum LaunchTarget {
    Shell,
    Agent { provider: AgentProviderId, profile: Option<AgentProfileId> },
}

pub(crate) struct Launchable {
    pub target: LaunchTarget,
    /// "Terminal", "Claude Code", "Work".
    pub label: String,
    /// "--model opus · CLAUDE_CONFIG_DIR", or the detected binary.
    pub detail: Option<String>,
    /// For the icon and the installation state.
    pub provider: Option<AgentProviderId>,
    pub enabled: bool,
    /// Stable key for `ui.agent_visible.*` and for `DefaultAgent`.
    pub key: String,
}

pub(crate) fn launchables(store: &Store, prefs: &Prefs) -> Vec<Launchable>;
```

Order: Terminal, then each visible provider followed by its visible profiles.
`sidebar::launcher`, `session_menu::new_session_button`,
`command_palette::entries` and the *Default agent* chips all become maps over
this list. It is the plan's key structural change.

### 8.2 Settings → Agents

```text
┌──────────────────────────────────────────────────────────────────────┐
│ ← Back to app │ Agents                                               │
│ SETTINGS      │ The CLIs Forge can launch, and the profiles it       │
│  ⚙ General    │ launches them with.                                  │
│  ⌁ Agents     │ ┌ When you press ⌘⇧A ──────────────────────────────┐ │
│  ◉ Personal.  │ │ [Ask ✓] [No agent] [Claude Code] [Personal]      │ │
│               │ │ [Work] [Codex CLI]                               │ │
│               │ └──────────────────────────────────────────────────┘ │
│               │ ┌ Agents ────── 3 of 4 detected · [Re-detect] ─────┐ │
│               │ │ ⌁ Claude Code                               [ ●] │ │
│               │ │   claude · 2.1.4 · /opt/homebrew/bin/claude      │ │
│               │ │   ├ Personal                          ⋯     [ ●] │ │
│               │ │   │ CLAUDE_CONFIG_DIR=~/.claude-personal         │ │
│               │ │   ├ Work                              ⋯     [ ●] │ │
│               │ │   │ ~/.claude-work · --model opus                │ │
│               │ │   └ + New profile                                │ │
│               │ │ ⌁ Codex CLI                                 [ ●] │ │
│               │ │   codex · 0.9.1 · /opt/homebrew/bin/codex        │ │
│               │ │   └ + New profile                                │ │
│               │ │ ⌁ OpenCode                                  [ ○] │ │
│               │ │   opencode, opencode2 · not on PATH              │ │
│               │ │ ⌁ Cursor CLI                                [ ●] │ │
│               │ │   agent, cursor-agent · 1.2.0                    │ │
│               │ └──────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

- The *Default agent* and *Agent visibility* groups are merged: there were two
  lists of the same thing. What is left is one group of chips (what `⌘⇧A`
  starts, now including profiles) and **one** catalogue.
- The switch always means the same thing: *it appears in the launch menus*.
  Hiding the provider does **not** hide its profiles — the real case is "I never
  launch bare `claude`, only Personal and Work".
- A provider that is not installed is still shown, dimmed, and does not accept
  new profiles (the button is disabled with the detection reason as a tooltip).
- `⋯` per profile: *Edit*, *Duplicate*, *Delete* (with confirmation,
  `dialogs.rs`).

### 8.3 Profile editor

It is a **subroute** of the page, not a modal: `dialogs.rs` says that anything
more than a confirmation goes in a panel. `SettingsSection::Agents` starts
carrying state: `AgentsView::{ List, Editor(ProfileDraft) }`.

```text
← Agents / New Claude Code profile

Name          [ Work                                        ]
Command       (•) Detected binary        claude
              ( ) Another executable     [                  ]
Arguments     [ --model opus                                ]
              Space-separated; quotes are respected.
Environment   [ CLAUDE_CONFIG_DIR ] = [ ~/.claude-work    ] [–]
              [ + Variable ]
Suggested     [+ Config directory]  [+ Default model]

Runs as       CLAUDE_CONFIG_DIR=~/.claude-work claude --model opus

                                          [ Cancel ]  [ Save profile ]
```

- **Suggested** are the descriptor's `profile_fields`: one click adds the row
  already labelled (and, for `is_directory`, proposes `~/.claude-<slug of the
  name>`). For OpenCode/Cursor the row does not appear.
- **Runs as** is the line that makes the model obvious: it is exactly the
  equivalent shell command, the one that lives in the fish function today. It is
  recomputed on every keystroke.
- The values of variables whose name contains `KEY`, `TOKEN` or `SECRET` are
  shown masked (`••••`) with an eye to reveal them. See §10.
- Saving validates locally (non-empty name, not duplicated, variable names
  `[A-Za-z_][A-Za-z0-9_]*`) and then against the daemon.

### 8.4 Text input

The `dialogs.rs` drafts are single-field and cursorless: the shell accumulates
keystrokes in a `String`. A form of 5+ fields with paths needs a cursor,
selection and **paste**. `the text-input control TextInput}` is
adopted (already available in the pinned version): `AppShell` starts owning an
`Option<ProfileEditor>` with an `Entity<InputState>` per field. It is the app's
first child entity; `settings_view` receives a `&ProfileEditor` and only renders.
`dialogs.rs` stays as it is — there is no reason to migrate it in this plan.

### 8.5 Session label

Today the title fallback ("Claude Code" when there is neither a user title nor
OSC) is computed in four places (`session_tabs`, `dialogs`, `command_palette`,
`sidebar`). It moves into a single helper that resolves **profile name →
provider name → "Shell"**, so a tab says `Work` and not `Claude Code` when it was
launched that way.

---

## 9. Preferences

- `DefaultAgent` gains `Profile(AgentProfileId)`, persisted as `profile:<uuid>`;
  a deleted profile degrades to `Ask`, just as an uninstalled provider does
  today.
- `agent_visible_key` starts accepting a launchable's key:
  `ui.agent_visible.claude` or `ui.agent_visible.profile:<uuid>`. The parsing in
  `Prefs::from_store` is still "only an explicit `false` hides".
- No migration: `app_state` is opaque text by design and unknown values degrade.

---

## 10. Security and edges

- **Environment values are stored in the clear** in the user's SQLite, like any
  other local config. The UI says so under the *Environment* block and
  recommends pointing at a config directory (`CLAUDE_CONFIG_DIR`) instead of
  pasting an `ANTHROPIC_API_KEY`. The masking of §8.3 is against shoulders and
  screenshots, not encryption.
- A profile cannot override `TERM`, `COLORTERM`, `FORGE_SESSION_ID` nor
  `FORGE_WORKSPACE`.
- `SpawnSpec.env` is still the **complete** environment: the overlay is applied
  over the login shell's resolved environment, it does not replace it.
- A local client could already ask for `SetProviderExecutable` with an arbitrary
  path; profiles do not widen the trust boundary, but they do widen the surface:
  hence the version probe on save.

---

## 11. Out of scope (with the door left open)

- **Usage per profile.** `ProviderUsage` is indexed by `provider_id`, and
  `usage/claude.rs` and `usage/codex.rs` derive the credentials from the resolved
  environment (`$HOME/.claude`, `$CODEX_HOME`). With two accounts, the status bar
  keeps showing the default account's. The natural extension is to pass the
  profile's overlay to the collector and key usage by `(provider, profile)`; that
  is a separate plan and it touches the status bar.
- **Custom providers from `config.toml` (§13.4).** Orthogonal: when they exist,
  they inherit profiles without touching anything.
- **Syncing/importing existing shell functions.** One could detect `~/.claude-*`
  and offer "create profile"; not in this iteration.
- **Per-project profiles.** Today they are global. The table would already allow
  an optional `project_id` later on.

---

## 12. Implementation order (executed)

1. `domain`: `AgentProfileId`, `AgentProfile`, `ProfileField`, the new fields in
   `AgentDescriptor` and `Session`.
2. `agents`: `profile_fields` in the builtins, `build_launch_with_env_overlay`,
   `ensure_profile_dirs`.
3. `persistence`: migration 4 + the `profiles()` repository
   (insert/upsert/delete/list) + the column in `sessions`.
4. `protocol`: the requests, the snapshot field, the event, `profile_id` in the
   two creation requests.
5. `daemon`: loading into `Inner`, validation on save, injection into the spawn,
   restart.
6. `client`: `Store.agent_profiles`, `RuntimeCommand::NewAgent { profile }`.
7. `ui` — **`launchables.rs` first**, and the four surfaces migrated onto it (no
   profiles yet: it must look identical on screen).
8. `ui` — the new catalogue of the Agents page (merging default + visibility).
9. `ui` — the profile editor with `InputState`.
10. `ui` — `DefaultAgent::Profile` and the session label.
11. `docs/agents.md`, `docs/domain.md`, `docs/persistence.md`,
    `docs/protocol.md`, `docs/ui.md`.

Steps 1-6 are verifiable without the GUI; step 7 was a pure refactor with the
existing tests as the safety net.

**Deviations from the design, and why:**

- **Restarting a session whose profile was deleted is rejected** instead of
  falling back to the bare provider. Starting without the `CLAUDE_CONFIG_DIR` the
  session had would mean entering with a different account without warning; an
  error is the honest answer.
- **Value masking is done by the component** (`InputState::masked` + the eye of
  `Input::mask_toggle`) instead of a `SettingsChoice` of our own. Less state of
  ours and the same result.
- **`build_spawn_spec` receives an `AgentLaunch`** (provider + profile + verified
  binary) instead of three loose parameters: with the profile it was eight
  arguments and clippy is right that this no longer reads.
- **`client` re-exports `ProviderInfo`** so `ui` can name what is in
  `Store::providers` without depending on `protocol` (§17).

---

## 13. Tests

| Layer | Case |
|-------|------|
| `domain` | serde round-trip of `AgentProfile`; `Session` with and without `agent_profile_id` |
| `agents` | the environment overlay wins over the resolved environment; it **cannot** override `TERM`; args = `default_args + profile.args`; `ensure_profile_dirs` creates with `0700` and is idempotent |
| `persistence` | migration 4 over a version 3 DB; name uniqueness per provider (case-insensitive); the session survives the profile's deletion |
| `protocol` | MessagePack round-trip of the new requests and the new event |
| `daemon` (E2E) | launching with a profile puts the variable in the child process (a fake script that prints its environment); saving with an invalid executable → `ProviderNotInstalled`; a duplicate name → `Conflict`; a restart keeps the profile |
| `ui` | `launchables()` orders provider→profiles and respects visibility; the argument tokeniser respects quotes; `DefaultAgent::parse("profile:<uuid>")` and the degradation of a deleted profile; the palette finds "Work" |
