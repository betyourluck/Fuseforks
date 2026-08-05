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

/** 配色。`style.css` の `:root[data-theme="light"]` と 1 対 1。 */
export type Theme = "dark" | "light";

export interface UiSettings {
  /**
   * サーヴァントの絆で絆（線）を切る前に確認を出すか。
   *
   * **既定 ON**（settings_contract で凍結）— 棚卸しで他 6 種の破壊的操作は
   * すべて確認ありで、線だけ無いのは整合性の破れだった。OFF は 1 回切れば
   * 済むが、ON を知らずに失うと接続は元に戻せない。
   */
  confirmEdgeDelete: boolean;
  /**
   * 配色（2026-08-05 利用者要望）。
   *
   * **既定は OS の設定から毎回決める。選ぶまで保存しない。** 起点は
   * 利用者の観察 —「一般はライトモードのほうを好む」。開発者の好み（ダーク）を
   * 既定に固定すると、多数派が初回に暗い画面を見ることになる。
   *
   * 言語（Spec 13 P3a）が「初回に OS から確定し、**再起動で再判定しない**」の
   * とは規律が違う。**保存先と影響範囲が違うから** — 言語は `world.json` に
   * 住み、System 行として**会話ログに焼き付く**ので、後から解釈が変わると
   * 保存済みの内容と食い違う。配色は端末側の見た目だけで、OS に追従して
   * 困る人がいない（困るなら選べばそこで固定される）。
   */
  theme: Theme;
}

/**
 * OS の配色設定。テスト環境（node）には `matchMedia` が無いので、
 * 無ければダーク＝これまでの見た目へ倒す。
 */
function osTheme(): Theme {
  if (typeof matchMedia !== "function") return "dark";
  return matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
}

const DEFAULTS: UiSettings = {
  confirmEdgeDelete: true,
  // 参照時に評価するため、実体は load() で入れる（モジュール読み込み順に依存しない）。
  theme: "dark",
};

/** 保存済みの設定を読む。壊れていたら・型が違ったら既定値へ落とす。 */
function load(): UiSettings {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULTS, theme: osTheme() };

    const parsed = JSON.parse(raw) as Partial<UiSettings>;
    return {
      // boolean 以外（手編集の文字列など）は既定へ倒す。?? で素通しにすると
      // 「真でも偽でもない値」が確認の分岐へ流れ込む。
      confirmEdgeDelete:
        typeof parsed.confirmEdgeDelete === "boolean"
          ? parsed.confirmEdgeDelete
          : DEFAULTS.confirmEdgeDelete,
      // **保存済みが 2 値のどちらかでなければ OS へ戻す。** 既定値の定数へ
      // 落とすと、手編集で壊れた村が「利用者はダークを選んだ」状態になる。
      theme: parsed.theme === "dark" || parsed.theme === "light" ? parsed.theme : osTheme(),
    };
  } catch {
    // 壊れた保存値で画面が開けなくなるほうが害が大きい。
    return { ...DEFAULTS, theme: osTheme() };
  }
}

const settings = reactive<UiSettings>(load());

/**
 * 配色を DOM へ映す。**属性が「ライト」のときだけ付く**のではなく常に付ける —
 * 付け外しで切り替えると、外した瞬間だけ既定（ダーク）へ戻る一瞬が生まれる。
 */
function applyTheme(theme: Theme): void {
  if (typeof document === "undefined") return;
  document.documentElement.dataset.theme = theme;
}

applyTheme(settings.theme);
watch(() => settings.theme, applyTheme);

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
