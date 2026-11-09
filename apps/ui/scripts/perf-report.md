# forge-ui perf report

Generated from `forge-ui` (release, Rust 1.99.1-aarch64-apple-darwin, Rune `938ba14f09bb`) on macos/aarch64.

| Fixture | p50 / p95 / max ms |
|---|---:|
| Session switch (click → painted frame) | 0.550 / 0.601 / 0.665 |
| View build | 0.112 / 0.124 / 0.180 |
| Terminal echo (key → grid → frame) | 0.524 / 0.608 / 0.643 |
| Socket ops / on the UI thread | 0 / 0 |

## Integrated terminal stream (125 Hz, 128×49, rich full scroll (every row rebuilt every frame))

| Clock | p50 / p95 / max ms |
|---|---:|
| Delta apply (`SharedTerminal`) | 0.003 / 0.006 / 0.016 |
| Rune update (poll+view+layout+paint) | 0.912 / 1.546 / 6.267 |
| Terminal build (diff+encode+upload) | 0.510 / 0.912 / 5.750 |
| Terminal encode (pass+draw) | 0.137 / 0.239 / 2.086 |
| Frame CPU (update+build+encode) | 1.632 / 2.743 / 7.087 |
| Host frame (external, update→submit) | 1.665 / 2.791 / 7.158 |
| Total CPU/frame (+ delta apply) | 1.636 / 2.747 / 7.092 |

300 deltas, 0 shell view commits, 14700 rows uploaded, 38002 instances/draw, 0 glyph misses, 20 build allocations (0 in cell encoding); GPU Apple M2 (Metal) (init 23.0 ms).

## Editor (88 mounted rows, 6 foldable, 125 Hz)

| Clock | p50 / p95 / max ms |
|---|---:|
| Mount (first frame) | 15.650 ms; view 0.337 ms |
| Caret-only view build | 0.135 / 0.193 / 0.399 |
| Caret-only frame CPU | 0.755 / 1.018 / 2.300 |
| Key-to-paint CPU proxy | 0.826 / 0.870 / 1.030 |

Rows rebuilt: mount 88, caret-only 0, key-to-paint 0. Display/vsync is **not measured**; key-to-paint is the named CPU proxy.

## Startup / RSS

- Cold (first launch after build): 247.0 ms to first drawn frame; RSS spawn / 0 / 1 / 2 s 1.75 / 115.88 / 116.36 / 116.36 MiB
- Warm median (two launches): 181.5 ms; RSS spawn / 0 / 1 / 2 s 0.19 / 116.44 / 117.44 / 117.52 MiB

## Tauri baseline

- Command: forge-tauri-latency with an isolated forge-daemon (prebuilt binaries) (canonical: `make latency-tauri`)
- Binary: `/Users/kaonashi/dev/forge-node/target/debug/forge-tauri-latency` against `/Users/kaonashi/dev/forge-node/target/debug/forge-daemon`
- key-to-delta p50 / p95 / p99: **9.1 / 9.9 / 10.0 ms** over 120 samples, 0 lost
- Frame on the wire: mean 572 B, max 1623 B
- isolated daemon (protocol-matched), same machine

## Measured vs proxy

- Fixture switch, echo, terminal delta apply, terminal build/encode and the editor view are wall-clock CPU measurements.
- Terminal build/encode are the compositor's CPU clocks; GPU time and presentation are not included.
- Editor key-to-paint is a CPU proxy (key handler → one 125 Hz frame); it is not a display measurement.
- Startup windowed runs measure launch → the first `FrameEncoded` reported as `Drawn`; cold means first after the build, not an OS page-cache purge.
