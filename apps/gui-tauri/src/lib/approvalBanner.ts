import type { CommandPolicyView } from "../types";

/**
 * 会話ペインの入力欄の上に出す「コマンドの承認待ち」の帯（2026-10-19）。
 *
 * 承認待ちはタイトルバーの「コマンド承認」のバッジにしか出ておらず、会話ペインを見ている間は
 * 見落としやすかった。件数は `run` が呼ばれるたびに取り直されている（`state.commandRequests`）ので、
 * 帯はそれを読むだけで、新しい取り直しも IPC も作らない。
 *
 * **会話の途中に行として挿さない** — 承認すると行が会話の途中から消えるか、古い行が残り続ける。
 * **コアの System 行にもしない** — 利用者宛の System 行は入退室の節として全個体のプロンプトへ入る
 * （`failures.md` #89 の経路）。帯は承認待ちがある間だけ出て、無くなれば消える。
 *
 * 訳語ではなく**辞書の鍵**を返す（純関数は i18n を知らない）。
 */
export interface ApprovalBanner {
  key: "chat.approvalWaitingOne" | "chat.approvalWaitingMany";
  params: {
    /** 名指しするサーヴァント（一覧の並びで最初に承認待ちを持つ個体）。 */
    name: string;
    /** ほかに承認待ちを持つ個体の数。 */
    others: number;
    /** 承認待ちの合計件数（タイトルバーのバッジと同じ数）。 */
    count: number;
  };
}

/**
 * 承認待ちの合計件数。**タイトルバーのバッジと帯が同じ関数で数える**（2 箇所で数えると、
 * バッジは 3 なのに帯は 2、の食い違いが生まれる）。
 */
export function pendingTotal(views: readonly CommandPolicyView[]): number {
  return views.reduce((sum, view) => sum + view.pending.length, 0);
}

/** 承認待ちが 1 件も無ければ `null`（帯を出さない）。 */
export function approvalBanner(views: readonly CommandPolicyView[]): ApprovalBanner | null {
  const waiting = views.filter((view) => view.pending.length > 0);
  const first = waiting[0];
  if (first === undefined) return null;
  const count = pendingTotal(waiting);
  const others = waiting.length - 1;
  return {
    key: others === 0 ? "chat.approvalWaitingOne" : "chat.approvalWaitingMany",
    params: { name: first.name, others, count },
  };
}
