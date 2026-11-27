/**
 * 中央下段ペイン（黒板 / 作業状況）の見出し行が**どの幅でも 1 行**であることの配線を
 * 機械で留める（2026-11-27 利用者要望 —「狭くするとラベルが 1 文字ずつ折り返して
 * 見出しが 2 行になる。アイコンに変わるのが良い」）。
 *
 * 出し分けは `style.css` の container query（`bottom-pane-header`）と、部品が付ける
 * `pane-head-wide` / `pane-head-narrow` の 2 つの印で決まる。**名前は 3 ファイルで手で
 * 揃えている**ので、片方だけ改名すると切り替えが黙って効かなくなる（型検査にも lint にも
 * 掛からない — `chatZoomWiring` と同じ形）。幅そのもの（閾値の値）は留めない — 実測で
 * 決めた数で、言語のラベルが変われば測り直すもの。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const css = read("../style.css");
const tabs = read("../components/BottomPaneTabs.vue");
const waves = read("../components/PlanWavePane.vue");
const board = read("../components/BlackboardPane.vue");

/** 部品の最初の `<header` から閉じタグまで（ペインの見出し行。黒板は付箋にも header がある）。 */
function paneHeader(src: string): string {
  const at = src.indexOf("<header");
  expect(at, "<header").toBeGreaterThan(-1);
  return src.slice(at, src.indexOf("</header>", at));
}

/** 開きタグ 1 つ（`<tag` から最初の `>` まで。属性に `>` を含まない要素だけに使う）。 */
function openTag(src: string, from: number): string {
  return src.slice(from, src.indexOf(">", from) + 1);
}

/** `@container bottom-pane-header (min-width: Npx) { … }` の塊を、閾値と中身の組で返す。 */
function containerBlocks(): Array<{ min: number; body: string }> {
  const blocks: Array<{ min: number; body: string }> = [];
  const re = /@container bottom-pane-header \(min-width: (\d+)px\) \{/g;
  for (let m = re.exec(css); m; m = re.exec(css)) {
    // 中の規則は 1 段だけ入れ子なので、`}` が 2 つ続く所（空行を挟む）までを中身とする。
    const end = css.indexOf("\n}\n", m.index);
    blocks.push({ min: Number(m[1]), body: css.slice(m.index, end) });
  }
  return blocks;
}

describe("bottomPaneHeader: コンテナと閾値（style.css）", () => {
  it("見出し行がコンテナで、名前は container query と同じ", () => {
    const at = css.indexOf(".bottom-pane-header {");
    expect(at, ".bottom-pane-header").toBeGreaterThan(-1);
    const rule = css.slice(at, css.indexOf("}", at));
    expect(rule).toContain("container-type: inline-size;");
    expect(rule).toContain("container-name: bottom-pane-header;");
    expect(containerBlocks().length, "@container bottom-pane-header").toBeGreaterThan(0);
  });

  it("狭い側が既定（文字は消え、アイコンは出たまま）", () => {
    expect(css).toMatch(/\.pane-head-wide \{\s*display: none;\s*\}/);
    // アイコンに既定の display を書かない（svg の既定のまま出る）。
    expect(css).not.toMatch(/^\.pane-head-narrow \{/m);
  });

  it("どの閾値でも、文字を出す規則とアイコンを消す規則が対になっている", () => {
    for (const { min, body } of containerBlocks()) {
      expect(body, `${min}px`).toMatch(/\.pane-head-wide \{\s*display: inline;/);
      expect(body, `${min}px`).toMatch(/\.pane-head-narrow \{\s*display: none;/);
    }
  });

  it("ja / zh は狭い閾値、それ以外（英語と、長さの分からない言語）は広い閾値", () => {
    const blocks = containerBlocks();
    const cjk = blocks.filter((b) => b.body.includes(":lang(ja)") && b.body.includes(":lang(zh)"));
    const rest = blocks.filter((b) => !b.body.includes(":lang("));
    expect(cjk.length, "ja / zh の塊").toBe(1);
    expect(rest.length, "言語を問わない塊").toBe(1);
    // 逆にすると、日本語の村は広い閾値まで文字を出さず、英語の村は狭い幅で選択欄が潰れる。
    expect(cjk[0].min).toBeLessThan(rest[0].min);
  });
});

describe("bottomPaneHeader: 見出し行は 1 行（黒板と作業状況）", () => {
  for (const [name, src] of [
    ["PlanWavePane", waves],
    ["BlackboardPane", board],
  ] as const) {
    it(`${name}: コンテナの印・折り返さない・高さ 38px`, () => {
      const header = openTag(src, src.indexOf("<header"));
      expect(header).toContain("bottom-pane-header");
      expect(header).toContain("whitespace-nowrap");
      expect(header).toContain("h-[38px]");
    });

    it(`${name}: 件数の文は選択欄より先に縮み、「…」で切れる`, () => {
      const head = paneHeader(src);
      const counts = head.match(/<span[^>]*shrink-\[100\][^>]*>/g) ?? [];
      expect(counts.length, "shrink-[100] の文").toBeGreaterThan(0);
      for (const tag of counts) {
        expect(tag).toContain("min-w-0");
        expect(tag).toContain("truncate");
      }
    });

    it(`${name}: 選択欄は縮める（min-w-0）。縮めないと狭い幅ではみ出す`, () => {
      const head = paneHeader(src);
      const select = openTag(head, head.indexOf("<select"));
      expect(select).toContain("min-w-0");
    });
  }
});

describe("bottomPaneHeader: タブ（BottomPaneTabs）", () => {
  it("名前は pane-head-wide、アイコンは pane-head-narrow の対", () => {
    expect(tabs).toMatch(/<svg\s+class="pane-head-narrow[^"]*"/);
    expect(tabs).toMatch(/<span class="pane-head-wide">\{\{ \$t\(tab\.labelKey\) \}\}<\/span>/);
  });

  it("承認待ちのバッジは名前の外（アイコンだけの状態でも消えない）", () => {
    const label = tabs.indexOf('<span class="pane-head-wide">');
    const badge = tabs.indexOf("data-waves-pending");
    expect(label).toBeGreaterThan(-1);
    expect(badge).toBeGreaterThan(label);
    // 名前の span は、バッジより前で閉じている。
    expect(tabs.slice(label, badge)).toContain("</span>");
  });

  it("名前は title と aria-label に残る（アイコンだけの状態の手掛かり）", () => {
    const button = openTag(tabs, tabs.indexOf("<button"));
    expect(button).toContain(':title="titleOf(tab)"');
    expect(button).toContain(':aria-label="titleOf(tab)"');
    // ホバーの 1 行目が名前。
    expect(tabs).toMatch(/return `\$\{t\(tab\.labelKey\)\}\\n\$\{detail\}`;/);
  });
});

describe("bottomPaneHeader: 作業状況の見出し（PlanWavePane）", () => {
  const head = paneHeader(waves);

  it("「検証役:」は狭いときアイコンになり、選択欄の名前は aria-label に残る", () => {
    const at = head.indexOf("data-default-verifier");
    expect(at, "data-default-verifier").toBeGreaterThan(-1);
    const label = head.slice(at, head.indexOf("</label>", at));
    expect(label).toMatch(/<svg\s+class="pane-head-narrow[^"]*"/);
    expect(label).toContain('<span class="pane-head-wide">{{ $t("waves.verifierLabel") }}</span>');
    expect(openTag(label, label.indexOf("<select"))).toContain(":aria-label=");
  });

  it("「すべて表示」は縮まず、縮む包みに入れない（包むとはみ出して検証役の欄に重なる）", () => {
    const at = head.indexOf("waveClear.restore()");
    expect(at).toBeGreaterThan(-1);
    const button = openTag(head, head.lastIndexOf("<button", at));
    expect(button).toContain("shrink-0");
    expect(head).toContain('<template v-if="hiddenCount">');
    // 狭いときは目のアイコン、文字は aria-label に残す。
    expect(button).toContain(":aria-label=");
    expect(head.slice(head.lastIndexOf("<button", at), head.indexOf("</button>", at))).toMatch(
      /<svg\s+class="pane-head-narrow/,
    );
  });
});
