/**
 * この画面の設定（{@link useUiSettings}）の規律。
 *
 * ここで固定するのは 3 点:
 * - **既定は確認 ON** — settings_contract の凍結値。ON を知らずに失うと
 *   接続は元に戻せない
 * - **読むだけでは書かない** — 設定ダイアログを開いて触らず閉じても
 *   localStorage は書き換わらない（settings_contract の検証項目）
 * - **壊れた保存値・型違いは既定へ落とす** — boolean 以外を素通しにすると
 *   「真でも偽でもない値」が確認の分岐へ流れ込む
 */
import { afterEach, describe, expect, it, vi } from "vitest";
import { nextTick } from "vue";

const STORAGE_KEY = "concordia.settings.v1";

/** localStorage の代役。書き込み回数を数える。 */
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
  return await import("./useUiSettings");
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("この画面の設定", () => {
  it("保存が無ければ既定は確認 ON で、読むだけでは書かない", async () => {
    const storage = fakeStorage();
    const { useUiSettings } = await freshModule(storage);

    const { settings } = useUiSettings();
    expect(settings.confirmEdgeDelete).toBe(true);

    await nextTick();
    expect(storage.writes).toBe(0);
  });

  it("保存された OFF を読み戻す", async () => {
    const storage = fakeStorage({
      [STORAGE_KEY]: JSON.stringify({ confirmEdgeDelete: false }),
    });
    const { useUiSettings } = await freshModule(storage);

    expect(useUiSettings().settings.confirmEdgeDelete).toBe(false);
  });

  it("壊れた JSON・boolean 以外の値は既定へ落とす", async () => {
    for (const raw of ["{壊れている", JSON.stringify({ confirmEdgeDelete: "yes" })]) {
      const storage = fakeStorage({ [STORAGE_KEY]: raw });
      const { useUiSettings } = await freshModule(storage);
      expect(useUiSettings().settings.confirmEdgeDelete).toBe(true);
    }
  });

  it("変えたときだけ保存され、保存形は契約のキー名を持つ", async () => {
    const storage = fakeStorage();
    const { useUiSettings } = await freshModule(storage);

    const { settings } = useUiSettings();
    settings.confirmEdgeDelete = false;
    await nextTick();

    expect(storage.writes).toBe(1);
    expect(JSON.parse(storage.dump(STORAGE_KEY) ?? "{}")).toEqual({
      confirmEdgeDelete: false,
    });
  });
});
