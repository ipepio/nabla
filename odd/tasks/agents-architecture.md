# agents-architecture — AGENTS.md + docs/ según dev-templates

Estado: en curso, rama `feat/pi-runtime` (misma rama de trabajo; commits de docs separados).
Alcance: implementar la arquitectura de documentación para agentes de github.com/ipepio/dev-templates, adaptada a Nabla (Tauri+Vue+Rust, NO NestJS).

## Specs

- **S1** «no tenemos ni un solo agents.md» — crear `AGENTS.md` raíz como contrato de trabajo para agentes en este repo.
- **S2** «implementa esta arquitectura https://github.com/ipepio/dev-templates para en un futuro tener organizados bien los plugins, integraciones etc.» — adaptar la estructura (AGENTS.md raíz, docs/ai/* del template frontend, práctica de ADRs del backend, CONTENTS.md como índice); **adaptar, no copiar crudo** (el template backend es NestJS y no aplica).
- **S3** ADRs: llevar las decisiones de `.docs/02-decisiones.md` (D-001..D-011) a `docs/adr/` versionados con el template adr del repo de referencia.
- **S4** Límites: `.docs/` sigue local y gitignored (D-004); lo nuevo en `docs/` es versionado; no tocar `odd/` (allí gobierna el orchestrator) ni `src/`/`src-tauri/`.
- **S5** Preparar la organización futura de plugins/integraciones: sección/ruta documentada donde vivirán las capacidades (mencionar `docs/ai/architecture.md` como mapa), sin crear carpetas de código que aún no tienen contenido.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| A1 | Root `AGENTS.md` + `CONTENTS.md` (índice) | S1, S2 | done | (unidad docs) |
| A2 | `docs/ai/*` (7 docs adaptados, anclados en código real) | S2, S5 | done | (unidad docs) |
| A3 | 8 ADRs + README + 3 templates, cada ADR cita sus D-IDs/S# | S3 | done | (unidad docs) |
| A4 | Revisión de consistencia (verify worker + spot-check orchestrator): rutas y comandos reales, 9 archivos en docs/adr/, sin links a .docs/ | — | done | — |

## Log

- **L1** Usuario: P.S. «He visto que no tenemos ni un solo agents.md, implementa esta arquitectura https://github.com/ipepio/dev-templates para en un futuro tener organizados bien los plugins, integraciones etc. please».
- **L2** El repo de referencia tiene dos templates (backend NestJS / frontend); su propio README manda a copiar y adaptar según el proyecto destino.
- **L3** Nabla es app de escritorio Tauri: la base es el template frontend (docs/ai/*) + práctica de ADRs; la conexión con el ODD en curso (odd/tasks/) se documenta en el workflow.

- **L4** Verificación: checks 1-3 del verify worker PASS (36/36, builds, contratos) + spot-check del orchestrator para A4: superficies dentro de lo autorizado, seam limpio, 9 archivos en docs/adr/ (README + 8 ADRs), ninguna referencia tipo link a `.docs/` en AGENTS.md/CONTENTS.md/docs/**.