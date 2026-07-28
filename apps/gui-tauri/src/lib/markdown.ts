/**
 * 会話バブル用の Markdown 描画。
 *
 * # 安全性の前提
 *
 * 入力は **LLM の出力**であり、信頼できないテキストとして扱う。
 * - `html: false` — 原文中の生 HTML はタグとして解釈せず、そのままテキストへ
 *   エスケープされる。サニタイザを後掛けするのではなく、HTML を解釈する経路を
 *   最初から持たない（DOMPurify を足す理由ごと消す）。
 * - リンク先は markdown-it 既定の `validateLink` が `javascript:` 等の
 *   危険スキームを弾く。
 *
 * # 改行の扱い
 *
 * `breaks: true` で単独の改行を `<br>` にする。従来の表示は
 * `whitespace-pre-wrap` で全改行が見えていたので、md 描画に切り替えても
 * 「改行したのに詰まった」という見た目の退行を起こさないため。
 */
import MarkdownIt from "markdown-it";

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
});

// リンクは新しいウィンドウ扱いにする。Tauri の webview 内でそのまま遷移すると
// アプリ画面ごとリンク先に置き換わって戻れなくなる。実際に外部ブラウザで開く
// 処理は ChatPanel 側のクリックハンドラ（plugin-opener）が担い、この属性は
// 「webview 内では遷移しない」ことの保険になる。
const defaultLinkOpen =
  md.renderer.rules.link_open ??
  ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
md.renderer.rules.link_open = (tokens, idx, options, env, self) => {
  tokens[idx].attrSet("target", "_blank");
  tokens[idx].attrSet("rel", "noopener noreferrer");
  return defaultLinkOpen(tokens, idx, options, env, self);
};

/**
 * Markdown を HTML 文字列へ描画する。戻り値は `v-html` で挿す前提。
 * 入力が Markdown として不成立でも例外にはならない（プレーンテキストとして出る）。
 */
export function renderMarkdown(source: string): string {
  return md.render(source);
}

/** 描画結果のキャッシュ上限。会話ログの上限（コア側 5,000 件）と同程度に持つ。 */
const CACHE_LIMIT = 5000;

const cache = new Map<string, string>();

/**
 * メッセージ ID をキーに描画結果を記憶する版。
 *
 * 発話は一度記録されたら不変なので、ID キャッシュは安全。バブルは
 * リアクティブ更新のたびに再描画されるため、素で `render` を呼ぶと
 * 新着 1 件のたびに全履歴を parse し直すことになる。
 */
export function renderMarkdownCached(id: string, source: string): string {
  const hit = cache.get(id);
  if (hit !== undefined) return hit;

  const rendered = renderMarkdown(source);
  if (cache.size >= CACHE_LIMIT) {
    // 挿入順の先頭 = 最も古い描画から捨てる。
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  cache.set(id, rendered);
  return rendered;
}
