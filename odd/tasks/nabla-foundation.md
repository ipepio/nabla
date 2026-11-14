# nabla-foundation — shell Tauri v2 + Vue/Vuetify + store

Estado: en curso, rama `feat/nabla-foundation` (scaffold baseline en `main`).
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
| T1 | Doc ODD + mirror Engram | — | done | (este commit) |
| T2 | Toolchain node/cargo | — | done | — (rustup 1.100.0 instalado con autorización) |
| T3 | Scaffold Tauri v2 + Vue + Vuetify | S1 | done | `99badca` (main) |
| T4 | Store SQLite: migraciones + workspaces/sessions (RED→GREEN, 8 tests) | S3 | done | (ver commit de esta unidad) |
| T5 | settings.json: load/save atómico, defaults, camelCase (4 tests) | S2 | done | (ver commit de esta unidad) |
| T6 | Comandos Tauri + contratos TS tipados | S4 | pending | |
| T7 | UI mínima: panel workspaces + settings | S1, S4 | pending | |
| T8 | Verificación final: cargo test + clippy + vite build | — | pending | |

## Log

- **L1** Usuario autoriza el alcance tras el diseño: «¿Largamos con el shell + store?» → «dale».
- **L2** Rust no estaba instalado; usuario elige «Instalá rustup (oficial)» (rustup 1.100.0, target aarch64-apple-darwin).
- **L3** settings.json adopta camelCase (`piBinary`) — convención del ecosistema JS/Tauri; decidió el test `unknown_fields_are_tolerated_not_fatal`.
- **L4** Corrección honesta: el mirror de Engram registró este doc como creado antes de que existiera. El doc se crea ahora; el mirror se corrige en la misma pasada.
- **L5** CSP activa desde el día uno (`style-src 'self' 'unsafe-inline'` para Vuetify/Vue) — decisión propia, corrije la bandera roja observada en StarkInternationalAI (tauri.conf.json con csp null).
