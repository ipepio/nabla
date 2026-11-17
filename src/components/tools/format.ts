// Small presentation helpers shared by the Files tree and its preview header.
//
// Formatting only: no request lives here, so a tool can render a size before any
// byte has been read.

/**
 * A file size the drawer can afford to show in a narrow row: `0 B`, `812 B`,
 * `3.4 KB`, `1.2 MB`. One decimal from the kilobyte up, never more, so the column
 * does not jump while the user scrolls. A negative or non-finite size is `—`
 * instead of `NaN`, because a DTO field can be missing.
 */
export function formatBytes(sizeBytes: number): string {
  if (!Number.isFinite(sizeBytes) || sizeBytes < 0) return "—";
  if (sizeBytes < 1024) return `${Math.round(sizeBytes)} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = sizeBytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value < 10 ? value.toFixed(1) : Math.round(value)} ${units[unit]}`;
}
