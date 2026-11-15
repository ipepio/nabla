# theme-modes-and-resizable-drawer

Estado: en curso, rama `feat/pi-runtime`.
Alcance: tema claro real (tokens + interruptor + persistencia) y panel derecho redimensionable.

## Specs

- **S1** Tema light: «aplica el tema light también» — ambos modos deben verse correctos, sin quedar el claro con los tokens del oscuro. Paletas por entorno: `VITE_COLOR_*` = paleta oscura (ya existente), `VITE_COLOR_LIGHT_*` = paleta clara (opcional; si falta, se usan los defaults neutros claros de `index.css`).
- **S2** Interruptor de tema funcional y persistido en `settings.json` (`theme`: "dark" | "light" | "system"), aplicado al arrancar sin parpadeo y desde el botón del header y desde el dialog de ajustes.
- **S3** Panel derecho redimensionable: «echo en falta que se pueda redimensionar la parte del workspace de la derecha» — handle de arrastre, ancho mínimo/máximo, accesible por teclado, sin romper el layout del chat.
- **S4** Sin duplicar estado: el ancho del panel lo posee un solo componente; el tema lo aplica un único módulo (`src/lib/theme.ts`).

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| T1 | Worker A: `theme.ts` inyecta dos paletas (`:root` claro / `.dark` oscuro) desde env; `setThemeMode()`; toggle cableado en ChatHeader/ChatPanel; aplicado al cargar settings y tras guardar en el dialog | S1, S2, S4 | done | 81afe90 |
| T2 | Worker B: `ResizableAside.vue` (handle, drag con pointer capture, clamp 240-560px y 60% viewport, teclado, doble clic reset) + `App.vue` | S3, S4 | done | d593adb |
| T3 | Verificación orchestrator: build OK, API de tema, cero escrituras inline de paleta, wiring completo, clamp/teclado presentes, un solo aside | — | done | — |
| T4 | Commit + cierre ODD | — | done | 81afe90, d593adb |

## Contrato

- `src/lib/theme.ts` (T1 lo posee; nadie más lo edita): exporta `applyTheme()`, `setThemeMode(mode: "dark" | "light" | "system")`, `THEME_VARS`.
- Claves nuevas: `VITE_COLOR_LIGHT_*` con el mismo sufijo que las oscuras (PRIMARY, PRIMARY_FOREGROUND, SECONDARY, … SIDEBAR_FOREGROUND) + `VITE_RADIUS`.
- Persistencia del ancho del panel: **no** en esta unidad (estado de sesión); anotado como follow-up si se quiere recordar entre arranques.

## Log

- **L1** Usuario: «echo en falta que se pueda redimensionar la parte del workspace de la derecha, y aplica el tema light también please. Después de esto, ya pasamos a trabajar el drawer izquierdo».
- **L2** Problema detectado en el código actual: `theme.ts` aplica las variables como estilos inline en `<html>`, por lo que ganan también en modo claro. T1 debe pasar a inyectar reglas `:root` y `.dark`.
- **L3** Superficies disjuntas para el paralelismo: T1 (theme.ts, ChatHeader, ChatPanel, AppSidebar, docs) vs T2 (App.vue + componente nuevo). Ninguno toca `.env` (bloqueado por el guard): solo documentan claves.

- **L4** Verificación orchestrator: `npm run build` OK; `theme.ts` exporta asThemeMode/isDarkMode/applyTheme/setThemeMode y no escribe variables inline; claves light derivadas por prefijo (`VITE_COLOR_X` → `VITE_COLOR_LIGHT_X`); toggle cableado en header, boot y dialog; `ResizableAside` con pointer capture, clamp y teclado; App.vue sin `<aside>` propio.
- **L5** Pendiente para el usuario: crear el fichero de entorno con la paleta clara (`VITE_COLOR_LIGHT_*`) si quiere un claro a medida; sin ellas el claro usa los defaults neutros de `index.css`.
