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
import { getCurrentWebview } from "@tauri-apps/api/webview";
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

/** One native drop: the OS paths it carried and where it happened. */
export interface FileDropEvent {
  /** Absolute paths of the dropped files. */
  paths: string[];
  /** Drop point in physical pixels, when the platform reports one. */
  position: { x: number; y: number } | null;
}

/**
 * S1 of `odd/tasks/composer-image-input.md`: a desktop file drop.
 *
 * On macOS (and on every Tauri window with drag-drop enabled) the OS hands the
 * paths to the window, not to the DOM: the webview's own `drop` event never fires
 * for a Finder drag, so a listener here is the only way the composer can accept
 * one. The event is window-wide, which is why the position travels with it — the
 * caller decides whether the drop landed on its own surface.
 *
 * Resolves with an unsubscribe function and never rejects: outside Tauri (browser
 * dev) or on a platform that does not report drops, the app keeps working.
 */
export async function onFileDrop(
  handler: (event: FileDropEvent) => void,
): Promise<() => void> {
  try {
    return await getCurrentWebview().onDragDropEvent((event) => {
      if (event.payload.type !== "drop") return;
      handler({
        paths: [...(event.payload.paths ?? [])],
        position: event.payload.position ?? null,
      });
    });
  } catch {
    return () => {};
  }
}
