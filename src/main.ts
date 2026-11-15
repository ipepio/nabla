import { createApp } from "vue";
import App from "./App.vue";
// Tailwind v4 entry (design tokens + base layer). The `.dark` class on <html>
// decides the palette; index.html ships it as the default.
import "./assets/index.css";
import { getSettings } from "./contracts/commands";
import { applyWindowMinimums } from "./contracts/window";
import { setLocale } from "./lib/i18n";
import { APP_MIN_HEIGHT, APP_MIN_WIDTH } from "./lib/layout";

/**
 * S2 of `odd/tasks/i18n-and-settings-polish.md`: the persisted language is read
 * and applied before the first mount, so the window never paints a frame of the
 * wrong language. A missing file — or a plain browser during `vite dev` — keeps
 * the catalogue default (English): a preference must never block the boot.
 */
async function boot(): Promise<void> {
  try {
    const settings = await getSettings();
    setLocale(settings.language);
  } catch {
    // English stays applied; the settings dialog reports read failures itself.
  }
  createApp(App).mount("#app");
}

void boot();

// The prompt box's half width sets the floor for the chat, the tools panel and
// therefore the window itself (see `src/lib/layout.ts`).
void applyWindowMinimums(APP_MIN_WIDTH, APP_MIN_HEIGHT);
