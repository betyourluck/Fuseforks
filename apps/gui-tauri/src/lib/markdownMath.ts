/**
 * 会話ペインの Markdown に数式（LaTeX）を足す markdown-it プラグイン（2026-10-24）。
 *
 * # 区切り
 *
 * GitHub の Markdown と同じ 4 形式 + LLM が既定で出す 2 形式を拾う:
 * - `$…$`（行内）/ `` $`…`$ ``（行内。Markdown と衝突する文字をそのまま書く形）
 * - `$$…$$`（ブロック。行内に置いても表示数式）/ ```` ```math ````（ブロック）
 * - `\(…\)`（行内）/ `\[…\]`（ブロック）
 *
 * `$…$` は金額と衝突するので Pandoc の規則で厳しく判定する（参考にした Outcast の
 * 本番には金額の `$` を含む行が 14 行あり、1 つも数式にしてはいけなかった）:
 * 開きの直前が英数字でない / 開きの直後が空白でない / 閉じの直前が空白でない /
 * 閉じの直後が数字でない / 改行を跨がない。**閉じは「開きの後に最初に現れた生の `$`」だけ** —
 * 条件を満たさない `$` を飛ばして先を探すと「料金は $10 … $50 …。式 $\frac{a$」で
 * 金額から数式の閉じまでを丸ごと数式にする（数式の中の `$` は `\$` と書くのが GitHub の決まり）。
 *
 * # 置き場が markdown-it の規則である理由
 *
 * Outcast は marked の前に数式を抜いてプレースホルダへ置き換え、描画後の DOM を KaTeX で
 * 書き換える（marked が `_` `*` を強調にする・`\(` をエスケープに食う・`>>N` をアンカーにする
 * ため）。この村の描画は markdown-it なので、**インライン規則を `escape` の前に置けば**同じことが
 * パーサの中で済み、コードスパンとフェンスの中はパーサが先に切り出すので触らない。
 * KaTeX の描画も `renderToString` で**この場で同期的に**行う — `renderMarkdown` は純関数で
 * ID キャッシュの対象なので、描画後の DOM 書き換え（`hydrate`）を持ち込む理由が無い
 * （Outcast が別チャンクへ遅延させるのはネットワーク越しのページだから。配布物は WebView の
 * ローカル読み込みで、数式の無い村でも KaTeX の読み込みが 1 回増えるだけ）。
 *
 * # 安全性
 *
 * - `trust: false` を外さない — `\href` `\url` `\htmlClass` `\htmlStyle` `\htmlData` を無効にし、
 *   KaTeX の出力に URL・任意の class・style を持ち込ませない（入力は LLM の出力）。
 * - `throwOnError: false` — 不正な TeX は例外ではなく原文を色付きで出す（読めないより原文が見える）。
 *   その色は `errorColor` に **CSS 変数の参照**を渡してテーマから引く（KaTeX は文字列をそのまま
 *   `style="color:…"` へ書く。既定の `#cc0000` は生の色でライトでもダークでも固定になる）。
 * - `maxExpand` / `maxSize` — 1 発話で WebView を固める・画面を覆う出力の上限。
 * - `strict: "ignore"` — 数式の中の日本語などで警告を console へ出さない。
 * - `output: "html"` — MathML を併記しない。併記すると同じ式が textContent に 2 度現れ、
 *   会話の検索（`data-chat-text` の文字列走査）と範囲選択のコピーが二重になる。
 */
import katex from "katex";
import "katex/dist/katex.min.css";
import type MarkdownIt from "markdown-it";
import type StateBlock from "markdown-it/lib/rules_block/state_block.mjs";
import type StateInline from "markdown-it/lib/rules_inline/state_inline.mjs";

/** KaTeX の描画の選択肢。`displayMode` だけ呼び出し側が決める。 */
const KATEX_OPTIONS = {
  throwOnError: false,
  trust: false,
  strict: "ignore",
  maxExpand: 1000,
  maxSize: 20,
  output: "html",
  errorColor: "var(--color-warn)",
} as const;

const isSpace = (c: string | undefined) => c === undefined || /\s/.test(c);
const isAsciiAlnum = (c: string | undefined) => c !== undefined && /[A-Za-z0-9]/.test(c);
const isDigit = (c: string | undefined) => c !== undefined && /[0-9]/.test(c);

/**
 * `$…$` の閉じを `[from, max)` で探す。見つからなければ -1。
 * 開きの後に最初に現れた生の `$` だけを閉じの候補にし、条件を満たさなければその開きは数式ではない。
 */
function findInlineClose(s: string, from: number, max: number): number {
  for (let j = from; j < max; j++) {
    const c = s[j];
    if (c === "\n") return -1;
    if (c === "\\") {
      j++; // `\$` などのエスケープは閉じにならない
      continue;
    }
    if (c !== "$") continue;
    if (s[j + 1] === "$") return -1; // `$$` が混ざる形は曖昧なので数式にしない
    return !isSpace(s[j - 1]) && !isDigit(s[j + 1]) ? j : -1;
  }
  return -1;
}

/** エスケープされていない `needle` を `[from, max)` で探す。 */
function findUnescaped(s: string, needle: string, from: number, max: number): number {
  for (let j = from; j <= max - needle.length; j++) {
    if (s[j] === "\\") {
      j++;
      continue;
    }
    if (s.startsWith(needle, j)) return j;
  }
  return -1;
}

interface Found {
  tex: string;
  display: boolean;
  /** 区切り込みの原文。KaTeX が描けなかったときはこれを文字として出す。 */
  raw: string;
  /** 数式の直後の位置。 */
  end: number;
}

/** `pos` に数式の開きがあれば切り出す。無ければ `null`。 */
function scanInline(src: string, pos: number, max: number): Found | null {
  const c = src[pos];
  if (c === "\\") {
    const n = src[pos + 1];
    if (n !== "(" && n !== "[") return null;
    const closer = n === "(" ? "\\)" : "\\]";
    const end = src.indexOf(closer, pos + 2);
    if (end === -1 || end + 2 > max) return null;
    const tex = src.slice(pos + 2, end).trim();
    if (!tex) return null;
    return { tex, display: n === "[", raw: src.slice(pos, end + 2), end: end + 2 };
  }
  if (c !== "$") return null;

  if (src[pos + 1] === "$") {
    const end = findUnescaped(src, "$$", pos + 2, max);
    if (end === -1) return null;
    const tex = src.slice(pos + 2, end).trim();
    if (!tex) return null;
    return { tex, display: true, raw: src.slice(pos, end + 2), end: end + 2 };
  }

  if (src[pos + 1] === "`") {
    // $`…`$ : n 個のバッククォートで開き、ちょうど n 個で閉じ、その直後が `$`。
    const run = /^`+/.exec(src.slice(pos + 1, max))![0].length;
    let j = pos + 1 + run;
    while (j < max) {
      const k = src.indexOf("`".repeat(run), j);
      if (k === -1 || k + run > max) return null;
      // 並びの長さがぴったり n で、直後が `$` のときだけ閉じ。
      const after = k + run;
      if (src[after] === "`") {
        j = after;
        while (src[j] === "`") j++;
        continue;
      }
      if (src[after] !== "$") return null;
      const tex = src.slice(pos + 1 + run, k).trim();
      if (!tex) return null;
      return { tex, display: false, raw: src.slice(pos, after + 1), end: after + 1 };
    }
    return null;
  }

  if (isAsciiAlnum(src[pos - 1]) || isSpace(src[pos + 1])) return null;
  const end = findInlineClose(src, pos + 1, max);
  if (end === -1) return null;
  return { tex: src.slice(pos + 1, end), display: false, raw: src.slice(pos, end + 1), end: end + 1 };
}

function mathInline(state: StateInline, silent: boolean): boolean {
  const found = scanInline(state.src, state.pos, state.posMax);
  if (!found) return false;
  if (!silent) {
    const token = state.push("math", "", 0);
    token.content = found.tex;
    token.markup = found.raw;
    token.meta = { display: found.display };
  }
  state.pos = found.end;
  return true;
}

/**
 * 行頭の `$$` / `\[` で始まるブロック。閉じは同じ行の末尾か、後ろの行の末尾。
 * 閉じの後ろに文字が続く行（`$$a+b$$ を見よ`）はブロックにせず、インライン規則へ任せる。
 */
function mathBlock(state: StateBlock, startLine: number, endLine: number, silent: boolean): boolean {
  if (state.sCount[startLine] - state.blkIndent >= 4) return false;
  const start = state.bMarks[startLine] + state.tShift[startLine];
  const first = state.src.slice(start, state.eMarks[startLine]);
  const open = first.startsWith("$$") ? "$$" : first.startsWith("\\[") ? "\\[" : null;
  if (!open) return false;
  const close = open === "$$" ? "$$" : "\\]";

  const lines: string[] = [];
  let nextLine: number;
  const rest = first.slice(2);
  const sameLine = findUnescaped(rest, close, 0, rest.length);
  if (sameLine !== -1) {
    if (rest.slice(sameLine + 2).trim() !== "") return false;
    lines.push(rest.slice(0, sameLine));
    nextLine = startLine + 1;
  } else {
    lines.push(rest);
    let line = startLine + 1;
    for (;;) {
      if (line >= endLine) return false; // 閉じない: 文字のまま
      const text = state.src.slice(state.bMarks[line] + state.tShift[line], state.eMarks[line]);
      const at = findUnescaped(text, close, 0, text.length);
      if (at !== -1 && text.slice(at + 2).trim() === "") {
        lines.push(text.slice(0, at));
        nextLine = line + 1;
        break;
      }
      lines.push(text);
      line++;
    }
  }
  const tex = lines.join("\n").trim();
  if (!tex) return false;
  if (silent) return true;

  const token = state.push("math_block", "", 0);
  token.block = true;
  token.content = tex;
  token.markup = open;
  token.map = [startLine, nextLine];
  state.line = nextLine;
  return true;
}

/**
 * TeX を HTML へ。`throwOnError: false` でも構文エラー以外は投げうるので、
 * そのときは区切り込みの原文をエスケープして文字のまま出す（消さない）。
 */
export function renderTex(tex: string, display: boolean, raw: string, escape: (s: string) => string): string {
  try {
    return katex.renderToString(tex, { ...KATEX_OPTIONS, displayMode: display });
  } catch {
    return `<span class="math-error">${escape(raw)}</span>`;
  }
}

/** `md.use(markdownMath)` で組み込む。フェンスの規則は**この後に上書きされない**順で呼ぶこと。 */
export function markdownMath(md: MarkdownIt): void {
  md.inline.ruler.before("escape", "math", mathInline);
  md.block.ruler.before("fence", "math_block", mathBlock, {
    alt: ["paragraph", "reference", "blockquote", "list"],
  });

  md.renderer.rules.math = (tokens, idx) => {
    const token = tokens[idx];
    return renderTex(token.content, Boolean(token.meta?.display), token.markup, md.utils.escapeHtml);
  };
  md.renderer.rules.math_block = (tokens, idx) => {
    const token = tokens[idx];
    const raw = `${token.markup}\n${token.content}\n${token.markup === "$$" ? "$$" : "\\]"}`;
    return `${renderTex(token.content, true, raw, md.utils.escapeHtml)}\n`;
  };

  // ```math はブロックの数式（GitHub と同じ）。それ以外のフェンスは元の規則（コピーボタンの包み込み）へ。
  const fence =
    md.renderer.rules.fence ??
    ((tokens, idx, options, _env, self) => self.renderToken(tokens, idx, options));
  md.renderer.rules.fence = (tokens, idx, options, env, self) => {
    const token = tokens[idx];
    if (token.info.trim() !== "math") return fence(tokens, idx, options, env, self);
    const tex = token.content.trim();
    const raw = `\`\`\`math\n${token.content}\`\`\``;
    return `${renderTex(tex, true, raw, md.utils.escapeHtml)}\n`;
  };
}
