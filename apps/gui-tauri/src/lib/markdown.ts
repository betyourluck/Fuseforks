/**
 * 会話バブル用の Markdown 描画。
 *
 * # 安全性の前提
 *
 * 入力は **LLM の出力**であり、信頼できないテキストとして扱う。
 * - `html: false` — 原文中の生 HTML はタグとして解釈せず、そのままテキストへ
 *   エスケープされる。サニタイザを後掛けするのではなく、HTML を解釈する経路を
 *   最初から持たない（DOMPurify を足す理由ごと消す）。
 * - リンク先は `lib/links.ts` の閉じた許容（http / https / mailto だけ描く）。
 *   markdown-it 既定の `validateLink` は `javascript:` 等を落とす除外リストで、
 *   カスタムスキームを通していた（2026-10-30 の監査 8）。
 *
 * # 改行の扱い
 *
 * `breaks: true` で単独の改行を `<br>` にする。従来の表示は
 * `whitespace-pre-wrap` で全改行が見えていたので、md 描画に切り替えても
 * 「改行したのに詰まった」という見た目の退行を起こさないため。
 */
import MarkdownIt from "markdown-it";

import { isRenderableLink } from "./links";
import { markdownMath } from "./markdownMath";

const md = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: true,
});

// 描くリンクは閉じた許容で決める（既定は除外リスト）。linkify が拾った素の URL も同じ門を通る。
// 落ちたリンクは `[text](url)` の文字のまま出る（黙って消えない）。
md.validateLink = (url) => isRenderableLink(url);

// **画像は展開しない**（2026-10-30 の監査）。`![](https://…)` を `<img>` にすると、描画した
// 瞬間に WebView がその URL へ GET を出す — 利用者のクリックなしに、本文へ混ぜた文字列が
// 外へ届く経路になる（web 検索・URL 取得・MCP の fetch で読んだページからの注入が入口）。
// 添付の画像は `blob:` の別経路で描くので、ここを切っても失うものは無い。
// 画像記法は `![alt](url)` のテキストのまま出る。
md.disable("image");

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
 * 描画の選択肢。`env` として markdown-it へ渡す（描画規則が読む）。
 */
export interface RenderOptions {
  /**
   * コードブロックの右上に置く「コードをコピー」ボタンのラベル（2026-10-15 利用者要望）。
   * **省く（既定）とボタンは生えず、出力は従来どおり**。ボタンを押したときの動作は
   * 描く側（ChatPanel の `onMarkdownClick`）が `data-code-copy` で受ける —
   * ここは純関数なので、クリップボードにも i18n にも触らない（ラベルは呼び手が訳して渡す）。
   */
  codeCopyLabel?: string;
}

/** `data-code-copy` の在る `<button>` を 1 本組む。ラベルはエスケープして属性へ。 */
function codeCopyButton(label: string): string {
  const escaped = md.utils.escapeHtml(label);
  return `<button type="button" class="code-copy" data-code-copy aria-label="${escaped}" title="${escaped}">⧉</button>`;
}

/**
 * フェンスコード（```）とインデントコードを `<div class="code-block">` で包み、ラベルが
 * 渡されたときだけボタンを添える。**`<pre>` 自体は既定の描画のまま** — 包むだけなので、
 * ラベル無しでは出力が 1 バイトも変わらない（`markdown.test.ts` の既存の期待がそのまま効く）。
 */
for (const rule of ["fence", "code_block"] as const) {
  const defaultRule =
    md.renderer.rules[rule] ??
    ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
  md.renderer.rules[rule] = (tokens, idx, options, env, self) => {
    const html = defaultRule(tokens, idx, options, env, self);
    const label = (env as RenderOptions | undefined)?.codeCopyLabel;
    if (!label) return html;
    return `<div class="code-block">${html}${codeCopyButton(label)}</div>`;
  };
}

// 数式（2026-10-24）。**コピーボタンの包み込みより後に組み込む** — ```math のフェンスは
// 数式の規則が先に受けてコードブロックにせず、それ以外のフェンスだけを上の包み込みへ渡す
// （順を逆にすると KaTeX の出力が `.code-block` に包まれてボタンが生える）。
md.use(markdownMath);

/**
 * Markdown を HTML 文字列へ描画する。戻り値は `v-html` で挿す前提。
 * 入力が Markdown として不成立でも例外にはならない（プレーンテキストとして出る）。
 */
export function renderMarkdown(source: string, options: RenderOptions = {}): string {
  return md.render(source, options);
}

/** 描画結果のキャッシュ上限。会話ログの上限（コア側 5,000 件）と同程度に持つ。 */
const CACHE_LIMIT = 5000;

interface CacheEntry {
  /** 描いたときのラベル。違えば描き直す（言語を切り替えた直後の 1 回だけ）。 */
  codeCopyLabel: string | undefined;
  html: string;
}

const cache = new Map<string, CacheEntry>();

/**
 * メッセージ ID をキーに描画結果を記憶する版。
 *
 * 発話は一度記録されたら不変なので、ID キャッシュは安全。バブルは
 * リアクティブ更新のたびに再描画されるため、素で `render` を呼ぶと
 * 新着 1 件のたびに全履歴を parse し直すことになる。
 * 選択肢（ボタンのラベル）はキャッシュの一部 — 一致しない古い描画は捨てる。
 */
export function renderMarkdownCached(
  id: string,
  source: string,
  options: RenderOptions = {},
): string {
  const hit = cache.get(id);
  if (hit !== undefined && hit.codeCopyLabel === options.codeCopyLabel) return hit.html;

  const rendered = renderMarkdown(source, options);
  if (hit === undefined && cache.size >= CACHE_LIMIT) {
    // 挿入順の先頭 = 最も古い描画から捨てる。
    const oldest = cache.keys().next().value;
    if (oldest !== undefined) cache.delete(oldest);
  }
  cache.set(id, { codeCopyLabel: options.codeCopyLabel, html: rendered });
  return rendered;
}
