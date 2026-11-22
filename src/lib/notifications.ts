/**
 * Configurable native OS notifications via `tauri-plugin-notification`.
 *
 * Notifications are strictly opt-in: nothing fires unless the user has enabled
 * the master switch, enabled the specific event kind, and granted OS permission.
 * Using `tauri-plugin-notification` ensures macOS displays Nabla's own app icon
 * and respects Do Not Disturb / Focus settings.
 */
import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { notificationPreferences } from "@/lib/preferences";

export type NotificationEventKind = "turnFinish" | "question" | "subagentFinish";

export type NotificationPermissionState =
  | "granted"
  | "denied"
  | "prompt"
  | "unsupported";

export async function checkNotificationPermission(): Promise<NotificationPermissionState> {
  try {
    const granted = await isPermissionGranted();
    if (granted) return "granted";
    if (typeof Notification !== "undefined" && Notification.permission === "denied") {
      return "denied";
    }
    return "prompt";
  } catch {
    return "unsupported";
  }
}

export async function ensureNotificationPermission(): Promise<boolean> {
  try {
    if (await isPermissionGranted()) return true;
    const result = await requestPermission();
    return result === "granted";
  } catch {
    return false;
  }
}

function isEventEnabled(kind: NotificationEventKind): boolean {
  const prefs = notificationPreferences.value;
  if (!prefs.enabled) return false;
  switch (kind) {
    case "turnFinish":
      return prefs.onTurnFinish;
    case "question":
      return prefs.onQuestion;
    case "subagentFinish":
      return prefs.onSubagentFinish;
  }
}

/**
 * Dispatches an OS notification when the user opted into `kind`, OS permission
 * is granted, and the window is currently hidden/unfocused.
 */
export async function sendAppNotification(
  kind: NotificationEventKind,
  title: string,
  body: string,
): Promise<boolean> {
  if (!isEventEnabled(kind)) return false;
  if (typeof document !== "undefined" && !document.hidden) return false;
  const cleanTitle = title.trim();
  const cleanBody = body.trim();
  if (!cleanTitle && !cleanBody) return false;
  try {
    if (!(await isPermissionGranted())) return false;
    sendNotification({
      title: cleanTitle || "Nabla",
      body: cleanBody,
    });
    return true;
  } catch {
    return false;
  }
}
