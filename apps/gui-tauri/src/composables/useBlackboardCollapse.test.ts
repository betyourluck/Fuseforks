/**
 * 黒板の付箋の畳み（{@link useBlackboardCollapse}）の規律。
 *
 * 固定するのは 4 点:
 * - **鍵は dir:name** — 同名の付箋が別の work_dir にあっても混ざらない
 * - **タブを離れても残る** — モジュール単位の状態 + localStorage
 * - **一覧に無い鍵は prune で落ちる** — 保存が育ち続けない。変化が無ければ書き込まない
 * - **壊れた保存値は空として読む** — タブが開けなくなるほうが害が大きい
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

const STORAGE_KEY = "fuseforks.blackboardCollapsed.v1";

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
  return await import("./useBlackboardCollapse");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

const A = { dir: "D:\\one", name: "ザリ.md" };
const A2 = { dir: "D:\\two", name: "ザリ.md" };
const B = { dir: "D:\\one", name: "ルナ.md" };

describe("useBlackboardCollapse", () => {
  it("鍵は dir:state/name。state 無しは dir:name のまま（2026-09-16 の保存値を壊さない）", async () => {
    const { noteKey } = await freshModule(fakeStorage());
    expect(noteKey(A)).toBe("D:\\one:ザリ.md");
    expect(noteKey({ ...A, state: "done" })).toBe("D:\\one:done/ザリ.md");
    // state が変わると鍵も変わる = move した付箋は開く（Spec 54 凍結 10。意図）。
    // 鍵を dir:name にすると、同名が 2 状態に並ぶ重複で片方を畳むと両方が畳まれる。
    expect(noteKey({ ...A, state: "doing" })).not.toBe(noteKey({ ...A, state: "done" }));
  });

  it("toggle で畳み、もう一度で開く。鍵は dir:name で別 work_dir の同名と混ざらない", async () => {
    const storage = fakeStorage();
    const { useBlackboardCollapse } = await freshModule(storage);
    const c = useBlackboardCollapse();
    expect(c.isCollapsed(A)).toBe(false);
    c.toggle(A);
    expect(c.isCollapsed(A)).toBe(true);
    expect(c.isCollapsed(A2)).toBe(false);
    c.toggle(A);
    expect(c.isCollapsed(A)).toBe(false);
  });

  it("保存は dir:name の配列で、別の呼び出し側（タブを戻った部品）からも同じ状態が見える", async () => {
    const storage = fakeStorage();
    const mod = await freshModule(storage);
    mod.useBlackboardCollapse().toggle(A);
    await nextTick();
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "[]")).toEqual(["D:\\one:ザリ.md"]);
    expect(mod.useBlackboardCollapse().isCollapsed(A)).toBe(true);
  });

  it("setCollapsed は渡した付箋だけを畳む / 開く。渡さなかった付箋（絞り込みで隠れた側）には触らない", async () => {
    const { useBlackboardCollapse } = await freshModule(fakeStorage());
    const c = useBlackboardCollapse();
    c.toggle(A2);
    c.setCollapsed([A, B], true);
    expect([c.isCollapsed(A), c.isCollapsed(B), c.isCollapsed(A2)]).toEqual([true, true, true]);
    c.setCollapsed([A, B], true); // 二度押しても鍵は重複しない
    c.setCollapsed([A], false);
    expect([c.isCollapsed(A), c.isCollapsed(B), c.isCollapsed(A2)]).toEqual([false, true, true]);
  });

  it("起動時に保存値を読む", async () => {
    const storage = fakeStorage({ [STORAGE_KEY]: JSON.stringify(["D:\\one:ルナ.md"]) });
    const { useBlackboardCollapse } = await freshModule(storage);
    expect(useBlackboardCollapse().isCollapsed(B)).toBe(true);
    expect(useBlackboardCollapse().isCollapsed(A)).toBe(false);
  });

  it("prune は一覧に無い鍵だけ落とし、変化が無ければ書き込まない", async () => {
    const storage = fakeStorage();
    const { useBlackboardCollapse } = await freshModule(storage);
    const c = useBlackboardCollapse();
    c.toggle(A);
    c.toggle(B);
    await nextTick();
    const before = storage.writes;
    c.prune([A, B]);
    await nextTick();
    expect(storage.writes).toBe(before);
    c.prune([B]);
    await nextTick();
    expect(c.isCollapsed(A)).toBe(false);
    expect(c.isCollapsed(B)).toBe(true);
    expect(storage.writes).toBe(before + 1);
  });

  it("壊れた保存値は空として読む", async () => {
    const storage = fakeStorage({ [STORAGE_KEY]: "{not json" });
    const { useBlackboardCollapse } = await freshModule(storage);
    expect(useBlackboardCollapse().isCollapsed(A)).toBe(false);
    const numbers = fakeStorage({ [STORAGE_KEY]: JSON.stringify([1, "D:\\one:ザリ.md"]) });
    const mod = await freshModule(numbers);
    expect(mod.useBlackboardCollapse().isCollapsed(A)).toBe(true);
  });
});
