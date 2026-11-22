// Pure Markdown transcript formatter (no DOM or Tauri dependencies).
import type { ChatMessage } from "@/lib/session-store";

/**
 * Formats a session's transcript into a clean, portable Markdown document.
 * Empty messages are skipped; user/assistant turns keep clear section headings.
 */
export function formatTranscriptMarkdown(
  workspaceName: string,
  sessionTitle: string | null,
  messages: readonly ChatMessage[],
): string {
  const title = sessionTitle?.trim() || workspaceName || "Nabla session";
  const lines: string[] = [`# ${title}`, ""];
  if (workspaceName.trim() !== "") {
    lines.push(`> Workspace: \`${workspaceName.trim()}\``, "");
  }

  for (const message of messages) {
    const body = message.text.trim();
    if (body === "") continue;
    const roleLabel =
      message.role === "user"
        ? "User"
        : message.role === "assistant"
          ? "Nabla"
          : "Tool";
    const stamp = message.at ? ` (${message.at})` : "";
    lines.push(`## ${roleLabel}${stamp}`, "", body, "");
  }

  return `${lines.join("\n").trimEnd()}\n`;
}
