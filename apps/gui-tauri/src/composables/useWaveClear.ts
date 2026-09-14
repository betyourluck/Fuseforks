/**
 * 作業状況タブ（波ペイン）の表示クリア（2026-09-15 利用者要望）。
 *
 * **消すのは表示だけで、波の記録は 1 件も消さない。** 記録の所有者はコアの
 * in-memory リング（上限 50・プロセス寿命）で、ここは画面の見え方だけを持つ。
 * 会話ペインの表示クリア（`useChatClear`）と同じ規律。
 *
 * ## 境界ではなく「押した時点で終わっていた波」の集合を持つ
 *
 * 会話ペインは時刻の境界 1 つで足りたが、波は同じ物差しで切れない:
 *
 * - **`planId` の境界は再起動で壊れる** — `planId` はプロセス内でだけ単調増加で、
 *   再起動すると 1 から振り直す。境界を保存すると、再起動後の新しい波が隠れる
 * - **`startedAtMs` の境界は実行中の波を後から消す** — 押した時点で走っていた
 *   古い波が、終わった瞬間に境界の手前として消える（見ている最中に列が抜ける）
 *
 * そこで「押した時点で終わっていた波」を `planId:startedAtMs` の鍵で覚える。
 * 確認待ち（Spec 43）と実行中の波は**押しても隠れない**し、後から終わった波が
 * 勝手に消えることもない。開始時刻を鍵に含めるので、再起動後に同じ `planId` が
 * 振られても別の波として扱われる。
 *
 * 保存先は `localStorage`（`useChatClear` と同じ棚）。**端末の見え方**であって
 * 村の内容物ではないので `world.json` には混ぜない。
 */

import { reactive, watch } from "vue";

import type { PlanWaveRecord } from "../types";

const STORAGE_KEY = "fuseforks.wavesCleared.v1";

/** 波の同一性の鍵。`planId` だけだと再起動後の波と衝突する。 */
export function waveKey(wave: PlanWaveRecord): string {
  return `${wave.planId}:${wave.startedAtMs}`;
}

/**
 * 隠してよい（終わった）波か。
 *
 * **確認待ちは隠さない** — 人の操作を待っている波で、隠すと承認し忘れる。
 * **実行中のタスクが 1 つでもあれば隠さない** — 動いているものを見えなくしない。
 * 破棄された波は終わった側。
 */
export function isSettled(wave: PlanWaveRecord): boolean {
  if (wave.state === "pending") return false;
  if (wave.state === "discarded") return true;
  return !wave.tasks.some((task) => task.state === "running");
}

/** 保存済みの鍵を読む。壊れていたら・文字列でない要素はそのぶんだけ捨てる。 */
function load(): string[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((key): key is string => typeof key === "string");
  } catch {
    // 壊れた保存値でタブが開けなくなるほうが害が大きい（useChatClear と同じ判断）。
    return [];
  }
}

const hidden = reactive<{ keys: string[] }>({ keys: load() });

watch(
  () => hidden.keys,
  (next) => {
    try {
      localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
    } catch {
      // 保存できなくてもその場のクリアは効く。
    }
  },
);

export function useWaveClear() {
  return {
    /** 表示から隠している波か。 */
    isHidden(wave: PlanWaveRecord): boolean {
      return hidden.keys.includes(waveKey(wave));
    },

    /**
     * いま終わっている波をすべて隠す。
     *
     * **集合は置き換える**（足し込まない）。前に隠した波がまだリングに居れば
     * 終わった側なので再び入り、リングから押し出された波の鍵はここで落ちる —
     * 保存が育ち続けない。
     */
    clear(waves: readonly PlanWaveRecord[]): void {
      hidden.keys = waves.filter(isSettled).map(waveKey);
    },

    /** 隠すのをやめる。 */
    restore(): void {
      hidden.keys = [];
    },
  };
}
