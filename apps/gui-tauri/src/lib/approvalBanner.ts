import type { CommandPolicyView } from "../types";

/**
 * 会話ペインの入力欄の上に出す「コマンドの承認待ち」の帯（2026-10-19 → Spec 72 D6）。
 *
 * 承認待ちはタイトルバーの「コマンド承認」のバッジにしか出ておらず、会話ペインを見ている間は
 * 見落としやすかった。件数は `run` が呼ばれるたびに取り直されている（`state.commandRequests`）ので、
 * 帯はそれを読むだけで、新しい取り直しも IPC も作らない。
 *
 * **会話の途中に行として挿さない** — 承認すると行が会話の途中から消えるか、古い行が残り続ける。
 * **コアの System 行にもしない** — 利用者宛の System 行は入退室の節として全個体のプロンプトへ入る
 * （`failures.md` #89 の経路）。帯は承認待ちがある間だけ出て、無くなれば消える。
 *
 * **Spec 72 で腕が 1 つ増えた** — 判断待ちを全部片付けた後も、承認待ちで止まった個体
 * （`stoppedForApproval`）が居る間は「{name} が承認待ちで止まっています」の帯を出す。
 * 出さないと「承認したのに何も起きない」が Spec 56 の起点の形で戻る。
 * **優先は判断待ち > 止まった個体**（承認が先、続行が後、の順を帯も示す）。
 * **文に「承認画面を開く」を入れない** — `ChatPanel` が帯の右隣にそのボタンを常時描く。
 *
 * 訳語ではなく**辞書の鍵**を返す（純関数は i18n を知らない）。
 */
export interface ApprovalBanner {
  key:
    | "chat.approvalWaitingOne"
    | "chat.approvalWaitingMany"
    | "chat.approvalStoppedOne"
    | "chat.approvalStoppedMany";
  params: {
    /** 名指しするサーヴァント（一覧の並びで最初に当たる個体）。 */
    name: string;
    /** ほかに同じ状態の個体の数。 */
    others: number;
    /** 承認待ちの合計件数（タイトルバーのバッジと同じ数）。止まった個体の帯では 0。 */
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

/** 承認待ちも止まった個体も無ければ `null`（帯を出さない）。 */
export function approvalBanner(views: readonly CommandPolicyView[]): ApprovalBanner | null {
  const waiting = views.filter((view) => view.pending.length > 0);
  const first = waiting[0];
  if (first !== undefined) {
    const count = pendingTotal(waiting);
    const others = waiting.length - 1;
    return {
      key: others === 0 ? "chat.approvalWaitingOne" : "chat.approvalWaitingMany",
      params: { name: first.name, others, count },
    };
  }
  const stopped = views.filter((view) => view.stoppedForApproval);
  const stoppedFirst = stopped[0];
  if (stoppedFirst === undefined) return null;
  const others = stopped.length - 1;
  return {
    key: others === 0 ? "chat.approvalStoppedOne" : "chat.approvalStoppedMany",
    params: { name: stoppedFirst.name, others, count: 0 },
  };
}
