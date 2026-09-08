# Investigación — feature 6 (test)

## Alcance recibido

`spec_raw` es exactamente `Test`. No describe una capacidad de producto ni un
caso de uso concreto, así que la spec trata esta entrada como una prueba del
flujo del harness y documenta el contrato de pruebas ya existente. No se
propone modificar código de aplicación.

## Gate vigente

- `scripts/dev check` ejecuta, en orden, `cargo fmt --all -- --check`,
  `cargo clippy --workspace --all-targets -- -D warnings` y
  `cargo test --workspace`.
- `./init.sh --fast` valida entorno, archivos base y coherencia de
  `harness/features.json`, pero omite deliberadamente Cargo.
- `./init.sh` completo delega en el gate de Cargo; es el gate que cuenta para
  cerrar una feature.

## Precedentes de pruebas

- `crates/test-support/src/fake_pty.rs` ofrece `FakePtyBackend`, que reproduce
  bytes, captura escritura y permite controlar la salida sin procesos reales.
- `crates/terminal-core/tests/engine.rs` conduce el motor directamente para
  daño, snapshots, deltas, resize, scrollback y eventos del terminal.
- `crates/daemon/tests/integration.rs` arranca un daemon en socket temporal,
  usa una base SQLite en memoria y ejercita clientes, PTYs, I/O, resize,
  reconexión y backpressure.
- `crates/git-service/tests/integration.rs` cubre operaciones Git con
  repositorios temporales; `crates/daemon/tests/scenario_*.rs` separa los
  escenarios de daemon por dominio.

## Fronteras relevantes

- El daemon es dueño del PTY y del único motor VT; `client` y `ui` no deben
  incorporar una segunda emulación para probar.
- Las pruebas de integración dependen de `git`, un shell/PTY real y un
  toolchain C para SQLite bundled; no se debe sustituir esa cobertura por
  Docker.
- No hay crate de producción identificado por `Test`; `crates[]` queda vacío.
