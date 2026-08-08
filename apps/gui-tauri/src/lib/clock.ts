/**
 * ステータスバーの時計の整形。
 *
 * # なぜ言語に追従させないか
 *
 * 形式は `YYYY-MM-DD HH:MM:SS` に固定し、日本語でも英語でも同じ文字列を出す。
 * ロケール整形（`toLocaleString`）を使わないのは、この時計の用途が
 * **スクリーンショットと `fuseforks.log` の突き合わせ**だから。
 * 診断ログの行頭は `diag.rs` の `%Y-%m-%d %H:%M:%S%.3f` で、ここを同じ形にすると
 * 画面の時刻からログの該当行を目で引ける（ミリ秒だけがログ側に多い）。
 *
 * ロケール整形にすると en では `8/3/2026, 10:15:03 PM` になり、
 * (1) 月日の順が読み手の国で変わる (2) 12 時間制で午前・午後の判別が要る
 * (3) ログの並びと突き合わせられない、の 3 つが同時に起きる。
 * **言語に追従しないのは追従漏れではなく、突き合わせのための固定**である。
 *
 * # なぜ現在時刻を引数で受け取るか
 *
 * `schedule.rs` の規律（現在時刻は必ず引数で受け取り、内部で `Local::now()` を
 * 呼ばない）と同じ。内部で `new Date()` を読むと、テストが壁時計に依存して
 * 特定の時刻でだけ落ちるものになる。ティッカーを持つのは呼び出し側。
 */

/** 2 桁ゼロ詰め。`5` → `"05"`。 */
function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

/**
 * 端末のローカル時刻を `YYYY-MM-DD HH:MM:SS` にする。
 *
 * - **ローカル時刻**（`getFullYear` 系）。`toISOString` は UTC なので使わない —
 *   ログが `Local::now()` である以上、画面が UTC だと 9 時間ずれて突き合わせが壊れる
 * - 24 時間制。`00:00:00` 〜 `23:59:59`
 * - 無効な `Date` は `—` を返す（`NaN-NaN-NaN` を画面に出さない。
 *   `format.ts` の `compactNumber` と同じ扱いに揃えてある）
 */
export function formatClock(now: Date): string {
  const t = now.getTime();
  if (!Number.isFinite(t)) return "—";

  const date = `${now.getFullYear()}-${pad2(now.getMonth() + 1)}-${pad2(now.getDate())}`;
  const time = `${pad2(now.getHours())}:${pad2(now.getMinutes())}:${pad2(now.getSeconds())}`;
  return `${date} ${time}`;
}
