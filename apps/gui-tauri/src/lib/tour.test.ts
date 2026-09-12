/**
 * 初回起動のナビゲーション（{@link shouldShowTour} ほか）の規律。
 *
 * ここで固定するのは 3 点:
 * - **初回だけ出す** — 見た印が無く、使った痕跡（モデル / サーヴァント）も無いとき。
 *   更新で初めて機構が入った人に「知っている手順」を教えない
 * - **スポットライトは画面外へ滲まない** — 広げる側を端で詰める
 * - **カードは画面に収まる側へ置き、三角は対象の中心を追う** — 寄せた瞬間に何も
 *   指さなくなる固定位置にしない
 *
 * 移植元（AppPromoVideo / Lorekeel）の実測で詰めた値をそのまま写している。
 */
import { describe, expect, it } from "vitest";

import { TOUR_STEPS, placeCard, shouldShowTour, spotlightBox, spotlightUnion } from "./tour";

const VP = { width: 1920, height: 1032 }; // 利用者のスクリーンショット（2026-09-09）の窓
const CARD = { width: 352, height: 200 };

describe("初回の判定", () => {
  it("印が無く、モデルもサーヴァントも無いときだけ出す", () => {
    expect(shouldShowTour({ done: false, templateCount: 0, agentCount: 0 })).toBe(true);
  });

  it("一度見たら出さない", () => {
    expect(shouldShowTour({ done: true, templateCount: 0, agentCount: 0 })).toBe(false);
  });

  /** 更新で初めて機構が入った人に「知っている手順」を教えない。 */
  it("既に使っている痕跡があれば出さない", () => {
    expect(shouldShowTour({ done: false, templateCount: 1, agentCount: 0 })).toBe(false);
    expect(shouldShowTour({ done: false, templateCount: 0, agentCount: 3 })).toBe(false);
  });
});

describe("スポットライト", () => {
  it("対象を少し広げる", () => {
    expect(spotlightBox({ left: 100, top: 50, width: 200, height: 40 }, 8)).toEqual({
      left: 92,
      top: 42,
      width: 216,
      height: 56,
    });
  });

  /** 画面外へ滲まない（左上が負にならない）。 */
  it("画面の端では広げる側を詰める", () => {
    const b = spotlightBox({ left: 3, top: 0, width: 50, height: 20 }, 8);
    expect(b.left).toBe(0);
    expect(b.top).toBe(0);
    expect(b.width).toBe(61); // 50 + 3（詰めた分）+ 8
    expect(b.height).toBe(28);
  });

  /** 絆の歩は「一覧の本体 + 地図のキャンバス」を束ねて間を覆う（包む div を足さない）。 */
  it("複数の矩形を束ねて間を覆う", () => {
    const u = spotlightUnion([
      { left: 20, top: 60, width: 300, height: 700 },
      { left: 360, top: 80, width: 560, height: 500 },
    ]);
    expect(u).toEqual({ left: 20, top: 60, width: 900, height: 700 });
  });

  /** `v-if` で消えている要素は (0,0,0,0) を返す — 束ねると左上へ引っ張られるので落とす。 */
  it("0 寸の矩形は束ねに入れない", () => {
    const u = spotlightUnion([
      { left: 0, top: 0, width: 0, height: 0 },
      { left: 400, top: 300, width: 100, height: 50 },
    ]);
    expect(u).toEqual({ left: 400, top: 300, width: 100, height: 50 });
  });

  it("対象が 1 つも無ければ null（呼ぶ側が代わりを決める）", () => {
    expect(spotlightUnion([])).toBeNull();
    expect(spotlightUnion([{ left: 0, top: 0, width: 0, height: 0 }])).toBeNull();
  });
});

describe("カードの置き場", () => {
  it("入るなら下に置く", () => {
    const p = placeCard({ left: 100, top: 100, width: 200, height: 40 }, VP, CARD, 14);
    expect(p.side).toBe("below");
    expect(p.top).toBe(154);
  });

  it("下に入らなければ上", () => {
    const p = placeCard({ left: 100, top: 900, width: 200, height: 40 }, VP, CARD, 14);
    expect(p.side).toBe("above");
    expect(p.top).toBe(900 - 14 - CARD.height);
  });

  /** 上下に入らない縦長の対象（一覧 + 地図の束ね）は横へ。 */
  it("上下に入らなければ右", () => {
    const p = placeCard({ left: 20, top: 10, width: 300, height: 1000 }, VP, CARD, 14);
    expect(p.side).toBe("right");
    expect(p.left).toBe(334);
  });

  it("右にも入らなければ左", () => {
    const p = placeCard({ left: 1500, top: 10, width: 400, height: 1000 }, VP, CARD, 14);
    expect(p.side).toBe("left");
    expect(p.left).toBe(1500 - 14 - CARD.width);
  });

  /** 画面からはみ出さない。 */
  it("右端の対象でもカードは画面内に収まる", () => {
    const p = placeCard({ left: 1830, top: 100, width: 80, height: 40 }, VP, CARD, 14);
    expect(p.left + CARD.width).toBeLessThanOrEqual(VP.width);
  });

  /** 寄せても三角は対象の中心を指し続ける（固定だと寄せた瞬間に何も指さない）。 */
  it("三角は対象の中心を追う", () => {
    const spot = { left: 1830, top: 100, width: 80, height: 40 };
    const p = placeCard(spot, VP, CARD, 14);
    const center = spot.left + spot.width / 2;
    expect(p.left + p.arrow).toBeCloseTo(center, 0);
  });
});

describe("歩数", () => {
  it("9 歩で、順序が案内の順序（利用者の並び）", () => {
    expect(TOUR_STEPS).toEqual([
      "models",
      "servant",
      "kizuna",
      "start",
      "chat",
      "titlebar",
      "board",
      "stats",
      "workdir",
    ]);
  });
});
