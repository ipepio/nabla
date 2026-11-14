import { createApp } from "vue";
import App from "./App.vue";

// Vuetify
import "vuetify/styles";
import "@mdi/font/css/materialdesignicons.css";
import { createVuetify } from "vuetify";

const vuetify = createVuetify({
  theme: {
    defaultTheme: "dark",
  },
});

createApp(App).use(vuetify).mount("#app");
