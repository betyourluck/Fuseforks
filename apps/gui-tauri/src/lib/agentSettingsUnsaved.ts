import { CONFIG_FILE_LABELS, type ConfigFileKind } from "../types";

/**
 * 設定ダイアログの未保存を 1 つにまとめたもの（2026-10-19）。
 *
 * ダイアログの左（設定フォーム）と右（`MarkdownEditor`）は**保存が別々**で、左の「保存」は
 * 右のファイルを保存しない。だから閉じるときの確認とフッターの表示は、**どちらが未保存かを
 * 名指しする**必要がある。それまでは左だけを見ていたので、右だけを書き換えて閉じると確認なしで
 * 編集が消えた。
 *
 * 一括保存にはしない — 左は `updateAgent`、右は `writeConfig` で失敗の仕方が違い（mcp.json /
 * run.json は壊れた JSON を拒否する）、片方だけ通った状態が生まれる。
 *
 * 訳語ではなく**辞書の鍵**を返す（`reasonDisplay` と同じ形。純関数は i18n を知らない）。
 */
export interface UnsavedSummary {
  /** 閉じるときの確認の本文。 */
  messageKey: "agentSettings.discardForm" | "agentSettings.discardFile" | "agentSettings.discardBoth";
  /** フッターの表示。 */
  noteKey: "agentSettings.unsaved" | "agentSettings.unsavedFile" | "agentSettings.unsavedBoth";
  /** 未保存のファイルの名前。左だけのときは空文字。 */
  params: { file: string };
}

/**
 * @param formDirty 左の設定フォームに未保存の変更があるか
 * @param file 右のエディタで未保存のファイルの種別。無ければ `null`
 *   （エディタはタブを切り替えるときに確認するので、未保存は常に高々 1 つ）
 * @returns どちらも保存済みなら `null` — 確認を出さない
 */
export function unsavedSummary(formDirty: boolean, file: ConfigFileKind | null): UnsavedSummary | null {
  const params = { file: file === null ? "" : CONFIG_FILE_LABELS[file] };
  if (formDirty && file !== null) {
    return { messageKey: "agentSettings.discardBoth", noteKey: "agentSettings.unsavedBoth", params };
  }
  if (file !== null) {
    return { messageKey: "agentSettings.discardFile", noteKey: "agentSettings.unsavedFile", params };
  }
  if (formDirty) {
    return { messageKey: "agentSettings.discardForm", noteKey: "agentSettings.unsaved", params };
  }
  return null;
}
