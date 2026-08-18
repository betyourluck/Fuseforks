/**
 * 統計画面（Spec 39 P3）の表示規則を機械で留める。
 *
 * 見るのは 3 点（Spec の Tasks）:
 * 1. **判定が `recordedSince` を見ているか** — 「記録が無い」と「0」を分ける唯一の
 *    材料。`turns === 0` で判定すると、記録はあるが払っていない会話（未来）と
 *    この版より前の会話（過去）が同じ表示になる
 * 2. **3 ペインを `v-show` で残しているか**（`App.vue`）— `v-if` に戻すと入力欄の
 *    途中の文・選択中の個体が捨てられる。型検査にもテストにも掛からない退行なので
 *    ソースを走査する（`defaultEnabledTools.test.ts` と同じ形）
 * 3. **`turnRecorded` の受け手が IPC を呼ばないか** — 数を進めるだけで、取り直すのは
 *    統計画面（`v-if` で足される間だけ）。ここで `sessionStats` を呼ぶと村の表示中に
 *    毎ターン IPC が走る
 *
 * 併せて描画の純関数（棒の線形・0 の扱い）と、終わり方の鍵が辞書 ja/en に揃っているか。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import type { SeriesPoint, StatsReport, TurnStop } from "../types";
import {
  STOP_LABEL_KEYS,
  formatDuration,
  formatPercent,
  inputBreakdown,
  seriesBars,
  statsNotice,
  stopTone,
} from "./statsView";

const here = dirname(fileURLToPath(import.meta.url));
const read = (rel: string) => readFileSync(resolve(here, rel), "utf-8");

function report(overrides: Partial<StatsReport["scopeMeta"]> & { turns?: number }): StatsReport {
  const turns = overrides.turns ?? 0;
  return {
    scope: { kind: "session", sessionId: "s1" },
    scopeMeta: {
      recordedSince: overrides.recordedSince ?? null,
      sessions: overrides.sessions ?? [],
    },
    totals: {
      turns,
      failed: 0,
      prompt: 0,
      cached: 0,
      completion: 0,
      reasoning: 0,
      cacheWrite: 0,
      cacheWrite1h: 0,
      effective: 0,
      cacheRate: 0,
      outputShare: 0,
      avgElapsedMs: 0,
      avgTokensPerTurn: 0,
    },
    byAgent: [],
    byStop: [],
    series: null,
  };
}

describe("statsNotice — 判定は recordedSince だけを見る", () => {
  it("報告が無ければ loading", () => {
    expect(statsNotice(null)).toBe("loading");
  });

  it("recordedSince が null なら、turns がいくつでも empty（記録が無い ≠ 0）", () => {
    expect(statsNotice(report({ recordedSince: null, turns: 0 }))).toBe("empty");
    // 集計側は recordedSince が null なら turns も 0 になるが、判定が turns に
    // 寄りかかっていないことをここで固定する。
    expect(statsNotice(report({ recordedSince: null, turns: 3 }))).toBe("empty");
  });

  it("recordedSince があれば ready（turns が 0 でも表を出す — 記録はある）", () => {
    expect(statsNotice(report({ recordedSince: 1, turns: 0 }))).toBe("ready");
  });
});

/**
 * `v-show="view === 'village'"` を持つグリッドの範囲（開始タグ〜対応する閉じタグ）。
 * `<div>` の深さを数える — インデントで判定すると整形で壊れる。
 */
function villageGridRange(app: string): [number, number] {
  const marker = app.indexOf(`v-show="view === 'village'"`);
  if (marker < 0) throw new Error("グリッドの v-show が見つかりません");
  const open = app.lastIndexOf("<div", marker);
  const re = /<div\b|<\/div>/g;
  re.lastIndex = open;
  let depth = 0;
  let m: RegExpExecArray | null;
  while ((m = re.exec(app))) {
    if (m[0] === "</div>") {
      depth -= 1;
      if (depth === 0) return [open, m.index];
    } else {
      depth += 1;
    }
  }
  throw new Error("グリッドの閉じタグが見つかりません");
}

describe("App.vue — 全画面の層はグリッドの外（Spec 39 P3 の退行の回帰）", () => {
  const app = read("../App.vue");
  const [start, end] = villageGridRange(app);
  const inside = app.slice(start, end);

  /**
   * **`display: none` の親の中では `fixed` でも描画されない。** グリッドは
   * `v-show` で隠れるので、内側に置いた全画面の層は統計画面で消える。
   * 実機で踏んだのはダイアログ（開いたのに見えず、村へ戻ると現れる = 二度押しを誘う）
   * だが、**重いのは `ConfirmHost`** — 確認が出ないと `askConfirm` が解決せず、
   * 統計画面から窓を閉じられなくなる。
   */
  it.each([
    ["<ToastHost", "トースト"],
    ["<ConfirmHost", "確認ダイアログ（出ないと窓が閉じられない）"],
    ["<OrdinanceDialog", "条例"],
    ["<RoleDialog", "役職"],
    ["<CommandApprovalDialog", "コマンド承認"],
    ["<McpDialog", "MCP"],
    ["<ScheduleDialog", "予定"],
    ["<SettingsDialog", "システム設定"],
    ['v-if="!state.ready"', "初期化の覆い"],
  ])("%s（%s）はグリッドの外にある", (needle) => {
    expect(app).toContain(needle);
    expect(inside).not.toContain(needle);
  });
});

describe("App.vue — 3 ペインは v-show で残し、StatsView は v-if で足す", () => {
  const app = read("../App.vue");

  it("グリッドは v-show（v-if だと入力途中・選択・視点が捨てられる）", () => {
    const grid = app.match(/<div\s+v-show="view === 'village'"[^>]*class="grid/);
    expect(grid, "3 ペインのグリッドに v-show=\"view === 'village'\" があること").not.toBeNull();
    expect(app).not.toMatch(/v-if="view === 'village'"/);
  });

  it("StatsView は v-if（開いている間だけ sessionStats を叩く）", () => {
    expect(app).toMatch(/<StatsView\s+v-if="view === 'stats'"/);
  });

  it("view は保存しない（起動は必ず村から）", () => {
    expect(app).toMatch(/const view = ref<"village" \| "stats">\("village"\)/);
    expect(app).not.toMatch(/localStorage[^\n]*view/);
  });
});

describe("入口はフッターに 1 つ（Spec 39・2026-08-16 に TitleBar から移した）", () => {
  const titleBar = read("../components/TitleBar.vue");
  const statusBar = read("../components/StatusBar.vue");
  const app = read("../App.vue");

  it("TitleBar は統計の入口を持たない（あの列はダイアログの入口だけ）", () => {
    expect(titleBar).not.toContain("toggle-stats");
    expect(titleBar).not.toContain("data-stats-toggle");
  });

  it("StatusBar が入口を持ち、開いている間は緑（--color-run）で発光する", () => {
    expect(statusBar).toContain("data-stats-toggle");
    expect(statusBar).toMatch(/toggle-stats/);
    expect(statusBar).toMatch(/\.stats-btn\.is-on\s*\{[^}]*--color-run/);
    // 色だけに頼らない（帯が細いので、状態は aria でも読めるようにする）。
    expect(statusBar).toContain("aria-pressed");
  });

  it("App.vue は StatusBar へ statsActive を渡し、そこから切り替える", () => {
    expect(app).toMatch(/<StatusBar\s+:stats-active="view === 'stats'"/);
  });

  /**
   * 統計を見ている間、ダイアログの入口は塞ぐ（利用者判断）。押せてしまうと
   * 「開いたつもりで見えない」か「村へ戻ってから現れる」になり、二度押しを誘う。
   * **窓操作は塞がない** — 最小化・最大化・閉じるはどの面でも要る。
   */
  it("統計中はダイアログの入口 6 つが disabled になり、窓操作は残る", () => {
    // `</button>` で切る — 切らないと最後のチャンクに `<style>` が混ざり、
    // `.tb-btn:disabled` のスタイル定義を「閉じるボタンの属性」と読んでしまう。
    const buttons = titleBar
      .split("<button")
      .slice(1)
      .map((b: string) => b.split("</button>")[0]);
    const dialogEntries = buttons.filter((b: string) => /@click="emit\('open-/.test(b));
    const windowControls = buttons.filter((b: string) => /@click="win\(/.test(b));
    expect(dialogEntries).toHaveLength(6);
    expect(windowControls).toHaveLength(3);
    for (const entry of dialogEntries) {
      expect(entry).toContain(':disabled="props.statsActive"');
    }
    for (const control of windowControls) {
      expect(control).not.toContain(":disabled");
    }
  });
});

describe("useOrchestrator — turnRecorded は数を進めるだけ", () => {
  const src = read("../composables/useOrchestrator.ts");

  it("受け手の case は IPC を呼ばない", () => {
    const start = src.indexOf('case "turnRecorded":');
    expect(start, "turnRecorded の受け手があること").toBeGreaterThan(-1);
    const end = src.indexOf("break;", start);
    const body = src.slice(start, end);
    expect(body).toMatch(/turnRecordedTick \+= 1/);
    expect(body).not.toMatch(/ipc\./);
    expect(body).not.toMatch(/sessionStats/);
  });
});

describe("seriesBars — 線形・0 の扱い", () => {
  const point = (effective: number, kind: TurnStop["kind"] = "completed"): SeriesPoint => ({
    tsMs: effective,
    agentId: "a",
    effective,
    prompt: 0,
    completion: 0,
    stop: kind === "failed" ? { kind, code: "X" } : kind === "repeat" ? { kind, tool: "t" } : ({ kind } as TurnStop),
  });

  it("空なら空", () => {
    expect(seriesBars([], 100, 50)).toEqual([]);
    expect(seriesBars([point(1)], 0, 50)).toEqual([]);
  });

  it("最大値が高さいっぱい、半分は半分（線形）", () => {
    const bars = seriesBars([point(100), point(50)], 200, 100, 0);
    expect(bars[0].height).toBe(100);
    expect(bars[0].y).toBe(0);
    expect(bars[1].height).toBe(50);
    expect(bars[1].y).toBe(50);
    expect(bars[0].width).toBe(100);
    expect(bars[1].x).toBe(100);
  });

  it("全部 0 なら高さ 0 の棒（描画領域を空にしない）", () => {
    const bars = seriesBars([point(0), point(0)], 100, 40);
    expect(bars).toHaveLength(2);
    expect(bars.every((b) => b.height === 0 && b.y === 40)).toBe(true);
  });

  it("色調は is_failure と同じ境界（完走の 3 値だけ ok）", () => {
    expect(seriesBars([point(1, "failed")], 10, 10)[0].tone).toBe("fail");
    expect(stopTone("completed")).toBe("ok");
    expect(stopTone("repeat")).toBe("ok");
    expect(stopTone("tool_limit")).toBe("ok");
    expect(stopTone("interrupted")).toBe("fail");
    expect(stopTone("budget_exhausted")).toBe("fail");
    expect(stopTone("reserve_short")).toBe("fail");
  });
});

describe("辞書 — 終わり方 7 値の鍵が ja / en に揃っている", () => {
  const ja = JSON.parse(read("../locales/ja.json")) as { stats: { stop: Record<string, string> } };
  const en = JSON.parse(read("../locales/en.json")) as { stats: { stop: Record<string, string> } };

  it("STOP_LABEL_KEYS の鍵が両方の辞書にある", () => {
    for (const key of Object.values(STOP_LABEL_KEYS)) {
      const leaf = key.replace(/^stats\.stop\./, "");
      expect(ja.stats.stop[leaf], `ja: ${key}`).toBeTruthy();
      expect(en.stats.stop[leaf], `en: ${key}`).toBeTruthy();
    }
    expect(Object.keys(STOP_LABEL_KEYS)).toHaveLength(7);
  });
});

describe("書式", () => {
  it("比と時間", () => {
    expect(formatPercent(0.1234)).toBe("12.3%");
    expect(formatPercent(Number.NaN)).toBe("—");
    expect(formatDuration(850)).toBe("850 ms");
    expect(formatDuration(12_340)).toBe("12.3 s");
    expect(formatDuration(-1)).toBe("—");
  });
});

describe("inputBreakdown", () => {
  // Spec 40 D4。**素の新規は引き算で出す** — 実測ではこれが 1 ラウンドあたり
  // 数トークンで、「未キャッシュ」の実体はほぼ全部が書き込みだった。
  it("splits the prompt into read / write / fresh", () => {
    expect(
      inputBreakdown({ prompt: 1000, cached: 800, cacheWrite: 190, cacheWrite1h: 190 }),
    ).toEqual({ cached: 800, cacheWrite: 190, cacheWrite1h: 190, fresh: 10 });
  });

  // **この版より前の会話**は cacheWrite が 0 で記録されている。そのとき fresh が
  // 過大に出るのは正しい表示 — 0 を「書き込みが無かった」と読ませない。
  it("puts everything in fresh when the write was never recorded", () => {
    const b = inputBreakdown({ prompt: 1000, cached: 800, cacheWrite: 0, cacheWrite1h: 0 });
    expect(b.fresh).toBe(200);
    expect(b.cacheWrite).toBe(0);
  });

  // 引き算が負になる入力でも 0 で止める（壊れた記録で画面を落とさない）。
  it("never returns a negative fresh count", () => {
    expect(
      inputBreakdown({ prompt: 100, cached: 90, cacheWrite: 50, cacheWrite1h: 0 }).fresh,
    ).toBe(0);
  });
});
