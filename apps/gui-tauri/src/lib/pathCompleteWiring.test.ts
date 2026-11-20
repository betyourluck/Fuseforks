/**
 * 入力欄の `@` パス補完の配線（#22 / N14）。部品をマウントする土台が無いので字面で留める。
 *
 * 留めているのは、外れても型検査に掛からず**黙って崩れる 3 つ**:
 *
 * 1. **宛先が変わったら補完を閉じる** — 閉じないと、前のサーヴァントの作業フォルダの一覧が残り、
 *    その相対パスが別のサーヴァント宛ての本文に入る（Spec 24「宛先を切り替えたら候補も切り替わる」）。
 *    宛先は Alt+↑↓ で変わり、そのときフォーカスは入力欄に残るので `blur` では閉じない
 * 2. **取り終えた一覧は、いまの宛先とトリガーを確かめてから入れる** — 取得の途中で宛先を変えると、
 *    前の宛先の一覧が後から届く。規則は `isCurrentListing`（pathComplete.test.ts の単体）
 * 3. **補完の ↑↓ は修飾なしだけ** — Alt+↑↓ はサーヴァント一覧の選択（App.vue の `onNavKey`）。
 *    `.exact` が無いと、補完を開いたまま Alt+↑↓ を押すと候補の選択も同時に動く
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const input = code(readFileSync(new URL("../components/ChatInput.vue", import.meta.url), "utf8"));

describe("宛先が変わったときの補完（#22）", () => {
  it("宛先が変わったら補完を閉じる", () => {
    expect(input).toMatch(/watch\(\s*\(\) => props\.agentId,\s*\(\) => closeCompletion\(\),?\s*\)/);
  });

  it("取り終えた一覧は、いまの宛先とトリガーを確かめてから入れる（成功も失敗も）", () => {
    const start = input.indexOf("await listWorkDirFiles(");
    expect(start, "listWorkDirFiles を待つ箇所が在ること").toBeGreaterThanOrEqual(0);
    const assign = input.indexOf("candidates.value = listing.paths", start);
    const failed = input.indexOf("candidatesFailed.value = true", start);
    expect(assign).toBeGreaterThan(start);
    expect(failed).toBeGreaterThan(assign);
    // 待った後、入れる前に確かめる。成功の枝も失敗の枝も。
    expect(input.slice(start, assign)).toContain("if (!current()) return;");
    expect(input.slice(assign, failed)).toContain("if (!current()) return;");
    // 確かめる中身は isCurrentListing の 1 実装で、いまの世代・宛先・種別を渡す。
    expect(input).toMatch(
      /const current = \(\) =>\s*isCurrentListing\(request, \{\s*generation: listingGeneration,\s*agentId: props\.agentId,\s*kind: trigger\.value\?\.kind \?\? null,\s*\}\);/,
    );
    expect(input).toContain("const request = { generation: ++listingGeneration, agentId: props.agentId };");
  });

  it("種別が変わったら取得中の印を下ろす（古い取得は届いても印に触らないので、残さない）", () => {
    const start = input.indexOf("() => trigger.value?.kind ?? null,");
    expect(start).toBeGreaterThanOrEqual(0);
    const head = input.slice(start, input.indexOf("if (kind === null) return;", start));
    expect(head).toContain("loadingCandidates.value = false");
    // 古い取得の後始末は、いまの取得のときだけ印を下ろす。
    expect(input).toMatch(/finally \{\s*if \(current\(\)\) loadingCandidates\.value = false;\s*\}/);
  });

  it("閉じると取得中の印も下ろし、古い一覧を捨てるための世代を進める", () => {
    const start = input.indexOf("function closeCompletion(): void {");
    expect(start).toBeGreaterThanOrEqual(0);
    const body = input.slice(start, input.indexOf("\n}\n", start));
    expect(body).toContain("listingGeneration += 1");
    expect(body).toContain("loadingCandidates.value = false");
  });
});

describe("補完の ↑↓ と Alt+↑↓ を分ける（N14）", () => {
  it("候補の選択は修飾なしの ↑↓ だけで動く", () => {
    expect(input).toContain('@keydown.down.exact="moveSelection(1, $event)"');
    expect(input).toContain('@keydown.up.exact="moveSelection(-1, $event)"');
    expect(input).not.toMatch(/@keydown\.(up|down)="/);
  });
});
