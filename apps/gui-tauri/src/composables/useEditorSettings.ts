/**
 * 外部エディタの設定（2026-11-08）。端末の棚 `editor.json` の写しを画面全体で 1 つだけ持つ。
 *
 * 読むのは「エディタで開く」を押せるか（会話ペインと黒板のメニュー）と、システム設定のページ。
 * **最初に使われたときに 1 回だけ読み**、保存したらこの写しを差し替える（ページと メニューが別々に
 * 読み直してずれる形を作らない）。読めなかったときは未設定として扱う（押せないだけで、壊れはしない）。
 */
import { computed, ref } from "vue";
import * as ipc from "../lib/ipc";
import type { EditorSettings } from "../types";

const settings = ref<EditorSettings>({ command: "", args: [] });
let loading: Promise<void> | null = null;

function ensureLoaded(): Promise<void> {
  loading ??= ipc
    .getEditorSettings()
    .then((value) => {
      settings.value = value;
    })
    .catch(() => {
      // 読めなければ未設定のまま（次に開いたときに読み直せるよう、待ちを捨てる）。
      loading = null;
    });
  return loading;
}

export function useEditorSettings() {
  void ensureLoaded();
  return {
    settings,
    /** 「エディタで開く」を押せるか。 */
    configured: computed(() => settings.value.command.trim().length > 0),
    reload: () => {
      loading = null;
      return ensureLoaded();
    },
    async save(next: EditorSettings): Promise<EditorSettings> {
      const saved = await ipc.setEditorSettings(next);
      settings.value = saved;
      return saved;
    },
  };
}
