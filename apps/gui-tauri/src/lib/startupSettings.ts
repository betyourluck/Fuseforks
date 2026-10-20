/**
 * システム設定 ＞ 全般 ＞ 起動と終了 の表示の判定（Spec 69 D4・D7）。純関数。
 *
 * 画面が出す注記は 3 つで、どれも「OS の登録を読んだ結果」から決まる — アプリは自動起動の写しを持たない。
 */

import type { AutostartView, StartupSettings } from "../types";

/**
 * 登録されているコマンドが、今動いている Fuseforks を指していないか。
 *
 * 開発ビルドで ON にした・MSI と NSIS が並んで入っている（`failures.md` #112 の形）と、登録は 1 つの名前を
 * 共有するので、最後に ON にした exe が登録される。**チェックが ON に見えても、起動するのは別の Fuseforks**
 * という形をここで見せる。Windows のパスは大文字小文字を区別しないので小文字で比べる。
 */
export function registeredElsewhere(view: AutostartView): boolean {
  if (!view.registeredCommand || !view.currentExe) return false;
  return !view.registeredCommand.toLowerCase().includes(view.currentExe.toLowerCase());
}

/**
 * 全利用者向けの登録（Windows の HKLM）が残っているか。Fuseforks は HKLM に書かないので、残っていれば
 * 別の経路で入ったもの。**管理者でない起動では消せない**ので、チェックを外しても ON のまま見える（Spec 69 D4）。
 */
export function machineRegistrationLeft(view: AutostartView): boolean {
  return view.scope === "machine" || view.scope === "both";
}

/**
 * 3 つとも ON か（Spec 69 D7・D10）。このときだけ「サインインするだけで、誰も見ていない間に予定が発火する」を出す。
 * 時間あたりの上限は持たない（トークン制限は依頼 1 本の因果ごと）ので、ここが唯一の知らせになる。
 */
export function runsUnattended(settings: StartupSettings, autostart: AutostartView | null): boolean {
  return settings.minimizeToTray && settings.batchStartOnLaunch && autostart?.enabled === true;
}
