/**
 * 閉じる前の確認に何を書くか（2026-08-08 利用者要望）。
 *
 * **確認そのものは小言になりうる。** 「終了しますか？」だけなら、押す前に
 * 一手増えるだけで情報は 1 ビットも増えない。**中身が本体** — 閉じると
 * 何が止まるかを、そのときの状態から数えて出す。
 *
 * **この村はタスクトレイに常駐しない**（2026-08-07 利用者判断。常駐は
 * 「開いただけで課金が始まる作りにしない」を裏返す）。ゆえに閉じている間は
 * **飛行中のターンも MCP の扉も予定の発火も全部止まる**が、
 * **その事実はいまどの画面にも出ていない。ここが唯一言う場所**になる。
 *
 * 純関数は i18n を知らないので、**訳語ではなく辞書の鍵**を返す
 * （`reasonDisplay` / `batchLabel` と同じ形）。
 */

/** 確認を組み立てるのに要る、閉じる時点の状態。 */
export interface CloseState {
  /** 稼働中のサーヴァントの数。飛行中のターンはここでしか失われない。 */
  runningAgents: number;
  /** MCP の扉が実際に開いているか。**設定が ON かどうかではない。** */
  mcpListening: boolean;
}

/** 1 行ぶんの表示指示。 */
export interface CloseConfirmLine {
  key: string;
  params?: Record<string, number>;
}

/**
 * 確認の本文を組み立てる。
 *
 * **常に出る 2 行**（前置きと予定）に、**そのとき当てはまる分だけ**を挟む。
 * 当てはまらない行を「0 体です」と書かないのは、**失われないものを
 * 並べても判断の材料にならない**ため。
 */
export function closeConfirmLines(state: CloseState): CloseConfirmLine[] {
  const lines: CloseConfirmLine[] = [{ key: "closeConfirm.lead" }];
  if (state.runningAgents > 0) {
    lines.push({ key: "closeConfirm.running", params: { count: state.runningAgents } });
  }
  if (state.mcpListening) {
    lines.push({ key: "closeConfirm.mcp" });
  }
  // 予定は**常に**書く。止まる事実は稼働状態に依らず、しかもこの村では
  // 他のどの画面にも出ていない（トレイに常駐しないことの帰結）。
  lines.push({ key: "closeConfirm.schedules" });
  return lines;
}
