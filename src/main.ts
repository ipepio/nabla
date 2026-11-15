import { createApp } from "vue";
import App from "./App.vue";
// Tailwind v4 entry (design tokens + base layer). The `.dark` class on <html>
// decides the palette; index.html ships it as the default.
import "./assets/index.css";

createApp(App).mount("#app");
