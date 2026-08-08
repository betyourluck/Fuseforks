/**
 * ツール名の表示（2026-08-08）。
 *
 * 画面に出ていたのは `room_log` のような**識別子そのもの**だった。起点は実機の
 * 利用者 —「`room_log` っていうのも会話をたどるっていうくらいでいいかもしれない」。
 *
 * **一番効くのは理由の出ないツール**（`ask` / `plan` / `room_log`）。
 * あれらは Spec 27 で理由欄の対象外なので、**名前だけが唯一の情報**になる。
 * `grep` は「意図: 〜」が意味を運ぶが、`room_log` は名前が運ぶしかない。
 *
 * **外部（MCP）のツールは訳さない。** 名付けたのは接続先で、こちらが訳語を当てると
 * **何が走ったかについて嘘をつく**ことになる（`wants_reason` を外部へ足さないのと同じ理由）。
 * 識別子のまま等幅で出すので、**書体そのものが「これは自分たちのものではない」**を示す。
 *
 * **識別子は消さない。** 呼び出し側が `title` に出すことで、撮った画面から
 * `fuseforks.log` の `name=` を引ける（ステータスバーの時計を `diag.rs` と
 * 同じ書式に固定したのと同じ規律 — 画面とログの対応を切らない）。
 *
 * これは型・IPC・ログを 1 つも変えない**表示名だけの改名**で、
 * 「波ペイン → 作業状況」「村の地図 → サーヴァントの絆」と同型（四例目）。
 */

/** 委譲ツールの接頭辞。orchestrator が宛先ごとに合成する。 */
const ASK_PREFIX = "ask_";
const TRANSFER_PREFIX = "transfer_to_";

/**
 * この村が名付けたツールの表示名（辞書の鍵の末尾）。
 *
 * **同梱 9 本 + 合成 3 種。** ここに無い名前は外部として扱われるので、
 * **同梱ツールを足したらここにも足す** — 忘れると自分たちのツールが
 * 「外部のもの」として等幅で出る。`toolLabel.test.ts` が Rust の
 * `BUNDLED_TOOL_NAMES` と突き合わせて機械で留めている。
 */
const KNOWN_TOOLS = new Set([
  // 同梱（BUNDLED_TOOL_NAMES ∪ {rag}）
  "remember",
  "grep",
  "fd",
  "diff",
  "sd",
  "yq",
  "file",
  "rag",
  "run",
  // orchestrator 合成
  "plan",
  "room_log",
  "handoff",
]);

/** 表示の指示。訳語ではなく**辞書の鍵**を返す（純関数は i18n を知らない）。 */
export type ToolLabel =
  /** この村が名付けたツール。`target` は委譲の宛先（表示名）。 */
  | { kind: "known"; key: string; target?: string }
  /** 外部（MCP）由来。訳さず識別子のまま出す。 */
  | { kind: "external"; id: string };

/**
 * ツール名を表示へ落とす。
 *
 * `nameOf` は宛先の表示名を引く関数。**引けなければ id をそのまま返す実装**を
 * 渡すこと（消えたサーヴァントへの委譲でも行は出る）。
 */
export function toolLabel(tool: string, nameOf: (agentId: string) => string): ToolLabel {
  if (tool.startsWith(ASK_PREFIX)) {
    return { kind: "known", key: "tools.ask", target: nameOf(tool.slice(ASK_PREFIX.length)) };
  }
  if (tool.startsWith(TRANSFER_PREFIX)) {
    return {
      kind: "known",
      key: "tools.transfer",
      target: nameOf(tool.slice(TRANSFER_PREFIX.length)),
    };
  }
  if (KNOWN_TOOLS.has(tool)) {
    return { kind: "known", key: `tools.${tool}` };
  }
  return { kind: "external", id: tool };
}

/** 表として読みたいときの参照（テスト用。実行時の判定はこれを使わない）。 */
export const KNOWN_TOOL_NAMES: readonly string[] = [...KNOWN_TOOLS];
