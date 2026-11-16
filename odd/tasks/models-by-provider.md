# models-by-provider

Estado: en curso, rama `feat/pi-runtime`.
Alcance: login compacto, selector de modelo agrupado por provider y edición de modelos de un provider.

## Specs

- **S1** «no he necesitado ni pulsar enlace ni introducir código ni nada (al menos para openai), por lo que podemos mostrar menos texto» — el login real completa por callback local en `127.0.0.1:1455`. La UI debe mostrar un estado corto ("Waiting for your browser…") y dejar la salida cruda detrás de un desplegable; la URL, el campo de pegado y el botón de abrir pasan a ser respaldo, no protagonistas.
- **S2** «a la hora de seleccionar el modelo deberíamos tener provider y por provider los modelos disponibles» — el selector del composer se agrupa por provider (etiquetas de grupo) y elegir un modelo fija **provider + modelo** en `settings`.
- **S3** «dentro de cada proveedor, estaría bien poder editar los modelos … poder poner contexto, input, output, activar modelo, desactivar modelo etc. al menos de los custom» — editor por modelo: `contextWindow`, `maxTokens`, coste de `input`/`output`, modalidades de entrada y un interruptor de habilitado; se aplica a los providers configurados en `models.json` (los custom incluidos).

## Contrato congelado (Rust ↔ TS)

```rust
// Lectura por provider, para el selector agrupado y el editor
struct ProviderModels { provider, name, models: Vec<ModelInfo> }
struct ModelInfo { id, name, context_window, max_tokens, input_cost, output_cost, modalities: Vec<String>, enabled }
list_provider_models() -> Vec<ProviderModels>          // lee ~/.pi/agent/models.json

// Escritura de un modelo dentro de su provider (mismo archivo, backup + atómico)
struct ProviderModelDraft {
  provider, id, name: String,
  context_window: Option<u64>, max_tokens: Option<u64>,
  input_cost: Option<f64>, output_cost: Option<f64>,
  modalities: Vec<String>,     // "text" | "image"
  enabled: bool
}
update_provider_model(draft: ProviderModelDraft) -> ModelInfo
remove_provider_model(provider: String, model_id: String) -> bool
```

- `enabled = false` se representa **sin borrar**: el modelo queda en el archivo con `"enabled": false` si pi lo respeta, y Nabla lo oculta del selector. Si pi ignorase la clave, se conserva igualmente (no rompe nada) y el editor la muestra.
- Nunca tocar providers no presentes en `models.json`; backup antes de cada escritura; merge sólo de `providers.<id>.models[<modelId>]`.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| M1 | Rust: `list_provider_models` + `update_provider_model` + `remove_provider_model` (backup, atómico, merge) + tests | S2, S3 | pending | |
| M2 | TS: login compacto (estado corto, salida tras desplegable, respaldo URL/pegado) | S1 | pending | |
| M3 | TS: selector agrupado por provider; elegir modelo fija provider + modelo | S2 | pending | |
| M4 | TS: editor de modelos por provider (contexto, max tokens, costes, modalidades, habilitado) | S3 | pending | |
| M5 | Verificación + commit | — | pending | |

## Log

- **L1** Usuario confirma que el login OAuth de OpenAI funcionó sin intervención; pide menos texto, selector agrupado por provider y edición de modelos por provider.
- **L2** Orden: primero lectura por provider (M1) porque habilita el selector agrupado y el editor; el login compacto (M2) es independiente y va en paralelo.
- **L3** `enabled` se persiste como marca, no como borrado: desactivar no debe destruir la configuración del modelo.
