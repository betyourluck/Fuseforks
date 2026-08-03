/**
 * この画面の設定（settings_contract の「画面の設定」側。Spec 13 P2）。
 *
 * 保存先は localStorage `concordia.settings.v1`。村の状態ではなく**表示・操作の
 * 都合**なので、`world.json` には混ぜない（usePaneLayout と同じ棚 —
 * 混ぜると設定を配布したときに相手の操作の好みまで押し付けることになる）。
 *
 * 書き込みは値が**変わったときだけ**（watch）。設定ダイアログを開いて触らず
 * 閉じても localStorage は書き換わらない（settings_contract の検証項目）。
 */

import { reactive, watch } from "vue";

const STORAGE_KEY = "concordia.settings.v1";

export interface UiSettings {
  /**
   * 村の地図で接続（線）を削除する前に確認を出すか。
   *
   * **既定 ON**（settings_contract で凍結）— 棚卸しで他 6 種の破壊的操作は
   * すべて確認ありで、線だけ無いのは整合性の破れだった。OFF は 1 回切れば
   * 済むが、ON を知らずに失うと接続は元に戻せない。
   */
  confirmEdgeDelete: boolean;
}

const DEFAULTS: UiSettings = {
  confirmEdgeDelete: true,
};

/** 保存済みの設定を読む。壊れていたら・型が違ったら既定値へ落とす。 */
function load(): UiSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS };

    const parsed = JSON.parse(raw) as Partial<UiSettings>;
    return {
      // boolean 以外（手編集の文字列など）は既定へ倒す。?? で素通しにすると
      // 「真でも偽でもない値」が確認の分岐へ流れ込む。
      confirmEdgeDelete:
        typeof parsed.confirmEdgeDelete === "boolean"
          ? parsed.confirmEdgeDelete
          : DEFAULTS.confirmEdgeDelete,
    };
  } catch {
    // 壊れた保存値で画面が開けなくなるほうが害が大きい。
    return { ...DEFAULTS };
  }
}

const settings = reactive<UiSettings>(load());

watch(settings, (next) => {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  } catch {
    // 保存できなくても操作は続けられる（usePaneLayout と同じ判断）。
  }
});

export function useUiSettings() {
  return { settings };
}
