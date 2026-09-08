# Requirements — feature 6 (test)

La especificación recibida fue `Test`. En ausencia de un comportamiento de
producto concreto, estos requirements fijan únicamente la documentación y
verificación del contrato de pruebas existente.

## R1 — Gate canónico

El sistema DEBE documentar que el gate completo del workspace ejecuta
`cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D
warnings` y `cargo test --workspace` en ese orden.

## R2 — Niveles de prueba

El sistema DEBE distinguir la cobertura determinista basada en fixtures de la
cobertura de integración que usa PTYs, Git, SQLite y el daemon real.

## R3 — Alcance sin implementación

El sistema DEBE mantener esta feature sin cambios en crates de aplicación ni
dependencias nuevas.

## Trazabilidad

| Acceptance | Requirement |
|---|---|
| La feature documenta el gate de pruebas vigente del workspace y sus comandos exactos | R1 |
| La feature distingue las pruebas unitarias deterministas de las pruebas de integración con PTY, Git y daemon | R2 |
| La feature no requiere cambios en código de aplicación ni añade dependencias | R3 |
