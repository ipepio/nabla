# providers-pane-refinements

Estado: en curso, rama `feat/pi-runtime`.
Alcance: selector provider→modelo en dos pasos, apertura instantánea de Providers, cuentas de suscripción acotadas y editor de modelos dentro de la edición del provider.

## Specs

- **S1** «en el desplegable quiero un dropdown con los providers y al seleccionar cada provider, entonces a la derecha aparecerán los modelos» — **revisado por el usuario**: «no quiero 2 dropdowns, quiero uno y en el de providers que se despliegue un dropdown lateral con los modelos del provider». Implementado como **un solo disparador** con submenús laterales por provider.
- **S2** «al pulsar Providers, tarda mucho en abrirse, ¿por qué? al final es leer un archivo, no?» — medido: `pi auth check` tarda **1,15 s** (arranca el CLI) y la sección ejecuta **6** (2 configurados + 4 conocidos) → ~7 s. El archivo se lee en milisegundos. La apertura debe ser inmediata y los badges llegar después.
- **S3** «la subscription accounts, ¿de dónde sacas HelmCode y nan? eso estaría en custom providers, ¿no?» — la sección de cuentas de suscripción debe listar **sólo** providers de suscripción (`openai`, `openai-codex`, `anthropic`, `google`); los configurados (HelmCode, nan, custom) pertenecen a la lista de Providers, no a cuentas.
- **S4** «en los custom, en lugar de tener models by provider, creo que es mejor tenerlo en un apartado de editar provider» — el editor de modelos deja de ser una sección suelta y pasa a vivir dentro de la edición del provider.

## Contrato congelado

```rust
pi_auth_check_many(providers: Vec<String>) -> Vec<PiAuthStatus>   // concurrencia acotada (4), un IPC, timeout por provider
pi_auth_check(provider: String) -> PiAuthStatus                    // ya existe, se mantiene
```

- La UI nunca bloquea la apertura del panel: renderiza lista y modelos desde el archivo y pide los estados en segundo plano.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| R1 | Rust: `pi_auth_check_many` (pool 4, orden preservado, fallo → unknown) + 4 tests | S2 | done | ac1fb90 |
| R2 | TS: composer con dos selects provider → modelos, persistiendo ambos en una escritura | S1 | done | a51d164 |
| R3 | TS: pinta desde el archivo y badgea en segundo plano con un solo `pi_auth_check_many`, caché de módulo y "Refresh status" | S2 | done | (esta unidad) |
| R4 | TS: cuentas = solo openai, openai-codex, anthropic, google | S3 | done | (esta unidad) |
| R5 | TS: editor de modelos dentro del diálogo Edit del provider; bloque suelto eliminado | S4 | done | (esta unidad) |
| R6 | Verificación: 150/150 cargo, clippy 0, build OK, i18n 198/198 | — | done | (esta unidad) |

## Log

- **L1** Usuario reporta los cuatro puntos y pide mirarlos.
- **L2** Medición del orchestrator: `pi auth check --provider openai --json` = **1,15 s** (`real`), 2 providers configurados + 4 conocidos = 6 spawns secuenciales ≈ 7 s. Causa raíz identificada: spawns por provider, no E/S de archivo.
- **L3** Decisión: un comando Rust que ejecuta los checks en paralelo (pool 4) y devuelve todo junto; la UI pinta primero lo que sale del archivo y rellena badges después, con caché por sesión.

- **L4** Verificación orchestrator: `cargo test` 150/150 (4 nuevos), clippy 0; `npm run build` OK; i18n 198/198; la sección de cuentas lista exactamente los cuatro providers de suscripción; el bloque `ProviderModelsEditor` ya no existe suelto (0 ocurrencias) y vive dentro del diálogo Edit; el panel tiene 3 separadores en total (General, Providers, y uno dentro del diálogo), dentro de lo pedido («algunas de vez en cuando»).
