/**
 * アプリ内の確認ダイアログ（はい／いいえ）と、選択肢から 1 つを選ばせる形（{@link askChoice}）。
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
import { computed, markRaw, reactive, type Component } from "vue";

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

/**
 * 選択ダイアログに差し込む絵（部品と props）。**ホストは個体や業務を知らない**ので、
 * アバターや向きの絵は呼び出し側の部品が描く（例: `TieCutVisual.vue`）。
 */
export interface ChoiceVisual {
  component: Component;
  props?: Record<string, unknown>;
}

/** 選択ダイアログの 1 つの選択肢（{@link askChoice}）。 */
export interface ChoiceOption {
  /** 呼び出し側へ返す値。 */
  key: string;
  /** 見出し（太字の 1 行）。 */
  label: string;
  /** 補足（見出しの下の 1 行）。選ぶと何が起きるか・何が残るかを書く。 */
  description?: string;
  /** 左の欄に出す絵。 */
  visual?: ChoiceVisual;
  /** 元に戻せない操作か（`fail` 色になる）。 */
  danger?: boolean;
  /** 最も影響の大きい選択肢として、地を `fail` で薄く塗る。 */
  emphasis?: boolean;
}

/**
 * 選択ダイアログの内容。**「はい」の代わりに選択肢を並べる**だけで、待ち行列も見た目も
 * 確認ダイアログと同じ 1 つを使う（2 つ置くと、同時に 2 枚出る形が生まれる）。
 */
export interface ChoiceOptions {
  title: string;
  /** 補足の文。`lead` があればそちらを出す（文は読み上げ用の代わりとして持つ）。 */
  message?: string;
  /** 見出しの下に出す絵。 */
  lead?: ChoiceVisual;
  choices: readonly ChoiceOption[];
  /** 取り消し側のラベル。既定は「キャンセル」。 */
  cancelLabel?: string;
}

/**
 * 待ち行列（reactive）へ入れる前に部品を `markRaw` する。しないと Vue が部品を reactive に
 * 包み、警告を出したうえで描画のたびに無駄な追跡をする。
 */
function rawVisual(visual: ChoiceVisual | undefined): ChoiceVisual | undefined {
  return visual ? { ...visual, component: markRaw(visual.component) } : undefined;
}

/** 待機中の 1 件。 */
interface PendingConfirm extends ConfirmOptions {
  /** 描画の鍵。 */
  id: number;
  /** 選択肢。あれば実行ボタンの代わりに並べる（{@link askChoice}）。 */
  choices?: readonly ChoiceOption[];
  /** 見出しの下の絵（{@link askChoice}）。 */
  lead?: ChoiceVisual;
  /**
   * 呼び出し側へ返す口。取り消しは `false`、確認の実行は `true`、選択肢は選んだ鍵。
   * 受け取る形は呼び出し側（`askConfirm` / `askChoice`）がそれぞれ写し直す。
   */
  resolve: (answer: boolean | string) => void;
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
    queue.push({ ...options, id: seq, resolve: (answer) => resolve(answer === true) });
  });
}

/**
 * 選択肢から 1 つを選ばせる。選んだ鍵を返し、取り消しなら `null`。
 *
 * 確認ダイアログと**同じ待ち行列**に並ぶ — 開いている確認の後ろで待ち、取りこぼさない。
 * 初期フォーカスは、選択肢に元に戻せない操作が 1 つでもあれば取り消し側（{@link ConfirmHost}）。
 */
export function askChoice(options: ChoiceOptions): Promise<string | null> {
  return new Promise<string | null>((resolve) => {
    seq += 1;
    queue.push({
      title: options.title,
      message: options.message,
      cancelLabel: options.cancelLabel,
      danger: options.choices.some((c) => c.danger),
      lead: rawVisual(options.lead),
      choices: options.choices.map((c) => ({ ...c, visual: rawVisual(c.visual) })),
      id: seq,
      resolve: (answer) => resolve(typeof answer === "string" ? answer : null),
    });
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

    /** 先頭の 1 件（選択ダイアログ）に、選んだ鍵で答えて閉じる。 */
    choose(key: string): void {
      const pending = queue.shift();
      pending?.resolve(key);
    },
  };
}

/** テスト用。待ち行列を空にする（待っている呼び出しは取り消し扱いで解決する）。 */
export function resetConfirmQueue(): void {
  while (queue.length) {
    queue.shift()?.resolve(false);
  }
}
