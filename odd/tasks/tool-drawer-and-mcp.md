# tool-drawer-and-mcp

Estado: definición (sin implementar). Rama prevista: `feat/tool-drawer`.
Alcance: convertir el panel derecho en un área de herramientas con pestañas, empezando por la configuración de MCP porque habilita las demás.

## Specs

- **S1** «primero que nada tenemos que darle a pi, la posibilidad de configurar mcps» — Ajustes gana una sección **MCP** que lista, añade, elimina, hace login/logout y muestra estado de servidores, sobre lo que pi ya sabe hacer.
- **S2** «en ese drawer quiero poner un terminal, un explorador de archivos con previsualización … un navegador … todo esto irá por pestañas dentro de ese drawer» — el panel derecho aloja **pestañas**: Files, Changes, Terminal, Browser.
- **S3** «el explorador de archivos … que cuando lo selecciones, te lo abre en la misma pestaña» — árbol del workspace; al elegir un archivo se previsualiza en la propia pestaña (no en otra vista).
- **S4** «el navegador, que el agente lo pueda utilizar» — la pestaña Browser muestra estado/capturas y el **agente** navega; el humano no conduce un navegador embebido.
- **S5** «estas herramientas podríamos activarlas en la parte de configuración» — cada herramienta se habilita/deshabilita en Ajustes y el estado persiste (`enabledTools`), con el mismo criterio que los flags actuales.
- **S6** «¿Qué pasa si se instala en un navegador que no es browserOS?» — el navegador objetivo es cualquier **Chromium con CDP** (Chrome, Edge, Brave, Chromium, BrowserOS). Safari/Firefox → pestaña deshabilitada con motivo visible. Alternativa soportada: registrar `chrome-devtools-mcp` como **servidor MCP**.

## Hechos verificados (no re-investigar)

- `pi mcp add <server> [--opciones] -- <cmd>` / `--url <url>`, `remove`, `list --json`, `login`, `logout`, con `-l` para `.pi/mcp.json`. Archivos: `~/.pi/agent/mcp.json` (global) y `.pi/mcp.json` (proyecto de confianza).
- El bridge de Nabla (`src-tauri/src/cli.rs`) tiene allowlist por subcomando; hoy incluye `auth`, `models`, `config`, `--version`, `--help`. **`mcp` debe añadirse**; `login` necesita PTY (ya implementado para `auth`).
- gentle-shell **no** da navegador al agente: abre el navegador del sistema para su propia UI y su único CDP es un script de capturas del TUI.
- pi.dev/packages: `pi-browser-harness` (CDP a Chrome real: navigate/click/type/screenshot/extract), `pi-browser-use` (envuelve chrome-devtools-mcp), `pi-smart-terminal` (PTY persistente + tools `terminal_*`).
- Nabla ya tiene `portable-pty` (entró para el login de pi): el terminal del usuario reutiliza ese primitivo.

## Fases (alcance cerrado)

### F0 — Configuración de MCP en Ajustes  · riesgo medio
Objetivo: que pi pueda usar MCPs configurados desde Nabla, sin editar archivos a mano.

- **Bridge**: allowlist += `mcp`; reutiliza la sesión PTY para `mcp login`.
- **Rust**: `mcp_list()` tipado desde `pi mcp list --json` (estado, tools, errores); `mcp_add(draft)` (stdio `-- command args`, HTTP `--url` + `--bearer-token-env-var`, headers `${ENV}`/`!cmd`); `mcp_remove(name, scope)`; `mcp_login(name)` / `mcp_logout(name)`; `mcp_file_path(scope)`.
- **UI**: sección MCP — lista con badge (`ready` / `needs sign-in` / `error` / deshabilitado), acciones Sign in/out y Remove con confirmación, alta manual, **catálogo curado en repo** (JSON versionado) con verificación ("arranca y expone N tools"), y dos ayudas: ver el JSON (sólo lectura) y abrir el archivo.
- **Fuera**: instalar paquetes arbitrarios; editor libre del JSON; gestión de `oauth.clientId/clientSecret` (queda en edición manual).
- **Verificación**: tests de parseo con fixtures de `list --json` (incluye el caso "necesita login" y el error); smoke con el MCP local ya configurado (`engram`) y con un HTTP público.
- **Depende de**: nada. **Habilita**: navegador vía MCP sin escribir extensión.

### F1 — Host de pestañas + Files + Changes  · riesgo bajo
Objetivo: convertir el panel derecho vacío en el área de herramientas.

- **Host**: registro de herramientas `{id, label, icon, flag, component}`; pestañas; estado vacío; recordar la pestaña activa en la sesión.
- **Rust**: `fs_list_dir(workspace, rel)` y `fs_read_preview(workspace, rel)` con confinamiento al workspace (rechaza `..` y symlinks que escapen), límite de tamaño y detección binaria; `git_status(workspace)` y `git_diff(workspace, rel)` (contra `HEAD`, untracked incluidos) usando el CLI de git.
- **UI Files**: árbol perezoso, iconos por tipo, clic → preview en la **misma** pestaña (texto, markdown, imagen); ruta y tamaño; estados de error legibles.
- **UI Changes**: lista por archivo con conteo de líneas, diff perezoso, refresco manual y al recuperar foco.
- **Fuera**: editar/guardar archivos, staging, commit, agregación multi-worktree (queda como F4).
- **Verificación**: tests Rust de confinamiento (intentos `..`, symlink externo, archivo enorme, binario) y de parseo de `git status --porcelain`; build del frontend.

### F2 — Terminal  · riesgo alto (opt-in)
Objetivo: una terminal real en el workspace.

- **Rust**: sesión PTY (ya disponible) con `cwd = workspace`, ciclo de vida por pestaña, kill al cerrar; eventos por canal propio.
- **UI**: emulador real (xterm.js) con resize, scrollback y copiar/pegar; estado "sesión terminada" con botón de reinicio.
- **Seguridad**: apagada por defecto; al activarla, aviso explícito de que ejecuta comandos arbitrarios; no se expone al agente (pi ya tiene su tool de shell).
- **Fuera**: allowlist de comandos (teatro de seguridad), multiplexores, perfiles de shell.
- **Verificación**: test Rust de arranque/cierre del PTY con un comando falso; smoke manual con `pwd`/`ls`.

### F3 — Browser  · riesgo medio (opt-in)
Objetivo: que el agente navegue y el humano vea qué pasa.

- **Decisión**: **lanzar** una instancia Chromium dedicada (perfil propio, `--remote-debugging-port`) en vez de adjuntar a la del usuario.
- **Detección**: buscar Chromium/Chrome/Edge/Brave/BrowserOS; si sólo hay Safari/Firefox, la pestaña queda **deshabilitada con motivo** y se ofrece la alternativa MCP (`chrome-devtools-mcp` desde F0).
- **Rutas**: (a) extensión `pi-browser-harness` (CDP, tools navigate/click/type/screenshot/extract); (b) MCP `chrome-devtools-mcp`. Nabla muestra estado, URL actual y capturas; **no** conduce el navegador.
- **Fuera**: navegador embebido en el drawer; Safari/Firefox.
- **Verificación**: detección y arranque en macOS; captura de pantalla en la pestaña; una navegación real desde el agente.

### F4 — Después (no en esta tanda)
Pestaña Review (candidato congelado), tarjetas de subagentes, medidor de contexto/coste por sesión, Changes con worktrees de subagentes.

## Reglas transversales

- Cada fase: ODD doc con `S#` citando al usuario, tasks con superficies disjuntas, verificación independiente, commits por unidad.
- Flags nuevos en `enabledTools` con defaults: `files` y `changes` **on**; `terminal` y `browser` **off**.
- Nada de dependencias nuevas salvo justificación explícita (xterm.js en F2; `git2` sólo si el CLI no basta en F1).

## Decisiones pendientes de confirmar

1. **Alcance de F0**: ¿sólo `list/add/remove` + login/logout, o también editar el JSON a mano desde la UI? (El editor da poder y riesgo de corromper el archivo; con backup mitigado.)
2. **Terminal**: ¿shell del sistema en el workspace (libre) o con allowlist de comandos? La primera es útil y peligrosa; la segunda es teatro de seguridad si el agente ya escribe archivos.
3. **Browser**: ¿adjuntar a un Chromium ya abierto (requiere que el usuario lo lance con el puerto) o **lanzar** nosotros una instancia dedicada con perfil propio? Lanzar es más fiable y aísla cookies; adjuntar respeta su sesión.
4. **Flags**: nombres y defaults de `enabledTools` (`files`/`changes` on; `terminal`/`browser` off).

## Log

- **L1** Usuario: MCP primero; drawer con pestañas (terminal, files con preview, browser usable por el agente); herramientas activables en Ajustes; dudas sobre navegadores no-BrowserOS y sobre cómo lo hace el harness de DeepSeek.
- **L2** Hallazgos: `pi mcp` cubre el ciclo completo y Navla sólo necesita UI + allowlist + PTY; dsh trata MCP como superficie de capacidades (cua-mcp, puente ACP); el navegador puede ser un MCP, no una extensión propia.
- **L3** Decisión propuesta: navegador = Chromium + CDP, con pestaña deshabilitada y motivo explícito si no hay ninguno; alternativa chrome-devtools-mcp desde la sección MCP.

## Catálogo de MCPs (decisión propuesta)

- **Sí a un catálogo curado de plantillas**, no a un instalador libre de paquetes.
- Una entrada del catálogo es **datos, no código**: `{ id, label, description, transport: "stdio" | "http", command?, url?, requiredKeys[] }`. Añadir un servidor sigue siendo `pi mcp add` con un argv/URL conocidos; el usuario ve **qué se va a ejecutar** antes de aceptarlo.
- Flujo: elegir plantilla → campos requeridos (API key/token) → **Verificar** (arranca y consulta `pi mcp list --json` para ver estado, tools y errores) → guardar.
- Mismo patrón que los **presets de provider** ya implementados: prellenar, no instalar a ciegas.
- **Fuera**: instalar paquetes arbitrarios desde npm/GitHub por URL. Es superficie de cadena de suministro, y por eso `pi install` ya está fuera de la allowlist del bridge.
- Las credenciales de MCP viven en `mcp.json` (texto plano), igual que las de provider: coherencia explícita, deuda anotada hacia el keychain del SO.

## MCP con OAuth (verificado en la doc de pi)

- Un servidor HTTP con OAuth **no lleva credenciales en `mcp.json`**: pi registra el cliente (DCR o Client ID Metadata Document), abre la página de autorización, guarda los tokens en **`~/.pi/agent/mcp-auth.json`** y los refresca cuando expiran o el servidor los rechaza. Un scope nuevo ⇒ pide login otra vez. Logout borra la credencial.
- Flujo de shell: `pi mcp login <server> [--timeout N]`, `pi mcp logout <server>`; `/mcp` (o `pi mcp list --json`) expone el estado, que incluye "necesita iniciar sesión".
- Si el navegador no está en la misma máquina (SSH), pi permite **pegar la URL de retorno** en la pantalla de login.
- Las credenciales pertenecen al **par nombre+URL**: dos entradas con la misma URL y distinto nombre inician sesión por separado; mismo nombre y URL comparten sesión entre archivos.
- Servidores sin registro dinámico: `oauth: { clientId, clientSecret, callbackPort }` en `mcp.json` (sección avanzada; puede quedarse en edición manual).
- OAuth sólo aplica a HTTP **sin** cabecera `Authorization`. Alternativa sin secretos en disco: `headers` con `${ENV}` o un comando `!command` (p. ej. `!echo Bearer $(gh auth token)`); y `pi mcp add ... --bearer-token-env-var VAR`.
- Ojo: en la prueba, un servidor inexistente devolvió **exit 0** ⇒ no confiar sólo en el código de salida; el estado se lee de `list --json`.
- Para Nabla: el login MCP va por el **mismo camino que el de provider** (sesión PTY + extracción de URL + campo para pegar el retorno), y el badge sale de `list --json`.
