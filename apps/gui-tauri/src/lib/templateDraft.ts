import type { ModelTemplate } from "../types";

/**
 * 新規の下書きの識別子が、登録済みのテンプレートと重なっているか。
 *
 * **新規かどうかは識別子から逆算しない**（呼び出し側が「新規」を押した事実で持つ）。2026-11-05 まで
 * 新規かどうかを「識別子が登録済みに無いか」で決めていたので、新規の入力中に既存の識別子と同じ文字列を
 * 打った瞬間に既存の扱いになり、欄が読み取り専用になって続きを打てなかった（`gpt5` → `gpt5_mini` の途中で
 * 止まる）。重なりは打ち続けられる状態のまま知らせ、保存だけを止める — 保存すると upsert が既存の行を
 * 上書きする。
 */
export function idTakenByAnother(
  creating: boolean,
  id: string,
  existingIds: readonly string[],
): boolean {
  return creating && existingIds.includes(id);
}

/**
 * モデルテンプレートの下書きが、保存済みの値と違うか（Spec 68 D1）。
 *
 * 接続テストは**保存済みのテンプレート**だけを確かめる（確認と本番の写しを 1 つにする）ので、下書きが保存済みと
 * 違う間はボタンを押せない。比較の規則をここ 1 実装に置く — 欄はすべて素の値（文字列・数・真偽・null）で、
 * 入力の側（温度の空欄 / 単価の未入力）は既に `null` へ揃えてから下書きへ入れている。欄が無いことと `null` は
 * 同じに扱う（古い記録や後から足した欄で、欄の有無だけの違いを未保存と読まない）。
 *
 * `saved` が `null` = まだ保存されていない新規の下書き → 未保存。
 */
export function isTemplateDirty(draft: ModelTemplate, saved: ModelTemplate | null): boolean {
  if (saved === null) return true;
  const a = draft as unknown as Record<string, unknown>;
  const b = saved as unknown as Record<string, unknown>;
  const keys = new Set([...Object.keys(a), ...Object.keys(b)]);
  for (const key of keys) {
    if ((a[key] ?? null) !== (b[key] ?? null)) return true;
  }
  return false;
}
