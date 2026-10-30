/**
 * リンクの閉じた許容（2026-10-30 の監査 8）。
 *
 * Markdown の href は LLM 出力・MCP の結果・読んだページから来る。markdown-it 既定の
 * `validateLink` は `javascript:` / `vbscript:` / `file:` / 画像でない `data:` を**落とす除外リスト**で、
 * それ以外のカスタムスキーム（`fuseforks://` や OS に登録された任意のハンドラ）はそのまま通る。
 * ここでは**描いてよいものを列挙する**（`run` の閉じた許容と同じ向き）。
 *
 * 描く集合と開く集合は別 — `mailto:` は描くが、押しても外部ブラウザへは渡さない
 * （開くのは http / https だけ。LLM 出力由来のリンクを信頼しない、は変えない）。
 * 2 つの述語はそれぞれ 1 実装で、会話ペインと黒板ペインが同じものを読む。
 */

/** Markdown がリンク（`<a href>`）として描いてよい href。http / https / mailto だけ。 */
export function isRenderableLink(href: string): boolean {
  return /^(?:https?:\/\/|mailto:)/i.test(href);
}

/** 押したとき外部ブラウザへ渡してよい href。http / https だけ（描ける集合の部分集合）。 */
export function isOpenableLink(href: string): boolean {
  return /^https?:\/\//i.test(href);
}
