# nabla-foundation — shell Tauri v2 + Vue/Vuetify + store

Estado: completado en rama `feat/nabla-foundation` (baseline en `main`).
Alcance autorizado por el usuario: shell + store; el spawn de pi queda para la siguiente feature.

## Specs

- **S1** Shell de la app: «empezar con el shell Tauri v2 + vue (utilicemos vuetify) después un rust que haga spawn de pi» — este feature cubre solo la primera mitad; el spawn es tarea futura.
- **S2** Persistencia combinada: «combinaremos, ficheros, sqlite y engramn en un futuro para la memoria de los agenets» — ficheros para config, SQLite para estado de la app, memoria de agentes fuera del alcance (ver S5).
- **S3** Store = metadatos propios: workspaces + índice de sesiones. Las sesiones de pi (JSONL) son de pi: solo se indexan (`pi_session_path`), nunca se copian transcripciones. Regla: «la DB es una proyección reconstruible; si entra en conflicto con los archivos de pi, ganan los archivos».
- **S4** Costura: «Vue jamás habla con pi directo — todo pasa por comandos/eventos de Rust y por el módulo de contratos». Los comandos Tauri consumen el store; los contratos TS espejan los payload.
- **S5** Memoria de agentes (futura, NO en este feature): «en lugar de integrar engram como tal» — entra como capacidad con **store propio** (regla: cada dueño, su propio almacenamiento; Engram es la referencia del patrón).

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| T1 | Doc ODD + mirror Engram | — | done | 2ff6848 |
| T2 | Toolchain node/cargo | — | done | — (rustup 1.100.0 instalado con autorización; clippy añadido en fase T8) |
| T3 | Scaffold Tauri v2 + Vue + Vuetify | S1 | done | 99badca (main) |
| T4 | Store SQLite: migraciones + workspaces/sessions (RED→GREEN, 8 tests) | S3 | done | 2ff6848 |
| T5 | settings.json: load/save atómico, defaults, camelCase (4 tests) | S2 | done | 2ff6848 |
| T6 | Comandos Tauri + contratos TS tipados (delegado a worker) | S4 | done | (ver commit de esta unidad) |
| T7 | UI mínima: panel workspaces + settings (delegado a worker) | S1, S4 | done | (ver commit de esta unidad) |
| T8 | Verificación independiente (worker verify) + higiene git + clippy | — | done | a052b57 + (esta unidad) |

## Log

- **L1** Usuario autoriza el alcance tras el diseño: «¿Largamos con el shell + store?» → «dale».
- **L2** Rust no estaba instalado; usuario elige «Instalá rustup (oficial)» (rustup 1.100.0, target aarch64-apple-darwin).
- **L3** settings.json adopta camelCase (`piBinary`) — convención del ecosistema JS/Tauri; decidió el test `unknown_fields_are_tolerated_not_fatal`.
- **L4** Corrección honesta: el mirror de Engram registró este doc como creado antes de que existiera. El doc se creó tras T5; el mirror se corrigió.
- **L5** CSP activa desde el día uno (`style-src 'self' 'unsafe-inline'` para Vuetify/Vue) — decisión propia, corrige la bandera roja observada en StarkInternationalAI.
- **L6** T6/T7 ejecutados en paralelo por dos workers con superficies disjuntas (src-tauri/src/** vs src/**) y verificación independiente (worker verify): cargo test 12/12 PASS, cargo build PASS, npm run build PASS, contraste de contratos Rust↔TS sin mismatches (6 comandos exactos), S3/S4/S2 PASS.
- **L7** Frontera de slice: el commit 2ff6848 superó las ~400 líneas porque `src-tauri/target/` quedó comiteado (faltaba `src-tauri/.gitignore` del template). Higiene corregida en a052b57 (untrack + ignore); los commits siguientes miden líneas de autoría reales.
- **L8** Clippy instalado (completa el toolchain autorizado en L2). 2 warnings dead-code (`SessionNew`, `upsert_session`) anotados con `#[allow(dead_code)]` + comentario: se usarán en la feature de runtime pi.
- **L9** Verificación de ventana pendiente de usuario: `npm run tauri dev` (el verificador no abre GUI).
