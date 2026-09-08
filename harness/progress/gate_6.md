# Human gate — spec approval

**Feature:** 6 (test)
**Title:** Test

**Question:** Do you approve this spec for implementation?

**Context:**
- Requirements: 3 (R1 gate canónico, R2 niveles de prueba, R3 alcance sin implementación)
- Crates: (not declared — the spec touches no crate)
- Design note: se descarta añadir un test Rust nuevo para demostrar que el
  harness funciona; eso convertiría una petición ambigua en una modificación de
  producto y duplicaría cobertura ya existente. Ver design.md.

**Caveat:** la petición original fue literalmente `Test`, sin comportamiento de
producto. El spec-author acotó la feature a documentar y verificar el contrato
de pruebas ya existente bajo `harness/`, sin tocar crates, `Cargo.toml`, CI ni
runtime. Aprobar significa aceptar ese alcance, no un cambio en Forge.

**Options:** `approve` | `revise` | `block`

**Spec:** `harness/specs/6-test/`

---
*Resolve with `/feature-go 6` (approve) or ask for changes before implementing.*
