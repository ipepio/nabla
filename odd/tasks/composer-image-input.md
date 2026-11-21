# Composer image input
First milestone: image input, not document export or per-turn tool selection.

## Specs
S1 «pegar captruas de pantalla o darle la funcionalidad al +» — accept a pasted, dropped or selected image; preview and remove before sending.
S2 «me parece bien, dale» — send images with their text, including queued turns; explain unsupported model input before sending. No silent model switch.

## Tasks
T1 S1–S2 Implemented and independently checked: 86 Node and 487 Rust tests; frontend build and Rust lint passed. Live GUI/provider acceptance remains manual. No publishing.

## Log
L1 «antes de eso, no sería interesante el pasar adjuntos a nabla? pegar captruas de pantalla o darle la funcionalidad al + ? [Image#23] y ahí darle algunas tools también para especificar? esto es un ejemplo de chatgpt, pero por ejemplo, no se si me explico»
L2 «me parece bien, dale»
