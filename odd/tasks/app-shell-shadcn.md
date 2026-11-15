# app-shell — esqueleto UI con shadcn-vue (drawer + chat central)

Estado: en curso, rama `feat/pi-runtime`.
Alcance: migración Vuetify→shadcn-vue + esqueleto de tres zonas. Lógica de runtime/contratos NO cambia.

## Specs

- **S1** Layout del usuario: «un drawer fijo en lateral izquierdo... ahí tener el logo, un botón para colapsarla. Una estructura central que ocuparía todo lo restante (aunque podamos meter un drawer a la derecha que después lo vemos) y esa estructura central que sea el chat».
- **S2** Componentes: «recuerda utilizar todo elementos de shadcn-vue» — migración completa; Vuetify se elimina (sin coexistencia).
- **S3** El drawer izquierdo aloja los workspaces (lista + añadir) — responde a «no se pueden añadir aún workspaces»; los workspaces activos son siempre visibles.
- **S4** Drawer derecho: solo costura documentada (estructura lo soporta; no se construye).
- **S5** La lógica de chat (contratos, listen, deltas, tool chips) se conserva; solo cambia la presentación.
- **S6** Settings accesible desde el footer del drawer (dialog): theme + piBinary.
- **S7** Logo del usuario: «ahí tienes 3 logos de la aplicación por si quieres utilizarlos en lugar de lo que hay» — usar el nabla metálico en el drawer; fallback al glifo ∇ si falta el asset.
- **S8** «el botón de colapsar que esté solo en el drawer» — quitar el SidebarTrigger del header central.
- **S9** Workspace por defecto y explorador: «tener un workspace por default que sea ~ y/o un explorador para seleccionar una carpeta» — sembrar `~` en el primer arranque si no hay workspaces (setup de Rust, idempotente) y añadir selector nativo de carpetas (tauri-plugin-dialog) como flujo primario, input manual como secundario.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| U1 | Migración de stack: Tailwind v4 + shadcn-vue (13 componentes base), remove Vuetify/@mdi/font | S2 | done | 9f70dc7 |
| U2 | Esqueleto: Sidebar (∇, collapse, workspaces con add, footer settings dialog) + ChatPanel con shadcn + costura drawer derecho | S1, S3-S6 | done | 9f70dc7 |
| U3 | Docs: component-architecture.md (convenciones shadcn-vue), AGENTS.md stack, ADR 0007 nota de CSP | — | done | 9f70dc7 |
| U4 | Verificación: build, cero imports vuetify, smoke de usuario | — | done | 9f70dc7 |
| U5 | public/brand/ (README de assets esperados), header con imagen + fallback ∇, trigger solo en drawer | S7, S8 | done | (unidad fixes) |
| U6 | seed_default_workspace (3 tests: vacío→siembra, poblado→no-op, idempotente) + tauri-plugin-dialog 2.8.4 (Rust+npm+capabilities) con botón FolderOpen como flujo primario | S9 | done | (unidad fixes) |

## Log

- **L1** Usuario tras la fundación: «no se pueden añadir aún workspaces, así que quizá podríamos empezar a crear el esqueleto, como lo ves? ... drawer fijo en lateral izquierdo (recuerda utilizar todo elementos de shadcn-vue) ... estructura central que sea el chat».
- **L2** Decisión del orchestrator: migración completa (no coexistencia) — doble librería = antipatrón; barato pre-release.
- **L3** El smoke e2e con pi real (streaming) queda cubierto por el mismo smoke de esta feature: el chat se verifica sobre el esqueleto nuevo.
- **L4** El worker detectó y reportó la contradicción del brief (se pedían docs dentro de superficies que las prohibían) y respetó la superficie: U3 lo ejecutó el orchestrator inline. Motivo del reporte parcial del worker.
- **L5** Verificación orchestrator: npm run build limpio, cero referencias a vuetify, 13 componentes ui instalados. Advisories informativas (at-rules de Tailwind v4; knip stale).
- **L6** Usuario aprueba el inicio («vale, es un inicio») y pide: default workspace `~` y/o explorador de carpetas, usar sus 3 logos, colapsar solo en el drawer («De momento fixea esto»). Los PNG llegaron en el chat pero el JSONL de la sesión aún no está volcado a disco; se usa fallback ∇ + ruta pública esperada (`/brand/nabla-logo.png`) para que funcione apenas el usuario suelte los archivos, sin cambios de código.

- **L7** Verificación orchestrator de la unidad fixes: 39/39 cargo (3 seed tests observados green), clippy 0, npm build OK, SidebarTrigger solo en AppSidebar.vue. Advisories: expects en tests (normal) y el del run() del template; knip stale sobre AppSidebar (sí está importado). Los PNG del usuario aún no están en el repo: la UI usa fallback ∇ hasta que se suelten en public/brand/ con los nombres del README.