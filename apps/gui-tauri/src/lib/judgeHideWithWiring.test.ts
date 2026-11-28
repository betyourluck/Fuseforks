/**
 * 判断役の「非表示に連動する」（2026-11-29 利用者要望）の**配線**を機械で留める。
 *
 * 規則（連動先が隠れているか）は `judges.ts` の純関数で単体が留めている。ここで見るのは、
 * その規則が地図の 3 箇所（ノード・地図に載る id・判断役 → サーヴァントの破線）に実際に
 * 通っていることと、更新の形がいつも連動先を写すこと。どちらも型検査には掛からない —
 * `state.judges` のままでも、`hideWithGroup: null` と書いても型は通る。
 *
 * 見るのは 4 つ:
 * - 地図は判断役を `shownJudges` から描く（`state.judges` を直接回さない）
 * - 判断役を更新する呼び出しはすべて `judgeSpecOf` を通す（写し忘れると、名前を直しただけ・
 *   有効を切り替えただけで連動が外れる）
 * - 編集ダイアログの選択欄は一番上が「なし」で、選んだ時点で保存する
 * - 画面でこの欄を読むのは判定と組み立て（`judges.ts`）・編集（`JudgeDialog.vue`）・
 *   新規作成（`JudgeList.vue`）・型（`types.ts`）だけ — 会話・宛先・起動の側は読まない（表示だけ）
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readdirSync, readFileSync, statSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

const SRC = fileURLToPath(new URL("../", import.meta.url));

function read(rel: string): string {
  return readFileSync(`${SRC}${rel}`, "utf8");
}

/** `src/` の下の .ts / .vue（テストを除く）を相対パスで返す。 */
function sourceFiles(dir = ""): string[] {
  const out: string[] = [];
  for (const name of readdirSync(`${SRC}${dir}`) as string[]) {
    const rel = `${dir}${name}`;
    if (statSync(`${SRC}${rel}`).isDirectory()) out.push(...sourceFiles(`${rel}/`));
    else if (/\.(ts|vue)$/.test(name) && !name.endsWith(".test.ts")) out.push(rel);
  }
  return out;
}

/** コメントの行を落とす（説明文に欄の名前を書くのは自由 — 見たいのは読んでいるか）。 */
function code(text: string): string {
  return text
    .split("\n")
    .filter((line) => !/^\s*(\/\/|\*|\/\*|<!--)/.test(line))
    .join("\n");
}

const map = read("components/TopologyMap.vue");

describe("地図は連動して隠れた判断役を描かない", () => {
  it("出す判断役は shownJudges で決める（隠しているグループの集合を渡す）", () => {
    expect(map).toContain("shownJudges(state.judges, state.groups, hiddenGroups.hidden)");
  });

  it("ノード・地図に載る id・判断役 → サーヴァントの破線の 3 箇所とも出す判断役だけを回す", () => {
    expect(map).toContain("for (const judge of shownJudgeList.value) {");
    expect(map).toContain("...shownJudgeList.value.map((j) => j.id)");
    expect(map).toContain("judgeEdges(shownJudgeList.value)");
    // 全員を回す形が残っていると、隠したはずの判断役が描かれる。
    expect(code(map)).not.toMatch(/for \(const judge of state\.judges\)/);
    expect(code(map)).not.toContain("judgeEdges(state.judges)");
  });

  it("サーヴァント → 判断役の線は地図に載る id で切る（片端が隠れた線を描かない）", () => {
    expect(map).toContain("visibleEdges(state.edges, visibleIds.value)");
  });

  it("隠れた判断役は移動のための選択からも外す（判断役だからと残さない）", () => {
    expect(map).toContain("moveSelection.value.filter((id) => ids.has(id as AgentId));");
  });
});

describe("更新の形は連動先を写す", () => {
  it("判断役を更新する呼び出しはすべて judgeSpecOf を通す", () => {
    const calls = sourceFiles()
      .flatMap((rel) => code(read(rel)).split("\n").map((line) => ({ rel, line })))
      .filter(({ line }) => line.includes("orchestrator.updateJudge("));
    // 走査が空振りしていないことの検定（0 件なら下の every が偶然通る）。
    expect(calls.length).toBeGreaterThanOrEqual(3);
    for (const { rel, line } of calls) {
      expect(line, `${rel}: ${line.trim()}`).toContain("judgeSpecOf(");
    }
  });

  it("新しく作る判断役は連動しない", () => {
    expect(read("components/JudgeList.vue")).toContain("enabled: true, hideWithGroup: null });");
  });
});

describe("編集ダイアログの選択欄", () => {
  const dialog = read("components/JudgeDialog.vue");

  it("一番上が「なし」で、続けてグループを村の並びで出す", () => {
    const select = dialog.slice(dialog.indexOf('id="judge-hide-with"'));
    const none = select.indexOf('<option value="">{{ $t("judges.hideWithNone") }}</option>');
    const groups = select.indexOf('<option v-for="group in state.groups"');
    expect(none).toBeGreaterThan(0);
    expect(groups).toBeGreaterThan(none);
  });

  it("選んだ時点で連動先だけを差し替えて保存する", () => {
    expect(dialog).toContain('@change="commitHideWith"');
    expect(dialog).toContain("judgeSpecOf(current, state.groups, { hideWithGroup: next })");
  });
});

describe("表示だけ — 画面でこの欄を読む場所", () => {
  it("判定・組み立て・編集・新規作成・型の外では読まない", () => {
    const readers = sourceFiles()
      .filter((rel) => code(read(rel)).includes("hideWithGroup"))
      .sort();
    expect(readers).toEqual([
      "components/JudgeDialog.vue",
      "components/JudgeList.vue",
      "lib/judges.ts",
      "types.ts",
    ]);
  });
});
