/// <reference types="vite/client" />

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<{}, {}, any>;
  export default component;
}

/**
 * ビルド時に埋め込まれる版番号（`vite.config.ts` の `define`）。
 * 直近の git タグから `v` を落としたもので、タグが無ければ `0.0.0`。
 */
declare const __APP_VERSION__: string;
