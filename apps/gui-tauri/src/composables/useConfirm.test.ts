/**
 * 確認ダイアログの待ち行列（{@link askConfirm}）の規律。
 *
 * ここで固定するのは 2 点:
 * - **必ず解決する** — 答えても取り消しても Promise が返る。返らないと、
 *   呼び出し側の `await` が永久に止まり、その画面が固まったように見える
 * - **取りこぼさない** — 開いている最中に来た 2 件目を捨てない。捨てると
 *   「押したのに何も起きない」が生まれる
 */
import { beforeEach, describe, expect, it } from "vitest";

import { askConfirm, resetConfirmQueue, useConfirmHost } from "./useConfirm";

const { current, answer } = useConfirmHost();

beforeEach(() => {
  resetConfirmQueue();
});

describe("確認ダイアログの待ち行列", () => {
  it("答えると Promise が解決する", async () => {
    const yes = askConfirm({ title: "実行しますか？" });
    expect(current.value?.title).toBe("実行しますか？");

    answer(true);
    await expect(yes).resolves.toBe(true);
    expect(current.value).toBeNull();

    const no = askConfirm({ title: "もう一度？" });
    answer(false);
    await expect(no).resolves.toBe(false);
  });

  it("開いている最中の 2 件目を捨てず、順に出す", async () => {
    const first = askConfirm({ title: "1 件目" });
    const second = askConfirm({ title: "2 件目" });

    expect(current.value?.title).toBe("1 件目");
    answer(true);
    await expect(first).resolves.toBe(true);

    // 1 件目が閉じたら 2 件目が出る。
    expect(current.value?.title).toBe("2 件目");
    answer(false);
    await expect(second).resolves.toBe(false);
    expect(current.value).toBeNull();
  });

  it("待ち行列を空にすると、待っている呼び出しは取り消し扱いで解決する", async () => {
    const pending = askConfirm({ title: "宙に浮かせない" });
    resetConfirmQueue();
    await expect(pending).resolves.toBe(false);
    expect(current.value).toBeNull();
  });

  it("危険な操作の目印と文言を持ち回る", () => {
    void askConfirm({
      title: "削除しますか？",
      message: "元に戻せません。",
      confirmLabel: "削除する",
      cancelLabel: "やめる",
      danger: true,
    });
    expect(current.value?.danger).toBe(true);
    expect(current.value?.confirmLabel).toBe("削除する");
    expect(current.value?.cancelLabel).toBe("やめる");
    expect(current.value?.message).toBe("元に戻せません。");
  });
});
