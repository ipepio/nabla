import { createApp } from "vue";
import App from "./App.vue";
// Tailwind v4 entry (design tokens + base layer). The `.dark` class on <html>
// decides the palette; index.html ships it as the default.
import "./assets/index.css";
import { applyWindowMinimums } from "./contracts/window";
import { APP_MIN_HEIGHT, APP_MIN_WIDTH } from "./lib/layout";

createApp(App).mount("#app");

// The prompt box's half width sets the floor for the chat, the tools panel and
// therefore the window itself (see `src/lib/layout.ts`).
void applyWindowMinimums(APP_MIN_WIDTH, APP_MIN_HEIGHT);
