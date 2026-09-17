/**
 * 条例へ挿入する黒板の節を、**読み手が固定している綴り**へ機械で縛る（2026-09-17）。
 *
 * 節の文面と黒板タブの読み手は別々のファイルに居て、ずれても型検査にも実行時にも掛からない。
 * ずれると「条例どおりに書いた付箋が『その他』や孤児へ落ちる」か、もっと悪いと
 * 「挿入したのに案内が消えない」になる。文の良し悪しは留めない — 留めるのは綴りと配線だけ。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import { NOTE_SEPARATOR, STATES, splitNoteName } from "./blackboardLanes";
import {
  BLACKBOARD_DIR,
  blackboardSection,
  hasBlackboardSection,
  withBlackboardSection,
} from "./blackboardOrdinance";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const LANGS = ["ja", "en"] as const;

describe("blackboardOrdinance: 節の綴り", () => {
  it("フォルダ名はコアの読み手と同じ綴り", () => {
    const rust = read("../../../../crates/fuseforks-core/src/blackboard.rs");
    expect(rust).toContain(`pub const BLACKBOARD_DIR: &str = "${BLACKBOARD_DIR}";`);
  });

  it.each(LANGS)("%s: 状態のフォルダを全部、コードの綴りのまま名指しする", (lang) => {
    const text = blackboardSection(lang);
    for (const state of STATES) {
      expect(text, state).toContain(`\`${state}\``);
    }
    // 着手時の置き場と、終わった付箋の置き場はパスの形で出る。
    expect(text).toContain(`${BLACKBOARD_DIR}/${STATES[0]}/`);
    expect(text).toContain(`${BLACKBOARD_DIR}/${STATES[STATES.length - 1]}/`);
    // 状態の数を文で言うなら、定数の数と一致する。
    expect(text).toContain(String(STATES.length));
  });

  it.each(LANGS)("%s: 区切りが読み手と同じ。まとめ.md は案内しない（Spec 55 D5）", (lang) => {
    const text = blackboardSection(lang);
    expect(text).toContain(`\`${NOTE_SEPARATOR}\``);
    expect(text).not.toContain("まとめ.md");
  });

  it.each(LANGS)("%s: 文面の例のファイル名を読み手が持ち主へ割れる", (lang) => {
    // 例は `blackboard/doing/<名前> - <仕事>.md` の形で 1 つ入っている。
    const text = blackboardSection(lang);
    const m = text.match(new RegExp(`${BLACKBOARD_DIR}/${STATES[0]}/([^\`<]+\\.md)`));
    expect(m, "例のパス").not.toBeNull();
    const owner = splitNoteName(m![1]).key;
    expect(owner.length).toBeGreaterThan(0);
    expect(owner).not.toContain(NOTE_SEPARATOR.trim());
    expect(m![1].startsWith(`${owner}${NOTE_SEPARATOR}`)).toBe(true);
  });
});

describe("blackboardOrdinance: 判定と挿入", () => {
  it("空の条例と、黒板に触れない条例には節が無い", () => {
    expect(hasBlackboardSection("")).toBe(false);
    expect(hasBlackboardSection("# 村の条例\n- 丁寧に話す\n- 黒板を大事にする")).toBe(false);
  });

  it.each(LANGS)("%s: 挿入した節は、判定が「ある」と読む（案内が消える）", (lang) => {
    expect(hasBlackboardSection(blackboardSection(lang))).toBe(true);
  });

  it("見出しを書き換えても、置き場の綴りが残っていれば「ある」", () => {
    const edited = blackboardSection("ja").replace("## 村の黒板", "## うちの掲示板の決まり");
    expect(hasBlackboardSection(edited)).toBe(true);
  });

  it("空の条例にはそのまま入り、あれば空行 1 つで区切って末尾へ足す", () => {
    const section = blackboardSection("ja");
    expect(withBlackboardSection("", "ja")).toBe(section);
    expect(withBlackboardSection("  \n", "ja")).toBe(section);
    expect(withBlackboardSection("# 条例\n- 規則\n\n\n", "ja")).toBe(`# 条例\n- 規則\n\n${section}`);
  });

  it("既に節があれば 1 字も変えない（二重に入れない）", () => {
    const once = withBlackboardSection("# 条例", "ja");
    expect(withBlackboardSection(once, "ja")).toBe(once);
    expect(withBlackboardSection(once, "en")).toBe(once);
  });
});

describe("blackboardOrdinance: 配線", () => {
  const dialog = read("../components/OrdinanceDialog.vue");
  const pane = read("../components/BlackboardPane.vue");
  const app = read("../App.vue");

  it("条例ダイアログのボタンは純関数で挿入し、節があれば押せない", () => {
    expect(dialog).toContain('from "../lib/blackboardOrdinance"');
    expect(dialog).toMatch(/text\.value = withBlackboardSection\(text\.value,/);
    expect(dialog).toMatch(/const hasBoardSection = computed\(\(\) => hasBlackboardSection\(text\.value\)\);/);
    const at = dialog.indexOf("data-insert-blackboard");
    expect(at, "data-insert-blackboard のボタン").toBeGreaterThan(-1);
    const button = dialog.slice(at, dialog.indexOf("</button>", at));
    expect(button).toContain("hasBoardSection");
    expect(button).toContain('@click="insertBoardSection"');
    // 挿入は下書きまで。ボタンから保存を呼ばない。
    expect(dialog).not.toMatch(/function insertBoardSection\(\): void \{[^}]*save/);
  });

  it("黒板タブの案内は、付箋が 0 枚で条例に節が無いと分かったときだけ出る", () => {
    expect(pane).toMatch(/ordinanceLacksBoard\.value = !hasBlackboardSection\(await readOrdinance\(\)\);/);
    expect(pane).toMatch(
      /v-if="loaded && notes\.length === 0 && ordinanceLacksBoard"\s+data-ordinance-hint/,
    );
    // 読めなかったときは「無い」と言わない。
    expect(pane).toMatch(/\} catch \{\s+ordinanceLacksBoard\.value = false;/);
    expect(pane).toContain("emit('openOrdinance')");
  });

  it("案内のボタンが条例ダイアログを開く", () => {
    expect(app).toMatch(/<BlackboardPane[^>]*@open-ordinance="ordinanceOpen = true"/s);
  });

  it("辞書の鍵が ja / en に揃い、初回案内の黒板の歩がボタンの名前を指す", () => {
    for (const loc of LANGS) {
      const dict = JSON.parse(read(`../locales/${loc}.json`)) as {
        ordinance: Record<string, string>;
        blackboard: Record<string, string>;
        tour: { steps: { board: { body: string } } };
      };
      for (const key of ["insertBlackboard", "insertBlackboardTitle", "insertBlackboardDone"]) {
        expect(dict.ordinance[key], `${loc} ordinance.${key}`).toBeTruthy();
      }
      for (const key of ["noOrdinanceSection", "openOrdinance"]) {
        expect(dict.blackboard[key], `${loc} blackboard.${key}`).toBeTruthy();
      }
      // 案内文と空表示の案内は、ボタンのラベルを逐語で名指しする（改名で片方だけ腐らない）。
      expect(dict.tour.steps.board.body, loc).toContain(dict.ordinance.insertBlackboard);
      expect(dict.blackboard.noOrdinanceSection, loc).toContain(dict.ordinance.insertBlackboard);
    }
  });
});
