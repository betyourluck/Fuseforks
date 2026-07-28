import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error @types/node を入れない方針のため
import process from "node:process";

const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [vue(), tailwindcss()],

  // Tauri 開発向けの設定。`tauri dev` / `tauri build` でのみ効く。
  //
  // 1. Rust のエラーを画面クリアで流させない
  clearScreen: false,
  // 2. Tauri は固定ポートを前提にするので、空いていなければ失敗させる
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. src-tauri は Vite の監視対象から外す（cargo 側が見る）
      ignored: ["**/src-tauri/**"],
    },
  },
}));
