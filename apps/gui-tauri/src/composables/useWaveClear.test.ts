/**
 * 作業状況タブの表示クリア（{@link useWaveClear}）の規律。
 *
 * 固定するのは 4 点:
 * - **確認待ちと実行中は押しても隠れない** — 承認し忘れ・動いているものの見失いを作らない
 * - **押した後に終わった波は隠れない** — 見ている最中に列が抜けない
 * - **鍵に開始時刻を含む** — 再起動で planId が振り直されても新しい波は隠れない
 * - **押し出された波の鍵は次のクリアで落ちる** — 保存が育ち続けない
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import type { PlanTaskState, PlanWaveRecord, PlanWaveState } from "../types";

const STORAGE_KEY = "fuseforks.wavesCleared.v1";

function fakeStorage(initial?: Record<string, string>) {
  const map = new Map(Object.entries(initial ?? {}));
  return {
    writes: 0,
    getItem(key: string): string | null {
      return map.get(key) ?? null;
    },
    setItem(key: string, value: string): void {
      this.writes += 1;
      map.set(key, value);
    },
    dump(key: string): string | null {
      return map.get(key) ?? null;
    },
  };
}

/** モジュール単位の状態を持つので、テストごとに新品を読み込む。 */
async function freshModule(storage: ReturnType<typeof fakeStorage>) {
  vi.resetModules();
  vi.stubGlobal("localStorage", storage);
  return await import("./useWaveClear");
}

function wave(
  planId: number,
  startedAtMs: number,
  state: PlanWaveState,
  taskStates: PlanTaskState[],
): PlanWaveRecord {
  return {
    planId,
    agentId: "agent_1",
    wave: 1,
    state,
    startedAtMs,
    tasks: taskStates.map((s, i) => ({
      to: `agent_${i + 2}`,
      state: s,
      elapsedMs: null,
      msgChars: 10,
    })),
    bundleChars: null,
    elapsedMs: null,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("隠してよい波", () => {
  it("確認待ちと実行中は隠さず、破棄と解決済みは隠す", async () => {
    const { isSettled } = await freshModule(fakeStorage());

    expect(isSettled(wave(1, 100, "pending", []))).toBe(false);
    // 1 つでも走っていれば隠さない。残りが解決済みでも同じ。
    expect(isSettled(wave(2, 200, "dispatched", ["answered", "running"]))).toBe(false);
    expect(isSettled(wave(3, 300, "dispatched", ["answered", "timed_out"]))).toBe(true);
    expect(isSettled(wave(4, 400, "discarded", []))).toBe(true);
  });
});

describe("表示クリア", () => {
  it("保存が無ければ何も隠さず、読むだけでは書かない", async () => {
    const storage = fakeStorage();
    const { useWaveClear } = await freshModule(storage);

    expect(useWaveClear().isHidden(wave(1, 100, "dispatched", ["answered"]))).toBe(false);
    await nextTick();
    expect(storage.writes).toBe(0);
  });

  it("押した時点で終わっている波だけを隠す", async () => {
    const { useWaveClear } = await freshModule(fakeStorage());
    const waveClear = useWaveClear();

    const done = wave(1, 100, "dispatched", ["answered"]);
    const running = wave(2, 200, "dispatched", ["running"]);
    const pending = wave(3, 300, "pending", []);
    waveClear.clear([done, running, pending]);

    expect(waveClear.isHidden(done)).toBe(true);
    expect(waveClear.isHidden(running)).toBe(false);
    expect(waveClear.isHidden(pending)).toBe(false);
  });

  it("押した後に終わった波は隠れない", async () => {
    const { useWaveClear } = await freshModule(fakeStorage());
    const waveClear = useWaveClear();

    // **開始時刻は終わった波より古い。** 時刻の境界で切る実装はこの波を隠すので赤になる。
    const olderRunning = wave(1, 100, "dispatched", ["running"]);
    const newerDone = wave(2, 200, "dispatched", ["answered"]);
    waveClear.clear([olderRunning, newerDone]);

    const olderFinished = wave(1, 100, "dispatched", ["answered"]);
    expect(waveClear.isHidden(olderFinished)).toBe(false);
    expect(waveClear.isHidden(newerDone)).toBe(true);
  });

  it("再起動で同じ planId が振られても、開始時刻が違えば別の波", async () => {
    const { useWaveClear } = await freshModule(fakeStorage());
    const waveClear = useWaveClear();

    waveClear.clear([wave(1, 100, "dispatched", ["answered"])]);
    expect(waveClear.isHidden(wave(1, 9_000, "dispatched", ["answered"]))).toBe(false);
  });

  it("保存され、読み戻せる。押し出された波の鍵は次のクリアで落ちる", async () => {
    const storage = fakeStorage();
    const first = await freshModule(storage);
    first.useWaveClear().clear([
      wave(1, 100, "dispatched", ["answered"]),
      wave(2, 200, "discarded", []),
    ]);
    await nextTick();
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "[]")).toEqual(["1:100", "2:200"]);

    const second = await freshModule(storage);
    const waveClear = second.useWaveClear();
    expect(waveClear.isHidden(wave(2, 200, "discarded", []))).toBe(true);

    // 1 番の波はリングから押し出され、3 番が新しく終わった。
    waveClear.clear([wave(2, 200, "discarded", []), wave(3, 300, "dispatched", ["answered"])]);
    await nextTick();
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "[]")).toEqual(["2:200", "3:300"]);
  });

  it("戻すとすべて見える", async () => {
    const { useWaveClear } = await freshModule(fakeStorage());
    const waveClear = useWaveClear();
    const done = wave(1, 100, "dispatched", ["answered"]);

    waveClear.clear([done]);
    waveClear.restore();
    expect(waveClear.isHidden(done)).toBe(false);
  });

  it("文字列でない保存値はそのぶんだけ捨て、壊れた JSON でも開ける", async () => {
    const mixed = await freshModule(
      fakeStorage({ [STORAGE_KEY]: JSON.stringify(["1:100", 2, null, "3:300"]) }),
    );
    const waveClear = mixed.useWaveClear();
    // **健全な鍵は残る。** 全部捨てる実装と区別が付くように、壊れた要素と正しい要素を混ぜている。
    expect(waveClear.isHidden(wave(1, 100, "dispatched", ["answered"]))).toBe(true);
    expect(waveClear.isHidden(wave(3, 300, "dispatched", ["answered"]))).toBe(true);

    const broken = await freshModule(fakeStorage({ [STORAGE_KEY]: "[壊れている" }));
    expect(broken.useWaveClear().isHidden(wave(1, 100, "dispatched", ["answered"]))).toBe(false);
  });
});
