/**
 * 会話ペインの表示クリア（2026-08-08 利用者要望）。
 *
 * **消すのは表示だけで、中身は 1 件も消さない。** 会話は `sessions.redb` に
 * 残り、モデルが読む `shared.log` にも残る。**押しても送信量は減らない** —
 * 減ると読まれると誤解になるので、ツールチップと戻す導線の両方で言い切る。
 *
 * ## 会話ごとに、境界を 1 つだけ持つ
 *
 * 持つのは「その会話をどの時刻まで隠したか」の `tsMs` 1 つ。件数や発話 ID では
 * なく時刻なのは、**発話とツール実行の両方が同じ物差しで切れる**から
 * （`AgentMessage.tsMs` と `ToolRun.tsMs`）。ID にすると 2 系統の突き合わせが要る。
 *
 * **会話ごとに持つ**ので、会話を切り替えれば別の会話は無傷。新規チャットと
 * 分岐は新しい `session_id` を持つので、境界が無く最初から全部見える。
 *
 * 保存先は `localStorage`（`usePaneLayout` / `useUiSettings` と同じ棚）。
 * **端末の見え方**であって村の内容物ではないので `world.json` には混ぜない —
 * 混ぜると村を配ったときに相手の画面まで隠すことになる。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "concordia.chatCleared.v1";

/** 会話 ID → その時刻以前を隠す（`tsMs`）。 */
type Boundaries = Record<string, number>;

/** 保存済みの境界を読む。壊れていたら・数でなければその会話ぶんを捨てる。 */
function load(): Boundaries {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as unknown;
    if (typeof parsed !== "object" || parsed === null) return {};

    const out: Boundaries = {};
    for (const [sessionId, value] of Object.entries(parsed as Record<string, unknown>)) {
      // 手編集の文字列・null・NaN は落とす。**素通しにすると比較が常に偽になり、
      // 「クリアしたのに効かない」が理由の分からない形で出る。**
      if (typeof value === "number" && Number.isFinite(value)) out[sessionId] = value;
    }
    return out;
  } catch {
    // 壊れた保存値で会話が開けなくなるほうが害が大きい（useUiSettings と同じ判断）。
    return {};
  }
}

const boundaries = reactive<Boundaries>(load());

watch(boundaries, (next) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくてもその場のクリアは効く。
  }
});

export function useChatClear() {
  return {
    /**
     * その会話の境界（無ければ 0 = 何も隠していない）。
     *
     * **0 を「境界なし」と兼ねてよい**のは、`tsMs` が epoch ミリ秒で
     * 0 より大きいから。別に `null` を持つと比較の前に分岐が 1 つ増える。
     */
    clearedAt(sessionId: string): number {
      return boundaries[sessionId] ?? 0;
    },

    /**
     * その時刻以前を隠す。
     *
     * **境界は「見えている最後の行の時刻」を渡す**（`Date.now()` ではない）。
     * 壁時計を使うと、押した瞬間に届いた発話が境界の前後どちらに落ちるかが
     * 実行のたびに変わる（`schedule.rs` の「内部で `Local::now()` を呼ばない」
     * と同じ規律）。
     */
    clear(sessionId: string, atMs: number): void {
      boundaries[sessionId] = atMs;
    },

    /** 隠すのをやめる。**行を消すのではなく鍵ごと落とす** — 0 を書くと保存が育つ。 */
    restore(sessionId: string): void {
      delete boundaries[sessionId];
    },
  };
}
