# Implementation plan — GitHub pull requests inside Forge

**Status:** **phases 1-3 implemented** (2026-08-25): `git-service` reads PRs
through `gh api graphql`, `domain::PullRequest` exists, the protocol carries
`RefreshPullRequests` / `PullRequestsUpdated` and the daemon caches and refreshes
without touching the network in `GetSnapshot`. **Phase 4 (the GUI panel) is not
done**: there is no `apps/tauri and no PR commands in
`ui/src/runtime.rs`, so today none of this is visible to the user. The decisions
were closed like this: `gh` as the transport (D1-A), "Mine" derived from the
projects with `[github] include_all_repos` to widen it (D2-A + optional D2-B),
manual refresh with `refresh_secs = 0` by default (D3-A), no persistence (D5-A).
D4 (where the panel lives) is still **open** and blocks phase 4.
**Date:** 2026-08-25
**Scope:** have Forge show, in the desktop GUI, the open pull
requests **per project** and the ones **assigned to me** in a separate section,
with title, description and general information, and an action to open the PR in
the browser. No diff, no comments, no reviewing from the app.

**Explicitly out of scope (the user's request):** seeing the files that changed,
the diff, the commits or the review detail. That lives in the browser and is one
click away.

---

## 0. Research findings (the real code, not `plan.md`)

Forge already knows how to talk to GitHub, but only in one direction:
**writing**. Everything about reading is yet to be built.

| Piece | Location | Status |
|-------|----------|--------|
| `git_service::create_pull_request` (through `gh pr create`) | `crates/git-service/src/github.rs:28` | Complete |
| `run_gh` — a `gh` subprocess with a 60 s timeout | `crates/git-service/src/github.rs:75` | Complete, **private** |
| `Request::CreatePullRequest` | `crates/protocol/src/request.rs:248` | Complete (asynchronous) |
| `DaemonEvent::PullRequestOpened` | `crates/protocol/src/event.rs:139` | Complete |
| `Daemon::create_pull_request` + `Inner::pr_opening` coalescing | `crates/daemon/src/core.rs:1556`, `core.rs:84` | Complete |
| `Request::FetchRemote` / `DaemonEvent::RemoteRefsUpdated` | `request.rs:212`, `event.rs:127` | Complete — **the asynchronous pattern to copy** |
| `Daemon::fetch_remote` + `Inner::fetching` coalescing | `core.rs:1432`, `core.rs:82` | Complete |
| `git_service::list_remotes` → `Vec<(name, url)>` | `crates/git-service/src/repository.rs:181` | Complete — the `owner/repo` comes from here |
| `external_agents::Cache` — a cache with TTL + fingerprint and its own lock | `crates/daemon/src/external_agents.rs:84` | Complete — **the cache pattern to copy** |
| `Store.external_agents` — a recomputed read-only list | `crates/client/src/store.rs:78` | Complete — **the domain pattern to copy** |
| `history_panel` — the right panel with scope + filter + search + cards | `apps/tauri | Complete — **the UI pattern to copy** |
| `RuntimeUpdate::PullRequestFinished` | `apps/tauri | Complete (only for the PR just opened) |
| Any reading of PRs | — | **Does not exist** |
| `domain::PullRequest` | — | **Does not exist** |
| The `git-pull-request.svg` icon | `apps/tauri/src/theme`/assets/icons/` | **Does not exist** (`git-branch.svg` and `external-link.svg` do) |

Five consequences you only see by reading the code:

1. **The repository's only `PullRequest` is a `struct` with a `url` field**
   (`github.rs:19`). It is the receipt of `gh pr create`, not a model. The domain
   type has to be written from scratch.

2. **`run_gh` already exists and is already hardened** (null stdin,
   `GH_PROMPT_DISABLED=1`, `GIT_TERMINAL_PROMPT=0`, a 60 s timeout in a thread
   with `recv_timeout`), but it is private and only accepts a `repo: &Path` as
   `current_dir`. Reading PRs does not need a directory — it needs an
   `owner/repo` and a host. It has to be generalised, not duplicated.

3. **`FetchRemote` already solved the hard problem.** The ack means *"it
   started"*, the result arrives by event, and there is per-project coalescing so
   two consecutive requests do not open two connections. Reading PRs has exactly
   the same shape: network, slow, and the GUI's command queue **also carries the
   keystrokes** (`AGENTS.md`, § *Boundaries And Invariants*).

4. **`external_agents` already solved the "data that is not mine" problem.**
   Agent transcripts are read-only, recomputed, have a 10 s TTL, a fingerprint of
   the scanned roots, their own lock (never the core's) and **zero SQLite
   columns**. A PR is the same thing: remote data, cacheable, never
   authoritative. That is the mould.

5. **The right panel is already taken.** `app_shell.rs:2536` puts the
   `history_panel` into a third `resizable_panel` governed by a `bool`
   (`history_open`) and a shortcut (`cmd-j`). A fourth panel would leave the
   window with four columns on a laptop. This has to be decided (D4) before
   writing UI.

---

## 1. The problem

Today, to know whether a PR is ready, the user leaves Forge. And Forge is exactly
where the context is: the worktree, the branch, the session of the agent that
wrote that code. The real cycle is:

> the agent finishes → `Draft with Juva` → `CreatePullRequest` → **gap** → Chrome
> → back to Forge for the next worktree

That gap is the goal. And there is a second, bigger problem that appears with
several projects open: **there is nowhere that answers "what do I have
pending?"**. The rail says what is alive, the history says what happened, and
nothing says what is waiting for me on the remote.

What the problem is **not**: reviewing code. GitHub is there for that, and
competing with its diff view is a whole product. This feature ends at the "open
in the browser" button.

---

## 2. How others solve it

| Tool | Transport | Auth | Filters | Refresh |
|---|---|---|---|---|
| **gh-dash** (`dlvhdr/gh-dash`) | a `gh` extension, GraphQL | `gh`'s | YAML sections with **GitHub search queries** (`is:open author:@me`, `review-requested:@me`) | manual + `refetchIntervalMinutes` |
| **VS Code — GitHub Pull Requests** | Octokit (REST+GraphQL) from the extension process | VS Code's auth provider (OAuth) | `githubPullRequests.queries`: a list of *(label, query)*. By default **"Waiting For My Review", "Assigned To Me", "Created By Me"** | configurable polling + on opening the view |
| **JetBrains / GeekHub** | the GitHub API from the IDE | a token in the IDE | a ToolWindow with lists per query | polling |
| **GitButler** | the forge's API (GitHub/GitLab/Bitbucket) | its own OAuth, stored by the app | PRs hanging off each branch *lane* | on push / manual |
| **lazygit** | `gh` to create, `open` to view | `gh`'s | none: it opens the current branch's PR | — |
| **`gh pr status`** | GraphQL | `gh`'s | three fixed buckets: current branch / created by you / **review requested from you** | per invocation |
| **Zed** | — | — | — | **it does not solve it**: there are open discussions asking for it (#34759, #40786, #54428) |

**What is worth copying:**

- **The idea of "sections defined by a query"** (gh-dash + VS Code). It is not a
  list of PRs, it is *N named lists*. "Mine" and "this project's" are two
  sections, not two filters over the same grid. It fits what the user asked for
  ("in a separate section").
- **The three buckets of `gh pr status`**: *assigned to me* ≠ *review requested
  from me* ≠ *I wrote it*. The user asked for "assigned", but in practice the one
  that hurts is "review requested". We model all three and let the UI choose.
- **`gh` as the transport** (gh-dash, lazygit): it reuses the login the user
  already has, it works with GitHub Enterprise, and it **does not put a token in
  the daemon's config**. It is exactly the reason already written in
  `github.rs:1-8`.

**What we do NOT copy:**

- **GitButler's own OAuth.** Adding an OAuth flow and a token in the keychain to
  read a list is disproportionate, and it contradicts the decision already taken
  in `github.rs`.
- **VS Code's polling by default.** This repository already rejected that
  explicitly for `git fetch`: `auto_fetch_secs = 0` with the comment *"background
  traffic nobody asked for costs battery"* (`docs/config.example.toml`). Same
  policy here.
- **Everyone's diff view.** Out of scope by explicit request.

**Sources:** [gh-dash](https://github.com/dlvhdr/gh-dash) ·
[vscode-pull-request-github](https://github.com/microsoft/vscode-pull-request-github) ·
[GitButler — GitHub integration](https://docs.gitbutler.com/features/forge-integration/github-integration) ·
[Zed discussion #34759](https://github.com/zed-industries/zed/discussions/34759) ·
[GeekHub for JetBrains](https://dev.to/geekhubdeveloper/new-release---geekhub-github-integration-for-jetbrains-ides-188k)

---

## 3. Proposed model

### 3.1 The transport: **one** `gh api graphql` per host, not N `gh pr list`

The obvious option is `gh pr list --repo owner/name --json ...` per project. I
reject it: that is N subprocesses and N HTTP requests per refresh, and each one
resolves the repository all over again.

Instead, **one GraphQL document with an alias per project**, plus the two
`search()` calls for the personal buckets, plus `viewer` to know which identity
we are talking with. All in one request.

Measured on this machine (`gh 2.98.0`), with two `search()` and one
`repository()`:

```
cost: 1   nodeCount: 2   time: ~1.4 s   graphql rateLimit: 5000/h
```

A complete refresh costs **1 point out of 5000 per hour**. With `gh search prs`
(the REST search API) the limit would be **30 requests per minute**, which is the
real reason not to use it.

The document (generated by the daemon, one `p{n}` alias per GitHub project):

```graphql
query($mine: String!, $review: String!) {
  viewer { login }

  p0: repository(owner: "hellopay-tech", name: "hellopay-backend") {
    nameWithOwner
    pullRequests(states: OPEN, first: 50,
                 orderBy: { field: UPDATED_AT, direction: DESC }) {
      totalCount
      nodes {
        number title url body isDraft createdAt updatedAt
        author { login }
        baseRefName headRefName
        reviewDecision
        additions deletions changedFiles
        comments { totalCount }
        labels(first: 8) { nodes { name color } }
        assignees(first: 8) { nodes { login } }
        reviewRequests(first: 8) {
          nodes { requestedReviewer { ... on User { login } ... on Team { slug } } }
        }
      }
    }
  }
  p1: repository(owner: "…", name: "…") { … }

  # Only if `include_all_repos` is on (see D2)
  mine:   search(query: $mine,   type: ISSUE, first: 50) { nodes { ...PrFields } }
  review: search(query: $review, type: ISSUE, first: 50) { nodes { ...PrFields } }

  rateLimit { cost remaining resetAt }
}
```

with `$mine = "is:open is:pr assignee:@me archived:false"` and
`$review = "is:open is:pr review-requested:@me archived:false"`.

**Why `repository(...) { pullRequests }` and not `search("repo:owner/name")` for
the per-project part:** `pullRequests` reads the repository directly, so it is
exact and immediate. GitHub's search index runs a few seconds behind — a PR just
opened from Forge itself would not show up in the list right after opening it,
which is the worst possible moment not to show up.

**Execution:** `gh api graphql --hostname <host> -f query=… -f mine=… -f
review=…`, with the same hardening `run_gh` already applies. One group of
projects per host (github.com, GHES) → one request per host.

### 3.2 The domain type

A new file `crates/domain/src/pull_request.rs`. It follows the `external.rs`
mould: **read-only, recomputed, never a SQLite column**.

```rust
/// An open pull request, as the GUI needs it.
///
/// Remote, cached data, never authoritative — the same contract as
/// `ExternalAgentSession`: it is recomputed, it is not persisted, and a stale
/// row is a stale answer to a stale question, not an incorrect model.
pub struct PullRequest {
    /// The Forge project it belongs to. `None` for a PR that arrived through the
    /// personal search from a repository that is not a project (see D2).
    pub project_id: Option<ProjectId>,
    /// `owner/name`, the row's stable identity.
    pub repository: String,
    /// The forge's host (`github.com` or the GHES one), to group and for the URL.
    pub host: String,
    pub number: u32,
    pub title: String,
    /// The markdown body, **truncated** to `MAX_PR_BODY` (see B6).
    pub body: String,
    pub body_truncated: bool,
    pub url: String,
    pub author: String,
    pub base_ref: String,
    pub head_ref: String,
    pub is_draft: bool,
    pub review_decision: Option<ReviewDecision>,
    pub labels: Vec<PullRequestLabel>,   // { name, color }
    pub assignees: Vec<String>,
    pub review_requests: Vec<String>,
    pub additions: u32,
    pub deletions: u32,
    pub changed_files: u32,
    pub comment_count: u32,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
    /// What relation I have with this PR. Derived in the daemon from
    /// `viewer.login`, so the GUI does not compare login strings.
    pub relations: PullRequestRelations,
}

#[non_exhaustive]
pub enum ReviewDecision { Approved, ChangesRequested, ReviewRequired, #[serde(other)] Unknown }

/// The three buckets of `gh pr status`, not mutually exclusive.
pub struct PullRequestRelations {
    pub assigned: bool,          // viewer ∈ assignees
    pub review_requested: bool,  // viewer ∈ reviewRequests
    pub authored: bool,          // viewer == author
}

impl PullRequestRelations {
    /// Whether it touches the user in any way. It is what feeds the "Mine"
    /// section and the rail's counter.
    pub fn is_mine(&self) -> bool { self.assigned || self.review_requested || self.authored }
}
```

**Key decision:** the relation is computed **in the daemon**, comparing against
`viewer.login` from the same answer. The GUI never sees a login to compare. It is
the same principle as "`ui` does not branch on `provider_id`": the policy lives
where the data is.

Add it to the `pub use` of `crates/domain/src/lib.rs` and to `docs/domain.md`.

### 3.3 The protocol

A literal copy of `FetchRemote`'s shape.

**Request** (`crates/protocol/src/request.rs`):

```rust
/// Refresh the pull requests in the background → **immediate** `Ack`;
/// `PullRequestsUpdated` when it finishes.
///
/// The same asynchronous shape as `FetchRemote`: it is a network call and the
/// GUI's command channel also carries the keystrokes. A refresh already in
/// flight is *coalesced*, not queued.
RefreshPullRequests {
    /// One specific project, or `None` for all the ones with a GitHub remote.
    /// The "Mine" section always needs `None`.
    project_id: Option<ProjectId>,
},
```

**Event** (`crates/protocol/src/event.rs`):

```rust
/// A background `RefreshPullRequests` finished.
///
/// It carries **the whole list**, like `ProviderUsageChanged` and
/// `AgentProfilesChanged`: a client that missed an event is left with a
/// coherent snapshot instead of a mixture of two moments.
PullRequestsUpdated {
    /// Every known PR after the refresh.
    pull_requests: Vec<PullRequest>,
    /// Which account was queried (`viewer.login`). The GUI shows it in the
    /// header: with two `gh` accounts configured, knowing which one answered is
    /// not a detail (see B3).
    viewer: Option<String>,
    /// Per-repository errors: `gh` answers with partial data (see B2).
    /// `(repository, message)`.
    failures: Vec<(String, String)>,
    /// A global failure — `gh` missing, not logged in, timeout. `None` on
    /// success.
    error: Option<String>,
},
```

**Snapshot** (`crates/protocol/src/response.rs`): add
`pull_requests: Vec<PullRequest>` to `Response::Snapshot`, served **only from the
cache, without touching the network**. A `GetSnapshot` cannot depend on a socket
to GitHub: it is the first thing the GUI does on connect and blocking it for
1.5 s (or up to the timeout) would break startup and reconnection.

At startup the cache is empty; the GUI sends `RefreshPullRequests` right after
connecting and the result arrives by event.

**`client`** (`crates/client/src/ipc.rs` + `store.rs`):
- `Client::refresh_pull_requests(project_id: Option<ProjectId>)`.
- `Store.pull_requests: Vec<PullRequest>` and `Store.pr_viewer: Option<String>`,
  populated by `apply_snapshot` and by `apply_event(PullRequestsUpdated)` (a
  complete replacement, like `ProviderUsageChanged` in `store.rs:252`).

### 3.4 The cache in the daemon

A new file `crates/daemon/src/pull_requests.rs`, modelled on
`external_agents::Cache`:

```rust
/// How long a complete refresh is reused before asking again.
///
/// Longer than the transcripts' one (10 s) because this is a network call
/// costing ~1.4 s: a PR does not change state several times a minute and
/// reconnecting the GUI must not trigger traffic to GitHub.
const CACHE_TTL: Duration = Duration::from_secs(60);

pub struct Cache {
    fetched_at: Option<Instant>,
    /// A fingerprint of the scanned (host, owner/repo) plus the viewer's login.
    /// Adding a project invalidates instantly, not when the TTL expires:
    /// scanning a different set of repos is a different question, not a stale
    /// answer to the same one.
    fingerprint: u64,
    pull_requests: Vec<PullRequest>,
    viewer: Option<String>,
    failures: Vec<(String, String)>,
}
```

- **Its own mutex in `Daemon`**, never the core lock, for exactly the reason
  documented in `core.rs:110-113` for `external_agents`: IO cannot block the
  daemon's state.
- **Coalescing**: `Inner::pr_refreshing: bool` (or
  `HashSet<Option<ProjectId>>`), sibling of `Inner::fetching` and
  `Inner::pr_opening`. And it is cleared in the error branch of
  `thread::Builder::spawn` too, as `fetch_remote` does (`core.rs:1500`), or the
  flag stays set forever.
- **A dedicated thread** `forge-pr-refresh`, which on finishing calls
  `registry.broadcast_domain(PullRequestsUpdated { .. })`.
- **Targeted invalidation** after a successful `PullRequestOpened`: the PR you
  just opened has to show up without waiting 60 s.

**Repository resolution, no network:** from `git_service::list_remotes(git_root)`
(already there) the default remote's URL is taken and parsed into
`(host, owner, name)`. Formats to cover: `git@github.com:owner/repo.git`,
`https://github.com/owner/repo.git`, `ssh://git@github.com/owner/repo`,
`https://ghe.company.com/owner/repo`. A project whose remote does not parse, or
that has no remote, simply **does not enter the query** and its section is not
drawn.

### 3.5 The GUI

A new file `apps/tauri sibling of `history_panel.rs` and with
the same anatomy (pure functions, state in `AppShell`, `Actions` as a callback):

```
┌─ Pull requests ───────────────── as @kaonashi-dev  ⟳ ─┐
│  [ Mine ] [ This project ] [ All ]                    │  ← scope
│  [ All ] [ Drafts ] [ Ready ]                         │  ← filter
│  🔍 search by title, author, branch or repo           │
├───────────────────────────────────────────────────────┤
│ ▾ hellopay-backend                              (3)   │  ← grouped by project
│   ┌─────────────────────────────────────────────────┐ │
│   │ ⇅ #54  Feat/wompi nequi                    ↗    │ │
│   │   kaonashi-dev · main ← feat/wompi-nequi        │ │
│   │   +1832 −25 · 36 files · 1 d ago         [assigned] │
│   │   ▸ description                                 │ │
│   └─────────────────────────────────────────────────┘ │
```

- **Scope** = the gh-dash/VS Code sections: *Mine* (a relation with the viewer),
  *This project* (the active workspace's, following navigation the same way as
  `HistoryScope`), *All*.
- **Filter** = draft / ready, and the `reviewDecision` as a badge.
- **Free search** over title, author, branch and repo — the same pure function as
  `matches_search` in `history_panel.rs:1033`.
- **Folded card**: number, title, author, `base ← head`, counters, relative age,
  badges (draft, approved, changes requested, assigned, review requested).
  **Unfolded**: the description (`body`, truncated, plain text).
- **Primary action: open in the browser** (`↗`, the `external-link.svg` icon that
  already exists) → `RuntimeCommand::OpenUrl`. Secondary: copy the URL.
- **Honest empty states**, following the `LoadState` precedent in
  `branch_picker.rs:47`: *loading* / *no open PRs* / *`gh` not installed* / *`gh`
  not logged in: run `gh auth login`* / *this project has no GitHub remote* /
  *no access to `owner/repo`*.

New state in `AppShell`: `pr_scope`, `pr_filter`, `pr_search`, `pr_collapsed`,
`pr_expanded`, `pr_load` — exact siblings of the `history_*` ones already in
`app_shell.rs:217-231`.

---

## 4. Blockers (the ones that really bite)

### B1 — It is a network call and the GUI's queue carries the keystrokes *(critical)*

`AGENTS.md` says it literally: *"the GUI drains a command channel on one thread
and that channel also carries `RuntimeCommand::Input`, so a synchronous network
write would freeze typing"*. A `gh api graphql` takes ~1.4 s on average and can
take 60 s if the network goes down.

**Solution:** `RefreshPullRequests` is asynchronous by construction — ack when it
*starts*, the result by event, coalescing. It is copying `fetch_remote`
(`core.rs:1432`) line by line.

### B2 — `gh api graphql` exits with code 1 even when it brings data *(subtle, breaks things)*

Verified on this machine. With one valid alias and one non-existent one:

```
exit = 1
stdout = {"data":{"viewer":{...},"a":{...},"b":null,"rateLimit":{"cost":1,...}},
          "errors":[{"type":"NOT_FOUND","path":["b"],"message":"Could not resolve…"}]}
stderr = gh: Could not resolve to a Repository with the name '…'
```

That is: **there is complete and correct data for every repo but one, and `gh`
exits with 1**. If the daemon treats the exit code as the truth (which is what
`run_gh` does today, `github.rs:96`), a single archived or unauthorised
repository wipes the whole list.

**Solution:** for this path, **always** parse stdout even if the exit is ≠ 0. A
`null` alias + its entry in `errors[].path` is a failure *of that repository* →
it goes into `failures` and that project's section says "no access". It is only a
global failure if `data` does not parse or comes back completely empty.

Other verified codes: an accessible repo with no PRs → `exit 0`, `[]`. An invalid
token → `exit 1`, `HTTP 401: Bad credentials` on stderr and **no** `data`, which
is a global failure indeed.

### B3 — `@me` is `gh`'s *active* account, and here there are two

`gh auth status` on this machine:

```
github.com
  ✓ kaonashi-dev   (Active account: true)
  ✓ samuel-monato  (Active account: false)
```

`assignee:@me` resolves to `kaonashi-dev`. A project living under the other
account will return `NOT_FOUND` for its alias (→ B2, it does not break the rest),
and "Mine" will only show the active account's. If the user runs
`gh auth switch`, the whole list changes with nothing in the UI explaining it.

**Solution:** `viewer { login }` travels in the same query (cost 0) and is
propagated up to the panel's header: **"as @kaonashi-dev"**. It is one line of
text that turns a reportable bug into an observation.

### B4 — Projects with no remote, without GitHub, or on GHES

This very repository **has no remotes** (`git remote -v` is empty). A project may
also point at GitLab, at Bitbucket or at a GitHub Enterprise.

**Solution:** the remote parsing classifies into
`{ GitHub { host, owner, name }, Other, None }`. `Other`/`None` do not enter the
query and their project draws no section — not an error, it simply does not
apply. Different hosts are grouped and each group carries its
`gh api graphql --hostname <host>`.

### B5 — `gh` missing or not logged in

**Solution:** a cheap, cached check on the first refresh (`gh --version`;
`gh auth status` exits ≠ 0 when not logged in), with the result in the panel's
empty state and **the exact command** to run. Never a storm of `DaemonNotice`:
once, in the panel, not in a notification per refresh. It is the same spirit as
`DetectionResult` for the agents.

### B6 — A PR's `body` is big

Measured against `cli/cli`: bodies of 2–3 KB are the norm and there are 20 KB
templates. 50 PRs × 20 KB ≈ 1 MB per event, and the event is **broadcast to every
client** on each refresh. The maximum frame is 16 MiB (ADR-004), so it does not
break — it wastes.

**Solution:** truncate in the daemon with an explicit constant
(`MAX_PR_BODY: usize = 4_000`, the same pattern as `MAX_PATCH_BYTES` in
`change.rs`) and mark `body_truncated`. The panel shows the description as
context, not as a document; whoever wants the whole text has the ↗ button.

### B7 — Opening a URL is running a command with a remote string

`history_panel.rs:901` does `Command::new("open").arg(path)`. Passing a string
that comes from GitHub's API in there without looking at it is the kind of thing
that turns into a CVE: a value starting with `-` is parsed as a flag, and
`file://` would open something local.

**Solution:** `RuntimeCommand::OpenUrl(String)` with validation before the spawn
— an `http`/`https` scheme, a non-empty host, and `--` before the argument. It
goes in the same place as `OpenInEditor` (off the render thread). A `browser.rs`
module or a function in `editors.rs`.

### B8 — The right panel is already taken

See D4. It is not technical, it is a product decision, but it blocks P4.

### B9 — Cost and limits

Measured: `cost: 1`, `remaining: 4993/5000` per hour on GraphQL. With a 60 s TTL
and no automatic polling, a day of intense work is dozens of requests. No risk.
**What would have caused problems** is `gh search prs`: it uses the REST search
API, **30 requests per minute**. Document why it is not used, or someone will
"simplify" it in six months.

### B10 — This feature changes a written invariant

`harness/CHECKPOINTS.md` C5 says:

> `FetchRemote` is still **the only asynchronous request**, with its ack when it
> *starts* and its per-project coalescing (`Inner::fetching`) intact.

That is already false today (`CreatePullRequest` is one too, and `AGENTS.md`
acknowledges it). `RefreshPullRequests` would be the third. The same C4 box says
*"Nothing in `git-service` writes to a remote: `push` is still absent"*, and
`push` has been there for a while.

**Solution:** the plan **includes** updating `AGENTS.md` and `CHECKPOINTS.md` in
the same batch, reformulating the invariant as *"every request that opens a
socket acks when it starts, reports by event and coalesces"*. Otherwise, the
harness' `reviewer` correctly rejects the work over an expired rule.

---

## 5. Open decisions (what has to be confirmed before P1)

### D1 — `gh` or our own token?

- **D1-A `gh` (recommended).** It reuses the existing login, it covers GHES, zero
  secrets in the daemon's config, coherent with `github.rs:1-8`. Cost: an
  external dependency the user must have (mitigated by B5).
- D1-B our own token in the config → tokens in a text file, and an HTTP client,
  pagination and rate-limit handling all have to be written. `ureq` is in the
  workspace but declared *"agents only: OAuth usage endpoints"*.
- D1-C our own OAuth (GitButler) → disproportionate for reading a list.

### D2 — "Mine": only from Forge projects, or from all of GitHub?

- **D2-A only Forge projects (recommended as the default).** Zero cost: they are
  derived from the same data already asked for per project, without `search()`.
  Predictable: what you see in the panel is what Forge knows.
- D2-B all of GitHub, always → it adds two `search()` calls to the same document
  (the same cost 1). Verified: it returns 5 assigned PRs spread over 3 repos, of
  which only some would be Forge projects. It is more useful and more surprising.
- **Proposal: D2-A by default, D2-B behind a setting** `[github]
  include_all_repos = false`. One config line, zero extra cost when it is off,
  and the right answer depends on whether the user wants a *project* panel or a
  panel of *their day*.

### D3 — When does it refresh?

- **D3-A manual + on opening the panel + after `PullRequestOpened`
  (recommended).** It is exactly the policy this repository already chose for
  `fetch` (`auto_fetch_secs = 0`, with its reason written down).
- D3-B polling by default (VS Code) → traffic nobody asked for, battery, and in a
  repo with expired credentials it is a failure that repeats forever.
- **Proposal: D3-A, with `[github] refresh_secs = 0` (0 = never)** for whoever
  wants it, with the same text and the same default as `auto_fetch_secs`.

### D4 — A new panel, or a tab of the right panel?

- **D4-A tabs in the right panel (recommended).** Change `history_open: bool` for
  `right_panel: Option<RightPanel>` with `{ History, PullRequests }`, and a
  two-tab header. One width, one `resizable_panel`, and the current `cmd-j` still
  means "open the right panel". A new shortcut `cmd-shift-p`… better
  `cmd-shift-r` (`cmd-shift-p` is usually the palette in other editors) for the
  PRs tab.
- D4-B a fourth panel → four columns on a 13" laptop.
- D4-C a section inside the sidebar → the PRs of several projects do not fit in a
  240 px column, and the sidebar is "what is alive", not "what is on the remote".
- Regardless: **a badge with the number of open PRs on the project's row** in the
  rail (`count_badge` already exists, `sidebar.rs:787`). That is cheap and it is
  what makes the panel get opened.

### D5 — Is anything persisted in SQLite?

- **D5-A no (recommended).** Just like `ExternalAgentSession` and
  `Workspace::status`: runtime-only, no migration, no out-of-sync remote data on
  disk. Cost: when the app starts the panel is empty for ~1.4 s.
- D5-B a `pull_requests` table → a migration, invalidation, and the real risk of
  showing as open a PR that was merged yesterday.

### D6 — What if a project has several remotes?

The default remote that `git_service::default_remote` already computes is used
(`origin`, otherwise the first one). A fork with `origin` + `upstream` will show
`origin`'s PRs. Enough for the MVP; if the need shows up, it becomes a
per-project setting.

---

## 6. Phased plan

Every phase leaves the tree green (`scripts/dev check`) and is separately
reviewable.

### Phase 1 — `git-service` can read PRs *(no protocol, no GUI)*

**Files:** `crates/git-service/src/github.rs` (grows),
`crates/git-service/src/lib.rs` (re-exports).

1. Generalise `run_gh`: accept an `Option<&Path>` as the cwd and a `hostname`,
   and add `GH_NO_UPDATE_NOTIFIER=1`, `NO_COLOR=1`, `GH_PAGER=cat` to the
   hardening it already has. **Do not duplicate the helper.**
2. `pub fn parse_remote_url(url: &str) -> Option<GitHubRepo>` →
   `{ host, owner, name }`. A pure function.
3. `pub fn list_pull_requests(repos: &[GitHubRepo], viewer_search: bool, timeout)
   -> Result<PullRequestPage, GitError>` — it builds the GraphQL document, runs
   `gh api graphql`, and **parses stdout even if the exit is ≠ 0** (B2).
4. `PullRequestPage { pull_requests, viewer, failures }` — the crate's own
   structs, no `domain` types (the crate's rule: `git-service` returns its results
   and the daemon maps them).

**Tests** (all of them without network, following the
`fetch_rejects_option_like_remote_names` precedent):
- `parse_remote_url` over SSH, HTTPS, `ssh://`, with and without `.git`, GHES,
  and URLs that are not GitHub's.
- Parsing a complete GraphQL answer captured by hand (a JSON fixture).
- Parsing the answer **with partial errors** — the one that exits with 1: the
  good repos arrive, the bad one goes to `failures`.
- Truncating the `body` at `MAX_PR_BODY`.
- A repo with no PRs (`totalCount: 0`) is not an error.

**Acceptance criterion:** `cargo test -p git-service` green, and a documented
manual example (`cargo run -p …` or an `#[ignore]` test) listing real PRs.

### Phase 2 — The domain and the protocol

**Files:** `crates/domain/src/pull_request.rs` (new), `domain/src/lib.rs`,
`protocol/src/{request,event,response}.rs`, `client/src/{ipc,store}.rs`.

1. `domain::PullRequest` + `ReviewDecision` + `PullRequestRelations` (§3.2).
2. `Request::RefreshPullRequests`, `DaemonEvent::PullRequestsUpdated`,
   `pull_requests` in `Response::Snapshot`.
3. `Client::refresh_pull_requests`, `Store.pull_requests`, `Store.pr_viewer`, and
   the arm in `Store::apply_event` (a complete replacement).

**Tests:** MessagePack round-trip of the event and the type;
`apply_event(PullRequestsUpdated)` replaces the whole list; `apply_snapshot`
populates it; `PullRequestRelations::is_mine`.

### Phase 3 — The daemon really refreshes

**Files:** `crates/daemon/src/pull_requests.rs` (new), `daemon/src/core.rs`,
`daemon/src/config.rs`, `docs/config.example.toml`.

1. `pull_requests::Cache` with a TTL, a fingerprint and `invalidate` (§3.4).
2. `Daemon::refresh_pull_requests` — coalescing in `Inner`, a `forge-pr-refresh`
   thread, broadcasting the event, **clearing the flag in the `spawn` error
   branch too**.
3. `Daemon::snapshot` serves `pull_requests` from the cache, **no network**.
4. A successful `PullRequestOpened` → `cache.invalidate()`.
5. Config `[github] { executable = "", refresh_secs = 0, include_all_repos =
   false, timeout_secs = 0 }`. `executable` allows pointing at a specific `gh` —
   and it is what makes the daemon's path testable (see below).

**Tests:** in `crates/daemon/tests/integration.rs`, with `[github] executable`
pointing at a fake `gh` script in a tempdir (the same spirit as the fake agents
in `test-support`):
- A refresh populates the store and the event arrives.
- Two consecutive refreshes → a single subprocess (coalescing).
- A project with no remote or with a non-GitHub remote does not enter the query.
- A `gh` that exits with 1 and partial data → `failures` populated,
  `error: None`.
- A missing `gh` → a readable `error: Some(..)`, and the daemon stays alive.

### Phase 4 — The panel

**Files:** `apps/tauri (new), `ui/src/app_shell.rs`,
`ui/src/runtime.rs`, `ui/src/lib.rs`, `theme tokens/src/actions.rs`,
`theme tokens/src/icons.rs`, `theme tokens/assets/icons/git-pull-request.svg`
(new — careful: there is a test that rasterises the SVGs, a malformed icon breaks
the build).

1. `RuntimeCommand::{RefreshPullRequests, OpenUrl}` and
   `RuntimeUpdate::{PullRequestsRefreshing, PullRequestsRefreshed { error }}` —
   siblings of `FetchStarted`/`FetchFinished`.
2. Refresh on opening the panel and on connecting; a ⟳ button in the header.
3. `pr_panel.rs`: scope, filter, search, grouping by project, foldable cards,
   empty states (§3.5).
4. D4-A: `right_panel: Option<RightPanel>` + tabs + shortcut.
5. An open-PR badge on the project's row in the rail.

**Tests** (pure functions, like the `history_panel`/`sidebar` ones):
- The scope filter: "Mine" only keeps `relations.is_mine()`.
- The search matches by title, author, branch and repo.
- The grouping sorts by `updated_at` descending.
- URL validation rejects `file://`, `-…` and odd schemes (B7).
- The empty states say different things for "no PRs" and "no `gh`".

### Phase 5 — Polish

- Optional automatic refresh (`refresh_secs`), off by default.
- Command palette entries: *Refresh pull requests*, *Open pull requests*.
- `include_all_repos` (D2-B).

### Later (outside this plan)

- **Creating a worktree from a PR** — it fits the worktree-first model perfectly
  (`worktree add <headRefName>` from the card) and is probably the best feature
  that comes out of here, but it is a write action and it deserves its own spec.
- CI status (`statusCheckRollup`) on the card.
- Comments, approving, merging — no; that is rebuilding GitHub.
- GitLab / Bitbucket.

---

## 7. Scope risks

1. **"While we are at it, the diff."** No. The user excluded it explicitly and it
   is 80 % of the work in this class of feature.
2. **"While we are at it, merging from the app."** Writing to a shared remote
   from an app under development. No.
3. **Turning the panel into a notifications dashboard.** Octobox already exists.
4. **Persisting "so it loads fast"** (D5-B) — it shows merged PRs as open and it
   is the hardest bug to believe exists.
5. **The right-panel refactor (D4-A) eating phase 4.** If it does, split it:
   first the panel with its own `bool` the way `history_open` does, and the tabs
   afterwards.

---

## 8. Documentation and invariants to update

It goes **in the same batch**, not afterwards:

- `docs/domain.md` — the `PullRequest` entity, and that it is runtime-only.
- `docs/protocol.md` — `RefreshPullRequests`, `PullRequestsUpdated`, the new
  snapshot field.
- `docs/ui.md` — the panel, the scope, the shortcuts.
- `docs/config.example.toml` — the `[github]` section with the same explanatory
  tone as `[git]`.
- `docs/README.md` — if the detail grows, a `docs/pull-requests.md` page.
- **`AGENTS.md`** — the "asynchronous requests" invariant reformulated (B10), and
  that `git-service` now also *reads* from the host, not only writes.
- **`harness/CHECKPOINTS.md` C4/C5** — the same boxes, which are already expired
  with respect to `push` and `CreatePullRequest`.

---

## 9. How it fits in the harness

`harness/features.json` allows **one active feature at a time**, and today there
is one `pending` (`daemon-stats`). This work does not fit into one: it is five
phases and three boundary crates.

**Proposal:** two consecutive features, each with its spec and its gate:

1. **`github-pr-read`** — phases 1–3. `crates: ["git-service", "domain",
   "protocol", "client", "daemon"]`. Acceptance: the daemon emits
   `PullRequestsUpdated` with real PRs, coalesces, survives a missing `gh` and
   partial errors, and does not touch the network in `GetSnapshot`.
2. **`github-pr-panel`** — phases 4–5. `crates: ["ui", "theme tokens"]`.
   Acceptance: the panel lists, filters, searches and opens in the browser; the
   empty states tell "no PRs" apart from "no `gh`".

Before starting the first one it is worth closing D1–D5 here, because the
`spec-author` must not decide the refresh policy nor where the panel lives.

**A note on the working tree:** right now there are 27 modified files without a
commit. This feature touches `core.rs`, `store.rs`, `app_shell.rs` and
`runtime.rs`, which are among them. It is worth landing what is already there
before starting.
