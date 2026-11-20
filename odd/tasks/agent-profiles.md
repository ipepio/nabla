# agent-profiles — perfiles de subagente, configurables y apagados por defecto

Extiende `subagent-helpers.md` (extensión, enlace y UI ya construidos) con lo que
convierte los subagentes en algo usable: **perfiles nombrados con su modelo**, y la
puerta cerrada hasta que el humano la abra.

## Specs

**S1 — Es de Nabla, no de pi.** «estamos hablando de nabla, no de omp ni de pi».
Los perfiles se guardan en `<data_dir>/agents.json` (junto a `settings.json` y
`permissions.json`), escritura atómica, DTO camelCase con espejo en
`src/contracts/types.ts`. Se **retira** la lectura de `join(cwd, CONFIG_DIR_NAME,
"agents")` (`resources/nabla-subagents.ts:272`): ni `.pi/agents` ni ficheros dentro
del proyecto. La extensión lee el fichero por `NABLA_DATA_DIR`, que ya recibe.

**S2 — Una interfaz donde configurarlo.** «la idea es meter una interfaz dónde
cofigurar esto». Ajustes → Subagentes: interruptor maestro, lista de perfiles con
su estado, y editor por perfil con nombre, descripción, tools (casillas), modelo
(desde el catálogo real de cuentas), effort e instrucciones del rol. Plantillas
(`explorer`, `researcher`, `worker`, `judge`, `judge-2`) **bajo demanda**: un botón
que las ofrece rellenas; ninguna se crea sola.

**S3 — Nada activo por defecto.** «esto no puede venir activo por defecto, se tiene
que activar y configurar». `subagentsEnabled: false` y cada perfil `enabled: false`.
Maestro apagado ⇒ la extensión no se carga y `run_agent` **no existe** para el
modelo: no es un error que descubre, es una capacidad que no tiene. Encendido sin
perfiles activos ⇒ mensaje honesto, nunca un fallo silencioso.

**S4 — Sin onboarding todavía.** «Qzuiá cuando esté todo, podemos montar un
onboarding, pero de momento se tiene que activar la opción». El interruptor es la
puerta; no hay flujo guiado en esta entrega.

**S5 — `run_agents` en paralelo entra aquí.** «3 en esta». Segunda tool que lanza
hasta 4 hijos a la vez, un JSONL por hijo, espera a que todos asienten y devuelve un
resumen por hijo. Es lo que hace usable el juicio dual (hoy son dos esperas
secuenciales).

**S6 — Modelo por perfil, con tres niveles.** `model` del perfil → ajuste
`subagentModelDefault` de la app → modelo del padre. Siempre validado contra el
registro autenticado de la sesión: el editor avisa al guardar y `run_agent` falla
honesto en ejecución. Un `effort` inválido, igual.

**S7 — Cuándo cada uno, escrito en el prompt.** Tabla de enrutado que solo se añade
al system prompt **cuando el maestro está encendido**: explorer = dónde está,
researcher = qué es cierto fuera, worker = cámbielo, judge = está bien. Dos jueces
con familias de modelo distintas (recomendación visible en la interfaz, no
imposición).

**S8 — Nada se rompe.** Con el maestro apagado la app se comporta exactamente como
hoy: sin tools nuevas, sin sección de prompt, sin puntos de estado.

## Tasks

| ID | Specs | Qué | Ruta | Commit |
| --- | --- | --- | --- | --- |
| U1 | S1, S3, S6 | `agents.json` + comandos Rust (lectura/escritura atómica, validación), `subagentsEnabled`, extensión leyendo de `NABLA_DATA_DIR`, retirada de `.pi/agents` | `src-tauri/src/*`, `resources/nabla-subagents.ts`, `src/contracts/*` | pending |
| U2 | S2, S4, S6 | Ajustes → Subagentes: maestro, lista, editor, plantillas bajo demanda | `src/components/settings/*`, `src/lib/*` | pending |
| U3 | S5 | `run_agents` paralelo, tope 4, un JSONL por hijo, espera a todos, resumen por hijo | `resources/nabla-subagents.ts` | pending |
| U4 | S7 | Tabla de enrutado condicionada al maestro | `resources/nabla-system-prompt.md` + composición en Rust | pending |

## Log

- **L1** «vale, guay, de momento no toques nada y plantea como configuramos los
  diferentes perfiles, cuando un researcher, cuando un explorer, cuando un worker,
  cuando un juez, cuando otro juez. Como planteamos todo esto? y como configuramos
  los diferentes modelos para queda perfil? plantea esto»
- **L2** «1 estamos hablando de nabla, no de omp ni de pi. 2 la idea es meter una
  interfaz dónde cofigurar esto, esto no puede venir activo por defecto, se tiene
  que activar y configurar. Qzuiá cuando esté todo, podemos montar un onboarding,
  pero de momento se tiene que activar la opción. 3 en esta»
- **L3** «me parece bien, tira hasta U4» — aprueba el planteamiento y autoriza la
  implementación de U1 a U4.
- **L4** Decisión del orquestador aceptada por silencio: **un único `agents.json`**
  en lugar de un markdown por perfil, porque el editor es la interfaz y ya tenemos
  JSON + DTO + espejo TS; no se inventa un lenguaje de serialización nuevo.
