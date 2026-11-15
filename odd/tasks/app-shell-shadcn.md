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

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| U1 | Migración de stack: Tailwind v4 + shadcn-vue (13 componentes base), remove Vuetify/@mdi/font | S2 | done | (unidad shell) |
| U2 | AppSidebar (∇, collapse, workspaces list+add, settings dialog) + SidebarInset con ChatPanel shadcn + costura drawer derecho | S1, S3-S6 | done | (unidad shell) |
| U3 | Docs actualizados (component-architecture shadcn-vue, AGENTS.md stack + regla 6, ADR 0007 unsafe-inline) | — | done | (unidad shell) |
| U4 | Verificación: build, cero imports vuetify, smoke de usuario | — | pending | |

## Log

- **L1** Usuario tras la fundación: «no se pueden añadir aún workspaces, así que quizá podríamos empezar a crear el esqueleto, como lo ves? ... drawer fijo en lateral izquierdo (recuerda utilizar todo elementos de shadcn-vue) ... estructura central que sea el chat».
- **L2** Decisión del orchestrator: migración completa (no coexistencia) — doble librería = antipatrón; barato pre-release.
- **L3** El smoke e2e con pi real (streaming) queda cubierto por el mismo smoke de esta feature: el chat se verifica sobre el esqueleto nuevo.

- **L4** El worker detectó y reportó la contradicción del brief (se pedían docs dentro de superficies que las prohibían) y respetó la superficie: U3 lo ejecutó el orchestrator inline. Motivo del reporte parcial del worker.
- **L5** Verificación orchestrator: npm run build limpio (vue-tsc+vite), cero referencias a vuetify en src/ y package.json, 13 componentes ui instalados, Advisories solo informativas (at-rules de Tailwind v4 desconocidas para el linter CSS; knip stale sobre AppSidebar que sí está importado).
- **L6** Re-ejecutar `npm run build` tras las ediciones de U3 antes del commit (docs no afectan al build; verificación final en la unidad).