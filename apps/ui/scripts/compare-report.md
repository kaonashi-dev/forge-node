# Tauri vs Rune

**Run:** final, quiet machine, rune-ui 7c629d5

Apple M2, 8 GiB, macOS 26.6.2 (25G83). Commit `7c629d5c2bb2`; daemon protocol 37 (`dist/macos/Forge Node.app/Contents/MacOS/forge-daemon`), load { 2.38 4.27 7.86 } at report time.

Tauri bundle 0.1.9 built 2026-11-09T16:33:05-0500 (0 relevant commits since); Rune bundle 0.1.9 built 2026-11-09T16:33:29-0500 (0 since).

| Metric | Tauri | Rune | Δ (Rune − Tauri) |
|---|---:|---:|---:|
| Bundle on disk (MB) | 54.2 | 70.7 | +16.5 (+30%) |
|   GUI binary (+launcher) (MB) | 18 | 40.4 | +22.4 (+124%) |
|   Shared daemon/editor/forgectl (MB) | 35 | 29 | -6.0 (-17%) |
| Startup cold, spawn→window (ms) | 1496 | 1788 | +292.0 (+20%) |
| Startup warm median of 5 (ms) | 229.3 | 200.6 | -28.7 (-13%) |
| Footprint at 10 s (MiB) | 160 | 122.1 | -37.9 (-24%) |
| Footprint at 60 s (MiB) | 145.8 | 121.2 | -24.6 (-17%) |
| Footprint after streaming (MiB) | 221.4 | 142.7 | -78.7 (-36%) |
| Idle CPU, 60 s avg (% of a core) | 0.15 | 0.3 | +0.1 (+100%) |
| Streaming CPU, app set, 30 s (% of a core) | 37.77 | 3.2 | -34.6 (-92%) |
| Streaming CPU, daemon (% of a core) | 16.85 | 15.73 | -1.1 (-7%) |

PTY rows×cols while attached: Tauri 42 138, Rune 35 103. Lines printed in the stream: Tauri 1.232e+06, Rune 1.2346e+06. Idle daemon CPU: Tauri 0.07%, Rune 0.06%.

**Unequal grids:** the two apps left the shared PTY at different sizes (42 138 vs 35 103 rows×cols), so the streaming rows compare different cell counts per frame.

Processes counted (name:pid:bytes at 60 s):

- Tauri: com.apple.WebKit.WebContent:70407:89883440;Forge:70089:32490432;com.apple.WebKit.GPU:70405:18482016;com.apple.WebKit.Networking:70406:6586920;com.apple.audio.SandboxHelper:70438:5472808
- Rune: forge-ui:77395:75760648;MTLCompilerService:77400:27902456;com.apple.SafariPlatformSupport.Helper:77442:15090216;MTLCompilerService:77416:8323432

## Key-to-screen: not measured identically

- Tauri: p50 9.4 / p95 9.7 ms — forge-tauri-latency: key written to the daemon socket -> the TerminalDelta carrying it back (no WebView, no paint)
- Rune: p50 0.255 / p95 0.31 ms — forge-ui --fixture --bench: recorded fixture, key -> replica -> one painted frame on a headless GPU (no daemon, no display)

These are different routes and are not a Δ.

## Method

- Both bundles run from copies in a temp directory with distinct bundle ids; the GUI executable is spawned directly (posix_spawn with responsibility disclaimed, as a Finder launch would) with FORGE_SOCKET/FORGE_HOME (Rune: FORGE_RUNE_HOME) naming one isolated daemon.
- One daemon, seeded with one project (a one-commit temp git repo) and one shell session; both apps attach to that same session.
- Startup: posix_spawn -> first on-screen layer-0 window >= 200x200 owned by the app pid (CGWindowListCopyWindowInfo, polled every 2 ms). This is window-on-screen, not content painted: Tauri maps its window before the WebView paints, and screen capture is not available to measure pixels.
- Cold is the first launch of a freshly copied bundle; the OS page cache is not purged, so it is first-launch-after-install, not a cold boot.
- Memory: phys_footprint from footprint(1), summed over the app pid plus every process whose responsible pid is the app (WebKit WebContent/Networking/GPU helpers for Tauri), excluding the daemon.
- Idle CPU: user+system time (proc_pid_rusage) of that set over 60 s starting 10 s after launch, divided by wall time; 100% = one core. The Tauri host schedules an update check 30 s after launch, inside this window.
- Streaming CPU: /bin/sh printing SGR-colored lines as fast as it can for 30 s in the attached session (SendShellCommand); app set and daemon process measured separately. The daemon caps emission at 125 deltas/s per attached terminal, so the app load is frame-bound.
- PTY size is read with `stty size` in the session after attaching; it reflects each app's window, so the grids may differ.
- Key-to-screen is not measured identically (no screen capture permission, no shared key-injection path); the two numbers under latency_different_methods are each client's own harness and are not comparable.
