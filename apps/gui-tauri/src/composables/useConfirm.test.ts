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

import { askChoice, askConfirm, resetConfirmQueue, useConfirmHost } from "./useConfirm";

const { current, answer, choose } = useConfirmHost();

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

describe("選択ダイアログ（askChoice）", () => {
  const choices = [
    { key: "both", label: "両方切る", danger: true },
    { key: "forward", label: "A → B だけ切る", danger: true },
  ];

  it("選んだ鍵で解決し、取り消しなら null", async () => {
    const picked = askChoice({ title: "どれを切りますか？", choices });
    expect(current.value?.choices?.map((c) => c.key)).toEqual(["both", "forward"]);
    choose("forward");
    await expect(picked).resolves.toBe("forward");

    const cancelled = askChoice({ title: "どれを切りますか？", choices });
    answer(false);
    await expect(cancelled).resolves.toBeNull();
  });

  it("確認と同じ待ち行列に並び、前の確認が閉じてから出る", async () => {
    const confirm = askConfirm({ title: "先の確認" });
    const choice = askChoice({ title: "後の選択", choices });

    expect(current.value?.title).toBe("先の確認");
    answer(true);
    await expect(confirm).resolves.toBe(true);

    expect(current.value?.title).toBe("後の選択");
    choose("both");
    await expect(choice).resolves.toBe("both");
  });

  it("元に戻せない選択肢が 1 つでもあれば danger が立つ（初期フォーカスが取り消し側へ行く）", () => {
    void askChoice({ title: "危ない", choices });
    expect(current.value?.danger).toBe(true);
    resetConfirmQueue();

    void askChoice({ title: "危なくない", choices: [{ key: "a", label: "A" }] });
    expect(current.value?.danger).toBe(false);
  });

  it("見出しの絵・選択肢の絵と補足を持ち回る（ホストは中身を知らずに描く）", () => {
    const Visual = { name: "Visual", render: () => null };
    void askChoice({
      title: "絵つき",
      lead: { component: Visual, props: { mode: "pair" } },
      choices: [
        { key: "both", label: "両方", description: "どちらも切る", visual: { component: Visual, props: { mode: "both" } }, emphasis: true },
      ],
    });
    expect(current.value?.lead?.props).toEqual({ mode: "pair" });
    expect(current.value?.choices?.[0]?.description).toBe("どちらも切る");
    expect(current.value?.choices?.[0]?.visual?.props).toEqual({ mode: "both" });
    expect(current.value?.choices?.[0]?.emphasis).toBe(true);
  });

  it("待ち行列を空にすると、選択も取り消し扱い（null）で解決する", async () => {
    const pending = askChoice({ title: "宙に浮かせない", choices });
    resetConfirmQueue();
    await expect(pending).resolves.toBeNull();
  });
});
