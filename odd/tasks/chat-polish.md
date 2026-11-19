# chat-polish — el pulido barato del chat

Estado: en curso, rama `feat/chat-polish` (apilada sobre `feat/webview-control`).
Alcance: los dos ajustes de conversación que quedan vivos de la fila «Pulido de chat barato» aceptada de la encuesta de repos. UI-only: nada de Rust, ningún contrato nuevo, ninguna dependencia nueva.

## Specs

- **S1** «hacemos primero lo barato? pulidp del chat» — la tanda de pulido aceptada de la encuesta (origen: chat-demo): parse de markdown **acelerado durante el streaming**, razonamiento como flujo plegable con estilo propio, borradores no enviados persistidos, copiar por mensaje.
- **S2** «lo barato» — UI-only: sin Rust, sin cambios de contratos, sin dependencias nuevas. El gate de verificación del frontend es `npm run build` (no existe runner de tests).
- **S3** (alcance verificado contra el código, no recortado a mano) de los cinco puntos aceptados, **tres ya existen** y se verifican sin re-construirse: el razonamiento plegable (`ReasoningBlock.vue`: plegable por defecto, estado por mensaje en `session-store`, preferencia `showReasoning`), copiar por mensaje (`MessageBubble.vue:34`) y por bloque de código (`CodeBlock.vue:26`), y parar (`handleStop` → `stopSession`). «Continuar» pertenece a la feature de continuidad de turno, no a esta. Quedan: **throttle del parse en streaming** y **borradores persistidos por sesión**.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| P1 | Throttle del parse de `segmentsOf` durante el streaming: el texto vivo se re-parsea como mucho cada ~120 ms; el resultado final es byte-idéntico al de hoy y el commit del turno no pinta una cola obsoleta | S1, S2, S3 | pending | |
| P2 | Borradores no enviados por sesión, persistidos en `localStorage` (primer uso del repo: clave `nabla.draft.*`), restaurados al cambiar/abrir sesión, guardados con debounce, borrados al enviar o encolar | S1, S2, S3 | pending | |
| P3 | Verificación: `npm run build` + QA manual del humano | S2 | pending | — |

## Log

- **L1** Usuario (verbatim): «hacemos primero lo barato? pulidp del chat. Después vemos como continuamos. me parece bien el orden que dices de puente, auto-open y continuidad. haz el pulido y empezamos el ODD».
- **L2** Origen de la tanda: encuesta de inspiración sobre 10 repos (nan-desktop, Stark pi-desktop, dsh, chat-demo, PI-Desktop vastsa, phosphor, gentle-shell, Gentleman.Dots, gentle-ai, gentle-shell-desktop). Nada se copia: patrones y decisiones de producto. La fila aceptada era «Parse de markdown acelerado durante el streaming, razonamiento como flujo plegable con estilo propio, borradores no enviados persistidos, copiar por mensaje, parar/continuar».
- **L3** Orden acordado para después de esta feature: **puente de preguntas del agente → auto-open del panel → continuidad de turno**.
- **L4** Recon (paths verificados): el render de texto no es una librería markdown — `AssistantText.vue:11` computa `segmentsOf()` de `src/components/chat/code-segments.ts` y la computada re-evalúa **por cada delta** (`session-store.ts:685` `streamingText += delta`), sin throttle en todo el camino del chat. No hay `localStorage` en el repo (P2 lo estrena). `ChatPanel.vue` no usa i18n (strings del propio archivo). `message.id` es la clave estable del transcript; la sesión se direcciona por `sessionId` string.
