/**
 * 会話ペインの表示クリア（{@link useChatClear}）の規律。
 *
 * 固定するのは 3 点:
 * - **会話ごとに独立** — 1 つの会話を隠しても別の会話は無傷（新規チャットと
 *   分岐は新しい session_id なので最初から全部見える）
 * - **戻すと鍵ごと落ちる** — 0 を書くと保存が育ち続ける
 * - **数でない保存値はその会話ぶんだけ捨てる** — 素通しにすると比較が常に偽に
 *   なり、「クリアしたのに効かない」が理由の分からない形で出る
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

const STORAGE_KEY = "concordia.chatCleared.v1";

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
  return await import("./useChatClear");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("表示クリアの境界", () => {
  it("保存が無ければ 0（何も隠していない）で、読むだけでは書かない", async () => {
    const storage = fakeStorage();
    const { useChatClear } = await freshModule(storage);

    expect(useChatClear().clearedAt("s1")).toBe(0);
    await nextTick();
    expect(storage.writes).toBe(0);
  });

  it("会話ごとに独立している", async () => {
    const { useChatClear } = await freshModule(fakeStorage());
    const chatClear = useChatClear();

    chatClear.clear("s1", 1000);

    // **別の会話は無傷。** 同じ値を両方へ入れると、会話を鍵にしていない
    // 実装（単一の境界を持つ実装）でも緑になる。
    expect(chatClear.clearedAt("s1")).toBe(1000);
    expect(chatClear.clearedAt("s2")).toBe(0);
  });

  it("保存され、読み戻せる", async () => {
    const storage = fakeStorage();
    const first = await freshModule(storage);
    first.useChatClear().clear("s1", 1234);
    await nextTick();

    expect(storage.writes).toBe(1);
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "{}")).toEqual({ s1: 1234 });

    const second = await freshModule(storage);
    expect(second.useChatClear().clearedAt("s1")).toBe(1234);
  });

  it("戻すと鍵ごと落ちる（0 を書き残さない）", async () => {
    const storage = fakeStorage();
    const { useChatClear } = await freshModule(storage);
    const chatClear = useChatClear();

    chatClear.clear("s1", 1000);
    chatClear.clear("s2", 2000);
    chatClear.restore("s1");
    await nextTick();

    // s1 が 0 で残っていないこと（残ると保存が育ち続ける）。
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "{}")).toEqual({ s2: 2000 });
    expect(chatClear.clearedAt("s1")).toBe(0);
  });

  it("数でない保存値はその会話ぶんだけ捨てる", async () => {
    const storage = fakeStorage({
      [STORAGE_KEY]: JSON.stringify({ s1: "1000", s2: null, s3: 3000 }),
    });
    const { useChatClear } = await freshModule(storage);
    const chatClear = useChatClear();

    // **健全な鍵は残る。** 全部捨てる実装と区別が付くように、同じ 1 件の
    // 保存値の中に壊れた鍵と正しい鍵を混ぜている。
    expect(chatClear.clearedAt("s1")).toBe(0);
    expect(chatClear.clearedAt("s2")).toBe(0);
    expect(chatClear.clearedAt("s3")).toBe(3000);
  });

  it("壊れた JSON でも開ける", async () => {
    const storage = fakeStorage({ [STORAGE_KEY]: "{壊れている" });
    const { useChatClear } = await freshModule(storage);
    expect(useChatClear().clearedAt("s1")).toBe(0);
  });
});
