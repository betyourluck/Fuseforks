/**
 * CodeMirror の検索パネルの文言を表示言語へ追従させる。
 *
 * `CodeEditor.vue` は `@codemirror/search` を積んでいるので、`Ctrl+F` を押すと
 * **検索欄と置換欄を持つパネル**が開く。そのボタンとプレースホルダは
 * ライブラリが英語で持っており、日本語表示の村でもそこだけ英語で残っていた。
 *
 * **鍵は CodeMirror が `state.phrase()` へ渡す英語の原文そのもの**
 * （`@codemirror/search` の実装が持つ文字列）。だから `locales/*.json` には
 * 置いていない — あちらの鍵は `editor.save` のような識別子で、こちらは
 * **ライブラリ側の文面が鍵**になる。`en` は既定が英語なので上書きせず
 * 空の表を返すので、`ja` / `en` の鍵集合一致テストの対象にもならない。
 *
 * **代償**: 三言語目を足すときはここに腕が 1 つ増える（辞書ファイルの追加だけでは
 * 追従しない）。`editorPhrases.test.ts` がライブラリ側の原文を実際に走査して
 * 表と突き合わせるので、**ライブラリの更新で文面が増えたときは赤くなる**。
 */
import type { Language } from "../types";

const JA: Readonly<Record<string, string>> = {
  // 検索パネル（`Ctrl+F`）
  Find: "検索",
  Replace: "置換後の文字列",
  next: "次へ",
  previous: "前へ",
  all: "すべて選択",
  "match case": "大文字小文字を区別",
  regexp: "正規表現",
  "by word": "単語単位",
  replace: "置換",
  "replace all": "すべて置換",
  close: "閉じる",
  // 行へ移動（`Ctrl+Alt+G`）
  "Go to line": "行へ移動",
  go: "移動",
  // 読み上げ用の通知（画面には出ないが、訳さないとそこだけ英語で読まれる）。
  // `$` は CodeMirror が引数で差し替える席なので、必ず 1 つ残す。
  "current match": "現在の一致",
  "on line": "行",
  "replaced match on line $": "$ 行目の一致を置換しました",
  "replaced $ matches": "$ 件の一致を置換しました",
};

/**
 * その言語で CodeMirror へ渡す文言表。**英語は空**（ライブラリの既定が英語なので、
 * 同じ文字列を写すと「訳した」のか「既定のまま」なのかが読めなくなる）。
 */
export function searchPhrases(locale: Language): Readonly<Record<string, string>> {
  return locale === "ja" ? JA : {};
}
