# i18n-and-settings-polish

Estado: en curso, rama `feat/pi-runtime`.
Alcance: i18n real (inglés + español) en toda la UI, selector de idioma persistido y limpieza visual del diálogo de Ajustes.

## Specs

- **S1** «quita todas las líneas divisorias, solo tienes que poner algunas de vez en cuando» — fuera los divisores fila a fila del panel de Ajustes; como mucho, separación entre bloques mayores.
- **S2** «mete ya i18n» — internacionalización real con al menos `en` y `es`, aplicada a todos los textos visibles, persistida en `settings.json` (`language`) y aplicada al arrancar.
- **S3** «utiliza un subagente de gemma4 para traducir todos los textos» — la traducción se delega; hoy no hay rol con `gemma4` (roles existentes: deepseek-v4-flash, qwen3.6, hy4, deepseek-v4-pro, glm5.3). Se pide autorización para añadir un rol `translate` con `nan/gemma4`. Mientras tanto las cadenas `es` se producen en el mismo cambio y son reemplazables sin tocar código.
- **S4** «Lo de abrir archivo de configuración, en que esté en un solo sitio sobra (mejor al final, podemos quitar el de arriba)» — una sola acción, al final del panel General; se retira la de la cabecera.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| I1 | `src/lib/i18n.ts` (locale de módulo, `t()` con `{var}`, `MessageKey` desde en.json) + `locales/{en,es}.json`, 97 claves, paridad garantizada por tipos | S2 | done | (unidad i18n) |
| I2 | Rust: `language` con `serde(default)` "en" (3 tests) + `AppSettings.language` | S2 | done | (unidad i18n) |
| I3 | Cadenas extraídas a `t()` en los 9 componentes (97 claves, incluidas aria-label/title/placeholder) | S2 | done | (unidad i18n) |
| I4 | Fila Idioma (English/Español, aplicada al instante y en el arranque desde main.ts); 2 divisores en total; abrir configuración sólo al final | S1, S2, S4 | done | (unidad i18n) |
| I5 | Traducción `es` completa y revisada (97/97). Pendiente: rol `translate` con gemma4 para regenerarla cuando el usuario lo autorice | S3 | done (sin gemma4) | (unidad i18n) |
| I6 | Verificación: 73/73 cargo, build OK, paridad de claves 97/97, 2 divisores, una acción de abrir | — | done | (esta unidad) |

## Contrato de i18n (lo fija el orchestrator)

```ts
// src/lib/i18n.ts
export const SUPPORTED_LOCALES = ["en", "es"] as const;
export type Locale = (typeof SUPPORTED_LOCALES)[number];
export const locale = ref<Locale>("en");          // estado de módulo
export function setLocale(next: Locale | string): void;
export function t(key: MessageKey, vars?: Record<string, string | number>): string;
export function useI18n(): { locale: Ref<Locale>; setLocale: typeof setLocale; t: typeof t };
```

- `MessageKey` se deriva de `en.json` (`keyof typeof en`) → una clave inexistente es error de compilación.
- `es.json` debe tener exactamente el mismo conjunto de claves (test/chequeo de paridad en el build del PR).
- Sin dependencias nuevas: la interpolación cubre `{name}`; pluralización compleja queda fuera hasta que haga falta.

## Log

- **L1** Usuario: quitar divisorios, meter i18n, traducir con gemma4, dejar una sola acción de abrir configuración (al final).
- **L2** Corrección al pedido: no hay rol configurado con gemma4; se registra la limitación y se pide autorización para crear el rol, en lugar de simular que se usó.
- **L3** Decisión: i18n propio y mínimo (sin dependencias) porque el proyecto evita deps innecesarias y el alcance actual es clave→texto con interpolación.

- **L4** Verificación orchestrator: `cargo test` 73/73 (3 nuevos de settings), `npm run build` OK, `set(en)==set(es)` con 97 claves cada uno y sólo 9 valores idénticos al inglés (marcas/nombres propios).
- **L5** Sin gemma4 disponible: la traducción se produjo en este cambio y es reemplazable sin tocar código (`es.json`). Para usar gemma4 hay que crear un rol `translate` con `nan/gemma4`, lo que requiere editar la configuración de subagentes (fuera del proyecto, necesita autorización).
