# composer-inputs — effort, / comandos y @ menciones

Estado: en curso, rama `feat/composer-inputs` (apilada sobre `feat/auto-open-pane`).
Alcance: tres controles del composer que cierran el dominio de la conversación: el nivel de razonamiento (effort), el autocompletado de comandos (`/`) y el de archivos (`@`).

## Specs

- **S1** «También destacar que debemos de poder cambiar el effort de los modelos» + «cambiar el effort y vincular / y @ ?» — el composer gana un selector de **effort** (nivel de thinking) y dos autocompletados: `/` comandos y `@` rutas del workspace.
- **S2 Effort en vivo y persistido**: el cambio se aplica a la sesión abierta por RPC (`set_thinking_level`) —no espera a un respawn— y el valor se guarda como default en `settings.json` para las sesiones nuevas. El selector ofrece los niveles que **ese modelo** acepta (`get_available_thinking_levels`, clampeado por pi) y el nivel real se confirma por el evento `thinking_level_changed`, no por lo que el UI esperaba.
- **S3 `/` comandos**: al escribir `/` el composer ofrece los comandos de la sesión (`get_commands` vía Nabla: `{name, description?, source}`) con badge de origen (extensión/prompt/skill); completar inserta el texto y **enviarlo lo invoca** — el `prompt` de pi ya ejecuta `/name`, incluso con stream en curso. Nabla no interpreta comandos: los lista y los delega.
- **S4 `@` menciones**: al escribir `@` el composer ofrece rutas del workspace (navegación por segmentos con `fs_list_dir`, confinada al workspace como el panel Files) y la selección **inserta la ruta en el texto del prompt**. Sin adjuntos, sin protocolo nuevo: el agente lee la ruta con sus tools. Si pi algún día expone adjuntos nativos, se cambia el destino final; el autocompletado queda.
- **S5** Los tres conviven con lo existente: Enter sigue enviando (y respondiendo preguntas input pendientes), la cola, el picker y el chip de permisos no se tocan.

## Hechos verificados (protocolo pi)

- **Effort**: flag `--thinking <off|minimal|low|medium|high|xhigh|max>` (docs/cli.md:71, override de sufijos de modelo, clampeado a sus capacidades); setting `defaultThinkingLevel`. **RPC en vivo**: `set_thinking_level {level}`, `get_available_thinking_levels`, `cycle_thinking_level` (rpc-types.d.ts:56-70, docs/rpc-commands.md:274-321); evento `thinking_level_changed`. TUI: `/thinking` + Shift+Tab.
- **Comandos**: `get_commands` → `{commands: [{name (sin barra), description?, source: "extension"|"prompt"|"skill", sourceInfo}]}` (rpc-types.d.ts:135-147). Invocación: prompt con `/name` (rpc-commands.md:790); se ejecutan incluso con stream en curso y expanden `/skill:name` y templates (rpc-commands.md:31-33). Sin marcador de argumentos en el RPC.
- **Menciones**: no hay adjuntos en RPC — `prompt`/`steer`/`follow_up` llevan `message` + `images?` base64 (rpc-types.d.ts:19-34). `@file` es posicional de CLI (inlinea el texto del archivo). Conclusión: `@` en Nabla es inserción de ruta, no adjunto.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| E1 | Rust: `thinkingLevel` default en `Settings` (retrocompatible); comandos `set_session_thinking_level` (RPC en vivo + persistencia) y `session_thinking_levels` (proxy de los niveles del modelo); variante tipada del evento `thinking_level_changed` en el codec (hoy cae en `Other`). Tests con fixtures. | S2 | bajo | pending | |
| E2 | TS: espejo de contratos + selector de effort en la fila del composer (junto al chip de permisos): niveles de la sesión, nivel actual por evento, persistencia del default. | S2 | bajo | pending | |
| S1 | Rust: comando `session_commands` (proxy de `get_commands` tipado: name/description/source). | S3 | bajo | pending | |
| S2 | TS: autocompletado de `/` en el composer (abre con `/` al inicio del texto, filtra, Enter/Tab completa, badge de origen, Esc cierra); enviar el texto invoca — sin camino nuevo. | S3 | bajo | pending | |
| A1 | TS: autocompletado de `@` (navegación por segmentos sobre `fs_list_dir`, confinada al workspace; inserta la ruta). | S4 | bajo | pending | |
| V | Verificación: `cargo test`, `cargo clippy`, `npm run build`; QA manual (cambiar effort en vivo, invocar un comando, mencionar un archivo). | — | — | pending | — |

## Log

- **L1** Usuario (verbatim): «También destacar que debemos de poder cambiar el effort de los modelos» y «siguiente paso? cambiar el effort y vincular / y @ ?».
- **L2** Origen: effort viene del propio mantenedor; `/` y `@` cierran la lista P1 de la encuesta (Stark: descubrimiento de comandos con badges y autocompletado).
- **L3** Recon (hechos arriba). Decisiones: effort **en vivo** (pi lo soporta y todo en este producto cambia en caliente) + default persistido; `/` delega la ejecución a pi (Nabla nunca interpreta comandos); `@` inserta ruta (no hay adjuntos en el protocolo y no los inventamos).
- **L4** Regla transversal: los tres conviven con Enter/pregunta-pendiente/cola/picker (S5); el composer es superficie compartida, así que las tres features se implementan **secuencialmente**, una worker, commits por feature.
# composer-inputs — effort, / comandos y @ menciones

Estado: en curso, rama `feat/composer-inputs` (apilada sobre `feat/auto-open-pane`).
Alcance: tres controles del composer que cierran el dominio de la conversación: el nivel de razonamiento (effort), el autocompletado de comandos (`/`) y el de archivos (`@`).

## Specs

- **S1** «También destacar que debemos de poder cambiar el effort de los modelos» + «cambiar el effort y vincular / y @ ?» — el composer gana un selector de **effort** (nivel de thinking) y dos autocompletados: `/` comandos y `@` rutas del workspace.
- **S2 Effort en vivo y persistido**: el cambio se aplica a la sesión abierta por RPC (`set_thinking_level`) —no espera a un respawn— y el valor se guarda como default en `settings.json` para las sesiones nuevas. El selector ofrece los niveles que **ese modelo** acepta (`get_available_thinking_levels`, clampeado por pi) y el nivel real se confirma por el evento `thinking_level_changed`, no por lo que el UI esperaba.
- **S3 `/` comandos**: al escribir `/` el composer ofrece los comandos de la sesión (`get_commands` vía Nabla: `{name, description?, source}`) con badge de origen (extensión/prompt/skill); completar inserta el texto y **enviarlo lo invoca** — el `prompt` de pi ya ejecuta `/name`, incluso con stream en curso. Nabla no interpreta comandos: los lista y los delega.
- **S4 `@` menciones**: al escribir `@` el composer ofrece rutas del workspace (navegación por segmentos con `fs_list_dir`, confinada al workspace como el panel Files) y la selección **inserta la ruta en el texto del prompt**. Sin adjuntos, sin protocolo nuevo: el agente lee la ruta con sus tools. Si pi algún día expone adjuntos nativos, se cambia el destino final; el autocompletado queda.
- **S5** Los tres conviven con lo existente: Enter sigue enviando (y respondiendo preguntas input pendientes), la cola, el picker y el chip de permisos no se tocan.

## Hechos verificados (protocolo pi)

- **Effort**: flag `--thinking <off|minimal|low|medium|high|xhigh|max>` (docs/cli.md:71, override de sufijos de modelo, clampeado a sus capacidades); setting `defaultThinkingLevel`. **RPC en vivo**: `set_thinking_level {level}`, `get_available_thinking_levels`, `cycle_thinking_level` (rpc-types.d.ts:56-70, docs/rpc-commands.md:274-321); evento `thinking_level_changed`. TUI: `/thinking` + Shift+Tab.
- **Comandos**: `get_commands` → `{commands: [{name (sin barra), description?, source: "extension"|"prompt"|"skill", sourceInfo}]}` (rpc-types.d.ts:135-147). Invocación: prompt con `/name` (rpc-commands.md:790); se ejecutan incluso con stream en curso y expanden `/skill:name` y templates (rpc-commands.md:31-33). Sin marcador de argumentos en el RPC.
- **Menciones**: no hay adjuntos en RPC — `prompt`/`steer`/`follow_up` llevan `message` + `images?` base64 (rpc-types.d.ts:19-34). `@file` es posicional de CLI (inlinea el texto del archivo). Conclusión: `@` en Nabla es inserción de ruta, no adjunto.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| E1 | Rust: `thinkingLevel` default en `Settings` (retrocompatible); comandos `set_session_thinking_level` (RPC en vivo + persistencia) y `session_thinking_levels` (proxy de los niveles del modelo); variante tipada del evento `thinking_level_changed` en el codec (hoy cae en `Other`). Tests con fixtures. | S2 | bajo | done | (unidad composer-inputs) |
| E2 | TS: espejo de contratos + selector de effort en la fila del composer (junto al chip de permisos): niveles de la sesión, nivel actual por evento, persistencia del default. | S2 | bajo | done | (unidad composer-inputs) |
| S1 | Rust: comando `session_commands` (proxy de `get_commands` tipado: name/description/source). | S3 | bajo | done | (unidad composer-inputs) |
| S2 | TS: autocompletado de `/` en el composer (abre con `/` al inicio del texto, filtra, Enter/Tab completa, badge de origen, Esc cierra); enviar el texto invoca — sin camino nuevo. | S3 | bajo | done | (unidad composer-inputs) |
| A1 | TS: autocompletado de `@` (navegación por segmentos sobre `fs_list_dir`, confinada al workspace; inserta la ruta). | S4 | bajo | done | (unidad composer-inputs) |
| V | Verificación: `cargo test`, `cargo clippy`, `npm run build`; QA manual (cambiar effort en vivo, invocar un comando, mencionar un archivo). | — | — | done (389 tests, clippy 1 preexistente, build verde; QA manual entregada) | — |

## Log

- **L1** Usuario (verbatim): «También destacar que debemos de poder cambiar el effort de los modelos» y «siguiente paso? cambiar el effort y vincular / y @ ?».
- **L2** Origen: effort viene del propio mantenedor; `/` y `@` cierran la lista P1 de la encuesta (Stark: descubrimiento de comandos con badges y autocompletado).
- **L3** Recon (hechos arriba). Decisiones: effort **en vivo** (pi lo soporta y todo en este producto cambia en caliente) + default persistido; `/` delega la ejecución a pi (Nabla nunca interpreta comandos); `@` inserta ruta (no hay adjuntos en el protocolo y no los inventamos).
- **L4** Regla transversal: los tres conviven con Enter/pregunta-pendiente/cola/picker (S5); el composer es superficie compartida, así que las tres features se implementan **secuencialmente**, una worker, commits por feature.

- **L6** **E1/A1 hechos** en una unidad: `Settings.thinkingLevel` (default `medium`, el documentado por pi; un archivo viejo no cambia el razonamiento de nadie), `SessionModel` agrupa lo elegido por spawn y emite `--thinking` tras `--model` (afirmado sobre el argv real del hijo), `AgentEvent::ThinkingLevelChanged` tipada, comandos `set_session_thinking_level` (en vivo + persiste), `session_thinking_levels` (los del modelo real) y `session_commands` (proxy de `get_commands`). TS: `ComposerEffort.vue` junto al chip de permisos (nivel por evento, no por expectativa), `ComposerSuggestions.vue` + `composer-completion.ts` (reglas puras: `/` sólo al inicio, `@` al inicio o tras espacio, navegación por segmentos con `fs_list_dir` confinada al workspace) y Enter completa con sugerencia abierta / envía cerrada / el picker sigue siendo dueño del composer. 389 tests, clippy 1 preexistente, `npm run build` verde. Nota: el plan decía commits por feature, pero las tres quedaron entrelazadas en `Composer.vue` (354 líneas compartidas) — una unidad coherent y constatado aquí, no escondido.
- **L7** **Defecto encontrado por el humano en QA** («veo siempre los mismos para todos los modelos y al cambiar de modelo se mantiene el valor anterior»): el selector de modelo era **sólo settings** — la sesión viva nunca cambiaba de modelo, así que la lista de efforts (la de su modelo de nacimiento) y el nivel no se movían. Mi L6 decía «la lista se re-pregunta por el modelo activo», técnicamente cierto y engañoso: ese modelo nunca cambiaba. Arreglo: `set_session_model` (commands.rs, espejo exacto de `set_thinking_level_for`: request correlado `set_model {provider, modelId}`, espera acotada, devuelve el `Model` efectivo de pi — su respuesta, no nuestro eco); `handleSelectModel` persiste SIEMPRE (default del próximo spawn) y además conmuta la sesión si está viva (persist-only silencioso si no lo está; error visible si pi rechaza). pi re-clampa el thinking al rango del nuevo modelo y emite `thinking_level_changed` — el selector ya confiaba en el evento, y la lista se relee al abrir. 393 tests.
- **L8** **Segundo defecto de QA del humano** («lo he hecho y se sigue manteniendo el effort seleccionado previamente»): su prueba se hizo SIN sesión viva (evidencia: ningún transcript escrito hoy; settings sí persistió el pick). El camino persist-only silencioso es indistinguible de una conmutación en vivo — defecto de visibilidad, no de lógica. Arreglo: etiqueta discreta «próxima sesión» junto a los pickers de model/effort cuando no hay sesión viva (`nextSessionOnly` en Composer, calculado en ChatPanel con `liveIds`, pasado también desde el empty state), con title que lo explica. Re-test protocol: sesión VIVA (enviar un prompt primero), cambiar modelo, y el effort debe re-clampar.
- **L9** Usuario (verbatim): «en settings metemos un default reasoning level general y al cambiar en lugar que esté desactivado pongamos ese? o ponemos en auto al cambiar en lugar de off?» — hechos verificados: pi **no tiene «auto»** (enum fijo, default `medium`; el Model no expone default de razonamiento) y **el clamp de pi nunca sube desde `off`** (es un nivel que todo modelo soporta), así que una sesión nacida sin razonamiento arrastraba su `off` a cualquier modelo. Decisión: opción (a) — tras una conmutación en vivo, el panel re-aplica el effort persistido y pi lo clampa al rango del nuevo modelo («la preferencia sigue al modelo»; un effort manual de sesión no sobrevive al cambio — regla deliberada y predecible). Sin sesión, el pick ya aplica a la próxima (spawn lee settings).