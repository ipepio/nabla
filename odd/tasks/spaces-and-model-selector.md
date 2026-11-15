# spaces-and-model-selector — sidebar de espacios + selector de modelo

Estado: en curso, rama `feat/pi-runtime`.
Alcance: sidebar de "espacios" (workspaces) con filtros/búsqueda/pin + chat con header (nombre de workspace + selector de modelo) + viewport derecho vacío.

## Specs

- **S1** Espacios: «los espacios si que los quiero» — espacios = los workspaces actuales (respuesta del usuario: sin jerarquía nueva).
- **S2** Filtros: «las opciones de los filtros» → respuesta del usuario: **Todos / Recientes / Pinned**.
- **S3** Lupa: búsqueda sobre la lista de espacios (nombre/ruta).
- **S4** "+" = browse a folder (ya existe vía tauri-plugin-dialog; se mantiene).
- **S5** Chat central: «quiero el nombre del workspace por defecto» (header con el nombre del workspace activo) + «seleccionar modelo» (selector con el provider NaN y sus modelos; ver S7).
- **S6** Viewport derecho: «deja un viewport vacío» — panel derecho placeholder para tools futuras (costura documentada).
- **S7** Modelos: «meter ... el provider de NaN que tenemos en pi y sus modelos» — provider `nan` + sus 11 modelos como fuente del selector; el spawn pasa `--provider <p> --model <m>`.
- **S8** Pin y recencia: soporte de datos para Pinned/Recientes (columna `pinned` + `last_used_at`), sin lógica de negocio compleja.

## Tasks

| ID | Tarea | Specs | Estado | Commit |
| --- | --- | --- | --- | --- |
| M1 | Rust: migración v2 + pinned/last_used_at + set_workspace_pinned/touch_workspace + NAN_MODELS (11) + settings.provider/model + spawn --provider/--model + 3 comandos | S7, S8 | done | (unidad spaces) |
| M2 | TS: contratos espejo + wrappers (setWorkspacePinned/touchWorkspace/listModels) | — | done | (unidad spaces) |
| M3 | UI: chips Todos/Recientes/Pinned, lupa, pin, header chat (nombre+selector modelo), viewport derecho 280px | S1-S6 | done | (unidad spaces) |
| M4 | Verificación: 49/49 cargo, clippy (1 warning test-only pre-existente), build OK, contratos 0 mismatches, seams limpias | — | done | — |

## Contrato (fuente: Rust, serde camelCase)

```
Workspace  { id, path, name, created_at, pinned: bool, last_used_at: Option<String> }  → WorkspaceInfo { id, path, name, createdAt, pinned, lastUsedAt: string|null }
ModelInfo  { id, name }  →  ModelInfo { id, name }
Settings   { theme, pi_binary, provider, model }  →  AppSettings { theme, piBinary, provider, model }

Comandos nuevos:
  set_workspace_pinned(workspaceId, pinned) -> Workspace
  touch_workspace(workspaceId) -> Workspace
  list_models() -> Vec<ModelInfo>

Defaults settings: provider="nan", model="deepseek-v4-flash"
Spawn añade: --provider <provider> --model <model>
Migración v2: ALTER TABLE workspaces ADD pinned INTEGER NOT NULL DEFAULT 0; ADD last_used_at TEXT;
list_workspaces orden default: pinned DESC, last_used_at DESC NULLS LAST, created_at, id
```

## Log

- **L1** Usuario describe el objetivo (imágenes no visibles para el modelo): sidebar espacios + filtros + lupa + "+" browse; chat con nombre de workspace y selector de modelo (provider NaN de pi); viewport derecho vacío. «utiliza los subagentes de juez a juez b worker reviewer... si tienes dudas... pregunta».
- **L2** Respuestas del usuario: espacios = workspaces actuales; filtros = Todos / Recientes / Pinned.
- **L3** Provider NaN extraído de `~/.pi/agent/models.json`: baseUrl api.nan.builders/v1, api openai-completions, 11 modelos (ids exactos en M1). El selector usa una constante en Rust (comentada con su fuente); la lista dinámica desde la config de pi queda para más adelante.
- **L4** PNGs del usuario en `public/` (logo.png, logo_texto.png, texto.png); se usan logo.png (sidebar) y logo_texto.png (header chat) con fallback al glifo ∇.
- **L5** Routing de subagentes: worker (implementar) + verify (verificación). Los jueces (jd-judge-a/b) son para revisión adversarial, no para construir; disponibles tras el diseño si el usuario quiere un juicio dual.

- **L6** Verificación independiente: 49/49 cargo tests, clippy limpio salvo 1 warning pre-existente solo en test (field_reassign_with_default, con --all-targets), cargo build y vue-tsc+vite OK. Contratos Rust↔TS consistentes (WorkspaceInfo pinned/lastUsedAt, ModelInfo, AppSettings provider/model; comandos set_workspace_pinned/touch_workspace/list_models). Migración v2 preserva v1, corre solo current<2. Sin modelos hardcodeados en TS; seam limpia; superficies correctas.