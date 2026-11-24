/**
 * Reactive bridge allowing the Document tool (or other side-drawer views) to
 * hand a selected excerpt directly to the active chat agent in ChatPanel.
 */
import { ref } from "vue";

export type DocumentAgentAction = "ask" | "explain" | "improve" | "summarize";

export interface DocumentAgentRequest {
  id: number;
  mode: "insert" | "send";
  prompt: string;
}

export const pendingDocumentAgentRequest = ref<DocumentAgentRequest | null>(null);

let nextRequestId = 1;

export function buildDocumentAgentPrompt(
  action: DocumentAgentAction,
  selectedText: string,
  filePath?: string | null,
): string {
  const clean = selectedText.trim();
  const refLabel = filePath?.trim() ? `\`${filePath.trim()}\`` : "the open document";
  const quoted = clean
    .split("\n")
    .map((line) => `> ${line}`)
    .join("\n");

  switch (action) {
    case "ask":
      return `Sobre este fragmento de ${refLabel}:\n${quoted}\n\n`;
    case "explain":
      return `Explica con claridad este fragmento de ${refLabel}:\n\n${quoted}`;
    case "improve":
      return `Mejora la redacción, claridad y estilo de este fragmento de ${refLabel} (y propón la versión mejorada):\n\n${quoted}`;
    case "summarize":
      return `Resume los puntos clave de este fragmento de ${refLabel}:\n\n${quoted}`;
  }
}

export function requestAgentWithDocumentSelection(
  action: DocumentAgentAction,
  selectedText: string,
  filePath?: string | null,
): void {
  const clean = selectedText.trim();
  if (!clean) return;
  pendingDocumentAgentRequest.value = {
    id: nextRequestId++,
    mode: action === "ask" ? "insert" : "send",
    prompt: buildDocumentAgentPrompt(action, clean, filePath),
  };
}
