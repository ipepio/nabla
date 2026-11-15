# providers-pane

Estado: en curso, rama `feat/pi-runtime`.
Alcance: pestaña **Providers** en Ajustes (sustituye a Models): lista de providers reales de pi y alta de un provider OpenAI-compatible con descubrimiento de modelos.

## Specs

- **S1** «en lugar de models, que sea "providers"» — la pestaña se llama Providers y muestra los providers que pi conoce, marcando cuál está activo (`settings.provider`).
- **S2** «configurar uno openai compatible … pondremos url, api-key y tendremos la opción de "descubrir modelos" si es así, llamará al endpoint de descubrir modelos y se podrán configurar» — formulario con id, nombre, baseUrl, apiKey; botón *Discover models* que llama a `GET {baseUrl}/models` con `Authorization: Bearer <key>`; los modelos devueltos se listan con casillas y se guardan con el provider.
- **S3** «En el models.json de pi» (respuesta del usuario) — el provider se escribe en `~/.pi/agent/models.json` bajo `providers.<id>` con el mismo shape que usa pi (`baseUrl`, `api`, `apiKey`, `models[]`), con **backup** previo y **escritura atómica**, preservando cualquier otra clave del archivo.
- **S4** «En el archivo (texto plano)» (respuesta del usuario) — la apiKey se guarda en ese archivo. Deuda anotada: keychain del SO si se quiere endurecer.
- **S5** Interruptores de entorno. Tres flags independientes, en tiempo de build (como el resto de la configuración de Nabla):
  - `VITE_ENABLE_CUSTOM_PROVIDERS` — muestra el alta de providers personalizados. **Implementado en esta unidad.**
  - `VITE_ENABLE_PROVIDER_LOGIN` — sección de login con las opciones de pi. Unidad siguiente.
  - `VITE_ENABLE_API_KEYS` — alta de api-keys de providers conocidos de pi. Unidad siguiente.
  Sin flags activos, esas secciones no existen (no hay controles muertos).
- **S6** `VITE_DEFAULT_PROVIDER` (respuesta del usuario: «cadena con url que es el provider») — si está definida, ese provider aparece siempre, pide su key y **no se puede cambiar**. Se deriva `id`/`name` del host (o `VITE_DEFAULT_PROVIDER_NAME` si se define). **Unidad siguiente**, junto con las flags de S5.

## Contrato congelado (Rust ↔ TS)

```rust
struct ProviderSummary { id, name, base_url, api, model_count, has_api_key }  // camelCase en JSON
struct ModelOption { id, name }
struct ProviderDraft { id, name, base_url, api, api_key, models: Vec<ModelOption> }

list_providers() -> Vec<ProviderSummary>                      // lee models.json; nunca devuelve la key
discover_models(base_url: String, api_key: String) -> Vec<ModelOption>   // GET {base}/models
save_provider(draft: ProviderDraft) -> ProviderSummary        // backup + merge + atomic write, devuelve resumen
```

- `api` por defecto `"openai-completions"`; se respeta el existente al actualizar.
- `discover_models` usa el cliente HTTP **en Rust** (la webview no debe ver la key ni pelear con CORS/CSP).
- `save_provider` valida: id `[a-z0-9-]{1,32}`, `base_url` http(s), apiKey no vacía; nunca borra otros providers.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| P1 | Rust: lectura/escritura de `~/.pi/agent/models.json` (backup, merge, atómico) + `ProviderSummary`/`ModelOption`/`ProviderDraft` + tests | S3, S4 | pending | |
| P2 | Rust: `discover_models` con require HTTP + tests (sin red real, mock de parsing) | S2 | pending | |
| P3 | TS: pestaña Providers (lista real + activo), formulario de alta gateado por `VITE_ENABLE_CUSTOM_PROVIDERS`, descubrimiento con selección de modelos | S1, S2, S5 | pending | |
| P4 | Verificación + commit | — | pending | |

## Notas

- `Models` desaparece como pestaña: el selector de modelo del chat sigue usando `list_models` (catálogo del provider activo), no esta pestaña.
- Añadir `reqwest` (con rustls) es la única dependencia nueva y está justificada: descubrir modelos requiere HTTP nativo; hacerlo desde la webview rompería el CSP y expondría la key.
- Próxima unidad: `VITE_DEFAULT_PROVIDER` (bloqueo) + `VITE_ENABLE_PROVIDER_LOGIN` + `VITE_ENABLE_API_KEYS`.

## Log

- **L1** Usuario: quitar el bloque *Configuration file* del centro (hecho); renombrar Models → Providers; provider OpenAI-compatible con url/api-key/descubrir modelos; tomar ideas de `gtrabanco/pi-nan-provider`; flags de entorno para login, api-keys y providers personalizados; y `default_provider` como URL bloqueada.
- **L2** Research de `pi-nan-provider`: es una *extensión* de pi que registra providers vía `pi.registerProvider(...)` con `piAi.createProvider({ id, name, baseUrl, auth, models, api })`; el usuario puede sobreescribir modelos en `models.json` bajo `providers.<id>.modelOverrides.<modelId>`. Conclusión: Nabla no necesita una extensión propia para un provider estático — basta escribir `providers.<id>` en el `models.json` que pi ya lee, que es exactamente lo aprobado por el usuario.
- **L3** Respuestas del usuario: `default_provider` = cadena con la URL; providers en el `models.json` de pi; keys en texto plano.
- **L4** Secuencia: esta unidad implementa S1-S5 (parte custom). `default_provider`, login y api-keys quedan para la siguiente, sin controles muertos.
