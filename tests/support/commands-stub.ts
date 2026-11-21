// Test double for the contracts seam (`src/contracts/commands.ts`).
//
// `src/lib/session-store.ts` is a pure store that happens to call Rust for the
// process it supervises. A Node test that pins the queue needs the store's own
// behavior — the queue entry, its label, its removal — and not a Tauri bridge, so
// `tests/composer-queue.test.ts` resolves the seam to this module instead. Every
// call here is a no-op or an explicit "the queue path does not do this", never a
// silent success, so a test cannot pass on behavior the store does not have.
//
// The shape mirrors the exports the store and `src/lib/preferences.ts` import;
// the real seam is the authority on their types, and `npm run build` checks it.

export function commandErrorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Unexpected bridge error.";
}

/** The real one is dev-only and goes through Rust; a test prints nothing. */
export function debugLog(_message: string): void {}

export async function liveSessionIds(): Promise<string[]> {
  return [];
}

export async function loadSessionPreview(): Promise<null> {
  return null;
}

export async function saveSessionPreview(): Promise<boolean> {
  return false;
}

export async function sessionTranscript(): Promise<[]> {
  return [];
}

export async function sendPrompt(): Promise<void> {
  throw new Error("the queue test never sends a prompt");
}

export async function openSession(): Promise<never> {
  throw new Error("the queue test never opens a session");
}

export async function startSession(): Promise<never> {
  throw new Error("the queue test never starts a session");
}

export async function subscribeAgentEvents(): Promise<() => void> {
  return () => {};
}

export async function getSettings(): Promise<never> {
  throw new Error("the queue test never reads settings");
}

export async function saveSettings(): Promise<never> {
  throw new Error("the queue test never writes settings");
}
