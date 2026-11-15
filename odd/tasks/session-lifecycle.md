# session-lifecycle

Estado: en curso, rama `feat/pi-runtime`.
Alcance: sesiones múltiples vivas por workspace, apertura perezosa al seleccionar y transcripción al recuperar.

## Specs

- **S1** «Tendríamos que vincular cada sesión del worpsace a una id» — la clave de runtime es el id de la fila (`sessions.id`), no el workspace. Un proceso de pi por sesión, identificado por ese id (el runtime ya usa `session_id` como clave; esta unidad lo hace explícito de punta a punta).
- **S2** «cuando se abra nabla, abrir esas sesiones (ojo, no quiero abrirlras todas de golpe» — al arrancar **no** se lanza ningún proceso: la lista sale del índice SQLite. Nada de spawn masivo.
- **S3** «si está cerrada al seleccionar una sesión existente, que abra esa y al menos que la muestre» — seleccionar una sesión sin proceso vivo la abre (`pi --session <jsonl existente>`) y **muestra su transcripción**.
- **S4** «al ir a otra sesión, no cerrarla, que siga corriendo» — cambiar de sesión **no** detiene el proceso anterior; sigue vivo y acumulando eventos.
- **S5** «si ya está abierta, pdoer recuperarla cuando se selecione» — si ya hay proceso para ese id, se reutiliza: nunca se duplica.

## Contrato (fijado por el orchestrator; Rust lo implementa, TS lo espeja)

```rust
// Idempotente: si ya hay proceso vivo para ese id, devuelve su estado sin spawnear.
open_session(session_id: String) -> SessionDto

// Transcripción leída del JSONL que pi posee (nunca copiada a la DB).
// Defensivo: líneas desconocidas se ignoran; se devuelven los últimos N.
session_transcript(session_id: String) -> Vec<TranscriptMessage>

struct TranscriptMessage { role: String, text: String, at: Option<String> }  // role: "user" | "assistant" | "tool"
live_session_ids() -> Vec<String>   // ids con proceso vivo (para pintar el estado en la lista)
```

- Eventos: el envelope `agent-event` ya lleva `sessionId`; el frontend enruta por ese id.
- `start_session` sigue creando una sesión nueva (fila nueva + proceso).

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| L1 | Rust: `open_session` idempotente (registry + fallback de carrera) + `live_session_ids` + `Store::session_by_id` + `ProcessManager::live_ids`, tests con fake-pi | S1, S3, S5 | done | (unidad sesiones) |
| L2 | Rust: `session_transcript` (parseo defensivo del JSONL, tope 400, archivo ausente → vacío) + tests con fixture | S3 | done | (unidad sesiones) |
| L3 | TS: `session-store` con `Map` de buffers por sesión + una sola suscripción que enruta por `sessionId`; ChatPanel pasa a renderizar el buffer activo | S4, S5 | done | (unidad sesiones) |
| L4 | TS: lista con punto de sesión viva (`live_session_ids`, refresco 5s) y `selectSession` que abre on-demand y siembra transcripción | S2, S3 | done | (unidad sesiones) |
| L5 | Verificación: 58/58 cargo, clippy, build; contratos alineados; `stopSession` sólo en el botón Stop | — | done | (esta unidad) |

## Notas de diseño

- El `ProcessManager` ya indexa por sesión y `stop_and_wait` es explícito; el bug actual está en el frontend: `ChatPanel` reinicia su estado por sesión y el store limpia la sesión activa. El backend no cierra procesos al cambiar de vista, así que S4 es sobre todo frontend.
- Supuesto a validar en el smoke: `pi --session <archivo existente>` reanuda esa sesión en vez de crear otra. Si no lo hiciera, la alternativa es un flag de reanudación explícito; queda anotado.
- Formato del JSONL: parseo defensivo (rol y contenido de `message`, ignorar el resto) para no acoplarnos a cada evento interno de pi; el fixture de test se construye con las formas conocidas.

## Log

- **L1** Usuario: «Estaba en una, cree una nueva y la vieja se perdió, están las dos en la barra pero al cambiar de sesión se pierde. … Plantealo en ODD y orquesta los subagentes».
- **L2** Requisitos explícitos: ids por sesión; sin apertura masiva al arrancar; apertura al seleccionar; no cerrar al cambiar; reutilizar si ya está abierta.
- **L3** Reparto: un worker Rust (L1+L2) y un worker TS (L3+L4) con superficies disjuntas (`src-tauri/src/**` vs `src/**`) y este contrato copiado verbatim en ambos briefs.

- **L4** Implementado el contrato completo (open_session / session_transcript / live_session_ids) con 9 tests nuevos en Rust; el frontend quedó con un buffer por sesión y una única suscripción, de modo que cambiar de sesión es sólo cambiar `activeSessionId`.
- **L5** Verificación orchestrator: `npm run build` OK; 58/58 tests; comandos registrados; `stopSession` aparece únicamente en `handleStop` (acción explícita del usuario), nunca en el cambio de sesión.
