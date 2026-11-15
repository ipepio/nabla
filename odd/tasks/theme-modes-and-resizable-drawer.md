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
| T1 | Worker A: `theme.ts` pasa a inyectar dos paletas (`:root` claro / `.dark` oscuro) desde env; `setThemeMode()`; toggle cableado en ChatHeader/ChatPanel; aplicar al cargar settings y tras guardar en el dialog | S1, S2, S4 | pending | |
| T2 | Worker B: nuevo componente de panel redimensionable + `App.vue` (handle, min/max, teclado) | S3, S4 | pending | |
| T3 | Verificación: build, ambos temas, resize, seams | — | pending | |
| T4 | Commit + cierre ODD | — | pending | |

## Contrato

- `src/lib/theme.ts` (T1 lo posee; nadie más lo edita): exporta `applyTheme()`, `setThemeMode(mode: "dark" | "light" | "system")`, `THEME_VARS`.
- Claves nuevas: `VITE_COLOR_LIGHT_*` con el mismo sufijo que las oscuras (PRIMARY, PRIMARY_FOREGROUND, SECONDARY, … SIDEBAR_FOREGROUND) + `VITE_RADIUS`.
- Persistencia del ancho del panel: **no** en esta unidad (estado de sesión); anotado como follow-up si se quiere recordar entre arranques.

## Log

- **L1** Usuario: «echo en falta que se pueda redimensionar la parte del workspace de la derecha, y aplica el tema light también please. Después de esto, ya pasamos a trabajar el drawer izquierdo».
- **L2** Problema detectado en el código actual: `theme.ts` aplica las variables como estilos inline en `<html>`, por lo que ganan también en modo claro. T1 debe pasar a inyectar reglas `:root` y `.dark`.
- **L3** Superficies disjuntas para el paralelismo: T1 (theme.ts, ChatHeader, ChatPanel, AppSidebar, docs) vs T2 (App.vue + componente nuevo). Ninguno toca `.env` (bloqueado por el guard): solo documentan claves.
