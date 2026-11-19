# permission-modes — lectura, escritura y yolo

Estado: en curso, rama `feat/permission-modes` (apilada sobre `feat/auto-open-pane`).
Alcance: un modo de permiso visible y respetado. **Lectura** no deja tocar nada; **Escritura** deja editar archivos y **pregunta** antes de ejecutar comandos; **Yolo** no pregunta. Gobernado por una extensión de pi, decidido por el humano desde la cabecera del chat.

## Specs

- **S1** «deberíamos de darle los permisos a nabla pues, no? … en alguna parte ver el modo en el que está, lectura, escritura y yolo. Además de que pregunte cuando está en escritura (yolo es yolo) para hacer las acciones» — tres modos, visibles en la interfaz, y el modo escritura pregunta antes de las acciones que ejecutan.
- **S2** El modo vive en `settings.json` y se cambia desde la cabecera del chat sin reiniciar la app ni la sesión: la siguiente llamada de tool ya obedece al modo nuevo.
- **S3** El veto llega al modelo como **error de tool con su motivo**, no como silencio: `lectura` bloquea `bash`/`edit`/`write` explicando que el humano puso el modo lectura y que puede cambiarlo. (pi entrega `reason` como el texto del error; un handler que revienta también bloquea — fail-safe.)
- **S4** En `escritura`, `bash` **pregunta** reusando el puente de preguntas ya construido: la tarjeta enseña el comando completo y el humano confirma o cancela. Cancelar = bloqueo con motivo, nunca ejecución silenciosa.
- **S5** `yolo` no pregunta nada. Y se dice en la interfaz, sin eufemismos.
- **S6** Default: **escritura** (el comportamiento de hoy para archivos; los comandos pasan a pedir permiso, que es el cambio intencionado). La tools del navegador no se gatean en esta v1 (no tocan el workspace).

## Hechos verificados (protocolo pi)

- Hook de extensión: `pi.on("tool_call", async (event, ctx) => …)`; `event.toolName` ∈ `bash|read|write|edit|grep|find|ls` (+ `powershell` en Windows), `event.toolCallId`, `event.input` mutable. **Permitir = devolver `undefined`**; **bloquear = `{ block: true, reason }`** (o `{ block: true, terminate: true }`). El `reason` se convierte en el texto del error que ve el modelo (`createErrorToolResult`), `isError: true`; la llamada no se ejecuta pero sí emite `tool_execution_end` con error. Un handler que lanza también bloquea (fail-safe).
- El hook puede ser async y **esperar la UI**: `await ctx.ui.confirm(...)` es el patrón del ejemplo oficial `permission-gate.ts`; hay que comprobar `ctx.hasUI`. Sin timeout propio, la espera es del humano.
- Las llamadas de tools hermanas hacen preflight secuencial y luego corren en paralelo: una pregunta bloqueante retrasa el preflight de sus hermanas (efecto conocido, se documenta).

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| M1 | Rust: `permissionMode: "read" \| "write" \| "yolo"` en `Settings` (default `write`, serde camelCase, retrocompatible), escritura de `<data_dir>/permissions.json` al arrancar y al cambiar (proyección que la extensión lee en cada llamada, así el modo cambia en caliente), registro en `-e` junto a `nabla-ask`, y comando para leer/cambiar el modo. Tests de la decisión pura (qué se bloquea en cada modo). | S1, S2, S5, S6 | medio (2: decide qué se ejecuta) | done | (unidad permission-modes) |
| M2 | Extensión `resources/nabla-permissions.ts`: `pi.on("tool_call")` que lee el modo del archivo y decide — lectura bloquea bash/edit/write con motivo; escritura permite archivos y pregunta por bash con `ctx.ui.confirm` mostrando el comando; yolo permite. Fail-safe: si el archivo no se puede leer, cae a `escritura` (nunca a yolo). | S3, S4, S5 | alto (2) | done | (unidad permission-modes) |
| M3 | UI: chip del modo en la cabecera del chat (clic cicla lectura → escritura → yolo), con el modo actual legible y sin eufemismos en yolo; espejo en contratos; i18n en+es. | S1, S2, S5 | bajo | done | (unidad permission-modes) |
| M4 | Verificación: `cargo test`, `cargo clippy`, `npm run build`; QA manual con los tres modos (bloqueo en lectura, pregunta real en escritura, yolo sin preguntar). | — | — | done (358 tests, clippy 1 preexistente, build verde; QA manual entregada) | — |

## Log

- **L1** Usuario (verbatim): «deberíamos de darle los permisos a nabla pues, no? me explico, en alguna parte ver el modo en el que está, lectura, escritura y yolo. Además de que pregunte cuando está en escritura (yolo es yolo) para hacer las acciones. no?» — confirma el diseño que quedó congelado en `nabla-permissions-modes-plan` y lo adelanta delante de continuidad de turno.
- **L2** Recon (hechos arriba): el mecanismo existe y permite exactamente el diseño; el veto es legible por el modelo. Sin el hook habría que inventar un bus de permisos — no hace falta.
- **L3** Decisiones v1: gate sobre `bash`/`edit`/`write` (los que tocan disco o ejecutan); `read`/`grep`/`find`/`ls` libres siempre; tools del navegador **no** gateadas en v1 (no tocan el workspace; su riesgo es de red, y se decide en su propia feature). Fuera: tiers por directorio, allowlists de comandos, permisos por workspace — v2 cuando duela.
- **L4** El modo se propaga por archivo, no por env: env queda fijo en el spawn y exigiría reiniciar la sesión para cambiar de modo. La extensión lo lee en cada llamada y cae a `escritura` si no puede leerlo: un fallo de lectura nunca debe degradar a yolo.

- **L5** **M1-M4 hechos**: `settings.rs` gana `permission_mode` (default `write`, camelCase, retrocompatible) + `PermissionMode`, `PermissionDecision`, `decide`, `permissions_projection_path` y `write_permission_projection` (atómica tmp+rename); la proyección se escribe al arrancar y en cada cambio, y `NABLA_DATA_DIR` se reenvía al hijo para que la extensión la encuentre. `resources/nabla-permissions.ts` lee el modo **en cada tool call** (cambio en caliente, L4) y decide: lectura bloquea shell+archivos con un motivo que nombra el modo y la cabecera; escritura permite archivos y pregunta por shell mostrando el comando verbatim (`event.input.command`), bloqueando si `ctx.hasUI` es falso; yolo permite. **Fail-safe siempre hacia `write`**: sin `NABLA_DATA_DIR`, fichero ausente, JSON inválido o excepción nunca degradan a yolo, y el `catch` del handler bloquea en vez de permitir. `PermissionModeChip.vue` en la cabecera del chat; claves en+es. Verificación: 358 tests, clippy 1 preexistente, `npm run build` verde. Nota: `resources/*.ts` está fuera de `tsconfig` por diseño (jiti los carga sin compilar), así que los diagnósticos de tipos que emite pi-lens sobre ellos son esperados y no afectan al gate.