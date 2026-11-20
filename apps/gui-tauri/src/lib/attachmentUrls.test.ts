import { describe, expect, it } from "vitest";

import { createAttachmentUrlCache } from "./attachmentUrls";
import type { Attachment } from "../types";

/**
 * 会話ペインの添付の object URL の投影キャッシュ（#20a / N12）。
 *
 * 読み込み・URL の作成・解放は外から渡す（IPC と `URL.createObjectURL` は vitest に無い）。
 * 読み込みは答えを外から与えられる形にして、「解放の後に届いた読み込み」を順序どおりに作る。
 */
function harness() {
  /** 添付 ID ごとの、まだ答えていない読み込み（古い順）。 */
  const pending = new Map<string, ((bytes: number[] | null) => void)[]>();
  const reads: string[] = [];
  const created: string[] = [];
  const revoked: string[] = [];
  let seq = 0;
  const store: Record<string, string | null> = {};
  const cache = createAttachmentUrlCache(store, {
    read: (id) => {
      reads.push(id);
      return new Promise<number[] | null>((resolve) => {
        pending.set(id, [...(pending.get(id) ?? []), resolve]);
      });
    },
    create: (_bytes, att) => {
      const url = `blob:${att.id}#${++seq}`;
      created.push(url);
      return url;
    },
    revoke: (url) => {
      revoked.push(url);
    },
  });
  /** 読み込みに答え（既定は最も古いもの）、`then` / `finally` の後まで進める。 */
  const answer = async (
    id: string,
    bytes: number[] | null = [1, 2, 3],
    which: "oldest" | "newest" = "oldest",
  ) => {
    const queue = pending.get(id) ?? [];
    const resolve = which === "oldest" ? queue.shift() : queue.pop();
    resolve?.(bytes);
    for (let i = 0; i < 5; i += 1) await Promise.resolve();
  };
  return { cache, store, reads, created, revoked, answer };
}

const image = (id: string, format: Attachment["format"] = "webp"): Attachment => ({
  id,
  format,
  fileName: `${id}.webp`,
});
const file = (id: string, format: Attachment["format"]): Attachment => ({
  id,
  format,
  fileName: `${id}.${format}`,
});

describe("画像の object URL", () => {
  it("1 回だけ読み、届くまでは undefined、届いたら URL", async () => {
    const h = harness();
    expect(h.cache.url(image("a"))).toBeUndefined();
    expect(h.cache.url(image("a"))).toBeUndefined();
    expect(h.reads).toEqual(["a"]);
    await h.answer("a");
    expect(h.cache.url(image("a"))).toBe("blob:a#1");
    expect(h.reads).toEqual(["a"]);
  });

  it("形式欄の無い古いレコードと生成画像の写し（jpeg）も画像として読む", async () => {
    const h = harness();
    h.cache.url(image("old", undefined));
    h.cache.url(image("gen", "jpeg"));
    expect(h.reads).toEqual(["old", "gen"]);
  });

  it("実体が無い（保持期間で消えた）なら null", async () => {
    const h = harness();
    h.cache.url(image("gone"));
    await h.answer("gone", null);
    expect(h.cache.url(image("gone"))).toBeNull();
    expect(h.created).toEqual([]);
  });
});

describe("画像以外は本体を読まない（N12）", () => {
  it("音声・動画・PDF は読み込みを呼ばず、URL も作らない", () => {
    const h = harness();
    for (const att of [file("m", "mp3"), file("w", "wav"), file("v", "mp4"), file("p", "pdf")]) {
      expect(h.cache.url(att)).toBeUndefined();
    }
    expect(h.reads).toEqual([]);
    expect(Object.keys(h.store)).toEqual([]);
  });
});

describe("会話を切り替えたら解放する（#20a）", () => {
  it("持っている URL をすべて解放し、キャッシュを空にする", async () => {
    const h = harness();
    h.cache.url(image("a"));
    h.cache.url(image("b"));
    h.cache.url(image("gone"));
    await h.answer("a");
    await h.answer("b");
    await h.answer("gone", null);
    h.cache.releaseAll();
    expect(h.revoked.sort()).toEqual(["blob:a#1", "blob:b#2"]);
    expect(Object.keys(h.store)).toEqual([]);
  });

  it("大きく見ている画像の URL は解放せず残す", async () => {
    const h = harness();
    h.cache.url(image("a"));
    h.cache.url(image("b"));
    await h.answer("a");
    await h.answer("b");
    h.cache.releaseAll("blob:b#2");
    expect(h.revoked).toEqual(["blob:a#1"]);
    expect(h.store).toEqual({ b: "blob:b#2" });
  });

  it("解放の後は、同じ添付をもう一度読み直す", async () => {
    const h = harness();
    h.cache.url(image("a"));
    await h.answer("a");
    h.cache.releaseAll();
    expect(h.cache.url(image("a"))).toBeUndefined();
    expect(h.reads).toEqual(["a", "a"]);
  });

  it("解放の前に始まった読み込みは、届いても URL を作らない（解放されない URL を残さない）", async () => {
    const h = harness();
    h.cache.url(image("a"));
    h.cache.releaseAll();
    await h.answer("a");
    expect(h.created).toEqual([]);
    expect(Object.keys(h.store)).toEqual([]);
  });

  it("解放の前の読み込みが後から届いても、解放の後に始めた読み込みの結果を上書きしない", async () => {
    const h = harness();
    h.cache.url(image("a"));
    h.cache.releaseAll();
    // 新しい会話で同じ添付を描く — 古い読み込みを待たずに読み直す。
    h.cache.url(image("a"));
    expect(h.reads).toEqual(["a", "a"]);
    await h.answer("a", [1, 2, 3], "newest");
    expect(h.cache.url(image("a"))).toBe("blob:a#1");
    await h.answer("a", [9, 9, 9], "oldest");
    expect(h.created).toEqual(["blob:a#1"]);
    expect(h.cache.url(image("a"))).toBe("blob:a#1");
  });

  it("解放の前の読み込みが先に届いても、解放の後の読み込みを二重に始めない", async () => {
    const h = harness();
    h.cache.url(image("a"));
    h.cache.releaseAll();
    h.cache.url(image("a"));
    await h.answer("a", [9, 9, 9], "oldest");
    // 古い読み込みの後始末が、新しい読み込みの「読み込み中」を消していないこと。
    expect(h.cache.url(image("a"))).toBeUndefined();
    expect(h.reads).toEqual(["a", "a"]);
    await h.answer("a");
    expect(h.created).toEqual(["blob:a#1"]);
  });
});
