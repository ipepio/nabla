import assert from "node:assert/strict";
import test from "node:test";
import { prepareTextForSpeech, splitTextForSpeech } from "../src/lib/voice-text.ts";

test("prepareTextForSpeech strips code blocks, links, urls, and markdown markers", () => {
  const input = [
    "## Resumen",
    "",
    "Aquí tienes **el enlace** a [Nabla](https://example.com) y `codigo`.",
    "```ts",
    "const secret = 42;",
    "```",
    "Mira https://foo.bar/baz para más detalles.",
  ].join("\n");

  const out = prepareTextForSpeech(input);
  assert.equal(
    out,
    "Resumen Aquí tienes el enlace a Nabla y codigo. Mira para más detalles.",
  );
});

test("prepareTextForSpeech strips emojis and applies Spanish pronunciation rules", () => {
  const input =
    "¡Hola! 🚀 Echo funciona con ChatGPT, OpenAI y GitHub 🎉 en macOS 🇪🇸. Hecho está.";
  const out = prepareTextForSpeech(input, { language: "es" });
  assert.equal(
    out,
    "¡Hola! Eko funciona con Chat gi pi ti, Open ei ai y Git Jab en mac o ese. Hecho está.",
  );
  assert.ok(!out.includes("🚀"));
  assert.ok(!out.includes("🇪🇸"));
});

test("prepareTextForSpeech uses English-safe pronunciation rules when language is en", () => {
  const input = "Echo works with ChatGPT on macOS! 🔥";
  const out = prepareTextForSpeech(input, { language: "en-US" });
  assert.equal(out, "Echo works with Chat G P T on mac O S!");
});

test("splitTextForSpeech prefers sentence endings and keeps first chunk shorter", () => {
  const text =
    "Primera frase corta para empezar rápido. Segunda frase que aporta algo más de contexto y detalle. Tercera frase que completa la respuesta con información adicional para comprobar que la segmentación funciona bien.";

  const chunks = splitTextForSpeech(text, { firstMaxChars: 60, maxChars: 110 });
  assert.ok(chunks.length >= 2);
  assert.equal(chunks[0], "Primera frase corta para empezar rápido.");
  for (const chunk of chunks) {
    assert.ok(chunk.length > 0);
  }
});

test("splitTextForSpeech handles empty and whitespace-only strings", () => {
  assert.deepEqual(splitTextForSpeech(""), []);
  assert.deepEqual(splitTextForSpeech("   \n\t  "), []);
});
