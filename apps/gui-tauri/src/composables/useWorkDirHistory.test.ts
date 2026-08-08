/**
 * 作業フォルダの履歴（Spec 29 の追加）の規律。
 *
 * 留めるのは 3 点:
 * - **同じパスは消さずに先頭へ寄せる**（往復する使い方が主）
 * - **上限で古いほうから落ちる**
 * - **壊れた保存値・文字列でない要素は捨てる**（ダイアログが開けなくなるより軽い害）
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

import { pushHistory, WORK_DIR_HISTORY_MAX } from "./useWorkDirHistory";

const STORAGE_KEY = "fuseforks.workDirHistory.v1";

function fakeStorage(initial?: Record<string, string>) {
  const map = new Map(Object.entries(initial ?? {}));
  return {
    getItem: (key: string) => map.get(key) ?? null,
    setItem: (key: string, value: string) => void map.set(key, value),
    dump: (key: string) => map.get(key) ?? null,
  };
}

async function freshModule(storage: ReturnType<typeof fakeStorage>) {
  vi.resetModules();
  vi.stubGlobal("localStorage", storage);
  return await import("./useWorkDirHistory");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("pushHistory", () => {
  it("新しいものが先頭に来る", () => {
    expect(pushHistory(["A"], "B")).toEqual(["B", "A"]);
  });

  it("同じパスは消さずに先頭へ寄せる（重複は作らない）", () => {
    // 往復する使い方が主なので、重複は「消す対象」ではなく「順序で表すもの」。
    expect(pushHistory(["A", "B", "C"], "C")).toEqual(["C", "A", "B"]);
  });

  it("上限を超えたら古いほうから落ちる", () => {
    const full = Array.from({ length: WORK_DIR_HISTORY_MAX }, (_, i) => `P${i}`);
    const next = pushHistory(full, "NEW");

    expect(next).toHaveLength(WORK_DIR_HISTORY_MAX);
    expect(next[0]).toBe("NEW");
    // 落ちるのは末尾（最も古い）— 先頭が落ちる実装と区別が付く入力。
    expect(next).not.toContain(`P${WORK_DIR_HISTORY_MAX - 1}`);
    expect(next).toContain("P0");
  });

  it("空白だけのパスは積まない", () => {
    for (const path of ["", "   "]) {
      expect(pushHistory(["A"], path)).toEqual(["A"]);
    }
  });

  it("trim してから積む", () => {
    expect(pushHistory([], "  D:\\work  ")).toEqual(["D:\\work"]);
  });

  it("元の配列を壊さない", () => {
    const original = ["A", "B"];
    pushHistory(original, "C");
    expect(original).toEqual(["A", "B"]);
  });
});

describe("保存と読み込み", () => {
  it("remember で積まれ、保存される", async () => {
    const storage = fakeStorage();
    const { useWorkDirHistory } = await freshModule(storage);

    useWorkDirHistory().remember("D:\\one");
    await nextTick();

    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "[]")).toEqual(["D:\\one"]);
  });

  it("保存済みを読み戻す", async () => {
    const storage = fakeStorage({ [STORAGE_KEY]: JSON.stringify(["D:\\a", "D:\\b"]) });
    const { useWorkDirHistory } = await freshModule(storage);

    expect([...useWorkDirHistory().history]).toEqual(["D:\\a", "D:\\b"]);
  });

  it("文字列でない要素は捨て、健全な要素は残す", async () => {
    // **全部捨てる実装と区別が付くように**、壊れた要素と正しい要素を混ぜる。
    const storage = fakeStorage({
      [STORAGE_KEY]: JSON.stringify(["D:\\a", 42, null, "", "D:\\b"]),
    });
    const { useWorkDirHistory } = await freshModule(storage);

    expect([...useWorkDirHistory().history]).toEqual(["D:\\a", "D:\\b"]);
  });

  it("壊れた JSON・配列でない保存値でも開ける", async () => {
    for (const raw of ["{壊れている", JSON.stringify({ a: 1 })]) {
      const storage = fakeStorage({ [STORAGE_KEY]: raw });
      const { useWorkDirHistory } = await freshModule(storage);
      expect([...useWorkDirHistory().history]).toEqual([]);
    }
  });
});
