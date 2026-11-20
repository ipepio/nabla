# turn-continuity — un turno cortado se explica y se puede continuar

Estado: en curso, rama `feat/turn-continuity` (apilada sobre `feat/auto-open-pane`).
Alcance: (1) parar de verdad por el camino de pi en vez de matar el proceso, (2) decir por qué terminó un turno cuando no terminó bien, (3) ofrecer **Continuar** en ese turno.

## Specs

- **S1** «me parece bien el orden que dices de puente, auto-open y continuidad» + «diría que podemos continuar» — esta feature. Un turno que no termina por sí solo debe **verse** como lo que es: cortado por longitud, por error, o porque el humano lo paró.
- **S2** Parar deja de matar: `stop_session` manda **`abort`** a pi, espera brevemente a que el turno se asiente (`turn_end` con `stopReason: "aborted"` + `agent_settled {aborted:true}`) y solo si no se asienta recurre al kill actual. La sesión sobrevive al stop, así que el arreglo de «re-abrir al enviar» queda como red de seguridad, no como camino normal.
- **S3** El motivo viaja tipado hasta la UI: `stopReason` (y `errorMessage` cuando lo hay) en el contrato. `stop` y `toolUse` son un turno normal y **no** se anuncian; `aborted`, `length` y `error` sí.
- **S4** Aviso por turno, dentro del grupo del turno: `Te detuviste` / `Se cortó por longitud` / `Se cortó por un error: <mensaje>`. Sobrio, sin alarmismo, y con el botón **Continuar**.
- **S5** **Continuar** envía un prompt de continuación a pi en esa sesión (`continue` con contexto de que el turno anterior no terminó) y aparece como un turno nuevo, con el agrupado que ya existe. No re-ejecuta el prompt anterior: continuar no es repetir.

## Hechos verificados (protocolo pi)

- Motivo en el **mensaje del asistente**: `stopReason: "pending" | "stop" | "length" | "toolUse" | "error" | "aborted" | "deferred"`, con `errorMessage?` y `rawStopReason?` (docs/message-types.md:134).
- `turn_end` = `{message, toolResults}` (el motivo va dentro de `message`); `agent_end` = `{messages, willRetry}`; `agent_settled` = `{aborted}` con `aborted: true` cuando el run terminó abortado (docs/json.md).
- Abort: método `abort` en RPC → `session.abort()`; el bucle emite `turn_end` (con `stopReason:"aborted"`, `toolResults:[]`), luego `agent_end` (`willRetry` forzado a false) y `agent_settled {aborted:true}`.
- **Nabla nunca manda `abort`**: solo aparece en un fixture de test (rpc/mod.rs:454). `stop_and_wait` cierra stdin y mata el árbol (process/mod.rs:736-759) — por eso el transcript queda con un registro del asistente vacío.
- No existe límite de pasos por turno ni comando nativo de continuar/reanudar.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| C1 | Rust: `StopReason` tipado en el codec (desde `message.stopReason`, con `errorMessage`), proyectado al evento `turn_end` del bridge; testeado con fixtures que hoy se descartan. | S3 | bajo | pending | |
| C2 | Rust: `stop_session` manda `abort` y espera el asentamiento (acotado, ~2 s) antes de matar; si el child no se asienta, kill como hoy. Tests con el fake-pi (abort → asentado; sin asentamiento → kill). | S2 | alto (4: concurrencia con el proceso) | pending | |
| C3 | TS: `stopReason`/`errorMessage` en contratos y store; aviso por turno en el grupo (`aborted`/`length`/`error`) con **Continuar**, i18n en+es. | S3, S4, S5 | bajo | pending | |
| C4 | Verificación: `cargo test`, `cargo clippy`, `npm run build`; QA manual (parar a mitad → aviso «te detuviste» + Continuar; un corte por longitud si se puede provocar). | — | — | pending | — |

## Log

- **L1** Usuario: «diría que podemos continuar» tras el cierre del bug 2 — arranca la feature que la encuesta puso tercera y el orden acordado confirmó.
- **L2** Origen: nan-desktop (`turn.rs` + `UnfinishedTurn.tsx`: motivo de fin, aviso «me detuve» y botón Continuar, con un auto-resume). Adaptado: aquí los motivos **existen** en pi y no hacen falta inventarlos; lo que faltaba era no tirarlos y no matar el proceso al parar.
- **L3** Recon (hechos arriba). Hallazgo que amplía el alcance: nuestro stop nunca usó `abort`, así que el registro vacío del asistente y el `stop_and_wait` brutal son la misma causa. Arreglarlo hace que el stop sea limpio y que la sesión sobreviva.
- **L4** Decisión: «Continuar» = prompt de continuación, no re-ejecución. Y sin auto-resume: el humano decide, como en el resto del producto.
# turn-continuity — un turno cortado se explica y se puede continuar

Estado: en curso, rama `feat/turn-continuity` (apilada sobre `feat/auto-open-pane`).
Alcance: (1) parar de verdad por el camino de pi en vez de matar el proceso, (2) decir por qué terminó un turno cuando no terminó bien, (3) ofrecer **Continuar** en ese turno.

## Specs

- **S1** «me parece bien el orden que dices de puente, auto-open y continuidad» + «diría que podemos continuar» — esta feature. Un turno que no termina por sí solo debe **verse** como lo que es: cortado por longitud, por error, o porque el humano lo paró.
- **S2** Parar deja de matar: `stop_session` manda **`abort`** a pi, espera brevemente a que el turno se asiente (`turn_end` con `stopReason: "aborted"` + `agent_settled {aborted:true}`) y solo si no se asienta recurre al kill actual. La sesión sobrevive al stop, así que el arreglo de «re-abrir al enviar» queda como red de seguridad, no como camino normal.
- **S3** El motivo viaja tipado hasta la UI: `stopReason` (y `errorMessage` cuando lo hay) en el contrato. `stop` y `toolUse` son un turno normal y **no** se anuncian; `aborted`, `length` y `error` sí.
- **S4** Aviso por turno, dentro del grupo del turno: `Te detuviste` / `Se cortó por longitud` / `Se cortó por un error: <mensaje>`. Sobrio, sin alarmismo, y con el botón **Continuar**.
- **S5** **Continuar** envía un prompt de continuación a pi en esa sesión (`continue` con contexto de que el turno anterior no terminó) y aparece como un turno nuevo, con el agrupado que ya existe. No re-ejecuta el prompt anterior: continuar no es repetir.

## Hechos verificados (protocolo pi)

- Motivo en el **mensaje del asistente**: `stopReason: "pending" | "stop" | "length" | "toolUse" | "error" | "aborted" | "deferred"`, con `errorMessage?` y `rawStopReason?` (docs/message-types.md:134).
- `turn_end` = `{message, toolResults}` (el motivo va dentro de `message`); `agent_end` = `{messages, willRetry}`; `agent_settled` = `{aborted}` con `aborted: true` cuando el run terminó abortado (docs/json.md).
- Abort: método `abort` en RPC → `session.abort()`; el bucle emite `turn_end` (con `stopReason:"aborted"`, `toolResults:[]`), luego `agent_end` (`willRetry` forzado a false) y `agent_settled {aborted:true}`.
- **Nabla nunca manda `abort`**: solo aparece en un fixture de test (rpc/mod.rs:454). `stop_and_wait` cierra stdin y mata el árbol (process/mod.rs:736-759) — por eso el transcript queda con un registro del asistente vacío.
- No existe límite de pasos por turno ni comando nativo de continuar/reanudar.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| C1 | Rust: `StopReason` tipado en el codec (desde `message.stopReason`, con `errorMessage`), proyectado al evento `turn_end` del bridge; testeado con fixtures que hoy se descartan. | S3 | bajo | pending | |
| C2 | Rust: `stop_session` manda `abort` y espera el asentamiento (acotado, ~2 s) antes de matar; si el child no se asienta, kill como hoy. Tests con el fake-pi (abort → asentado; sin asentamiento → kill). | S2 | alto (4: concurrencia con el proceso) | pending | |
| C3 | TS: `stopReason`/`errorMessage` en contratos y store; aviso por turno en el grupo (`aborted`/`length`/`error`) con **Continuar**, i18n en+es. | S3, S4, S5 | bajo | pending | |
| C4 | Verificación: `cargo test`, `cargo clippy`, `npm run build`; QA manual (parar a mitad → aviso «te detuviste» + Continuar; un corte por longitud si se puede provocar). | — | — | pending | — |

## Log

- **L1** Usuario: «diría que podemos continuar» tras el cierre del bug 2 — arranca la feature que la encuesta puso tercera y el orden acordado confirmó.
- **L2** Origen: nan-desktop (`turn.rs` + `UnfinishedTurn.tsx`: motivo de fin, aviso «me detuve» y botón Continuar, con un auto-resume). Adaptado: aquí los motivos **existen** en pi y no hacen falta inventarlos; lo que faltaba era no tirarlos y no matar el proceso al parar.
- **L3** Recon (hechos arriba). Hallazgo que amplía el alcance: nuestro stop nunca usó `abort`, así que el registro vacío del asistente y el `stop_and_wait` brutal son la misma causa. Arreglarlo hace que el stop sea limpio y que la sesión sobreviva.
- **L4** Decisión: «Continuar» = prompt de continuación, no re-ejecución. Y sin auto-resume: el humano decide, como en el resto del producto.

- **L5** **C1-C3 hechos**: `StopReason` tipado en `rpc/mod.rs` (`Unknown(String)` conserva palabras nuevas de pi y nunca rompe el frame; `TurnEnd` gana `stopReason`/`errorMessage` con `skip_serializing_if`), elevado en el único punto de decodificación (`with_turn_report`) y espejado en contratos. **C2**: `ProcessManager::abort_and_wait` manda `abort` con el codec correlacionado y espera un latch de asentamiento (`abort_grace` = 2 s, inyectable): lo suelta un `turn_end` con `stopReason:aborted`, la respuesta `abort` de pi o la salida del hijo. Si asienta, **no se toca el hijo**: la sesión sobrevive, conserva stdin y transcripción; si no, `stop_and_wait` como hoy. Sin latches se pierde el sentido del stop; el latch se resetea al empezar para que solo este stop lo suelte. **C3**: motivo y error viajan al turno, aviso sobrio en el grupo (`aborted`/`length`/`error`, nada para `stop`/`toolUse`) y botón **Continuar** que envía un prompt de continuación (no re-ejecuta). Verificación: 373 tests, clippy 1 preexistente, `npm run build` verde.
- **L6** **El rallado del 21:22, cerrado**. El transcript del padre probó la cadena: `run_agent` en vuelo + kill de la app = toolCall sin resultado en el JSONL; al reabrir, el modelo alucinó el estado del hijo (ask_input ×3, bash ×4, read) y terminó pidiendo abrir el navegador para rehacerlo él. Fix en spawn: escaneo en flujo del JSONL (`pending_tool_calls`) y, si cuelgan toolCalls, nota honesta en `--append-system-prompt` (nada vive, no hay nada que esperar, pregunta antes de rehacer; un `run_agent` perdido apunta a los transcripts de los hijos en `<data_dir>/sessions/subagents/`). Fichero de sesión intacto: pi es dueño del transcript. Regresión S8: prompt y argv byte-idénticos con fichero limpio. Commit 0f1cec8, 455 tests.
