/**
 * ステータスバーの表示規則を機械で留める（Spec 25）。
 *
 * # 何を留めるか
 *
 * **「扉が開いているときだけ出す」の 1 点だけ。** 起点は利用者の
 * 「誰もいないのにつけっぱなしにしないように」で、それが成立するのは
 * 印が常に画面にあるわけではないときだけ — OFF でも何か出ていると、
 * **印が付いていること自体の意味が消える**。
 *
 * # 何を留めないか
 *
 * 文言・色・位置は留めない（見た目は実機で決まるもので、縛ると次に触る人が
 * 窮屈な枠へ詰め込む。`dialogShell.test.ts` と同じ判断）。
 *
 * # なぜソースの走査なのか
 *
 * この村のフロントには DOM を組むテストの土台が無く、`StatusBar` は
 * `useOrchestrator`（IPC を持つ）に依存する。**判定の分岐が 1 つしかない**
 * 種類の規則なので、その分岐が `listening` を見ていることを走査で確かめれば
 * 足りる（`roleColorTokens.test.ts` がビルド成果物を走査したのと同じ形）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

const source = readFileSync(
  fileURLToPath(new URL("./StatusBar.vue", import.meta.url)),
  "utf8",
);

describe("ステータスバーの MCP サーバー表示（Spec 25）", () => {
  it("**`listening` で出し分ける** — `enabled` では出さない", () => {
    // 設定が ON でもポートが埋まっていれば開いていない。見せたいのは
    // 「実際に受け付けている」ことで、「そう設定してある」ことではない。
    expect(source).toContain("state.mcpHost?.listening === true");
    expect(source).not.toContain("state.mcpHost?.enabled");
  });

  it("**開いているときだけ出す**（OFF のときの印を持たない）", () => {
    expect(source).toContain('v-if="listening"');
    // `v-else` の枝は右寄せを保つ詰め物だけで、**文言を 1 つも持たない**。
    // 自己閉じでも閉じタグでも拾えるよう、要素 1 つ分だけを切り出して見る。
    const elseBranch = source.match(/v-else[\s\S]{0,200}?(\/>|<\/span>)/)?.[0] ?? "";
    expect(elseBranch).not.toBe("");
    expect(elseBranch).not.toContain("$t(");
  });

  it("ポート番号を添える（どの扉かが分かる）", () => {
    expect(source).toContain("statusBar.mcpHost");
    expect(source).toContain("port:");
  });
});
