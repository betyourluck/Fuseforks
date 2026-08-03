/**
 * アプリ内の確認ダイアログ（はい／いいえ）。
 *
 * ブラウザの `confirm()` を使わない理由は 2 つ:
 *
 * 1. **WebView のダイアログはアプリの名前を名乗らない。** Tauri の WebView が出す
 *    ネイティブのダイアログには `localhost` が表示される。デスクトップアプリとして
 *    配っているものが、操作の途中でだけ「ブラウザで開いた何か」に見える。
 * 2. **文言と配色を制御できない。** ボタンのラベルは OS の言語設定で決まり、
 *    危険な操作とそうでない操作を見分けさせることもできない。
 *
 * 形は {@link ToastHost} と同じ「モジュール内に状態を持ち、ホストを 1 つだけ
 * 画面に置く」型にしてある。呼ぶ側は Promise を待つだけで、どこから呼んでも
 * ダイアログの実体は 1 つ。
 *
 * ```ts
 * if (!(await askConfirm({ title: "削除しますか？", danger: true }))) return;
 * ```
 */
import { computed, reactive } from "vue";

/** 確認ダイアログの内容。 */
export interface ConfirmOptions {
  /** 見出し。**問いそのもの**を書く（「削除しますか？」）。 */
  title: string;
  /** 補足。結果として何が起きるかを書く。改行はそのまま表示される。 */
  message?: string;
  /** 実行側のラベル。既定は「はい」。 */
  confirmLabel?: string;
  /** 取り消し側のラベル。既定は「いいえ」。 */
  cancelLabel?: string;
  /**
   * 元に戻せない操作か。
   *
   * true にすると実行ボタンが `fail` 色になり、**初期フォーカスが取り消し側**へ
   * 移る。Enter を押しっぱなしにしていた指が、削除を確定させないようにするため。
   */
  danger?: boolean;
}

/** 待機中の 1 件。 */
interface PendingConfirm extends ConfirmOptions {
  /** 描画の鍵。 */
  id: number;
  /** 呼び出し側へ返す口。 */
  resolve: (ok: boolean) => void;
}

/**
 * 待ち行列。**先頭だけが画面に出る。**
 *
 * 1 件しか持たない設計にすると、開いている最中に来た 2 件目を捨てるか、
 * 1 件目を黙って置き換えるしかない。どちらも「押したのに何も起きない」を作る。
 */
const queue = reactive<PendingConfirm[]>([]);

let seq = 0;

/**
 * 確認を求める。`true` なら実行、`false` なら取り消し。
 *
 * **解決するまで待つ。** 呼び出し側は `await` するだけでよく、
 * ネイティブの `confirm()` を置き換える形がそのまま書ける。
 */
export function askConfirm(options: ConfirmOptions): Promise<boolean> {
  return new Promise<boolean>((resolve) => {
    seq += 1;
    queue.push({ ...options, id: seq, resolve });
  });
}

/** ホスト（{@link ConfirmHost}）から使う口。画面に出すのはこの 1 箇所だけ。 */
export function useConfirmHost() {
  return {
    /** いま出すべき 1 件。無ければ `null`。 */
    current: computed<PendingConfirm | null>(() => queue[0] ?? null),

    /**
     * 先頭の 1 件に答えて閉じる。
     *
     * **必ず解決してから取り除く。** 取り除くだけにすると、呼び出し側の
     * `await` が永久に返らず、その画面が固まったように見える。
     */
    answer(ok: boolean): void {
      const pending = queue.shift();
      pending?.resolve(ok);
    },
  };
}

/** テスト用。待ち行列を空にする（待っている呼び出しは取り消し扱いで解決する）。 */
export function resetConfirmQueue(): void {
  while (queue.length) {
    queue.shift()?.resolve(false);
  }
}
