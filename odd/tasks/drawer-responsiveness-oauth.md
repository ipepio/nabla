# Drawer responsiveness and native popup tabs
DeepSeek implements; parent reviews. Preserve prior uncommitted transcript work and Pi command prewarm. No publishing.

## Specs
S1 «cuando abres el drawer en lugar de tener que pulsar nueva pestaña y ahí seleccionar la aplicación, que ya esté esa primera pestaña abierta y selecciones la tool que quieres»
S2 «de momento me gustan» — accepted sidebar refresh, preview deduplication, shared manifest read, deferred terminal loading and settings read reuse. Preserve Pi prewarm.
S3 «permitir al webview arbrir pestañas nuevas (para autenticaciones oauth que van en pestaña separada) dentro del mismo drawer que puedes tener varias pestañas» — prove opener callback and close before claiming OAuth support; protect human auth and agent target.
S4 «me parece bien, orquestalo todo utilizando a deepseek para el desarrollo, que no pare hasta que tenga tu OK»

## Tasks
T1 S1,S2 DeepSeek: verified first picker, startup improvements. No commit.
T2 S2 DeepSeek: verified targeted refresh, serialized preview and manifest deduplication after B1 correction. No commit.
T3 S3 DeepSeek: integrated after L3 decision; popups open as tabs inside the Browser pane, manual close returns to opener; opener/postMessage preserved, popup excluded from agent targets; window.close() gap documented. Verified: 73 Node + 473 Rust tests, build, diff --check; GUI acceptance pending (manual). No commit.
T4 S4 Parent: completed scoped review; 61 Node tests and 463 Rust tests passed on final run; build passed. First Rust run had one timeout test failure. Independent verification of B1 passed. No commit.

## Log
L1 me parece bien, orquestalo todo utilizando a deepseek para el desarrollo, que no pare hasta que tenga tu OK
L2 Previously modified transcript files, locale files, architecture docs and tests predate this task. Preserve them.
L3 «claro, lo que quiero es abrir una nueva pestaña en el browser, no en el drawer. me explcio?» — popups open as tabs inside the Browser pane, not drawer tabs; unblocked T3 with manual close.
