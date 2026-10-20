/**
 * 閉じる前の確認に何を書くか（2026-08-08 利用者要望）。
 *
 * **確認そのものは小言になりうる。** 「終了しますか？」だけなら、押す前に
 * 一手増えるだけで情報は 1 ビットも増えない。**中身が本体** — 閉じると
 * 何が止まるかを、そのときの状態から数えて出す。
 *
 * **終了すると止まるもの**を並べる確認。トレイ常駐（Spec 69）が OFF（既定）の間は ✕ が終了で、
 * 閉じると**飛行中のターンも MCP の扉も予定の発火も全部止まる**が、
 * **その事実はいまどの画面にも出ていない。ここが唯一言う場所**になる。トレイ常駐が ON のときは
 * ✕ は窓を隠すだけで（`hidesOnClose`）、この確認はトレイの「終了」に掛かる — 本文は同じく正しい。
 * （2026-08-07 の「常駐しない」の判断は Spec 69 で既定 OFF のオプトインとして覆した。）
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
  // 予定は**常に**書く。止まる事実は稼働状態に依らず、しかも他のどの画面にも出ていない
  // （終了すれば予定は発火しない。トレイに隠れている間は終了していないので、この確認は出ない）。
  lines.push({ key: "closeConfirm.schedules" });
  return lines;
}

/** 閉じる操作を「隠す」に変えるかの判定に要る状態（Spec 69 D2）。 */
export interface HideOnCloseState {
  /** 閉じるとトレイに隠れる設定。まだ読んでいなければ `false` として扱う。 */
  minimizeToTray: boolean;
  /** 組み立てが済んでいるか（`state.ready`）。 */
  ready: boolean;
  /** 組み立てに失敗しているか（`state.initError` がある）。 */
  failed: boolean;
}

/**
 * 閉じる操作（✕ / OS の閉じる操作）で窓を隠すか（Spec 69 D2。`launch_exit_contract` 2）。
 *
 * **組み立てていない間は隠さない。** 村を開けていないプロセスがトレイに残ると、多重起動の枠だけを
 * 握り、次の起動が「前面化して閉じる」に吸われて村を開けなくなる。隠さないときは今のまま
 * （閉じる確認 → 終了）。**隠すときは確認を出さない** — 何も止まらない。
 */
export function hidesOnClose(state: HideOnCloseState): boolean {
  return state.minimizeToTray && state.ready && !state.failed;
}

/**
 * トレイの「終了」が押されたことを知らせるイベント名（Spec 69 D2）。Rust の `tray.rs` の
 * `QUIT_EVENT` と同じ文字列 — 走査テストが 2 つを突き合わせる。
 */
export const TRAY_QUIT_EVENT = "tray://quit-requested";
