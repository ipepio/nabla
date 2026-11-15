# settings-dialog

Estado: en curso, rama `feat/pi-runtime`.
Alcance: diálogo de Ajustes de dos paneles (General, Modelos) con apertura del archivo de configuración. Sólo se cablean controles con backend real.

## Specs

- **S1** «haz que sea así [referencia] de momento tendremos General, Modelos y ya está» — diálogo tipo panel doble: cabecera con título, acción de abrir configuración y cerrar; navegación izquierda con *General* y *Modelos*; panel derecho con scroll.
- **S2** «Abrir archivo de configuración, abrirá el yml de configuración que tenemos» — abre el archivo de configuración **real**. Hoy es `settings.json` (JSON, no YAML): el botón abre ese archivo en el editor del sistema. Migrar a YAML sería otra decisión (anotada abajo).
- **S3** General cableado de verdad: **Apariencia** (Claro / Oscuro / Sistema) persistida en `settings.json` y aplicada al instante; fila que muestra la ruta del archivo de configuración con su botón de abrir.
- **S4** Modelos cableado de verdad: selector de modelo (lista del provider que ya expone `list_models`), provider mostrado en modo lectura.
- **S5** No hay controles muertos: lo que aún no tiene backend (permisos, tamaño de fuente, detalles de trabajo, vista de código, idioma, atajos) **no** se pinta como si funcionara; queda listado como próximas unidades.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| A1 | Rust: `config_file_path()` y `open_config_file()` (opener del sistema, crea el archivo si falta) + 2 tests | S2 | done | (unidad settings) |
| A2 | `components/settings/SettingsDialog.vue`: dos paneles, General (tarjetas de apariencia + fila del archivo) y Modelos (select + provider) | S1-S4 | done | (unidad settings) |
| A3 | Engranaje del footer abre el diálogo; formulario antiguo retirado (AppSidebar ya no lee settings) | S1 | done | (unidad settings) |
| A4 | Verificación: 70/70 cargo, clippy 0, build OK, comandos registrados, seam limpia | — | done | (esta unidad) |

## Notas

- El botón «Abrir archivo de configuración» usa el opener ya instalado (`tauri-plugin-opener`); no expone la ruta al webview más de lo necesario.
- Si más adelante se quiere YAML de verdad: sería cambio de formato + migración + parser (decisión aparte, no se hace aquí).
- Próximas unidades candidatas, en orden de valor: tamaño de fuente de la conversación (afecta al chat), idioma (necesita i18n), más detalle de trabajo (necesita contrato de eventos con niveles), atajos de teclado.

## Log

- **L1** Usuario: «vamos a trabajar en la parte de Settings … haz que sea así [referencia] de momento tendremos General, Modelos y ya está. Abrir archivo de configuración, abrirá el yml de configuración que tenemos. Y vamos a empezar con la parte de general».
- **L2** Corrección honesta registrada: el archivo es JSON (`settings.json`), no YAML; el botón abre el archivo real.
- **L3** Referencia visual incluye permisos, idioma, tamaño de fuente, detalles de trabajo, vista de código y atajos; se implementan sólo los que tienen backend, el resto queda para unidades siguientes.

- **L4** Verificación orchestrator: `cargo test` 70/70 (2 nuevos), `clippy` 0 warnings, `npm run build` OK; `config_file_path`/`open_config_file` registrados; AppSidebar ya no llama a getSettings/saveSettings (el estado de settings vive en el diálogo); sin imports de APIs de Tauri fuera de `src/contracts`.
- **L5** El diálogo abre `settings.json` real; migrar a YAML sigue siendo decisión aparte.
