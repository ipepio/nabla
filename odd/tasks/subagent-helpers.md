# subagent-helpers — coordinación de agentes visible donde ocurre

Estado: **planificado**, sin rama ni código. Documento de planteamiento para P2 de P-001.
Alcance: cuando un agente delega trabajo en otros agentes, Nabla lo muestra **en el chat que lo lanzó** — estado, hilo narrado, seguir en vivo, parar — en vez de una lista global descontextualizada.

## Specs

- **S1** «4 cerramos lo que hay ahora y planteamos P2» — este documento es el planteamiento, no una implementación. P1 (asistente de escritorio) se cierra con el lote actual.
- **S2** La unidad es el **chat que delega**: los agentes que un turno lanza aparecen con tarjeta propia dentro de esa conversación, no en un panel global separado. Es la idea que la encuesta marcó como la mejor para P2 (gentle-shell-desktop: *Helpers*; gentle-shell: overlay *Agents*), y encaja en el drawer que ya existe.
- **S3** Cada tarjeta muestra: rol/etiqueta, estado (en cola · trabajando · hecho · fallo), tiempo y **sus tools colapsadas** (el patrón que ya usamos por turno), con **Follow live** (ver su hilo mientras corre) y **Stop**.
- **S4** Un subagente que necesita una decisión del padre la pide **como pregunta**, con el puente `ask_*` que ya tenemos: tarjeta en el chat, no un mensaje suelto en un log.
- **S5** Nada de esto entra en P1: si el agente no puede delegar todavía, la feature no tiene sujeto. **La primera tarea es averiguar eso**, no diseñar UI.

## Lo que NO sabemos (recon obligatoria antes de diseñar)

1. **¿pi tiene subagentes nativos?** En las sesiones de Nabla el agente ve `read`, `bash`, `edit`, `write` + MCP; no aparece ninguna tool de delegación. Hay que verificar en la doc de pi si existe un mecanismo nativo (tool `task`/`agent`, `--subagent`, extensión `builtin:*`) o si delegar es **una extensión que habría que construir**.
2. **¿Cómo se observaría?** Si pi lanza hijos, ¿emite eventos RPC por ellos (inicio, salida, tools, fin) o solo el resultado final del padre? De esto depende toda la UI: tarjetas en vivo (eventos) o solo un resumen al terminar.
3. **¿Son sesiones de pi o procesos propios?** Si son sesiones, la topología que ya tenemos (una por conversación) los absorbe; si son otra cosa, hay que decidir el modelo.
4. **Coste/permisos**: un subagente hereda el modo de permiso del padre (lectura/escritura/yolo)? ¿O pide el suyo? Nuestra decisión previa de seguridad dice que la capa se monta **antes** de la autonomía, así que esto se decide aquí.

## Fases propuestas (se afinan con el recon)

| ID | Tarea | Notas |
| --- | --- | --- |
| H0 | Recon: subagentes en pi (existencia, eventos, topología, permisos heredados) | bloquea el diseño; una sesión de explorador |
| H1 | Modelo de datos: subagente ↔ chat que lo lanzó, estado y hilo | con lo que el recon permita observar |
| H2 | Tarjetas de Helpers en el chat (estado, tools colapsadas, Follow, Stop) | reusa el agrupado por turno y el componente de chips |
| H3 | Pregunta hijo→padre por el puente `ask_*` | ya construido; aquí solo se cablea |
| H4 | Permisos heredados y su reflejo en la tarjeta | decisión de producto, no de UI |

## Log

- **L1** Usuario (verbatim): «4 cerramos lo que hay ahora y planteamos P2, vale?».
- **L2** Origen: encuesta de 10 repos. `gentle-shell-desktop` resuelve esto con una pestaña *Helpers* por chat («queued/running/done/failed, hilo narrado, Follow live, Stop, nunca una lista global»); `gentle-shell` con un overlay *Agents* (modelo, tokens, coste, hilo retenido, pregunta del hijo como diálogo). Nada se copia: el patrón es el que Nabla ya usa para turnos y chips.
- **L3** Decisión de orden: antes de esta feature se cierran los pendientes de P1 (QA del lote, T4 screenshot si llega a doler).
- **L4** Honestidad: hoy **no está demostrado que el agente pueda delegar** en Nabla. Construir tarjetas de subagentes sin subagentes sería teatro; por eso H0 va primero y puede reescribir todo lo demás.
