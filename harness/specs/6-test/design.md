# Design — feature 6 (test)

## Decisión

Esta es una feature de validación del harness, no una modificación de Forge.
Los artefactos viven exclusivamente bajo `harness/`; no se toca ningún crate,
`Cargo.toml`, script de producción ni CI.

La evidencia de R1 es la definición de `scripts/dev check` y el gate completo
de `./init.sh`. La evidencia de R2 es la estructura existente en
`crates/test-support`, `crates/terminal-core/tests`,
`crates/daemon/tests` y `crates/git-service/tests`. R3 se verifica con el
estado del árbol y la lista de archivos modificados.

## Archivos

- `harness/features.json`: registro y estado de la feature.
- `harness/progress/current.md`: bitácora de la sesión.
- `harness/progress/explore_6_test.md`: investigación localizada.
- `harness/specs/6-test/requirements.md`: requirements y trazabilidad.
- `harness/specs/6-test/design.md`: esta decisión y sus límites.
- `harness/specs/6-test/tasks.md`: tareas verificables.

No hay firmas nuevas, migraciones SQLite ni cambios de protocolo.

## Alternativa descartada

Se descarta añadir un test Rust nuevo para demostrar que el harness funciona:
eso convertiría una petición ambigua en una modificación de producto y
duplicaría cobertura ya existente. La alternativa elegida prueba el contrato
mediante los gates y la inspección de precedentes ya presentes.

## Invariantes

Como no se modifica runtime, se conservan las fronteras de `AGENTS.md`: el
daemon sigue siendo dueño del único motor VT, las pruebas de integración siguen
usando recursos reales donde el repositorio lo exige y no se introducen
migraciones ni dependencias.
