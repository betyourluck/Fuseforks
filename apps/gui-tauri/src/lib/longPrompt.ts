/**
 * 会話ペインで**送信済みの自分の発言**を畳むかの規則（2026-09-14 利用者要望）。
 *
 * **対象は利用者の発言だけ**（利用者裁定）。サーヴァントの返答や、サーヴァント同士の依頼文は
 * 畳まない。
 *
 * 判定は**本文だけで決める**（描画後の高さを測らない）。会話ペインは CSS の `zoom` が掛かる
 * 場所で、座標を読む計算を 1 つも持たない（CLAUDE.md「会話の表示倍率」— zoom を選んだ
 * 決め手の 1 つ）。測る形にすると、その前提をここで崩す。
 */

/** これを超える行数なら畳む。 */
export const LONG_PROMPT_LINES = 12;

/**
 * これを超える文字数なら畳む。**文字は code point で数える** — `length` は UTF-16 の
 * 単位なので、絵文字などで枠がずれる（`MAX_*_CHARS` を `len()` で数えて日本語で
 * 枠の 1/3 になった事故の同族を作らない）。改行の無い長い 1 段落はこちらで拾う。
 */
export const LONG_PROMPT_CHARS = 800;

/** 畳んだときに見せる行数（Tailwind の `line-clamp-6` と揃える）。 */
export const COLLAPSED_LINES = 6;

export function isLongPrompt(content: string): boolean {
  if (content.split("\n").length > LONG_PROMPT_LINES) return true;
  return [...content].length > LONG_PROMPT_CHARS;
}
