# forge-ui perf report

Generated from `forge-ui` (release, Rust 1.99.1-aarch64-apple-darwin, Rune `938ba14f09bb`) on macos/aarch64.

| Fixture | p50 / p95 / max ms |
|---|---:|
| Session switch (click → painted frame) | 0.396 / 0.673 / 36.157 |
| View build | 0.056 / 0.108 / 1.832 |
| Terminal echo (key → grid → frame) | 0.350 / 0.667 / 0.984 |
| Socket ops / on the UI thread | 0 / 0 |

## Integrated terminal stream (125 Hz, 128×49, rich full scroll (every row rebuilt every frame))

| Clock | p50 / p95 / max ms |
|---|---:|
| Delta apply (`SharedTerminal`) | 0.003 / 0.006 / 0.013 |
| Rune update (poll+view+layout+paint) | 0.563 / 1.115 / 6.930 |
| Terminal build (diff+encode+upload) | 0.487 / 1.034 / 42.658 |
| Terminal encode (pass+draw) | 0.134 / 0.217 / 1.808 |
| Frame CPU (update+build+encode) | 1.200 / 2.275 / 43.871 |
| Host frame (external, update→submit) | 1.247 / 2.328 / 43.899 |
| Total CPU/frame (+ delta apply) | 1.203 / 2.279 / 43.874 |

300 deltas, 0 shell view commits, 14700 rows uploaded, 38019 instances/draw, 0 glyph misses, 22 build allocations (0 in cell encoding); GPU Apple M2 (Metal) (init 20.8 ms).

## Editor (88 mounted rows, 6 foldable, 125 Hz)

| Clock | p50 / p95 / max ms |
|---|---:|
| Mount (first frame) | 31.061 ms; view 7.766 ms |
| Caret-only view build | 0.081 / 0.126 / 0.324 |
| Caret-only frame CPU | 0.565 / 0.872 / 1.705 |
| Key-to-paint CPU proxy | 0.625 / 0.652 / 0.733 |

Rows rebuilt: mount 88, caret-only 0, key-to-paint 0. Display/vsync is **not measured**; key-to-paint is the named CPU proxy.

## Startup / RSS

- Cold (first launch after build): 540.0 ms to first drawn frame; RSS spawn / 0 / 1 / 2 s 0.47 / 118.30 / 109.16 / 108.67 MiB
- Warm median (two launches): 186.0 ms; RSS spawn / 0 / 1 / 2 s 0.42 / 122.32 / 116.60 / 106.25 MiB

## Measured vs proxy

- Fixture switch, echo, terminal delta apply, terminal build/encode and the editor view are wall-clock CPU measurements.
- Terminal build/encode are the compositor's CPU clocks; GPU time and presentation are not included.
- Editor key-to-paint is a CPU proxy (key handler → one 125 Hz frame); it is not a display measurement.
- Startup windowed runs measure launch → the first `FrameEncoded` reported as `Drawn`; cold means first after the build, not an OS page-cache purge.
