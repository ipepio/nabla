# pi-runtime — spawn headless de pi + streaming

Estado: en curso, rama `feat/pi-runtime` (apilada sobre feat/nabla-foundation).
Alcance: runtime RPC headless + puente de eventos + chat mínimo. La UI de workspaces/sesiones completa viene después.

## Specs

- **S1** Embebido: «pi tiene que estar dentro de tauri, no tiene que ir a parte y correr en el ordenador» — proceso hijo **headless**, dueño y supervisado por el host de Nabla, muere con la app; nada visible para el usuario. (El empaquetado del binario para distribución es T-002, decisión aparte.)
- **S2** RPC-first: spawn de `settings.piBinary` con `--mode rpc`, protocolo NDJSON sobre stdin/stdout.
- **S3** Un proceso de pi **por sesión** (topología Stark validada en referencias).
- **S4** Ciclo de vida: start/stop/restart, máquina de arranque `spawning → ready`, crash detection con **un solo reintento y solo ante crash, nunca ante timeout** (patrón FaqFirebase), `process_exit` sintético con cola de stderr.
- **S5** Kill cross-platform (macOS/Linux/Windows) — corrige el defecto Unix-only de Stark (`process_manager.rs:409`).
- **S6** Streaming: eventos de pi → contratos tipados → eventos Tauri → Vue; el texto del asistente se ve crecer en el chat.
- **S7** Índice de sesiones: al crear una sesión de pi se registra con `upsert_session` (activa el dead-code de nabla-foundation); contenido jamás se guarda.
- **S8** Tests sin red ni pi real: codec con fixtures; ProcessManager con un **script fake-pi** que emite NDJSON conocido.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| R1 | rpc/mod.rs: codec NDJSON + structs, tests con fixtures (RED→GREEN, 15 tests) | S2 | done | (unidad runtime) |
| R2 | process/mod.rs: ProcessManager (spawn headless, máquina de vida, retry-once-on-crash, kill por grupo Unix + taskkill/CREATE_NO_WINDOW Windows, env allowlist), 7 tests con fake-pi | S1, S3-S5, S8 | done | (unidad runtime) |
| R3 | Comandos runtime + puente agent-event + upsert_session activado | S1, S6, S7 | done | (unidad runtime) |
| R4 | agent-events.ts + wrappers de comandos + ChatPanel.vue con streaming (espeja Rust verbatim) | S6, S4(costura nabla-foundation) | done | (unidad runtime) |
| R5 | Verificación independiente: 36/36 cargo, clippy 0, vue-tsc+vite, contratos 0 mismatches (incl. null-vs-omitido), seams limpias | — | done | — |

## Log

- **L1** Usuario: «¡Vamos!» tras el diseño del spawn headless; «Supongo que después ya iremos con la interfaz, workspaces etc.» — la UI completa queda para después.
- **L2** Referencias de ejecución: FaqFirebase `pi-rpc-manager.ts` (ciclo de vida, frame decoder) y `ipc-contracts.ts:461-614` (eventos); Stark `process_manager.rs` (topología por sesión) y `rpc-agent-adapter.ts` (proyección al frontend). Clones en /tmp/nabla-pi-desktops.vYixxw.
- **L3** Desacoplamiento de contratos: R1-R3 (Rust) fijan el contrato en código; R4 espeja lo ya escrito — nunca a la inversa.

- **L4** Verificación independiente (worker verify): 36/36 cargo tests, clippy 0 warnings, cargo build, vue-tsc+vite build; contraste de contratos Rust↔TS campo a campo sin mismatches (11 variantes de evento, incluida la sutileza `stderrTail: []` serializado aunque sea None); seam guard limpio (frontend no importa APIs de Tauri fuera de src/contracts); S1-S8 PASS con evidencia; sin dependencias de red. Verificador aclaró una desviación de setup (fix de tar en su scratch /tmp; cero escrituras en el workspace).