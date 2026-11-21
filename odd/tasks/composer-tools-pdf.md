# Composer tools and PDF input
Build on feat/composer-image-input. No publication requested.

## Specs
S1 «dale con 1 2 y 3» — validate direct and queued images, separating deterministic proof from a live model/UI check.
S2 «Adjuntar» / «Usar una herramienta» — per-turn removable web search or browser intent. No execution on selection or preference changes. Do not advertise unverified SearXNG as available.
S3 «Resume este informe y dime en qué página aparece cada conclusión» — attach selectable-text PDF locally, preserve page boundaries, report scans without inventing text. Respect permissions, bounded processing and queue ownership.

## Tasks
T1 S1 Direct and queued bytes verified. Live GUI + vision-provider recognition still pending.
T2 S2 Implemented and tested; search menu gated pending real SearXNG QA.
T3 S3 Implemented and tested with real pdftotext and synthetic multipage files. Final gates: 110 Node tests, 497 Rust tests, pnpm build, cargo clippy.

## Log
L1 «dale con 1 2 y 3»
