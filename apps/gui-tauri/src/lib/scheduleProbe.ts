/**
 * 予定の前判定（Spec 28）の表示規則。**純関数だけ。**
 *
 * テンプレートの中で分岐すると、表示規則がテストの外へ出る
 * （`reasonDisplay` / `batchLabel` と同じ規律）。**訳語ではなく辞書の鍵**を
 * 返すので、この層は言語を知らない。
 */

import type { ProbeReport, ScheduleProbe, ScheduleView } from "../types";

/**
 * 引数欄のテキストを配列へ。**1 行 1 引数**（Spec 15 P4 の判断を踏襲）。
 *
 * 空白区切りにするとシェルの引用規則が要り、「シェルを介さない」という
 * Spec 28 D4 の設計と衝突する。**空行は落とす** — 末尾改行で空文字の引数が
 * 生えると、それがそのまま `argv` の 1 要素になる。
 */
export function parseProbeArgs(text: string): string[] {
  return text
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
}

/**
 * 前判定のコマンド行を 1 本の文字列へ。**承認の判断材料そのもの。**
 *
 * 承認ダイアログはこれを出す。**中身を見ずに押せる形にすると、承認が
 * 「読まずにクリックする儀式」に落ちる**（Spec 28 D10 の残余 —
 * 機構が肩代わりできるのは「配られた村が素通りしない」ところまで）。
 */
export function probeCommandLine(task: ScheduleView): string {
  if (!task.probe) return "";
  return commandLineOf(task.probe);
}

/**
 * 後判定（Spec 46）のコマンド行。承認の判断材料である点は前判定と同じ —
 * 承認は 1 回で前後両方に効くので、ダイアログは**両方の行**を出す。
 */
export function acceptanceCommandLine(task: ScheduleView): string {
  if (!task.acceptance) return "";
  return commandLineOf(task.acceptance);
}

/** コマンド行の組み立て 1 実装（前判定と後判定で共有）。 */
function commandLineOf(probe: ScheduleProbe): string {
  const parts = [probe.command, ...probe.args];
  const cwd = probe.cwd ? ` (cwd: ${probe.cwd})` : "";
  return `${parts.join(" ")}${cwd}`;
}

/** 直近の判定の表示に必要な素材。訳語は呼び出し側が引く。 */
export interface ProbeDisplay {
  /** 結末の辞書キー。 */
  labelKey: string;
  /**
   * 失敗理由（閉じた列挙の値）。**`outcome === "error"` のときだけ非 null。**
   *
   * 他の結末では計器が `-` を入れるので、そのまま出すと画面に `-` が並ぶ。
   */
  reason: string | null;
  /** 判定時刻（epoch ミリ秒）。 */
  atMs: number;
}

/**
 * 直近 1 回の判定を表示用に畳む。まだ走っていなければ `null`。
 *
 * **「走っていない」と「空の結末」を型で分ける** — 空文字を返すと
 * 「判定はしたが結末が無い」という別の主張になる（Spec 27 P2 の
 * `reasonDisplay` で踏んだのと同じ形）。
 */
export function probeDisplay(report: ProbeReport | null | undefined): ProbeDisplay | null {
  return reportDisplay(report, "schedule.probeOutcome");
}

/**
 * 直近 1 回の**検収**の表示（Spec 46）。
 *
 * **辞書の枝は前判定と分ける** — `probeOutcome.match` の訳語は
 * 「一致（依頼しました）」で、検収の一致は依頼ではなく確定。同じ 5 値でも
 * 文脈で意味が逆向きなので、訳語の表を共有すると片方が嘘をつく。
 */
export function acceptanceDisplay(report: ProbeReport | null | undefined): ProbeDisplay | null {
  return reportDisplay(report, "schedule.acceptanceOutcome");
}

/** 結末 → 表示素材の 1 実装（辞書の枝だけが違う）。 */
function reportDisplay(
  report: ProbeReport | null | undefined,
  prefix: string,
): ProbeDisplay | null {
  if (!report) return null;
  return {
    labelKey: `${prefix}.${report.outcome}`,
    reason: report.outcome === "error" ? report.reason : null,
    atMs: report.atMs,
  };
}

/**
 * 追加フォームが送信できるか（前判定の部分だけ）。
 *
 * **コア側でも読み込み時に弾かれる**（`InvalidProbe`）。ここで先に止めるのは
 * 手戻りを減らすためで、囲いではない — 検査の本体はコアにある。
 */
export function probeFormValid(input: {
  command: string;
  expect: string;
  timeoutSecs: number;
}): boolean {
  if (!input.command.trim()) return false;
  if (!input.expect.trim()) return false;
  return input.timeoutSecs >= 1 && input.timeoutSecs <= 3600;
}

/**
 * 後判定の欄が送信できるか（Spec 46 D5）。
 *
 * 判定部は前判定と同じ述語 + `maxAttempts` の値域（1..=5。整数でなければ拒否 —
 * `type="number"` は小数も通す）。コア側の `validate` が本体で、ここは手戻り減。
 */
export function acceptanceFormValid(input: {
  command: string;
  expect: string;
  timeoutSecs: number;
  maxAttempts: number;
}): boolean {
  if (!probeFormValid(input)) return false;
  return (
    Number.isInteger(input.maxAttempts) && input.maxAttempts >= 1 && input.maxAttempts <= 5
  );
}
