// Pure live session metrics helpers (elapsed timer & estimated context usage).

/** Rough token estimate (~4 chars per token) for quick UI context telemetry. */
export function estimateTokens(texts: readonly string[]): number {
  let chars = 0;
  for (const text of texts) chars += text.length;
  return Math.ceil(chars / 4);
}

/** Formats token counts compactly (`840`, `4.2k`, `128k`, `1.2M`). */
export function formatCompactTokens(tokens: number): string {
  if (tokens < 1000) return `${Math.max(0, Math.round(tokens))}`;
  if (tokens < 100_000) return `${(tokens / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  if (tokens < 1_000_000) return `${Math.round(tokens / 1000)}k`;
  return `${(tokens / 1_000_000).toFixed(1).replace(/\.0$/, "")}M`;
}

/** Formats elapsed seconds as `MM:SS` (or `HH:MM:SS` past one hour). */
export function formatElapsed(totalSeconds: number): string {
  const safe = Math.max(0, Math.floor(totalSeconds));
  const hours = Math.floor(safe / 3600);
  const minutes = Math.floor((safe % 3600) / 60);
  const seconds = safe % 60;
  const pad = (n: number) => String(n).padStart(2, "0");
  if (hours > 0) return `${pad(hours)}:${pad(minutes)}:${pad(seconds)}`;
  return `${pad(minutes)}:${pad(seconds)}`;
}

/** Clamped 0..1 context usage ratio, or `null` when the model reports no limit. */
export function contextUsageRatio(
  estimatedTokens: number,
  contextWindow: number | null,
): number | null {
  if (contextWindow === null || contextWindow <= 0) return null;
  return Math.min(1, Math.max(0, estimatedTokens / contextWindow));
}
