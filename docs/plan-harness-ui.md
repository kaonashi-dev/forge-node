# Plan — Harness en la UI

**Estado:** implementado (2026-08-28)
**Depende de:** `docs/harness.md` (ciclo en disco), `docs/ui.md` (§16.8 PR compose como precedente)
**Alcance:** cómo el usuario arranca una feature desde Forge, ve el progreso del harness y accede a los agentes hijos.

---

## 1. Diagnóstico: hoy no hay harness en la ventana

El harness vive **fuera** de la GUI:

| Capa | Qué hace hoy | Dónde |
|------|----------------|-------|
| Estado | `features.json`, specs, progress, event log | `harness/` en el repo |
| Orquestación | Skills `/feature`, `/feature-go` | Cursor / Claude Code |
| CLI humana | `scripts/harness status\|resume\|timeline…` | Terminal del desarrollador |

La UI de Forge **no** lee `harness/`, **no** muestra el ciclo spec → implement → review, y **no** usa el grafo de sesiones (`parent_session_id`, `SessionRole`) que el dominio y el daemon ya soportan.

### Lo que sí existe y debemos reutilizar

| Patrón | Módulo | Relevancia |
|--------|--------|------------|
| Prompt + lanzar agente | `pr_compose.rs` | Entrada de tarea, botón Launch, `initial_prompt`, seguimiento de `SessionId` |
| Resolver con AI | `git_panel.rs` | Prompt prefabricado → `NewAgent` |
| Catálogo de lanzables | `launchables.rs` + `session_menu.rs` | Un solo lugar para “empezar X” |
| Pestaña de vista | `ViewTab` en `app_shell.rs` | Feature tab al lado de Diff / PR compose |
| Panel derecho | `history_panel.rs`, `pr_panel.rs` | Lista filtrable con refresh |
| Grafo de sesiones | `domain::Session`, `CreateChildSession` | Árbol padre/hijo en el rail |
| Roles | `SessionRole::{Orchestrator, Researcher, Executor, Reviewer}` | Etiquetas en tabs y sidebar |

### Anti-patrón a evitar

**No** construir un chat genérico que duplique el harness en memoria. El harness ya decidió que el estado vive en disco (12-factor: unificar ejecución y negocio). La UI debe **reflejar** `features.json` + `events_<id>.jsonl` + artefactos, no inventar un segundo state machine.

---

## 2. Flujo objetivo

```
Usuario escribe tarea (prompt inicial)
        │
        ▼
┌───────────────────────────────────────────────────────────────┐
│  Feature tab (vista principal)                                 │
│  • spec_raw = texto del usuario (verbatim)                     │
│  • timeline = events_<id>.jsonl                                │
│  • gate / approve cuando status = spec_ready                    │
└───────────────────────────────────────────────────────────────┘
        │
        ▼
Orchestrator (sesión raíz, role=Orchestrator)
        │  initial_prompt = instrucción /feature + spec_raw
        ├──► Explore ×N (child, Researcher) — terminales opcionales
        ├──► spec-author (child, Planner) — o Task externo si no hay PTY
        ├──► ══ human gate ══ (botón Approve en UI = /feature-go)
        ├──► implementer (child, Executor, worktree)
        └──► reviewer (child, Reviewer)
        │
        ▼
Diff en working tree; commit opcional (/feature-commit)
```

### Dos superficies complementarias (no “o chat o nada”)

1. **Feature tab (estructurado)** — para el *qué* y el *dónde* del ciclo: estado, timeline, gates, enlaces a specs y reviews. Es la “conversación” del harness: el event log, no burbujas de chat.

2. **Terminales hijas (vivo)** — para el *cómo* se ejecuta cada subagente: cada hijo es un PTY como hoy. El usuario hace clic en un nodo del árbol y ve el stream del agente (Claude, Cursor, etc.).

El input inicial **no** es un chat continuo con el orquestador: es el `spec_raw` que se congela en `features.json`. Los mensajes posteriores son eventos (`human_gate_resolved`, `review_verdict`) o texto en la terminal del hijo activo.

---

## 3. Superficies UX concretas

### 3.1 Entrada — “New Feature…”

**Dónde:** menú `+` del tab strip (`session_menu.rs`), command palette (grupo Create), checkout `⋯` (opcional).

**Comportamiento:**

1. Abre una **Feature tab** vacía con un `InputState` multilínea (mismo control que PR compose / diálogos).
2. Usuario escribe la tarea → **Start**:
   - Registra feature en `harness/features.json` (`pending`, `gate_attempts: 0`).
   - Append `feature_registered` en `events_<id>.jsonl`.
   - Lanza **orquestador**: `CreateAgentSession` con `role: Orchestrator`, `initial_prompt` que incluye el skill `/feature` y el `spec_raw` verbatim.
   - Guarda `feature_id` ↔ `root_session_id` en estado de la tab (runtime-only en UI, no columna SQLite).

**Alternativa:** `from-issue` ya existe en CLI; la UI puede ofrecer “From GitHub issue…” que llama al mismo camino vía daemon.

### 3.2 Feature tab (`feature_view.rs` — nuevo)

Hermano de `pr_compose.rs`. Estados:

| `FeatureStage` | UI |
|----------------|-----|
| `Draft` | Caja de texto + Start |
| `Running` | Timeline + árbol de hijos + acciones contextuales |
| `Gate` (`spec_ready`) | Resumen spec + **Approve** / **Revise** / **Block** |
| `Done` / `Blocked` | Veredicto + enlaces a impl/review |

**Contenido del panel central:**

- **Timeline** — filas desde `events_<id>.jsonl` (mismo formato que `scripts/harness timeline`).
- **Spec preview** — primeras líneas de `requirements.md` + enlace “open in editor”.
- **Gate card** — contenido de `gate_<id>.md` cuando aplique.
- **Acciones:**
  - `Approve spec` → dispara `/feature-go` (nuevo prompt al orquestador o `HarnessAdvance { feature_id, action: ApproveSpec }`).
  - `Open orchestrator` / `Open implementer` → foco en tab de sesión.
  - `Show diff` → tab Diff del mismo checkout.

**No** mostrar el diff completo dentro de la feature tab (misma regla que compose).

### 3.3 Panel derecho “Features” (opcional fase 2)

Pestaña en el panel derecho, patrón `pr_panel.rs`:

- Lista todas las features del repo (`scripts/harness list` o `ListHarnessFeatures`).
- Filtro por status, búsqueda por slug.
- Clic → abre Feature tab.

Útil cuando hay varias features `done` y una `spec_ready` esperando.

### 3.4 Sidebar — árbol de sesiones bajo el checkout

Hoy las sesiones son **planas** bajo cada worktree. Cambio:

```
▾ feature/auth  primary
   ⌁ Orchestrator · feature #2     ← root
      ├ Explore ipc        (Researcher)
      ├ Spec author        (Planner) 
      ├ Implementer        (Executor)  ← activo
      └ Reviewer           (Reviewer)  ← pendiente
   ◌ $ zsh
```

**Reglas de render:**

- Agrupar por `root_session_id` cuando `parent_session_id` no es `None`.
- Icono/glyph por `SessionRole` (extender `session_glyph` en `session_tabs.rs`).
- Badge de harness status en el root si `session_id` está ligado a una feature.
- **NEEDS YOU** sigue funcionando: un hijo que hace bell aparece arriba como hoy.

### 3.5 ¿Y el “chat”?

| Necesidad | Solución en Forge |
|-----------|-------------------|
| “Quiero decir qué hacer” | Input inicial en Feature tab → `spec_raw` |
| “¿En qué va el proceso?” | Timeline de eventos + status en tab |
| “Quiero hablar con el agente que implementa” | Terminal del hijo Executor |
| “Aprobar la spec” | Botón en gate card, no mensaje en chat |
| Historial legible | `events_<id>.jsonl` + artefactos markdown |

Un chat dedicado solo tendría sentido para el **orquestador** si el lead no corre en un PTY externo. Fase 3 opcional: panel de mensajes *solo* para `SessionRole::Orchestrator`, alimentado por OSC o por un futuro `SessionTranscript` — fuera del alcance inicial.

---

## 4. Lectura del harness desde la UI (protocolo)

ADR-012 prohíbe `std::fs` sobre el **checkout** del workspace; `harness/` es metadata del **repositorio** (como `.git`), no del worktree aislado.

**Opción recomendada:** comandos síncronos en el daemon, igual que `GetWorkspaceDiff` / `ListFiles`:

```rust
// domain — runtime-only, sin columna
pub struct HarnessFeature { /* espejo de features.json entry */ }
pub struct HarnessEvent { /* una línea del jsonl */ }

// protocol::Request
ListHarnessFeatures,           // lee harness/features.json del project root
GetHarnessFeature { id },
GetHarnessTimeline { id },     // parsea events_<id>.jsonl
ReadHarnessArtifact { id, kind: Spec|Gate|Context|Impl|Review, file },

// Mutaciones (orquestador o UI con confirmación humana)
RegisterHarnessFeature { spec_raw, title, crates, acceptance },
HarnessEvent { id, event_type, data },
SetHarnessFeatureStatus { id, status },  // solo transiciones válidas
```

**Resolución de ruta:** `project.root_path` (checkout primary) → `harness/features.json`. Si no existe, el panel Features muestra “Harness not initialized” con enlace a docs.

**Ejecución:** fuera del core lock, como `fs-service` / `git-service`. Sin broadcast: la UI hace poll o refresh explícito tras mutaciones (igual que diff).

**Alternativa mínima (solo dogfooding):** `RuntimeCommand::HarnessCli(Vec<String>)` que ejecuta `scripts/harness` en el project root. Más rápido de prototipar, peor para producto (requiere Bun en PATH, parsing de texto).

---

## 5. Lanzamiento de hijos desde el orquestador

El daemon ya expone `CreateChildSession`. La UI hoy siempre manda `parent: None`.

**Wire en cliente:**

```rust
client.create_child_session(
    parent_session_id,
    workspace_id,
    provider_id,
    profile_id,
    role,           // Researcher | Executor | Reviewer | …
    initial_prompt,
)?;
```

**Quién llama:**

| Fase | Quién crea el hijo |
|------|---------------------|
| MVP | El **orquestador** (PTY) invoca herramientas / el humano lanza Tasks en Cursor — la UI solo **muestra** hijos si el daemon los crea |
| Fase B | La UI, al recibir `HarnessAdvance(StartImplement)`, crea hijo Executor con prompt de `context_<id>.md` |
| Fase C | Daemon `HarnessRunner` que traduce eventos a `CreateChildSession` sin depender del PTY del lead |

Para forge-node dogfooding, Fase A+B bastan: el skill `/feature-go` ya describe el protocolo; la UI dispara el advance y abre la terminal del hijo.

**Mapeo rol harness → `SessionRole`:**

| Harness | `SessionRole` |
|---------|---------------|
| Lead (orquestador) | `Orchestrator` |
| Explore | `Researcher` |
| spec-author | `Planner` |
| implementer | `Executor` |
| reviewer | `Reviewer` |

---

## 6. Atención humana (factor 7 en UI)

| Evento harness | UI |
|----------------|-----|
| `human_gate_opened` | Feature tab en modo Gate + notificación suave en status bar |
| `spec_ready` | Badge en tab “Feature #2 — approve?” |
| `review_verdict: CHANGES_REQUESTED` | Timeline + re-lanzar implementer |
| `feature_blocked` | NEEDS YOU en root orchestrator + mensaje en `current.md` |
| Agente hijo hace bell | NEEDS YOU existente en ese hijo |

---

## 7. Fases de implementación

### Fase 0 — Documentación y contrato (este archivo)

- [x] Plan UI
- [x] Entrada en `docs/ui.md` § Harness
- [x] Tipos en `domain` + requests en `protocol`

### Fase 1 — Leer harness en la app (sin orquestar)

- [x] `ListHarnessFeatures` / `GetHarnessTimeline` en daemon
- [x] Panel derecho **Features** o sección en History
- [x] `scripts/harness`-free path en producción

### Fase 2 — Feature tab + entrada de tarea

- [x] `feature_view.rs`, `ViewTab::Feature`
- [x] “New Feature…” en `+`
- [x] Register + timeline en vivo (poll cada N s o refresh on focus)
- [x] Gate card con Approve → escribe `human_gate_resolved` + status

### Fase 3 — Grafo de sesiones + lanzar orquestador

- [x] `CreateAgentSession { role: Orchestrator, initial_prompt }`
- [x] Sidebar agrupa por `root_session_id`
- [x] Role badges en tabs
- [x] Ligadura `feature_id` ↔ `session_id` en estado de tab

### Fase 4 — Hijos y advance automático

- [x] `create_child_session` en cliente
- [x] `HarnessAdvance` para implementer/reviewer
- [x] Worktree isolation visible (“implementing in worktree …”)

### Fase 5 — Pulido

- [x] `from-issue` en UI
- [x] Committer vía botón “Commit feature…” (`/feature-commit`)
- [x] Gate status en `scripts/dev check` / icono en status bar

---

## 8. Qué no hacer

- **No** duplicar `features.json` en SQLite.
- **No** un chat como fuente de verdad del spec.
- **No** meter `terminal-core` en la UI para “preview” del orquestador.
- **No** auto-commit desde la UI sin `committer` + gate verde.
- **No** lanzar implementer sin gate humano (`require_human_spec_approval`).

---

## 9. Decisión

**Sí** al flujo: input inicial → feature tab + timeline → gates en UI → terminales hijas visibles en el árbol.

**No** a un chat monolítico; el “hilo” es `events_<id>.jsonl` + markdown en `harness/progress/`.

**Siguiente paso de código:** cerrado — fases 0–5 + embed inline de agentes.

## 8. Agent embed (Feature tab)

Cada agente del harness puede mostrarse en tres modos:

| Modo | Acción UI | PTY attach |
|------|-----------|------------|
| Colapsado | fila en Agents | no |
| Preview inline | **Preview** / panel bajo la lista | sí (`AttachHarnessPreview`, 100×12) |
| Expandido | **Expand** | sí (32 filas en la misma pestaña) |
| Tab completo | **Tab** | `SelectSession` (terminal principal) |

**Settings → Harness** elige qué agente (y perfil) lanza cada rol: orquestador, implementer y reviewer. Los valores viven en `app_state` (`ui.harness.*`); un perfil con `--model` es la forma de fijar el modelo.

Al arrancar un feature, el orquestador se lanza con `preview_only` y aparece en preview sin robar la pestaña de terminal activa. Un solo preview a la vez (coste en `docs/performance.md`).

Ver también: `docs/harness.md`, `docs/plan-subagent-harness.md` §1.1, `apps/tauri/src/workbench/PrComposeView.tsx`.
