/**
 * Window-level contract (S2 of `odd/tasks/theme-modes-and-resizable-drawer.md`
 * owns theming; this file owns geometry): the only place that may import the
 * Tauri window API for sizing, so components stay behind the contracts seam.
 *
 * The minimum comes from `src/lib/layout.ts`, where the prompt box's half width
 * determines everything else. Applying it at runtime — instead of only in the
 * static config — keeps the window's floor tied to the same constants the
 * composer and the tools panel use.
 */
import { LogicalSize } from "@tauri-apps/api/dpi";
import { getCurrentWindow } from "@tauri-apps/api/window";

/**
 * Sets the window's minimum size. Safe to call outside Tauri (browser dev) and
 * never throws: a window that refuses the change must not break the app.
 */
export async function applyWindowMinimums(
  width: number,
  height: number,
): Promise<void> {
  try {
    await getCurrentWindow().setMinSize(new LogicalSize(width, height));
  } catch {
    // Browser dev server, missing capability, or a platform that ignores it:
    // the static values in `tauri.conf.json` remain the fallback.
  }
}
