# chat-turn-groups — un desplegable por turno, no por paso

Estado: en curso, rama `feat/chat-turn-groups` (apilada sobre `feat/auto-open-pane`).
Alcance: agrupar el trabajo del agente por **turno del humano**, no por paso. Un turno con 13 pasos (razonamientos + tools) se lee como **una** fila plegable con la respuesta final visible; abrirla muestra los pasos en orden. UI-only.

## Specs

- **S1** «así es imposible seguirlo, debería de estar todo junto y desplegarse. y que fuera un desplegable por turno» — un turno = lo que ocurre entre un mensaje del humano y el siguiente. Todo su trabajo (razonamientos intermedios + tools) va **junto**, en un único desplegable; no una caja por paso.
- **S2** La **respuesta final** del turno (el último texto del asistente) queda visible sin desplegar nada: es lo que el humano vino a leer. Los pasos intermedios son el «cómo», no el «qué».
- **S3** Turno en curso: el paso actual se ve en vivo (razonamiento y chips de ese paso); los pasos anteriores de ese mismo turno ya están dentro del desplegable. Al terminar el turno, todo se pliega menos la respuesta.
- **S4** Los fallos siguen sin esconderse: la fila plegada del turno muestra `N pasos · fallos: M` en rojo si alguno falló (regla ya establecida en `chat-polish` L7).
- **S5** La atribución actual es al mensaje del asistente (un step), por eso salen 26 cajas; pasa a ser al **turno del humano**, mintiendo el id en `pushUserTurn`. Los tools conservan además su step para agruparse dentro del turno.

## Hechos verificados

- `ChatMessage` (session-store) no tiene id de turno; `ToolEntry.turnId` (commit `529e78e`) apunta al **mensaje del asistente**, no al turno del humano — es la causa directa de la captura.
- pi emite un mensaje del asistente (con su `thinking_delta`) **por paso**; el turno del humano se reconoce por su `role: "user"` en el transcript.
- El transcript se restaura del JSONL de pi; grupos y turnos deben derivarse de los mensajes existentes, sin estado nuevo persistido.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| G1 | Store: id de turno del humano (mintido en `pushUserTurn`), heredado por los mensajes del asistente y por los chips; `stepId` conservado en los chips para agrupar dentro del turno | S1, S5 | done | (unidad turn-groups) |
| G2 | UI: un componente de grupo por turno (respuesta final visible, un desplegable con los pasos: razonamiento + chips de cada uno, fallos en la cabecera); turno vivo con el paso actual fuera del desplegable | S1-S4 | done | (unidad turn-groups) |
| G3 | Verificación: `npm run build` + QA manual del humano | — | done (build verde; QA manual entregada) | — |

## Log

- **L1** Usuario (verbatim): «así es imposible seguirlo, debería de estar todo junto y desplegarse. y que fuera un desplegable por turno» (captura con 13 `Reasoning` + 13 filas `1 actions`).
- **L2** Diagnóstico de la misma captura: las tools **sí funcionaron** (28 resultados en el transcript: navegaciones a Google con query, `eval` con contenido real, UOC leída). Los `failures: 1` vienen de: primer `navigate` antes de auto-open, un `list_targets` sin vista, y un bug real de **id de vista obsoleto** (`embed: no embedded browser embed-1 is open` con la vista registrada). Ese bug es aparte y no se arregla aquí.
- **L3** La agrupación anterior (`529e78e`) agrupaba por mensaje del asistente = paso, de ahí las 26 cajas. El turno correcto es el del humano.

- **L4** **G1-G3 hechos**: `SessionBuffer.turnId` acuñado en `pushUserTurn` (mismo id que el mensaje humano, sin segundo espacio de ids), `ChatMessage.turnId` estampado en `flushStreaming`, y en `ToolEntry` se separó lo que antes era una sola cosa: `turnId` = turno del humano, **`stepId`** = mensaje del asistente (`pushTool` sigue siendo el único escritor; un chip nunca queda huérfano: `turnId = buffer.turnId ?? stepId`). `settleQueuedMessage` y `settleQuestion` abren turno — sin eso la respuesta a un prompt encolado o a una pregunta se atribuía al turno anterior y se leía fuera de orden. UI: `turn-groups.ts` (`deriveTurnGroups`, turno = mensaje humano que lo abre) + `TurnGroup.vue`; **`ToolSummary` se reutilizó como la fila plegable del turno** en vez de retirarlo (ya era chevron + desglose + `fallos: N` en rojo), `chat.turn.steps` en en+es. Turno vivo: pasos anteriores dentro, paso actual (razonamiento + chips) fuera, respuesta visible. Restaurado del JSONL: grupos derivados por orden de mensajes, sin ids inventados. `npm run build` verde.