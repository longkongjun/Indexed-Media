import { createApp } from "vue";
import { createPinia } from "pinia";
import App from "./App.vue";
import { identityClientKey, mediaFlowClient } from "./app/client";
import { useSessionStore } from "./app/session";
import { createAppRouter } from "./router";
import "./styles/tokens.css";
import "./styles/base.css";
import "./styles/layout.css";
import "./styles/responsive.css";

const app = createApp(App);
const pinia = createPinia();
app.use(pinia);
app.provide(identityClientKey, mediaFlowClient);
useSessionStore(pinia).configureClient(mediaFlowClient);

const router = createAppRouter({
  pinia,
  getBootstrapStatus: mediaFlowClient.getBootstrapStatus,
  getSession: mediaFlowClient.getSession,
});
app.use(router);
app.mount("#app");
