import { defineConfig } from "vite";
import vue from "@vitejs/plugin-vue";
import tailwindcss from "@tailwindcss/vite";
// @ts-expect-error @types/node を入れない方針のため
import process from "node:process";
// @ts-expect-error 同上
import { execSync } from "node:child_process";

const host = process.env.TAURI_DEV_HOST;

/**
 * ビルド時に git の直近タグを取り込む（`__APP_VERSION__` で参照）。
 *
 * **`tauri.conf.json` の `version` とは独立**。あちらは配布物（installer）に
 * 乗る版で、CI がタグから書き換える。こちらは**画面に出す版**で、
 * タグを手で bump しなくても追従する。
 *
 * **タグが無い / git が無いときは `0.0.0`**。空文字にして非表示にする手もあるが、
 * ステータスバーの目的は**撮った画面がどの状態かを示すこと**なので、
 * 「版が出ていない」と「版が 0.0.0 である」を画面上で区別できるほうがよい
 * （`0.0.0` は「まだタグを打っていない開発ビルド」の印として読める）。
 * `tauri.conf.json` の `0.1.0` へ落とさないのは、**打っていないリリースを
 * 名乗ることになる**ため。
 */
function gitVersion(): string {
  try {
    const tag = execSync("git describe --tags --abbrev=0", {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
    // 表示は `Version: 0.1.0`。タグの `v` は表記であって版の一部ではない。
    return tag.replace(/^v/, "") || "0.0.0";
  } catch {
    return "0.0.0";
  }
}

// https://vite.dev/config/
export default defineConfig(() => ({
  plugins: [vue(), tailwindcss()],

  define: {
    __APP_VERSION__: JSON.stringify(gitVersion()),
  },

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
