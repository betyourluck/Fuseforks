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
   * ウィンドウを閉じる前に確認を出すか（2026-08-08 利用者要望）。
   *
   * **既定 ON。ON のときは常に聞く** — 「何か走っているときだけ聞く」は
   * 冗長だと利用者が裁定した（2026-08-08）。加えて、**チェックボックスが
   * 「確認する」なのに聞かない場合があると壊れて見える**。
   *
   * **小言にしないのは中身の側の仕事。** この村はタスクトレイに常駐しないので、
   * 閉じると**飛行中のターン・MCP の扉・予定の発火がすべて止まる**。
   * その事実はいまどの画面にも出ておらず、**このダイアログが唯一言う場所**になる。
   */
  confirmClose: boolean;
  /**
   * 入退室の通知（`… が稼働を開始しました` ほか）を会話ペインに出すか
   * （2026-08-08 利用者要望）。
   *
   * **既定 ON = これまでどおり出す。** 隠すほうをオプトインにしたのは、
   * 既定を変えると既存の村の画面が黙って変わるため。
   *
   * **これは表示だけの設定で、モデルへ届く量は 1 バイトも変わらない。**
   * 入退室がプロンプトへ乗る経路は `compose_presence_notices`（コア）で、
   * そちらは生ログの直近 `room_log_window` 件から System 発を拾う別の機構。
   * 隠すと減ると読まれると誤解になるので、説明文でも言い切る。
   *
   * **失敗による停止は隠さない**（`presenceNotice.ts` の述語が外している）。
   * カードは「いまの状態」しか示さないので、過去に落ちた事実の置き場が
   * 会話ログしか無い。
   */
  showPresenceNotices: boolean;
  /**
   * サーヴァントの絆で、**ペインの大きさが変わった後に Fit を掛け直すか**
   * （2026-08-08 利用者要望）。
   *
   * **既定 OFF。** 掛けるのは「変化のたび」ではなく**リサイズの後だけ**だが、
   * それでも panzoom で寄せた視点が窓の大きさを変えた瞬間に戻る。
   * 既定を ON にすると既存の村の見え方が黙って変わるので、オプトインにする
   * （`showPresenceNotices` と同じ判断）。
   *
   * **Vue Flow に「追従し続ける」プロパティは無い**（1.48.2 が持つのは
   * 初期化時 1 回の `fitViewOnInit` と命令的な `fitView()` だけ）。ここが
   * 見ているのは**コンテナの箱の変化だけ**で、ノードの移動や辺の増減では
   * 発火しない — ドラッグ中に視点が動くと Spec 21 の drop の座標とずれる。
   */
  autoFitOnResize: boolean;
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
  confirmClose: true,
  showPresenceNotices: true,
  autoFitOnResize: false,
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
      confirmClose:
        typeof parsed.confirmClose === "boolean"
          ? parsed.confirmClose
          : DEFAULTS.confirmClose,
      showPresenceNotices:
        typeof parsed.showPresenceNotices === "boolean"
          ? parsed.showPresenceNotices
          : DEFAULTS.showPresenceNotices,
      autoFitOnResize:
        typeof parsed.autoFitOnResize === "boolean"
          ? parsed.autoFitOnResize
          : DEFAULTS.autoFitOnResize,
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
