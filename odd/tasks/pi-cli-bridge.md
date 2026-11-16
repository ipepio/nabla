# pi-cli-bridge

Estado: en curso, rama `feat/pi-runtime`.
Alcance: puente genérico y seguro hacia la CLI de `pi`, y sobre él la sección de cuentas (login con suscripción, comprobación y logout) en la pestaña Providers.

## Specs

- **S1** «al final tenemos que poder ejecutar comandos de pi también, así que monta el bridge hacía la cli de pi» — un puente reutilizable que ejecute `pi <args>` como **proceso hijo con argv tipado**, nunca a través de un shell, y que sirva para las próximas funciones (auth, config, modelos).
- **S2** Seguridad: **allowlist en Rust** del primer argumento (`auth`, `models`, `config`, `--version`, `--help`); nada de `pi install`, ejecución de paquetes ni interpolación de shell. Timeout obligatorio en las llamadas no interactivas y salida acotada.
- **S3** Login con suscripción de pi: «yo digo el /login con suscripción» — flujo OAuth por **device code** (`--no-browser`): el puente arranca `pi auth login <provider> --no-browser`, la UI muestra la salida en vivo (URL + código) y permite pegar el código de retorno al stdin del proceso.
- **S4** Estado y cierre de sesión: `pi auth check --provider <p> --json` por provider (badge) y `pi auth logout <p>` con confirmación. Las credenciales viven en `auth.json`, **no** las tocamos nosotros.
- **S5** La sección se muestra con `VITE_ENABLE_PROVIDER_LOGIN` (reintroducida, ON por defecto con kill switch, igual que `VITE_ENABLE_CUSTOM_PROVIDERS`).

## Contrato congelado (Rust ↔ TS)

```rust
// No interactivo
pi_cli_run(args: Vec<String>, timeoutMs: Option<u64>) -> PiCliResult
PiCliResult { stdout: String, stderr: String, exitCode: i32 | null, timedOut: bool, truncated: bool }
// Allowlist: args[0] ∈ {"auth","models","config","--version","--help"}; timeout por defecto 20s; salida acotada (64 KiB por stream).

// Interactivo (login OAuth)
pi_cli_start(args: Vec<String>) -> String           // devuelve cliSessionId; mismo allowlist; sin timeout
pi_cli_write(cliSessionId: String, text: String) -> bool
pi_cli_stop(cliSessionId: String) -> bool
// Evento Tauri "pi-cli": { cliSessionId, stream: "stdout"|"stderr"|"exit", line?: string, exitCode?: number }

// Atajo tipado para el estado de cuenta
pi_auth_check(provider: String) -> PiAuthStatus
PiAuthStatus { provider: String, state: "ready"|"not_ready"|"invalid"|"unknown", credential: bool }
```

- `pi_auth_check` parsea el JSON de `pi auth check --json`; si el JSON no se entiende, `state = "unknown"` (nunca panic).
- El binario se resuelve como hoy (`settings.pi_binary`), con el mismo allowlist de entorno que usa el runtime.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| C1 | Rust: `pi_cli_run` (allowlist, deadline + kill, 64 KiB por stream) + `pi_auth_check` + 16 tests | S1, S2, S4 | done | 2cdd5a7 |
| C2 | Rust: `pi_cli_start/write/stop` con hilos, evento `pi-cli`, registry y script fake | S1, S3 | done | 2cdd5a7 |
| C3 | TS: contratos + `subscribePiCli`; `AccountsSection.vue` (badge, sign in con salida en vivo y código pegado, sign out) gateada por flag | S3, S4, S5 | done | (unidad accounts) |
| C4 | Verificación: 120/120 cargo, clippy 0, build OK, i18n 158/158, seam limpia | — | done | (esta unidad) |

## Notas de seguridad

- argv tipado: no hay superficie de inyección de shell; el riesgo restante es *qué* subcomando se permite, de ahí el allowlist.
- `pi auth print-api-key` / `print-bearer-token` **no** se exponen a la UI: devolverían secretos al webview.
- Nada de `pi install`/`update`: ejecutan código de terceros.

## Log

- **L1** Investigación previa: `pi auth login|logout|check|print-api-key|print-bearer-token`; `auth.json` guarda OAuth (`access`, `refresh`, `expires`, `clientId`, `scopes`) y keys simples; `--no-browser` da device code, la ruta portable en escritorio.
- **L2** Usuario aprueba el puente a la CLI y pide que sirva para ejecutar comandos de pi en general.
- **L3** Decisión: allowlist + argv, sección de cuentas por device code; `print-*` excluido para no filtrar secretos al webview.

- **L4** Verificación orchestrator: `cargo test` 120/120 (16 nuevos), `clippy` 0; cinco comandos registrados; `npm run build` OK; `en`/`es` 158 claves con paridad; ninguna importación de APIs de Tauri fuera de `src/contracts`.
- **L5** Deuda consciente: login por device code probado sólo con el script fake; el flujo real con `pi auth login --no-browser` se valida en el smoke del usuario.
- **L6** Smoke del usuario reveló el defecto real: `pi auth login` exige TTY y con pipes responde `requires an interactive terminal` (exit 1), sin URL. Verificado a mano con `script -q /dev/null`: con TTY imprime `Open this URL to sign in: <url>`, avisa del callback en `http://127.0.0.1:1455/auth/callback` y espera input en el prompt. Conclusión: las sesiones interactivas del bridge deben usar **PTY** (no pipes), limpiar códigos ANSI y la UI debe extraer la URL del stream y abrirla en el navegador (fallback: pegar la URL final).
