# chat-preferences-and-queue

Estado: en curso, rama `feat/pi-runtime`.
Alcance: una sola acción por fila de cuenta, toggle "Mostrar razonamiento" y toggle "Encolar mensajes".

## Specs

- **S1** «si está signed in no se puede hacer sign in y al revés, si está not signed, no se puede hacer sign out, ahí una sola opción sobra» — cada fila muestra **solo** la acción aplicable: `ready` → *Sign out*; cualquier otro estado → *Sign in*.
- **S2** «en General, en la parte de Apariencia, añadir una opción que sea un toggle, activada por defecto, que sea "Mostrar razonamiento". Si está desactivado, que no se muestre el razonamiento» — preferencia persistida (`showReasoning`, default `true`); apagada, el bloque de razonamiento no se renderiza (ni en vivo ni en el historial).
- **S3** «otra opción que sea un toggle … "Encolar mensajes" de forma que cuando el agente está trabajando, se puedan enviar mensajes y mandarlos a la cola, al igual que se puede hacer con pi» — preferencia persistida (`queueMessages`, default `true`): con el agente trabajando, el composer acepta el envío y el mensaje queda **en cola** por sesión; al terminar el turno se envía el siguiente. Con el toggle apagado, el comportamiento es el de hoy (el envío se bloquea mientras trabaja).

## Contrato congelado

```rust
Settings += show_reasoning: bool  (serde default true, camelCase showReasoning)
Settings += queue_messages: bool  (serde default true, camelCase queueMessages)
```

- UI: `src/lib/preferences.ts` (refs + carga desde settings + setters que persisten) para que Ajustes y Chat compartan estado sin duplicarlo.
- Cola: por sesión, en `session-store`; se vacía en `turn_end`/`agent_end`.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| Q1 | Rust: dos campos en `Settings` (defaults retrocompatibles) + tests | S2, S3 | pending | |
| Q2 | TS: una sola acción por cuenta según estado | S1 | pending | |
| Q3 | TS: `preferences.ts` + toggles en General (Apariencia) + ocultar razonamiento | S2 | pending | |
| Q4 | TS: cola de mensajes por sesión en el composer + drenaje en turn_end | S3 | pending | |
| Q5 | Verificación + commit | — | pending | |

## Log

- **L1** Usuario pide las tres cosas con captura de las cuentas.
- **L2** Decisiones: acciones derivadas del estado; ambas preferencias persistidas en `settings.json`; la cola se activa por defecto (como pi) y el toggle permite volver al comportamiento bloqueante.
- **L3** **Defecto reportado por el humano**: «ahora parece que no se encolan los mensajes» — pulsar Enter con el agente trabajando no hacía nada. Causa: `handleSend` empezaba con `if (text === "" || agentWorking.value) return`, así que descartaba el prompt antes de mirar la preferencia; sólo el botón explícito de encolar llegaba a `handleQueue`. Incumplía S3, que pide que el composer **acepte** el envío y lo mande a la cola con el toggle activado. Arreglo: con `agentWorking` y `queueMessages` activo, `handleSend` llama a `handleQueue` (la misma cola, mismo drenaje); con el toggle apagado se conserva el comportamiento bloqueante. Sin runner de tests en el frontend (`npm run build` es el único gate), así que la verificación es typecheck + build.
