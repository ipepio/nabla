# auto-open-pane — el agente abre el panel del navegador que necesita

Estado: en curso, rama `feat/auto-open-pane` (apilada sobre `feat/agent-question-bridge`).
Alcance: cuando una tool de `nabla-browser` necesita una vista y no hay ninguna abierta, la app abre el panel del navegador ella misma — ida y vuelta a la UI, que es quien conoce el rectángulo real — en vez de responder «pide al humano que abra la pestaña». Reabre de forma correcta la decisión que T2 de `webview-control` dejó fuera (no auto-crear vistas) sin violar su lección: **Rust nunca inventa geometría**.

## Specs

- **S1** «me parece bien el orden que dies de puente, auto-open y continuidad» + «dale con el ODD» — la tool que necesita una vista, sin ninguna abierta, consigue que el panel se abra. Nada más se abre solo (heurística nan-desktop: solo lo que el turno en curso necesita).
- **S2** (decisión de diseño, revisable en QA): **abrir y enfocar** — el drawer se abre si estaba cerrado, la pestaña browser se crea si no existe y se activa si existía inactiva. El humano ve lo que el agente hace; es la mitad del valor del navegador embebido. Resuelve la pregunta A/B/C que quedó sin respuesta.
- **S3** La geometría sigue siendo de la UI (lección de T2): Rust emite un evento y espera; Vue crea la vista con su rect real por el flujo que ya existe. El agente jamás gana creación de vistas con geometría libre.
- **S4** Espera acotada (`VIEW_WAIT`, ~10 s). Si no aparece vista: el error honesto de hoy, con un mensaje que nombra las dos causas posibles (panel cerrado por el humano o tool desactivada). Si el humano cierra el panel/vista **durante** la espera, la espera aborta con ese mismo error: el «no» del humano es un no (respect-close de nan-desktop).
- **S5** `enabledTools` manda: si el browser está desactivado en Ajustes (default OFF), el ensure-view **no lo fuerza** — no pasa nada y la espera acaba en el error honesto. Nunca se activa una tool por la puerta de atrás.
- **S6** Las 10 tools no cambian cuando ya hay vista; `list_targets` sigue respondiendo sin vista (lista vacía = respuesta válida).

## Hechos verificados (recon, no re-investigar)

- Drawer: estado módulo en `src/components/tools/dock.ts` — `tabs` :76, `activeTabId` :79, `addTab` :196, `activateTab` :211, `setTabTool` :275. **`toolsOpen` es un ref local de `App.vue:21`** (gate `v-if` :60): el listener del evento DEBE vivir en App.vue, es el único sitio que puede abrir el drawer.
- BrowserTool **solo construye su vista con la pestaña activa** (`active` prop, BrowserTool.vue:76): asegurar la pestaña exige `activateTab`, no basta con que exista. Flujo de creación: `onMounted → start() → createView()` (:540/:519) → `embedCreate` con el rect de `bodyEl.getBoundingClientRect()` (:238), bounds throttled 30 ms (:270).
- `enabledTools`: browser **OFF por defecto** (`DEFAULT_ENABLED_TOOLS = ["files","changes"]`, preferences.ts:58).
- Eventos Rust→Vue: wrappers `listen` exportados en `src/contracts/commands.ts` (patrón «the listener lives here», `onAgentEvent` :704). Precedente de emisión Rust: `embed-nav` desde embed.rs. El `EmbedDriver` de mcp_server ya retiene `tauri::AppHandle` → puede emitir.
- embed.rs: `reserve()` registra la vista **antes** de que el webview exista (momento de notificar a un waiter); `close`/`close_all` la quitan (momento de abortar la espera). `EmbedSessions` ya es `Arc` compartido con el driver.
- mcp_server: `call_tool` es síncrono y corre en el hilo del runtime propio del servidor; una espera acotada ahí no toca el runtime de Tauri.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| A1 | Rust: `wait_for_first_view(timeout)` en `EmbedSessions` (Condvar notificado en `reserve` y en `close`/`close_all` para abortar); método `ensure_view` en `WebviewDriver` que emite `embed-need-view` y espera acotada; las tools que necesitan vista (las 9, no `list_targets`) lo llaman cuando `views()` está vacío; mensaje de error honesto ampliado (S4). Test-first con fakes (RED). | S1, S3, S4, S5, S6 | medio | done | (unidad auto-open) |
| A2 | TS: `onEmbedNeedView` en contracts (patrón existente); App.vue escucha → `toolsOpen = true`, busca pestaña con hoja browser → `activateTab`, o `addTab` + `setTabTool(browser)` → el mount activo crea la vista con su rect (flujo existente, cero cambios en BrowserTool). | S1, S2, S3 | bajo | done | (unidad auto-open) |
| A3 | Verificación: `cargo test` + `cargo clippy` + `npm run build`; QA manual (pedir navegación con el drawer cerrado y el browser desactivado/enabled). | — | — | done (340 tests, clippy 1 preexistente, build verde; QA manual entregada) | — |

## Log

- **L1** Usuario (verbatim): «me parece bien el orden que dices de puente, auto-open y continuidad» (aprobación de la cola) y «dale con el ODD» (arranque de esta feature).
- **L2** Origen: encuesta (nan-desktop `autoOpen.ts`: solo lo que el turno en curso necesita + respect-close). Adaptado: aquí no hay deliverables que abrir, lo que se abre es la superficie que la **tool** necesita.
- **L3** La pregunta A/B/C (abrir+enfocar / abrir silencioso / exigir humano) quedó sin respuesta explícita del usuario; se aplica la recomendación A (abrir+enfocar), ya hecha dos veces sin objeción. Revisable en QA con cambio menor.
- **L4** Recon (hechos arriba, con paths). Dato que define el diseño: `toolsOpen` es local de App.vue — el listener vive ahí o no existe.
- **L5** La lección de T2 (`webview-control` L7) se respeta por construcción: Rust pide, la UI crea con su rect. La reversión es de la *decisión* (no auto-crear), no del *principio* (geometría de la UI).

- **L6** **A1-A3 hechos** (unidad auto-open): `EmbedSessions::wait_for_first_view` (Condvar + contador de cierres; notifica `reserve`, y `forget`/`drain` SIEMPRE — para una tool que espera, «esta vista no está» y «no hay nada» son el mismo no); `WebviewDriver::ensure_view` requerido (sin default — los fakes siguen honestos); `EmbedDriver` emite `embed-need-view` con el AppHandle que ya retenía y espera; `call_tool` asegura vista sólo cuando `views()` está vacío, `list_targets` nunca espera; `NO_VIEW_ERROR` nombra las dos causas (panel cerrado / tool desactivada) y manda preguntar al humano. App.vue: `subscribeEmbedNeedView` → `openBrowserPane()` (abre drawer, activa pestaña browser existente o la crea; BrowserTool intacto — su mount activo ya crea la vista con el rect real). 340 tests (10 nuevos), clippy 1 preexistente, `npm run build` verde.