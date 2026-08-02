import { createApp } from "vue";

// Vue Flow の既定スタイル。style.css の上書きより先に読み込む必要がある。
// controls は自前の style.css を別に持つ — これを読まないと `<Controls />` の
// ボタンは DOM に居ても寸法・背景を持たず、見えない（実際に起きた）。
import "@vue-flow/core/dist/style.css";
import "@vue-flow/core/dist/theme-default.css";
import "@vue-flow/controls/dist/style.css";
import "./style.css";

import App from "./App.vue";

const app = createApp(App);

/**
 * 境界で捕まえきれなかった例外の最後の受け皿。
 *
 * 既定では Vue が console へ出して終わりになり、画面には何も出ないまま
 * 部分木だけが消える。何が起きたのか分からない状態を残さない。
 */
app.config.errorHandler = (error, _instance, info) => {
  console.error(`[Vue] ${info}`, error);
};

app.mount("#app");
