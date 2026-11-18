# webview-control — el agente conduce la vista embebida

Estado: abierta, sin empezar. Rama: `feat/webview-control` (apilada sobre `feat/pi-runtime`).
Alcance: un servidor **MCP** dentro de Tauri cuyas tools envuelven los comandos `embed_*`, y el
registro automático de ese servidor para que cada sesión de pi lo traiga puesto.

## Specs

- **S1** «Ahora me gustaría darle control total sobre el webview de tauri, como lo hacemos? tiro de una sesión nueva?» — el agente debe poder **conducir** la vista embebida que hoy sólo conduce el humano. Es feature nueva de código, no un cambio de sesión.
- **S2** «1 mcp pues» — la vía es **MCP**: Tauri aloja un servidor *streamable HTTP* y pi lo consume como un servidor más. Descartada la extensión de pi con puente propio (artefacto custom, y hay que re-encender extensiones en el spawn).
- **S3** «2 up to you, pero no se para que» — screenshot queda **diferido** (ver T4 y L3). Primero DOM/texto. La decisión es del orquestador, no del humano.
- **S4** «3 dale, el auto» — **registro automático**: Nabla escribe el servidor en el `mcp.json` que lee pi al arrancar, así ninguna sesión necesita configuración manual.
- **S5** Nada de protocolo nuevo: las tools envuelven los comandos `embed_*` que ya existen (`embed_create`, `embed_set_bounds`, `embed_navigate`, `embed_eval`, `embed_url`, `embed_close`, `embed_close_all`). Reutiliza F0 (sección MCP de Ajustes) y `pi mcp add/remove`.
- **S6** Seguridad explícita: el servidor escucha sólo en `127.0.0.1` y exige un **token por ejecución** en la URL o en una cabecera. `eval` ejecuta lo que se le pasa en una vista que carga páginas con las cookies del humano: sin token, cualquier proceso local tendría esa capacidad.

## Decisión pendiente (bloquea T1)

**Transporte HTTP: dependencia o mano.** Tauri ya trae un runtime async, pero no un servidor HTTP.
Opciones:

- **(a) `axum` + `tokio`** — MCP streamable HTTP real (POST + SSE opcional), ruteo y extracción de
  cuerpos resueltos. Coste: dos dependencias nuevas y su árbol.
- **(b) servidor HTTP/1.1 mínimo a mano sobre `std::net::TcpListener`** — cero dependencias, pero
  hay que parsear cabeceras, `Content-Length`, `Transfer-Encoding` y mantenerlo. MCP sólo necesita
  POST con JSON y respuestas JSON, así que es viable.

Recomendación: **(a)**, y sólo `axum` (que ya arrastra `tokio`/`hyper`) sobre el runtime existente.
El argumento de "nada de dependencias nuevas" de `tool-drawer-and-mcp` obliga a justificar: parsear
HTTP a mano es donde se cuelan los bugs de un puente que ejecuta JavaScript, y `axum` es la vía
estándar. Se decide antes de escribir la primera línea de T1.

## Tasks

| ID | Tarea | Specs | Riesgo | Estado | Commit |
| --- | --- | --- | --- | --- | --- |
| T1 | Servidor MCP mínimo: JSON-RPC 2.0 sobre POST local, `initialize`, `tools/list`, `tools/call`; tools `navigate` y `eval`. Test-first del despachador puro (sin red) + smoke HTTP en puerto efímero. | S1, S2, S5, S6 | alto (2) | done | (unidad webview-control) |
| T2 | Resto del catálogo DOM: `get_url`, `get_title`, `click`, `type`, `back`, `forward`, `reload`, `list_targets`. Cada tool envuelve su `embed_*`; alta de la vista si no existe. | S1, S5 | medio | pending | — |
| T3 | Registro automático: Nabla escribe la entrada `nabla-browser` en el `mcp.json` de pi con la URL y el token de la ejecución; `pi mcp remove` al cerrar. | S4 | medio | done | (unidad webview-control) |
| T4 | Screenshot: **investigar antes de prometer**. Tauri 2.12.5 no expone captura de webview; requiere captura de ventana del SO (ScreenCaptureKit en macOS) o `html2canvas` por `eval`. Sólo si T1-T3 demuestran que el DOM no basta. | S3 | alto | pending | — |
| T5 | Verificación independiente: ciclo completo pi→MCP→`embed_*`→webview con pi real, aislamiento del puerto, token obligatorio, y que sin el servidor la app funciona igual. | S6 | alto | pending | — |

## Log

- **L1** Usuario (18 nov 08:47): «Ahora me gustaría darle control total sobre el webview de tauri, como lo hacemos? tiro de una sesión nueva?» — respuesta dada entonces: servidor MCP dentro de Tauri envolviendo `embed_*`, con tres decisiones abiertas. La sesión se cortó ahí (corte de luz), sin código.
- **L2** Usuario (18 nov): «1 mcp pues, 2 up to you, pero no se para que, 3 dale, el auto». Las tres quedan cerradas: MCP, screenshot diferido, registro automático. La petición original no era crear una sesión nueva: es una feature con su rama y su ODD.
- **L3** *Para qué sirve screenshot* (contexto de la decisión diferida): es la única forma de que el agente **vea** la página renderizada — layout roto, imágenes, `canvas`/WebGL, un modal que tapa un botón, si un formulario "parece" enviado. `eval` lee el DOM, que es el árbol y no el dibujo: no distingue "el elemento existe y está oculto" de "el usuario lo ve", ni lee texto dentro de un `canvas`. Se difiere porque: (1) Tauri 2.12.5 **no tiene** API de captura de webview — verificado en el crate `tauri-2.12.5`, donde lo único que menciona captura es `contentProtected`; (2) capturar exige una vía externa (ScreenCaptureKit del SO, que necesita permisos, o `html2canvas` inyectado, que es un parche que miente con `canvas` y CORS); (3) el transporte de imágenes por MCP (bloques base64) hay que validarlo; (4) no bloquea el ciclo navigate→eval→click. Vuelve cuando T1-T3 estén verdes y un caso real demuestre que el DOM no basta.
- **L4** Contexto de la vista que se va a conducir: el navegador **embebido** entró en `f18d160` y se borró el path externo por CDP (`src-tauri/src/browser.rs`), invirtiendo la decisión L3 de `tool-drawer-and-mcp`. Esa doc quedó corregida en su F3/S4/S6 y apunta aquí.
- **L5** Usuario: «dale con axum» — cerrada la decisión de transporte: `axum` + `tokio`, y `getrandom` para el token. Los tres están ya en el árbol por tauri, así que se añade un pin de versión, no un grafo nuevo. **T1 hecho** en `src-tauri/src/mcp_server.rs`: despachador puro (JSON-RPC; `initialize`/`ping`/`tools/list`/`tools/call`; notificaciones sin cuerpo; errores `-32601`/`-32602`; fallo de tool como `isError`), binding streamable HTTP con `Mcp-Session-Id`, puerto efímero en `127.0.0.1` y token por ejecución en cabecera `Authorization: Bearer` **o** query `?token=` (para el registro por URL de T3). El `EmbedDriver` adapta `embed::EmbedSessions`, y `embed::views()` dejó de ser `#[cfg(test)]` porque el servidor necesita listar las vistas. 12 tests nuevos, 297 en total, clippy sin warnings nuevos. Verificado el ciclo real por socket: 401 sin token, 200 con el catálogo, 405 en GET.
- **L6** **T3 hecho.** El registro va por `pi mcp add` (el camino canónico, no escribir el JSON a mano) con scope **global**: el scope de proyecto escribiría `.pi/mcp.json` dentro del repositorio del humano, y esta entrada pertenece a la app. El token viaja en la cabecera `Authorization: Bearer`, así la url que queda en el archivo — y la que muestra `pi mcp list` — no lleva secreto. Reparación idempotente `remove`→`add` al arrancar: una entrada dejada por un cierre sucio apunta a un puerto muerto y se sustituye sola. El alta corre en un hilo aparte (arranca el CLI de pi; ni un binario lento ni uno ausente pueden retrasar el pintado de la ventana) y la baja es un `pi mcp remove` corto y sincrónico en `RunEvent::Exit`, porque un hilo no sobrevive a la salida. Todo best-effort: sin registro la app funciona igual, sólo sin control del navegador por parte del agente. **Consecuencia asumida**: una entrada `nabla-browser` que el humano hubiera escrito a mano se sobrescribe y se borra al salir — el nombre es de la app.
