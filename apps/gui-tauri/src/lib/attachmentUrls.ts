/**
 * 会話ペインの添付の object URL の投影キャッシュ（Spec 23 / #20a / N12）。
 *
 * 鍵は添付 ID、値は object URL。`null` = 実体が無い（保持期間で消えた。D9 のプレースホルダを出す）、
 * 鍵が無い = まだ読んでいない（[`AttachmentUrlCache.url`] が裏で読みに行く）。
 *
 * - **読むのは画像だけ**（N12）。音声・動画・PDF は会話ペインでファイル名を出すだけで、本体（最大 12 MB）を
 *   読んで object URL を持ち続ける理由が無い
 * - **会話を切り替えたら [`AttachmentUrlCache.releaseAll`] でまとめて解放する**（#20a）。会話ペインは
 *   アプリの寿命まで unmount されないので、解放しないと見た会話の数だけ Blob が溜まる
 *   （アイコンの object URL は `useOrchestrator.ts` が解放している。添付だけ同じ扱いになっていなかった）
 * - 解放の前に始まった読み込みの結果は捨てる（世代で見分ける）。捨てずに入れると、解放の後に作った
 *   URL が次の解放までキャッシュに残る
 *
 * 読み込み・URL の作成・解放は外から渡す（IPC と `URL.createObjectURL` を vitest で差し替えるため）。
 * キャッシュの入れ物（`store`）も外から渡す — 会話ペインは `reactive` を渡して、読み終わりで描き直させる。
 */
import { kindOfFormat } from "./attachment";
import type { Attachment } from "../types";

/** キャッシュが外から借りるもの。 */
export interface AttachmentUrlDeps {
  /** 添付の本体を読む（`readAttachment`）。`null` = 保持期間で消えた。 */
  read: (id: string) => Promise<number[] | null>;
  /** 読んだ本体から object URL を作る。MIME は添付の形式から引く。 */
  create: (bytes: number[], att: Attachment) => string;
  /** object URL を解放する。 */
  revoke: (url: string) => void;
}

/** 会話ペインが使う口。 */
export interface AttachmentUrlCache {
  /**
   * 描画用: 画像の添付の現在の状態（URL / `null` = 消えた / `undefined` = 読み込み中）。
   * 初回は裏で読みに行く。**画像以外は読まずに `undefined` を返す**（呼び手は画像以外で呼ばない）。
   */
  url(att: Attachment): string | null | undefined;
  /**
   * 持っている URL をすべて解放し、キャッシュを空にする。`keep` の URL（大きく見ている画像）だけは
   * 解放せず残す — 開いている間に消すと、プレビューの絵が消える。
   */
  releaseAll(keep?: string | null): void;
}

export function createAttachmentUrlCache(
  store: Record<string, string | null>,
  deps: AttachmentUrlDeps,
): AttachmentUrlCache {
  /** 読み込み中の添付 ID（二重に読まない）。 */
  const loading = new Set<string>();
  /** 解放の世代。解放のたびに進め、前の世代の読み込みの結果を捨てる。 */
  let generation = 0;

  function ensure(att: Attachment): void {
    const id = att.id;
    if (id in store || loading.has(id)) return;
    loading.add(id);
    const started = generation;
    void deps
      .read(id)
      .then((bytes) => {
        if (started !== generation) return;
        store[id] = bytes ? deps.create(bytes, att) : null;
      })
      .catch(() => {
        if (started !== generation) return;
        store[id] = null;
      })
      .finally(() => {
        // 前の世代の後始末は、解放の後に始めた読み込みの「読み込み中」を消さない。
        if (started === generation) loading.delete(id);
      });
  }

  return {
    url(att) {
      if (kindOfFormat(att.format) !== "image") return undefined;
      ensure(att);
      return store[att.id];
    },
    releaseAll(keep = null) {
      generation += 1;
      loading.clear();
      for (const [id, url] of Object.entries(store)) {
        if (url !== null && url === keep) continue;
        if (url !== null) deps.revoke(url);
        delete store[id];
      }
    },
  };
}
