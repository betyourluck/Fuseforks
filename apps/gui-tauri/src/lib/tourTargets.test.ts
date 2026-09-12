/**
 * 初回の案内の**配線**を機械で留める（2026-09-13）。
 *
 * 案内の対象は `data-tour="…"` で実物の要素に付けた印。**部品同士は互いを知らない**ので、
 * 印を消しても案内は落ちず、画面中央に小さな丸が出るだけ = **静かに壊れる**。
 * 型検査にも lint にも掛からない壊れ方なので、ソースを走査して留める
 * （`chatZoomWiring.test.ts` / `editorSaveWiring.test.ts` と同じ形）。
 *
 * 見るのは 4 つ:
 * - すべての歩に印が付いた要素がある（束ねる 2 歩は印が 2 つ、他は 1 つ）
 * - 歩ごとの辞書の鍵が ja / en の両方にある（`FirstRunTour.vue` は鍵を字面で持つ）
 * - 呼び直しの入口（設定 → App）が繋がっている — 一度使った村では初回判定で出ないので、
 *   ここが消えると二度と見られない
 * - 判定が `state.ready` の後に走る — 前に数えると空の村に見え、使い込んだ村にも出る
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync, readdirSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { TOUR_STEPS } from "./tour";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const componentsDir = fileURLToPath(new URL("../components/", import.meta.url));
const componentFiles: string[] = (readdirSync(componentsDir) as string[]).filter((f) =>
  f.endsWith(".vue"),
);
/** 案内の部品自身は除く — 自分の中の `data-tour` 文字列を「対象がある」と数えないため。 */
const sources = [
  ...componentFiles.filter((f) => f !== "FirstRunTour.vue").map((f) => read(`../components/${f}`)),
  read("../App.vue"),
];
const blob = sources.join("\n");
const tourVue = read("../components/FirstRunTour.vue");
const app = read("../App.vue");
const settings = read("../components/SettingsDialog.vue");

/** 束ねて照らす歩（印が 2 つ）。それ以外は 1 つ。 */
const PAIRED: readonly string[] = ["kizuna", "titlebar"];

describe("案内の対象", () => {
  it("すべての歩に印が付いた要素がある", () => {
    const missing = TOUR_STEPS.filter((s) => !blob.includes(`data-tour="${s}"`));
    expect(missing).toEqual([]);
  });

  it("束ねる歩は印が 2 つ、それ以外は 1 つ", () => {
    for (const s of TOUR_STEPS) {
      const n = blob.split(`data-tour="${s}"`).length - 1;
      expect(n, s).toBe(PAIRED.includes(s) ? 2 : 1);
    }
  });

  /** 網自身の検出力: ソースを読めていなければ「全部ある」でも「全部無い」でも通ってしまう。 */
  it("ソースを実際に読めている", () => {
    expect(sources.length).toBeGreaterThan(10);
    expect(blob).toContain("data-tour=");
  });

  /** 空の村にカードは無いので、起動の歩はカードのトグルではなく一覧ヘッダの一括 ▶ を照らす。 */
  it("起動の歩は一覧ヘッダの一括ボタンに付いている", () => {
    const agentList = read("../components/AgentList.vue");
    expect(agentList).toMatch(/data-tour="start"\n\s+@click="runBatch"/);
  });
});

describe("案内の文言", () => {
  function get(dict: unknown, path: string): unknown {
    return path.split(".").reduce<unknown>((node, key) => {
      if (typeof node !== "object" || node === null) return undefined;
      return (node as Record<string, unknown>)[key];
    }, dict);
  }
  const KEYS = [
    "tour.welcomeTitle",
    "tour.welcomeLead",
    "tour.start",
    "tour.skip",
    "tour.next",
    "tour.back",
    "tour.finish",
    "tour.keys",
    "settings.menuTour",
    "settings.tour.heading",
    "settings.tour.intro",
    "settings.tour.show",
    "settings.tour.showNote",
    ...TOUR_STEPS.flatMap((s) => [`tour.steps.${s}.title`, `tour.steps.${s}.body`]),
  ];

  it("ja / en の両方にある", () => {
    for (const [lang, dict] of [
      ["ja", ja],
      ["en", en],
    ] as const) {
      for (const k of KEYS) expect(get(dict, k), `${lang} / ${k}`).toBeTruthy();
    }
  });

  /** `FirstRunTour.vue` は鍵を字面で持つ（組み立てると grep に掛からない）。表と歩が揃っているか。 */
  it("部品の表が全歩の鍵を字面で持つ", () => {
    for (const s of TOUR_STEPS) {
      expect(tourVue, s).toContain(`"tour.steps.${s}.title"`);
      expect(tourVue, s).toContain(`"tour.steps.${s}.body"`);
    }
  });

  /** 案内の中から登録や起動はさせない（移植元と同じ流儀）— 幕の文でそう言い切る。 */
  it("幕の文で「ここからはしない」と言う", () => {
    expect(ja.tour.welcomeLead).toContain("ここから登録や起動はしません");
    expect(en.tour.welcomeLead).toContain("nothing is registered or started from here");
  });
});

describe("案内の入口", () => {
  it("設定画面から呼び直せる（ユーザーインターフェース ＞ 案内）", () => {
    expect(settings).toContain("emit('show-tour')");
    expect(settings).toContain("page === 'tour'");
    expect(settings).toContain('"settings.menuTour"');
  });

  it("App が受けて、設定を閉じ・村の画面へ戻してから出す", () => {
    expect(app).toContain('@show-tour="replayTour"');
    // 対象の要素はダイアログの下・村の画面に居るので、開いたまま出すと照らせない。
    expect(app).toMatch(
      /function replayTour\(\)[\s\S]*?settingsOpen\.value = false[\s\S]*?view\.value = "village"[\s\S]*?showTour\.value = true/,
    );
  });

  /** `ready` は `refreshAll()` の後に立つ。その前に数えると空の村に見える。 */
  it("初回判定は state.ready が立ってから走る", () => {
    expect(app).toMatch(/\(\) => state\.ready,[\s\S]*?if \(ready\) decideTour\(\)/);
    expect(app).toContain("shouldShowTour({");
  });

  it("案内の部品は動的 import（初回にしか読まれない）", () => {
    expect(app).toMatch(/defineAsyncComponent\(\(\) => import\("\.\/components\/FirstRunTour\.vue"\)\)/);
  });
});
