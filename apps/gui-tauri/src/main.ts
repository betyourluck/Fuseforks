import { createApp } from "vue";

// Vue Flow の既定スタイル。style.css の上書きより先に読み込む必要がある。
import "@vue-flow/core/dist/style.css";
import "@vue-flow/core/dist/theme-default.css";
import "./style.css";

import App from "./App.vue";

createApp(App).mount("#app");
