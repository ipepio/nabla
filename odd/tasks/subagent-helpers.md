# subagent-helpers — coordinación de agentes visible donde ocurre

Estado: **diseñado tras recon** (H0 resuelto), sin rama ni código. P2 de P-001.
Alcance: el agente delega en subagentes (extensión first-party nuestra, patrón de `pi-subagents-j0k3r` sin fork — MIT, estudiado como referencia), y Nabla muestra las **sesiones ligadas** al chat que las lanzó, abribles en vivo como Herdr abre sus agentes.

## Specs

- **S1** «estudia esta extensión y la creamos propia (no forkees) la adaptamos a como necesitemos. La idea es ver las sesiones ligadas y poder abrirlas tipo en herdr» — extensión propia, adaptada; sesiones de subagentes **ligadas al chat padre y abribles**.
- **S2** La unidad es el **chat que delega**: la tarjeta del subagente vive en el turno que lo lanzó (estado, progreso), y su sesión aparece **anidada** en la barra lateral bajo la padre — no en una lista global.
- **S3** Abrir una sesión de subagente la muestra **en vivo y en solo lectura** (renderizador de transcript que ya existe, siguiendo el JSONL mientras crece). La interacción con el subagente (mandarle mensaje, continuar) va **por el chat padre** con las tools de la extensión, no escribiendo en su vista — la vista es para mirar, el mando sigue en la conversación.
- **S4** Definiciones de agente en markdown en el workspace (`.pi/agents/*.md`, frontmatter: name/description/tools/model/effort) — misma convención que la referencia, para que definiciones existentes sirvan.
- **S5** Seguridad: allowlist de tools por definición **+** nuestro gate de permisos (el niño corre en proceso dentro del pi padre; su `tool_call` pasa por el mismo hook — **verificar** en H1). Sin recursiones en v1. Límite de hijos paralelos.
- **S6** Preguntas: los subagentes **no preguntan al humano** en v1 (son trabajadores autónomos con allowlist); el padre pregunta por el puente `ask_*` si hace falta una decisión.

## Hechos verificados (recon de `pi-subagents-j0k3r@1.6.1`, MIT)

- **No hay subagentes nativos en pi**: son una **extensión**, y el patrón es el SDK en proceso — `createAgentSession({cwd, model, thinkingLevel, tools})` dentro del proceso padre. Ni un proceso nuevo.
- **El niño escribe su propio JSONL** (`nested_session_path`): un transcript de pi real, legible con la maquinaria de `session_transcript` que ya tenemos. Ésta es la pieza que hace barata la vista "tipo herdr".
- **Observabilidad en vivo**: el SDK expone `session.subscribe(...)` dentro del proceso; la referencia suscribe y vuelca actividad a su SQLite con flush de 150-250 ms. Para el HOST, los enganches disponibles son: los eventos de tools del padre (`tool_execution_start/update/end` sobre `subagent_run` — **ya tipados en nuestro bridge**), el JSONL del niño (append mientras corre), y el polling de sus tools de estado.
- **La referencia registra 8 tools** (run/continue/status/result/list/cancel/send_message/list_agents) y bloquea la tool hasta el fin en modo tarea, con `onUpdate` colando progreso en el tool result; `background` responde al momento. Sus detalles de tool result llevan `tasks` con estado y `live_activity`.
- **Vinculación**: la referencia usa su propia SQLite (`transcript`, `thread_snapshot_json`, `result`, `nested_session_path`). Para Nabla **no la necesitamos**: el vínculo padre→niño se puede tomar del `details` del tool result del padre (lleva la ruta del JSONL del niño) — contrato de host más simple, sin base de datos nueva.

## Diseño de la adaptación (lo que construimos)

```text
pi padre (sesión del chat, ProcessManager, eventos → puente)
 ├─ extensión nabla-subagents.ts (first-party, -e junto a nabla-ask)
 │    ├─ run_agent(agent, task, {mode: task|background})  → createAgentSession(allowlist, model, effort)
 │    ├─ agent_status / agent_cancel / agent_send_message (mínimo en v1)
 │    └─ el niño: JSONL propio (nested_session_path), sin extensiones cargadas
 ├─ tarjeta en el turno del padre: estado + progreso (onUpdate → tool_execution_update que ya tipamos)
 └─ Rust: indexa los JSONL de niños con parent_session_id → sidebar anidada + transcript en vivo
```

- **Extensión propia** (`resources/nabla-subagents.ts`): menos tools que la referencia (run + status + cancel + send_message; continue y batch fuera de v1), mismas convenciones de definición.
- **Vínculo**: capturar `nested_session_path` del tool result del padre → fila en el índice con `parent_session_id` (columna nueva, migración v5) → **sidebar anidada**.
- **Vista**: transcript del niño con el renderizador existente (grupos por turno ya funcionan con cualquier JSONL de pi) + tail en vivo mientras `status = running`.
- **Permisos**: el gate global ya corre en el proceso del padre; si el hook cubre a los niños (H1), el modo lectura/escritura/yolo aplica también a ellos por herencia real, no por configuración.

## H1 — verificado (responde las cuatro)

1. **El hook NO ve a los niños**: cada `createAgentSession` arma su propio `Agent` y un runner de extensiones privado (`dist/core/sdk.js:69`); el `tool_call` del padre no intercepta nada del niño. **Consecuencia**: el gate de permisos se aplica **al construir la allowlist** — nuestra extensión lee la proyección de permisos (como `nabla-permissions`) y en modo `read` quita `bash/powershell/edit/write` de la allowlist del niño. Enforcement real, no indirecto.
2. **La allowlist `tools` ES de verdad** (sdk.d.ts: una tool no listada no se registra ni corre). Matices: MCP queda *registrada* para codemode salvo entrada `mcp__*` o lista vacía — irrelevante para niños limpios sin extensiones; una lista de sólo `+name/-name` son modificadores, no allowlist. V1: nombres llanos explícitos + `excludeTools: ["mcp__*"]` por cinturón y tirantes.
3. **El JSONL del niño lo colocamos nosotros**: `CreateAgentSessionOptions.sessionManager` acepta un manager propio (constructor: cwd, sessionDir, sessionFile…) → el niño escribe en `<NABLA_DATA_DIR>/sessions/subagents/<uuid>.jsonl`, determinista, y la extensión reporta la ruta al host. **Sin depender de la convención `nested_session_path` de pi ni de la SQLite de la referencia.**
4. **Niños limpios** (decisión del humano: «así se ciñe a lo que le pide el padre»): el SDK no carga extensiones en las sesiones que crea; el import es por el main del paquete (`createAgentSession` exportado de `dist/index.js`), mismo patrón que `nabla-ask`.

**Contrato de host (vínculo)**: la extensión escribe un **manifiesto** `<NABLA_DATA_DIR>/subagents/<clave-padre>.json` con los hijos (nombre, ruta JSONL, estado, timestamps) al crear/terminar cada niño; Rust lo lee para indexar y enlazar. La clave del padre: la extensión debe aprender su propia identidad de sesión (verificar acceso a sessionFile desde el ExtensionAPI/ctx; si no existe, manifest por workspace y enlace por el padre activo — más débil, y se declara).

## Tasks (se refinan con H1)

| ID | Tarea | Estado |
| --- | --- | --- |
| H1 | Recon fino: las 4 verificaciones de arriba | pending |
| H2 | Extensión `nabla-subagents.ts`: tools run/status/cancel/send_message + definiciones markdown | pending |
| H3 | Rust: `parent_session_id` en el índice (migración v5), captura del vínculo desde el tool result, listado de hijos | pending |
| H4 | UI: tarjeta en el turno (estado/progreso/abrir/parar) + sidebar anidada + vista de transcript en vivo solo-lectura | pending |
| H5 | Verificación: cargo test + build + QA manual (delegar una tarea, ver la sesión ligada, abrirla en vivo, pararla) | pending |

## Log

- **L1** Usuario (verbatim): «estudia esta extensión y la creamos propia (no forkees) la adaptamos a como necesitemos. La idea es ver las sesiones ligadas y poder abrirlas tipo en herdr … ¿Como adaptamos los sub agentes?».
- **L2** Encuesta: Helpers por chat como mejor idea P2 (gentle-shell-desktop / gentle-shell).
- **L3** Plan inicial con incógnitas (hoy resueltas por recon): la extensión de referencia NO forkea — es MIT, la estudiamos como patrón y escribimos la nuestra con menos superficie (4 tools vs 8) y **sin su SQLite**: el vínculo vive en el tool result del padre y en nuestro índice, no en una base nueva.
- **L4** Dato de arquitectura que ordena todo: los niños corren **en proceso vía SDK** y escriben **JSONL de pi** — nuestro lector de transcripts y el renderizado por turnos sirven tal cual. La vista "tipo herdr" es una lectura de archivo, no un protocolo nuevo.
- **L5** Decisiones v1: vista del niño **solo lectura** (mando por el chat padre, como pide S3); niños sin preguntas al humano (S6); sin recursiones; interacción por `agent_send_message` desde el padre.
# subagent-helpers — coordinación de agentes visible donde ocurre

Estado: **diseñado tras recon** (H0 resuelto), sin rama ni código. P2 de P-001.
Alcance: el agente delega en subagentes (extensión first-party nuestra, patrón de `pi-subagents-j0k3r` sin fork — MIT, estudiado como referencia), y Nabla muestra las **sesiones ligadas** al chat que las lanzó, abribles en vivo como Herdr abre sus agentes.

## Specs

- **S1** «estudia esta extensión y la creamos propia (no forkees) la adaptamos a como necesitemos. La idea es ver las sesiones ligadas y poder abrirlas tipo en herdr» — extensión propia, adaptada; sesiones de subagentes **ligadas al chat padre y abribles**.
- **S2** La unidad es el **chat que delega**: la tarjeta del subagente vive en el turno que lo lanzó (estado, progreso), y su sesión aparece **anidada** en la barra lateral bajo la padre — no en una lista global.
- **S3** Abrir una sesión de subagente la muestra **en vivo y en solo lectura** (renderizador de transcript que ya existe, siguiendo el JSONL mientras crece). La interacción con el subagente (mandarle mensaje, continuar) va **por el chat padre** con las tools de la extensión, no escribiendo en su vista — la vista es para mirar, el mando sigue en la conversación.
- **S4** Definiciones de agente en markdown en el workspace (`.pi/agents/*.md`, frontmatter: name/description/tools/model/effort) — misma convención que la referencia, para que definiciones existentes sirvan.
- **S5** Seguridad: allowlist de tools por definición **+** nuestro gate de permisos (el niño corre en proceso dentro del pi padre; su `tool_call` pasa por el mismo hook — **verificar** en H1). Sin recursiones en v1. Límite de hijos paralelos.
- **S6** Preguntas: los subagentes **no preguntan al humano** en v1 (son trabajadores autónomos con allowlist); el padre pregunta por el puente `ask_*` si hace falta una decisión.

## Hechos verificados (recon de `pi-subagents-j0k3r@1.6.1`, MIT)

- **No hay subagentes nativos en pi**: son una **extensión**, y el patrón es el SDK en proceso — `createAgentSession({cwd, model, thinkingLevel, tools})` dentro del proceso padre. Ni un proceso nuevo.
- **El niño escribe su propio JSONL** (`nested_session_path`): un transcript de pi real, legible con la maquinaria de `session_transcript` que ya tenemos. Ésta es la pieza que hace barata la vista "tipo herdr".
- **Observabilidad en vivo**: el SDK expone `session.subscribe(...)` dentro del proceso; la referencia suscribe y vuelca actividad a su SQLite con flush de 150-250 ms. Para el HOST, los enganches disponibles son: los eventos de tools del padre (`tool_execution_start/update/end` sobre `subagent_run` — **ya tipados en nuestro bridge**), el JSONL del niño (append mientras corre), y el polling de sus tools de estado.
- **La referencia registra 8 tools** (run/continue/status/result/list/cancel/send_message/list_agents) y bloquea la tool hasta el fin en modo tarea, con `onUpdate` colando progreso en el tool result; `background` responde al momento. Sus detalles de tool result llevan `tasks` con estado y `live_activity`.
- **Vinculación**: la referencia usa su propia SQLite (`transcript`, `thread_snapshot_json`, `result`, `nested_session_path`). Para Nabla **no la necesitamos**: el vínculo padre→niño se puede tomar del `details` del tool result del padre (lleva la ruta del JSONL del niño) — contrato de host más simple, sin base de datos nueva.

## Diseño de la adaptación (lo que construimos)

```text
pi padre (sesión del chat, ProcessManager, eventos → puente)
 ├─ extensión nabla-subagents.ts (first-party, -e junto a nabla-ask)
 │    ├─ run_agent(agent, task, {mode: task|background})  → createAgentSession(allowlist, model, effort)
 │    ├─ agent_status / agent_cancel / agent_send_message (mínimo en v1)
 │    └─ el niño: JSONL propio (nested_session_path), sin extensiones cargadas
 ├─ tarjeta en el turno del padre: estado + progreso (onUpdate → tool_execution_update que ya tipamos)
 └─ Rust: indexa los JSONL de niños con parent_session_id → sidebar anidada + transcript en vivo
```

- **Extensión propia** (`resources/nabla-subagents.ts`): menos tools que la referencia (run + status + cancel + send_message; continue y batch fuera de v1), mismas convenciones de definición.
- **Vínculo**: capturar `nested_session_path` del tool result del padre → fila en el índice con `parent_session_id` (columna nueva, migración v5) → **sidebar anidada**.
- **Vista**: transcript del niño con el renderizador existente (grupos por turno ya funcionan con cualquier JSONL de pi) + tail en vivo mientras `status = running`.
- **Permisos**: el gate global ya corre en el proceso del padre; si el hook cubre a los niños (H1), el modo lectura/escritura/yolo aplica también a ellos por herencia real, no por configuración.

## H1 — verificado (responde las cuatro)

1. **El hook NO ve a los niños**: cada `createAgentSession` arma su propio `Agent` y un runner de extensiones privado (`dist/core/sdk.js:69`); el `tool_call` del padre no intercepta nada del niño. **Consecuencia**: el gate de permisos se aplica **al construir la allowlist** — nuestra extensión lee la proyección de permisos (como `nabla-permissions`) y en modo `read` quita `bash/powershell/edit/write` de la allowlist del niño. Enforcement real, no indirecto.
2. **La allowlist `tools` ES de verdad** (sdk.d.ts: una tool no listada no se registra ni corre). Matices: MCP queda *registrada* para codemode salvo entrada `mcp__*` o lista vacía — irrelevante para niños limpios sin extensiones; una lista de sólo `+name/-name` son modificadores, no allowlist. V1: nombres llanos explícitos + `excludeTools: ["mcp__*"]` por cinturón y tirantes.
3. **El JSONL del niño lo colocamos nosotros**: `CreateAgentSessionOptions.sessionManager` acepta un manager propio (constructor: cwd, sessionDir, sessionFile…) → el niño escribe en `<NABLA_DATA_DIR>/sessions/subagents/<uuid>.jsonl`, determinista, y la extensión reporta la ruta al host. **Sin depender de la convención `nested_session_path` de pi ni de la SQLite de la referencia.**
4. **Niños limpios** (decisión del humano: «así se ciñe a lo que le pide el padre»): el SDK no carga extensiones en las sesiones que crea; el import es por el main del paquete (`createAgentSession` exportado de `dist/index.js`), mismo patrón que `nabla-ask`.

**Contrato de host (vínculo)**: la extensión escribe un **manifiesto** `<NABLA_DATA_DIR>/subagents/<clave-padre>.json` con los hijos (nombre, ruta JSONL, estado, timestamps) al crear/terminar cada niño; Rust lo lee para indexar y enlazar. La clave del padre: la extensión debe aprender su propia identidad de sesión (verificar acceso a sessionFile desde el ExtensionAPI/ctx; si no existe, manifest por workspace y enlace por el padre activo — más débil, y se declara).

## Tasks (se refinan con H1)

| ID | Tarea | Estado |
| --- | --- | --- |
| H1 | Recon fino: las 4 verificaciones de arriba | pending |
| H2 | Extensión `nabla-subagents.ts`: tools run/status/cancel/send_message + definiciones markdown | done |
| H3 | Rust: `parent_session_id` en el índice (migración v5), manifiesto como contrato de host, listado de hijos con estado vivo | done |
| H4 | UI: tarjeta en el turno (estado/progreso/abrir/parar) + sidebar anidada + vista de transcript en vivo solo-lectura | pending |
| H5 | Verificación: cargo test + build + QA manual (delegar una tarea, ver la sesión ligada, abrirla en vivo, pararla) | pending |

## Log

- **L1** Usuario (verbatim): «estudia esta extensión y la creamos propia (no forkees) la adaptamos a como necesitemos. La idea es ver las sesiones ligadas y poder abrirlas tipo en herdr … ¿Como adaptamos los sub agentes?».
- **L2** Encuesta: Helpers por chat como mejor idea P2 (gentle-shell-desktop / gentle-shell).
- **L3** Plan inicial con incógnitas (hoy resueltas por recon): la extensión de referencia NO forkea — es MIT, la estudiamos como patrón y escribimos la nuestra con menos superficie (4 tools vs 8) y **sin su SQLite**: el vínculo vive en el tool result del padre y en nuestro índice, no en una base nueva.
- **L4** Dato de arquitectura que ordena todo: los niños corren **en proceso vía SDK** y escriben **JSONL de pi** — nuestro lector de transcripts y el renderizado por turnos sirven tal cual. La vista "tipo herdr" es una lectura de archivo, no un protocolo nuevo.
- **L5** Decisiones v1: vista del niño **solo lectura** (mando por el chat padre, como pide S3); niños sin preguntas al humano (S6); sin recursiones; interacción por `agent_send_message` desde el padre.

- **L6** **H2+H3 hechos** (410 tests, clippy 1, build verde): la extensión registra las 4 tools; allowlist = definición − propias − (read ⇒ shell/editores), tope de 4 hijos por sesión; el niño escribe su JSONL bajo `<data_dir>/sessions/subagents/`; el manifiesto se clavea con sha256(ruta del padre) — `ctx.sessionManager.getSessionFile()` SÍ expone la ruta propia (el caso fuerte), fallback `workspace:<cwd>` declarado; Rust lee manifiestos frescos en cada llamada, upserta hijos con `parent_session_id` (v5, cascade) y `subagent_sessions` fusiona estado vivo. El transcript del niño se lee con `session_transcript` tal cual. Pendiente: H4 (tarjeta + sidebar anidada + vista en vivo) y H5.