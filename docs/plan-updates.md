# Plan — In-app updates

> **Phases 1 and 2 are built.** §0 records the decisions taken at the human
> gate; §11 says what each phase covers and which are still open (3 and 4).

One release ritual, one notification, one click. A running Forge Node learns a
new version exists, downloads it in the background, and swaps itself in without
the user losing a session.

## 0. Decisions

| # | Question | Decision |
| --- | --- | --- |
| D1 | Where releases live | GitHub Releases on `kaonashi-dev/forge-node`, which is `origin`. The `origin.cursor.com` remote is kept as `cursor` |
| D2 | How far "no restart" goes | The **GUI** relaunches; the **daemon** does not. Sessions, PTYs and scrollback survive |
| D3 | First platform | macOS arm64. Universal and Linux are later, same pipeline |
| D4 | What signs an update | minisign, through `tauri signer`. The private key is a CI secret; the public key ships in `tauri.conf.json` |
| D5 | Who bundles | `scripts/package-macos`, unchanged. It grows one `--updater` flag that tars and signs what it already built |
| D6 | Who checks | The Rust host, on its runtime thread. The WebView stays passive and is told through an event, like `shell:menu` |
| D7 | Where the notification lives | The status bar, right side, as a pill. Never a modal, never a blocking dialog |
| D8 | When it checks | 30 s after launch, every 6 h, on window focus if 6 h stale, and on demand from the app menu |
| D9 | How a protocol-breaking release is handled | The manifest says so, the app classifies before downloading, and asks for confirmation because sessions will die |

## 1. What exists today

- The version is written in three places and none of them agree by
  construction: `[workspace.package] version` in `Cargo.toml`,
  `"version"` in `apps/tauri/src-tauri/tauri.conf.json`, and `"version"` in
  `apps/tauri/package.json`. All three read `0.1.0` today by coincidence.
- The bundle is hand-rolled. `scripts/package-macos` writes its own
  `Info.plist` and lays out `Contents/MacOS/Forge` beside
  `Contents/MacOS/forge-daemon`, because `daemon::locator` finds the runtime as
  a **sibling of its own executable**. `tauri build` does not produce that
  layout, so it is not the bundler here and this plan does not make it one.
- `CFBundleIdentifier` is `dev.forgenode.forge` (from `FORGE_BUNDLE_ID`) while
  `tauri.conf.json` `identifier` is `dev.forgenode.forge.tauri`. Two identities
  for one app; §4 picks one.
- Distribution is `scripts/dist build` → a tarball → `scp`. There are no git
  tags, no GitHub Releases, and no signed artifacts of any kind.
- Nothing in the tree mentions an updater.

## 2. Why a relaunch is nearly free here

This is the load-bearing observation, and it is a property of the existing
architecture rather than something this plan adds:

- The daemon is spawned **detached** (`connect_or_spawn`, `Stdio::null()` on
  all three descriptors) and is not a child the GUI reaps. It outlives the GUI.
- The GUI is a passive replica. It reconnects to the same socket, gets a
  snapshot, and rebuilds. `client::Client` is the only state that dies.

So relaunching the GUI costs a WebView repaint. The PTYs keep running, the
agent keeps writing, the scrollback is intact, and the user is looking at the
same session a second later. That is the "imperceptible" the user asked for,
and it needs no frontend hot-swap to get there.

**The exception is the protocol.** `PROTOCOL_VERSION` requires exact equality
(`crates/protocol/src/hello.rs`), and `docs/architecture.md` is blunt about the
consequence: *a PTY never survives the daemon*. A release that bumps the
protocol cannot reuse the running daemon, so it must stop it, and every live
session becomes `Orphaned` — restartable, but with no scrollback.

## 3. The two kinds of update

**Soft** — `PROTOCOL_VERSION` unchanged. Download, swap the bundle, relaunch
the GUI. The new GUI connects to the old daemon and everything is where the
user left it. One click, no confirmation, no warning.

**Hard** — `PROTOCOL_VERSION` bumped. The new GUI cannot talk to the old
daemon. The app must stop the daemon, let `connect_or_spawn` start the new one,
and the user loses live terminals. This one asks first, in plain words, and
names how many sessions are running.

The app has to know which it is **before it downloads**, not after it
relaunches into a rejected handshake. `tauri-plugin-updater` surfaces the
manifest's `notes` field as `update.body` and nothing else that we control, so
the machine-readable bit rides in the first line of `notes`:

```
protocol: 7
<the human release notes follow from here>
```

The host parses that line, strips it before showing the notes, and compares it
against `protocol::PROTOCOL_VERSION`. A missing or unparseable line is treated
as **hard** — the conservative direction, since the cost of a wrong guess is a
silently dead session.

Stopping the daemon needs a mechanism the daemon does not have today. It has no
`stop` subcommand and `lockfile.rs` is explicit that the PID in the lock file
is informative and the `flock` is the authority, so signalling that PID is a
guess. The clean answer is a `Request::Shutdown` in `protocol` — which itself
bumps `PROTOCOL_VERSION`, so the first release carrying it is itself a hard
update. That is fine and it is a one-time cost.

## 4. One version, one identity

- Delete `"version"` from `tauri.conf.json`. Tauri falls back to the crate's
  version, which is `version.workspace = true`, which is `Cargo.toml`. One
  source, no drift, nothing to remember on release day.
- `apps/tauri/package.json` is `"private": true` and never published; its
  version is dead weight. Leave it, or drop it — it does not reach the bundle.
- Make the two bundle identifiers one. `FORGE_BUNDLE_ID` in
  `scripts/package-macos` becomes `dev.forgenode.forge.tauri`, matching
  `tauri.conf.json`. Doing this after a signed release ships would be a
  migration; doing it now costs nothing.
- `scripts/dist` already reads the workspace version with `version_of_workspace()`
  and needs no change.

## 5. The release artifact

`scripts/package-macos` gains `--updater`. After it has built and (optionally)
signed `dist/macos/Forge Node.app`, it produces:

```
dist/release/ForgeNode-<ver>-darwin-aarch64.app.tar.gz
dist/release/ForgeNode-<ver>-darwin-aarch64.app.tar.gz.sig
```

The tarball is a gzipped tar whose single top-level entry is `Forge Node.app`,
built with `COPYFILE_DISABLE=1` like the existing `scripts/dist` archive so no
AppleDouble files ride along. The `.sig` comes from:

```
pnpm tauri signer sign --private-key-path "$key" "$tarball"
```

with `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` in the environment.

Two constraints on the swap that are easy to get wrong:

- **Signing must be consistent across releases.** If a release is signed and
  notarized, its replacement must be too — Gatekeeper kills an unsigned bundle
  that replaces a signed one. Unsigned → unsigned is fine for internal builds.
- **The first install is not the update.** Without `FORGE_CODESIGN_IDENTITY`
  the bundle is only ad-hoc, linker-signed, so a browser download carries
  `com.apple.quarantine` and Gatekeeper refuses to open it until
  `xattr -dr com.apple.quarantine "Forge Node.app"` clears it. That is a
  first-install cost only: the updater fetches over its own HTTP client, which
  sets no quarantine attribute, so a self-applied update never hits it. A
  Developer ID identity removes the step for good.
- **The app must be able to write its own bundle.** `/Applications` installed
  by the current user is fine; a root-owned install is not, and the updater
  will fail with a permission error the UI has to show rather than swallow.

Keys are generated once, by hand, and never committed:

```
pnpm tauri signer generate -w ~/.tauri/forge-updater.key
```

The public half goes into `tauri.conf.json` `plugins.updater.pubkey`. The
private half goes into the mirror's GitHub Actions secrets as
`TAURI_SIGNING_PRIVATE_KEY` / `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. Losing the
private key means no existing install can ever be updated again; it is backed
up outside the repo.

## 6. The manifest

One `latest.json` per release, attached to the release, reachable at a stable
URL that always points at the newest one:

```
https://github.com/kaonashi-dev/forge-node/releases/latest/download/latest.json
```

```json
{
  "version": "0.2.0",
  "notes": "protocol: 7\nDrag-reorderable session tabs, safer agent profiles.",
  "pub_date": "2026-09-07T12:00:00Z",
  "platforms": {
    "darwin-aarch64": {
      "signature": "<contents of the .sig file>",
      "url": "https://github.com/kaonashi-dev/forge-node/releases/download/v0.2.0/ForgeNode-0.2.0-darwin-aarch64.app.tar.gz"
    }
  }
}
```

`version` is compared against the running app's version by the plugin; nothing
in our code does semver arithmetic. Adding `darwin-x86_64`, `darwin-universal`
or `linux-x86_64` later is another key in `platforms` and another matrix entry
in CI — no app change.

## 7. The app side

`tauri-plugin-updater` and `tauri-plugin-process` are added to the workspace
dependencies and registered in `lib.rs` beside `tauri_plugin_dialog::init()`.
The capability file grants `updater:default`; the relaunch goes through the
Rust `app.restart()`, so no `process` permission reaches the WebView.

**The check runs on the host, not in the WebView.** The frontend does not do IO
and does not own a policy; it renders what it is told. The host owns a small
`updates` module beside `runtime/`:

- `check()` — calls the plugin, parses the `protocol:` line, classifies
  soft/hard, and emits `shell:update` with the resulting state.
- `download()` — `update.download()` with a progress callback that emits
  `shell:update` at most a few times a second. The download is a network call
  and must not run on the command channel that also carries
  `RuntimeCommand::Input`; it gets its own thread, the same rule the
  `FetchRemote` / `CreatePullRequest` workers already follow.
- `install()` — `update.install(bytes)` then, for a soft update,
  `app.restart()`. For a hard update, `Request::Shutdown` to the daemon first,
  then restart; `connect_or_spawn` starts the new daemon from the new bundle.

Three `#[tauri::command]`s (`check_for_update`, `download_update`,
`install_update`) let the UI drive it. The state travels as an event, which is
how the shell already learns about menu clicks.

The state the frontend stores, in a new `updateStore`:

```
idle | checking | available{version, notes, kind} | downloading{percent}
     | ready{version, kind} | installing | failed{message}
```

## 8. What the user sees

**The pill.** Right side of the status bar, beside the usage meters. Hidden in
`idle` and `checking` — a check that finds nothing is not news. In `available`
it is a small button: `Update to 0.2.0`. Clicking it downloads; the pill
becomes a progress state. When the download finishes it becomes
`Restart to apply` and stays there, patient, until the user is ready. Nothing
about this ever steals focus or blocks work.

**The notes.** Clicking the version in the pill opens a popover with the
release notes — the same `notes` string with the `protocol:` line stripped.

**The hard-update confirmation.** Only for a protocol bump, and only at the
moment of applying, not at the moment of noticing:

> This update also replaces the runtime. Your 3 running sessions will stop and
> their terminals will be empty when they restart. Update now, or keep working
> and update later?

**The menu.** `Check for Updates…` in the `Forge Node` submenu, between
`About Forge Node` and `Settings…`. It goes through the same `shell:menu`
emitter that is already there and forces a check regardless of the 6 h clock,
reporting "you're up to date" when there is nothing — the one case where a
negative result is worth saying, because the user asked.

## 9. CI

A new `.github/workflows/release.yml` on the mirror, triggered by a tag push
matching `v*`. `ci.yml` is untouched.

1. `macos-14` runner (arm64), checkout, `rustup show`, pnpm install.
2. Assert the tag matches the workspace version — a mismatch fails the build
   rather than shipping a mislabelled release.
3. `scripts/dev check` — the same gate as everywhere else. A release does not
   get a weaker bar than a PR.
4. `scripts/package-macos --updater`, with the signing and notarization
   variables from secrets when present.
5. Generate `latest.json`: version and `pub_date` from the tag, `signature`
   read from the `.sig` file, `notes` assembled from `PROTOCOL_VERSION` plus
   the tag's annotation message.
6. `gh release create` with the tarball, the `.sig` and `latest.json`.

`origin` is GitHub, so a release is `git push origin v0.2.0` and nothing else.
The tag's annotation message becomes the release notes, under the `protocol:`
line `scripts/release-manifest` puts there.

## 10. Cutting a release

```
# 1. bump the single source
$EDITOR Cargo.toml                    # [workspace.package] version = "0.2.0"
cargo check --workspace               # refresh Cargo.lock
git commit -am "Release 0.2.0"

# 2. tag and push to the mirror
git tag -a v0.2.0 -m "Drag-reorderable session tabs, safer agent profiles."
git push origin main v0.2.0
```

CI does the rest. Within a few minutes every running Forge Node that checks in
sees the pill.

**Never delete a tag that already has a release.** GitHub demotes the release
to a draft when its tag disappears, a draft is not `latest`, and
`releases/latest/download/latest.json` silently falls back to the previous
version — so every installed app stops being offered the update and nothing
anywhere reports an error. Re-pushing the tag does not undo it; the release has
to be published again (`gh release edit <tag> --draft=false --latest`). To move
a release, cut the next version instead.

## 11. Phases

**Phase 1 — the plumbing, no UI.** Single-source the version (§4), unify the
bundle identifier, add `--updater` to `scripts/package-macos`, generate the
signing keys, add `release.yml`, cut `v0.1.1` by hand and confirm the release
assets are shaped right. Nothing ships to the app yet.

**Phase 2 — the soft path.** Plugins registered, `updates` module, the three
commands, the `shell:update` event, `updateStore`, the status-bar pill, the
menu item. Only soft updates: a hard update is detected and refuses with
"update from the installer for this one" rather than doing something clever.
This is the phase that delivers the whole user-visible promise.

**Phase 3 — the hard path.** `Request::Shutdown` in `protocol` (bumping
`PROTOCOL_VERSION`, which makes its own release the first hard one),
the daemon-restart sequence, the confirmation dialog with a live session count.

**Phase 4 — reach.** `darwin-universal` and `linux-x86_64` in the matrix and in
`platforms`. AppImage for Linux, since the updater can replace one in place and
cannot replace a tarball.

## 12. What this deliberately does not do

- **No frontend hot-swap.** Serving the Solid bundle from a writable directory
  would remove the relaunch entirely, but the generated TypeScript types are
  compiled against a specific host binary: a swapped-in bundle could invoke a
  handler the running binary does not have. The relaunch costs a second and
  keeps one contract instead of two.
- **No delta updates.** The whole bundle every time. It is tens of megabytes on
  a developer's machine, not a phone.
- **No rollback and no staged rollout.** A bad release is fixed by cutting a
  better one.
- **No silent auto-install.** Downloading is one click and applying is another.
  An editor that restarts itself under a running agent is not a good neighbour.
