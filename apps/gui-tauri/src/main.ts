import { createApp } from "vue";

import "./style.css";

import App from "./App.vue";
import { i18n } from "./i18n";
import { keepsNativeMenu } from "./lib/contextMenu";

// 配布ビルドでは WebView の既定右クリックメニューを抑止する。判定の理由は
// `lib/contextMenu.ts` の doc に書いてある（dev で残すのは右クリック →「検証」が
// 開発者ツールの入口だから。`import.meta.env.DEV` はビルド時に畳まれる）。
if (!import.meta.env.DEV) {
  window.addEventListener("contextmenu", (event) => {
    const hasSelection = Boolean(window.getSelection()?.toString());
    if (!keepsNativeMenu(event.target, hasSelection)) event.preventDefault();
  });
}

const app = createApp(App);
app.use(i18n);

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
