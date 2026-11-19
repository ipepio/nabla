# agent-question-bridge — el agente pregunta, el humano responde

Estado: en curso, rama `feat/agent-question-bridge` (apilada sobre `feat/chat-polish`).
Alcance: que el agente pueda hacer al humano preguntas estructuradas — elegir entre opciones, confirmar, pedir un texto — y recibir la respuesta desde el chat de Nabla. Hoy el agente solo puede escribir texto y esperar.

## Specs

- **S1** «me parece bien el orden que dices de puente, auto-open y continuidad» + «Con que seguimos?» — el puente de preguntas es la siguiente feature. El agente gana tools para preguntar al humano (`ask_select`, `ask_confirm`, `ask_input`); el humano responde desde una tarjeta interactiva en el chat; la respuesta llega al agente como valor de retorno del tool y el turno continúa con ella.
- **S2** (hecho verificado del protocolo, define la arquitectura) las peticiones de UI en pi RPC son **solo de extensiones**: el hijo emite `extension_ui_request` (`select`/`confirm`/`input`/`editor` + fire-and-forget) y el host contesta con una línea `extension_ui_response` `{id, value|confirmed|cancelled}` — sin respuesta de comando normal. No existe ningún `builtin:ask`. Por eso la feature incluye una **extensión first-party de pi** (`nabla-ask`, TypeScript, cargada con `-e <ruta absoluta>`, que convive con `--no-extensions`).
- **S3** La pregunta y la respuesta viven en el chat de la sesión que las originó: tarjeta con las opciones/confirmación/campo + cancelar, visible mientras esté pendiente; el agente ve la respuesta (o el cancelado) como resultado del tool. Varias peticiones pendientes a la vez se apilan en orden.
- **S4** Sin timeout por defecto: una pregunta sin responder bloquea el turno del agente — como hace este propio harness — y cancelar o parar la sesión la cierra. `editor`, `notify`, `setStatus`, `setWidget` y `setTitle` quedan **fuera** de esta tanda (non-goals).
- **S5** La identidad (`resources/nabla-system-prompt.md`) anuncia los tools de pregunta — lección de L8 en `webview-control.md`: una capacidad sin prompt está a medias.

## Hechos verificados (no re-investigar)

- Protocolo: `extension_ui_request` con `{id: uuid, method, title, ...}` — `select{options: string[]}`, `confirm{message}`, `input{placeholder?}`; respuesta `{"type":"extension_ui_response","id":..., "value"|"confirmed"|"cancelled"}` por stdin; sin record de respuesta; peticiones concurrentes ilimitadas en un `Map` por uuid (pi `dist/modes/rpc/rpc-types.d.ts:406-474`, `docs/rpc-extension-ui.md`).
- Extensión: `-e <path>` carga archivo o directorio, repetible, y sobrevive a `-ne` (cli.md:196-199). Layout local: `index.ts` con factory default-export `export default function (pi: ExtensionAPI)`; carga por jiti, TypeScript sin compilación (extensions.md:144-187, 245). UI: `await ctx.ui.select(title, items)`, `ctx.ui.confirm(title, msg)`, `ctx.ui.input(title, placeholder)` — Promises; timeout opcional (extensions.md:2823-2876).
- Nuestro código: `AgentEvent::Other { kind }` existe y hoy traga `extension_ui_request` sin tipo (rpc/mod.rs:160, test mod.rs:522-526). El stdin se escribe por `ProcessManager::send` (process/mod.rs:615; writer ~:403). Los flags del spawn están en process/mod.rs (~:764, lista con `-e builtin:mcp`). Precedente de recurso embebido: `resources/nabla-system-prompt.md` + `include_str!`.
- Regla del repo: el contrato fluye Rust → TS (regla 5 de AGENTS.md); el evento se tipa en Rust y se espeja en `src/contracts/`.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| B1 | Extensión `nabla-ask` en `resources/` (tools ask_select/ask_confirm/ask_input vía `ctx.ui.*`), materializada en `data_dir` al arrancar y cargada con `-e` en el spawn; línea en la identidad que la anuncie | S1, S2, S5 | medio (2: ejecuta JS en el hijo) | done | 52d78d9 |
| B2 | Rust: variante tipada `ExtensionUiRequest` en el codec (RED con fixtures del protocolo), proyección al bridge `agent-event`, y comando de respuesta que escriba la línea `extension_ui_response` exacta | S1, S2, S3 | medio | done | 52d78d9 |
| B3 | TS: espejo del evento en contratos, tarjeta interactiva en ChatPanel (opciones/confirmación/input + cancelar), apilada por pendientes, limpiada al parar la sesión | S3, S4 | bajo | done | a528d63 |
| B4 | Verificación: `cargo test` (fixtures + comando), `cargo clippy`, `npm run build`; QA manual del humano (pregunta real en una sesión) | — | — | done (330 tests, clippy 1 preexistente, build verde; QA manual entregada) | — |

## Log

- **L1** Usuario (verbatim): «me parece bien el orden que dices de puente, auto-open y continuidad. haz el pulido y empezamos el ODD» y «nah, tiene buena pinta tbh. Con que seguimos?».
- **L2** Origen: encuesta de 10 repos. Stark pi-desktop hace exactamente este puente (`extension_ui_request` → UI → `extension_ui_response`); su patrón de extensión bundled inspiró el enfoque first-party. Nada se copia.
- **L3** Recon de protocolo (dos pases): hechos verificados arriba. Hallazgo que define el diseño: **extension-only** — sin extensión no hay preguntas, y pi no trae ninguna de fábrica.
- **L4** Decisión de alcance: `editor`/`notify`/`setStatus`/`setWidget`/`setTitle` fuera (S4). Sin timeout por defecto (S4): el humano manda el ritmo; la cancelación existe en la tarjeta.
- **L5** **B1-B4 hechos** (`52d78d9` backend: extensión + Rust + espejo de contratos; `a528d63` UI: pila de pendientes + QuestionCard). Detalles que importan: la extensión reporta la cancelación con un mensaje que prohíbe inventar la respuesta (el fallo que esta feature existe para evitar); `ask_select` rechaza una lista de una opción; el filtro `is_dialog_ui_method` deja editor/notify/setStatus/setWidget en `Other` (fuera de tanda); `ProcessManager::answer_ui_request` es hermano de `send` porque una respuesta de UI no es un request; `encode_ui_response` es puro y testeado contra la línea exacta. Los unwraps/expects nuevos viven solo en tests (verificado). La verdadera prueba de fuego del `pi.registerTool` API es la primera sesión real (jiti carga el .ts en el hijo): QA manual del humano — reiniciar app, sesión nueva, pedir algo que exija preguntar.

- **L6** Usuario (verbatim, 19 nov): «me siento más comodo que reemplace la parte de chat y sea un picker seleccionable con las flechas, e incluso, si son varias preguntas, que se mueva ahí» → pass de UI: `QuestionPrompt.vue` ocupa el hueco del composer (select/confirm → picker `↑/↓`, Enter confirma, Esc cancela; ask_input → el composer ES el campo de respuesta, con el placeholder de la pregunta, Enter responde); varias pendientes: Tab/⇧Tab camina la pila + indicador 1 de N + auto-avance al responder, la más antigua primero; `QuestionCard.vue` reescrita como asiento de solo lectura en el transcript (✓ en la opción elegida, badge Answered/Cancelled), y `clearQuestions` asienta como canceladas las preguntas que mueren con el proceso — el Q&A ya no desaparece del historial. Consecuencia deliberada y aceptada por el usuario: sin input de mensaje mientras hay pregunta pendiente (el composer está ocupado siendo la respuesta; Esc cancela y libera). Gate: `npm run build` verde; QA manual entregada.