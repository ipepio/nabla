# sidebar-rework

Estado: en curso, rama `feat/pi-runtime`.
Alcance: rework del drawer izquierdo — nueva sesión, cabecera de workspaces con 3 acciones, lista de workspaces colapsable con sesiones y menú de 3 puntos. La edición (renombrar/borrar) se implementa en la siguiente unidad.

## Specs

- **S1** «quita la linea que hay ahí descentrada» — fuera el `SidebarSeparator` entre el header y el contenido.
- **S2** «Debajo pon un botón de "Nueva sesión"» — botón de ancho completo, redondeado, icono + texto centrado, justo bajo el header. «ese botón creará una nueva sesión en el Workspaces seleccionado».
- **S3** «Debajo tendremos esto tal cual … solo que en lugar de "Espacios de traba" será workspaces y ahí esos 3 iconos tendrán las funcionalidades. De buscar editar o explorar carpetas» — cabecera de sección: título `Workspaces` + 3 acciones: lupa (buscar), ajustes/editar (modo edición), carpeta+ (añadir carpeta).
- **S4** «Cada workspace será una ruta» — la fila muestra nombre y ruta.
- **S5** «Debajo de esa linea saldrán los workspaces, se podrán colapsar o expandir» — cada workspace es un nodo colapsable; al expandirse muestra sus sesiones.
- **S6** «se podrán cambiar el nombre … Por defecto se llamará el nombre de la última carpeta, pero nosotros podremos ponerle un alias … en la linea del workspace estarán los típicos 3 puntos para editar el workspace» — el nombre mostrado es el alias si existe; por defecto, el último segmento de la ruta (ya lo hace el store). Menú `⋯` por fila con las acciones de edición (implementación en la unidad siguiente; esta unidad deja el menú y sus acciones cableadas a los diálogos existentes o como placeholder explícito).

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| N1 | `src/lib/session-store.ts` (activeSession, sessionGeneration, newSession/openSession/clearSession); ChatPanel lo consume y crea por un único camino | S2 | done | (unidad sidebar) |
| N2 | Rework de `AppSidebar.vue` + `sidebar/{WorkspaceRow,SessionsList}.vue`: sin separador, New session, cabecera con 3 acciones, lista colapsable con sesiones, menú ⋯ (Edit como placeholder) | S1-S6 | done | (unidad sidebar) |
| N3 | Verificación: build OK, S1-S6 presentes, seam limpia, un solo camino de creación de sesión | — | done | (esta unidad) |

## Contrato (Rust ya existe, no se toca)

- `startSession(workspaceId) → SessionDto { sessionId, workspaceId, piSessionPath, pid, status }`
- `listSessions(workspaceId) → SessionSummary[]` (title puede ser null; mostrar "Session" + hora)
- `touchWorkspace(workspaceId)` al seleccionar
- El título de sesión aún se crea como `null`: la lista muestra un fallback, y poblar títulos desde el primer mensaje queda como follow-up.

## Log

- **L1** Usuario: rework de sidebar con referencias visuales: botón «Nueva sesión» bajo el header; cabecera «Workspaces» con lupa, ajustes y carpeta+; lista de workspaces colapsable con sus sesiones; alias editable y menú de 3 puntos. «Haz esto y después vamos a lo de editar».
- **L2** Alcance de esta unidad: estructura y cableado de crear/abrir sesión; los diálogos de renombrar/borrar y la persistencia del alias («editar») van en la unidad siguiente.
- **L3** Decisión de arquitectura: la sesión activa deja de vivir dentro de ChatPanel y pasa a un store de módulo (`session-store.ts`), porque ahora dos componentes (drawer y chat) crean y seleccionan sesiones.

- **L4** Pendiente explícito para la unidad «editar»: el item *Edit workspace* abre un diálogo de solo lectura; renombrar (alias persistido), borrar con confirmación y revelar en Finder quedan para la siguiente tanda.
- **L5** Verificación orchestrator: `npm run build` OK; cero `SidebarSeparator`; New session + 3 acciones + filas colapsables con `listSessions` + menú ⋯ presentes; `session-store.ts` con `newSession` como único camino; sin imports de APIs de Tauri en `src/components/sidebar/`.
