# chat-shell-and-theme-tokens — chat estilo Claude Code + theming configurable

Estado: en curso, rama `feat/pi-runtime`.
Alcance: rediseño del panel central (empty state, header, tabs, burbujas, composer) y sistema de tokens de color/imagen configurables.

## Specs

- **S1** Chat vacío: «¿Como lo quiero? [imagen] así. En lugar de la ballena, el logo de nabla» — estado vacío centrado con el logo de marca (variante dark) y el composer debajo; sin header de conversación.
- **S2** Tras enviar un mensaje: «[imagen] así tiene que estar después de enviar un mensaje» — header superior con nombre del workspace/sesión, chip de modo, acciones a la derecha; tabs `Chat | Linaje`; mensajes del usuario alineados a la derecha como burbuja con hora y botón copiar; errores renderizados como bloque con punto rojo y etiqueta de código; composer fijo abajo con «+», «Escritura en el espacio de…», «Seleccionar modelo» y botón circular de enviar; línea de metadatos al pie (turnos/pasos).
- **S3** Imágenes configurables: «quiero que las imagenes ... sean todo configuraciones ... las imagenes está claro que es cambiar el public» — un único módulo de marca resuelve las rutas; los componentes nunca hardcodean rutas; reemplazar el archivo en `public/` cambia la marca sin tocar código. Variantes white (dark theme) y black (light theme).
- **S4** Colores configurables: «quiero que definas colores primarios, secundarios terciarios de diferentes tipos y que en las .env se cambie el hex de ese color y así poder repersonalizar la aplicación más fácil» — tokens semánticos (primary/secondary/tertiary/accent/muted/destructive/success/warning/border/ring/background/foreground/sidebar…) alimentados desde `.env` (hex), aplicados como CSS variables al arrancar, con fallback en CSS si no hay env.
- **S5** «IMPORTANTE, PARA TODO EL PROYECTO, ACTUALIZA LOS AGENTS.MD SI ES NECESARIO» — regla de proyecto: nunca colores/rutas de marca hardcodeadas; tokens y módulo de marca son la única vía; documentado en AGENTS.md + docs/ai + ADR.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| T1 | Worker A: `src/lib/theme.ts` + `src/lib/brand.ts` + tokens CSS + docs/AGENTS.md/ADR 0009. **Ficheros dotenv BLOQUEADOS** por el guard del harness (patron `dotenv` = sensitive): contenido publicado en el chat, los crea el usuario | S3, S4, S5 | done (parcial: dotenv pendiente) | (unidad chat-theme) |
| T2 | Worker B: `ChatPanel.vue` + `chat/{ChatEmptyState,ChatHeader,Composer,MessageBubble}.vue` | S1, S2 | done | (unidad chat-theme) |
| T3 | Verificación: build OK, tokens en CSS compilado, cero hex en componentes, brand.ts importado, seam limpia | — | done | — |
| T4 | Commit + cierre ODD | — | done | (ver git log) |

## Contrato fijado por el orchestrator (ambos workers lo respetan)

`src/lib/brand.ts` (lo crea T1; T2 solo lo importa):
```ts
export const BRAND = {
  mark: "/logo_white.png",        // cuadrada, dark theme
  markLight: "/logo.png",         // cuadrada, light theme
  logo: "/logo_texto_white.png",  // marca + wordmark, dark
  logoLight: "/logo_texto.png",
  wordmark: "/text_white.png",
  wordmarkLight: "/texto.png",
} as const;
export type BrandAsset = keyof typeof BRAND;
export function brandAsset(kind: BrandAsset): string;
```

Tokens `.env` (T1 los define; T2 solo usa clases Tailwind que ya mapean a ellos):
`VITE_COLOR_PRIMARY`, `VITE_COLOR_PRIMARY_FOREGROUND`, `VITE_COLOR_SECONDARY`, `VITE_COLOR_SECONDARY_FOREGROUND`, `VITE_COLOR_TERTIARY`, `VITE_COLOR_TERTIARY_FOREGROUND`, `VITE_COLOR_ACCENT`, `VITE_COLOR_ACCENT_FOREGROUND`, `VITE_COLOR_BACKGROUND`, `VITE_COLOR_FOREGROUND`, `VITE_COLOR_MUTED`, `VITE_COLOR_MUTED_FOREGROUND`, `VITE_COLOR_DESTRUCTIVE`, `VITE_COLOR_SUCCESS`, `VITE_COLOR_WARNING`, `VITE_COLOR_BORDER`, `VITE_COLOR_RING`, `VITE_COLOR_SIDEBAR`, `VITE_COLOR_SIDEBAR_FOREGROUND`, `VITE_RADIUS`.
Variables CSS nuevas expuestas a Tailwind: `--tertiary`, `--tertiary-foreground`, `--success`, `--warning` (además de las existentes).

## Log

- **L1** Usuario: «vamos a trabajarlo un poco más, recuerda orquestar todo si es necesario. Empecemos con el componente central del chat.» con dos referencias visuales (Claude Code desktop): estado vacío con logo + composer, y estado tras enviar (header, tabs Chat/Linaje, burbuja de usuario a la derecha con hora y copiar, bloque de error, composer con +/espacio/modelo/enviar, pie de turnos/pasos).
- **L2** Usuario añadió variantes white de los logos para dark theme (`logo_white.png`, `logo_texto_white.png`, `text_white.png` en `public/`).
- **L3** Requisito transversal: colores e imágenes configurables; `.env` con hex; AGENTS.md actualizado para todo el proyecto.
- **L4** Nota honesta: `.env` se hornea en build (Vite), así que re-brandear requiere rebuild; theming en runtime desde settings queda como evolución futura (el módulo `theme.ts` aísla el punto de cambio).
- **L5** Routing: 2 workers en paralelo (superficies disjuntas: infra/docs vs UI) + 1 verificador independiente. Los jueces quedan disponibles para juicio dual posterior si el usuario lo pide.

- **L6** Guard del harness: las rutas de ficheros dotenv son "sensitive path" y se bloquean para lectura y escritura (worker A lo reporto como `interaction_required`; el orchestrator lo reprodujo al intentar el commit). No se intenta evadir: los ficheros de entorno quedan para que los cree el usuario con el contenido publicado en el chat. `theme.ts` funciona con defaults sin ellos y lee `import.meta.env.*` en build.
- **L7** Verificacion orchestrator: build OK; tokens nuevos presentes en el CSS compilado; `brandAsset()` importado en ChatEmptyState; sin hex en componentes; sin APIs de Tauri fuera de `src/contracts`.
