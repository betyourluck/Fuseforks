/**
 * 会話の表示倍率の**配線**を機械で留める（2026-09-09）。
 *
 * 倍率は 3 つのファイルにまたがる — `useUiSettings` が `:root` へ `--chat-zoom` を置き、
 * `style.css` の `.chat-zoom` がそれを読み、`ChatPanel` がそのクラスを 2 つの要素へ付ける。
 * **変数名は手で揃えている**ので、片方だけ改名すると倍率が黙って効かなくなる
 * （`defaultEnabledTools.test.ts` が Rust と TS の表を突き合わせているのと同じ形）。
 * 型検査にも lint にも掛からない壊れ方で、実機で選んでも何も起きないという形で出る。
 *
 * **掛けない場所も同じ強さで留める。** 掛かる範囲が広がる壊れ方は「大きくなる」ので
 * 一見動いて見えるが、`.chat-panel` は container query のコンテナ
 * （`container-type: inline-size`）なので倍率が入ると `@container` の幅の評価がずれ、
 * ペインの見出しは 4 ペイン共通の 38px なので隣との段差になる。
 *
 * **値の良し悪しは見ない**（`theme.test.ts` と同じ規律）。どの倍率が読みやすいかは
 * 人が実機で決めることで、ここで刻みの妥当性を検査すると誤った確信を作る。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

/** コメントを落とす — 散文に出てくる語を定義と読み違えないため。 */
const css = read("../style.css").replace(/\/\*[\s\S]*?\*\//g, "");
const settings = read("../composables/useUiSettings.ts").replace(/\/\*[\s\S]*?\*\//g, "");
const panel = read("../components/ChatPanel.vue");
/**
 * テンプレート部分だけ（`<script>` の文字列や `<style>` の定義を数えない）。
 *
 * **閉じは `lastIndexOf`。** このファイルは `<template v-for>` や名前つきスロットを
 * 使っており `</template>` が 6 個ある — `indexOf` で切ると 762 行のスロットの閉じで
 * 止まり、入力欄（1055 行）が範囲から落ちる。実際に一度そう書いて、下の「2 つ」の
 * 検査が 1 しか数えずに赤くなった。**範囲を切る検査は、範囲の切り方自体が壊れうる。**
 */
const panelTemplate = panel.slice(
  panel.indexOf("<template>"),
  panel.lastIndexOf("</template>"),
);

const VAR = "--chat-zoom";

describe("会話の表示倍率の配線", () => {
  it("style.css の .chat-zoom が変数を読み、既定は等倍", () => {
    const rule = /\.chat-zoom\s*\{([^}]*)\}/.exec(css);
    expect(rule, ".chat-zoom の定義が style.css に無い").not.toBeNull();
    const body = rule![1];
    expect(body).toMatch(/\bzoom:\s*var\(--chat-zoom,\s*1\)/);
  });

  it("useUiSettings が同じ変数名を :root へ置く", () => {
    // 変数名の突き合わせが本体。片方を改名すると倍率が黙って効かなくなる。
    expect(settings).toContain(`setProperty("${VAR}"`);
    expect(settings).toContain("document.documentElement.style");
  });

  it("掛ける先は会話の流れと入力欄の 2 つ", () => {
    const uses = panelTemplate.match(/chat-zoom/g) ?? [];
    expect(uses).toHaveLength(2);
    // 流れ（スクロールする側）と入力欄の両方に付いていること。
    expect(panelTemplate).toMatch(/class="chat-zoom[^"]*overflow-y-auto/);
    expect(panelTemplate).toMatch(/<ChatInput[\s\S]{0,120}class="chat-zoom"/);
  });

  it("ペインの見出し行と .chat-panel には掛けない", () => {
    // 見出し行（4 ペイン共通の 38px）。
    const header = /<header[\s\S]*?>/.exec(panelTemplate);
    expect(header).not.toBeNull();
    expect(header![0]).not.toContain("chat-zoom");
    // ルート（container query のコンテナ）。
    const root = /<div class="chat-panel[^"]*"/.exec(panelTemplate);
    expect(root, ".chat-panel のルートが見つからない").not.toBeNull();
    expect(root![0]).not.toContain("chat-zoom");
  });

  it(".chat-panel は container query のコンテナのまま", () => {
    // ここが失われると上の「ルートに掛けない」理由の半分が消える。
    expect(panel).toMatch(/container-type:\s*inline-size/);
  });
});
