/**
 * ショートカット一覧（`lib/shortcuts.ts`）が**実装と食い違っていない**ことを機械で留める（2026-10-27）。
 *
 * 一覧は鍵を受ける handler の写しで、どこからも参照されない。handler の鍵を変えても
 * 型検査にも lint にも掛からず、**画面のチートシートだけが黙って嘘になる**。そこで項目ごとに
 * 「その鍵を受けている実装の字面」を下の表に書き、ソースにあることを確かめる —
 * - 一覧に項目を足して根拠を書かない → 赤（表と一覧の id の集合が一致しない）
 * - handler の鍵を変える（例: 検索の Enter / Shift+Enter を入れ替える）→ 赤（字面が消える）
 * - 辞書の鍵を欠く・古い鍵を残す → 赤（鍵を実行時に組むので vue-tsc も i18n の検査も通る）
 *
 * 字面はコメントを落としてから探す（説明文に同じ文が残っていても緑にしない）。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { SHORTCUT_SECTIONS } from "./shortcuts";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

/** `//` と `/* *\/` と `<!-- -->` のコメントを落とす（説明文に名前を書くのは自由）。 */
function code(source: string): string {
  return source
    .replace(/<!--[\s\S]*?-->/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "")
    .replace(/^\s*\/\/.*$/gm, "");
}

function between(source: string, start: string, end: string): string {
  const from = source.indexOf(start);
  expect(from, `${start} が無い`).toBeGreaterThanOrEqual(0);
  const to = source.indexOf(end, from + start.length);
  return source.slice(from, to === -1 ? undefined : to);
}

interface Evidence {
  /** このファイルからの相対パス。 */
  file: string;
  /** その鍵を受けている実装の字面（コメントを落とした後に在ること）。 */
  text: string;
}

const CHAT_INPUT = "../components/ChatInput.vue";
const CHAT_PANEL = "../components/ChatPanel.vue";
const CODE_EDITOR = "../components/CodeEditor.vue";
const TOUR = "../components/FirstRunTour.vue";

/** 一覧の項目 → その鍵を受けている実装。**項目を足したらここにも足す。** */
const EVIDENCE: Record<string, readonly Evidence[]> = {
  showShortcuts: [
    { file: "./shortcuts.ts", text: 'return event.key === "?";' },
    { file: "../App.vue", text: 'openSettings("shortcuts");' },
  ],
  focusChat: [{ file: "./chatFocus.ts", text: 'return event.key === "/";' }],
  selectAgent: [
    {
      file: "./agentNav.ts",
      text: "if (!event.altKey || event.ctrlKey || event.metaKey || event.shiftKey) return null;",
    },
    { file: "./agentNav.ts", text: 'if (event.key === "ArrowDown") return 1;' },
    { file: "./agentNav.ts", text: 'active.hasAttribute("data-agent-search")' },
  ],
  searchChat: [
    { file: "./chatSearch.ts", text: 'return event.key.toLowerCase() === "f";' },
    { file: "./chatSearch.ts", text: "if (event.ctrlKey === event.metaKey) return false;" },
  ],
  send: [
    { file: CHAT_INPUT, text: '@keydown.enter.exact="onEnter"' },
    { file: CHAT_INPUT, text: "void send();" },
  ],
  // `.exact` なので Shift+Enter は送信に拾われず、textarea の改行になる。
  newline: [{ file: CHAT_INPUT, text: '@keydown.enter.exact="onEnter"' }],
  pasteImage: [
    { file: CHAT_INPUT, text: '@paste="onPaste"' },
    { file: CHAT_INPUT, text: 'if (item.kind === "file") {' },
  ],
  completePath: [{ file: "./pathComplete.ts", text: 'kind: found.length === 1 ? "file" : "message",' }],
  quoteMessage: [{ file: "./pathComplete.ts", text: 'kind: found.length === 1 ? "file" : "message",' }],
  pickCandidate: [
    { file: CHAT_INPUT, text: '@keydown.down="moveSelection(1, $event)"' },
    { file: CHAT_INPUT, text: '@keydown.up="moveSelection(-1, $event)"' },
  ],
  confirmCandidate: [
    { file: CHAT_INPUT, text: "confirmSelected();" },
    { file: CHAT_INPUT, text: '@keydown.tab="onTab"' },
  ],
  closeCompletion: [{ file: CHAT_INPUT, text: '@keydown.esc="onEscape"' }],
  nextMatch: [{ file: CHAT_PANEL, text: "moveSearch(event.shiftKey ? -1 : 1);" }],
  prevMatch: [{ file: CHAT_PANEL, text: "moveSearch(event.shiftKey ? -1 : 1);" }],
  closeSearch: [
    { file: CHAT_PANEL, text: 'if (event.key === "Escape") {' },
    { file: CHAT_PANEL, text: "if (back?.isConnected) back.focus();" },
  ],
  clearAgentSearch: [
    { file: "../components/AgentList.vue", text: '@keydown.escape="onSearchEscape"' },
    { file: "../components/AgentList.vue", text: "if (searchQuery.value) clearSearch();" },
  ],
  cancelLink: [
    { file: "../components/TopologyMap.vue", text: 'if (event.key !== "Escape") return;' },
    { file: "../components/TopologyMap.vue", text: "else if (linkSource.value) cancelLink();" },
  ],
  resizePane: [
    { file: "../components/PaneSplitter.vue", text: 'if (event.key === forward) emit("delta", 16);' },
    { file: "../components/PaneSplitter.vue", text: 'tabindex="0"' },
  ],
  saveFile: [{ file: CODE_EDITOR, text: 'key: "Mod-s",' }],
  findInFile: [{ file: CODE_EDITOR, text: "...searchKeymap," }],
  noReload: [{ file: CODE_EDITOR, text: '{ key: "Mod-r", run: () => true },' }],
  sendAssist: [
    {
      file: "../components/AssistPanel.vue",
      text: 'if (event.key === "Enter" && (event.ctrlKey || event.metaKey)) {',
    },
  ],
  cancelConfirm: [{ file: "../components/ConfirmHost.vue", text: 'if (event.key === "Escape" && current.value) {' }],
  tourNext: [{ file: TOUR, text: '} else if (e.key === "Enter" || e.key === "ArrowRight") {' }],
  tourBack: [{ file: TOUR, text: '} else if (e.key === "ArrowLeft" && phase.value === "tour") {' }],
  tourClose: [{ file: TOUR, text: 'if (e.key === "Escape") {' }],
};

const itemIds = SHORTCUT_SECTIONS.flatMap((s) => s.items.map((i) => i.id));
const sectionIds = SHORTCUT_SECTIONS.map((s) => s.id);

describe("一覧の項目と実装の突き合わせ", () => {
  it("根拠の表と一覧の項目が同じ集合（足したら両方に書く）", () => {
    expect(Object.keys(EVIDENCE).sort()).toEqual([...itemIds].sort());
  });

  for (const id of itemIds) {
    it(`${id} の鍵を受ける実装が在る`, () => {
      for (const evidence of EVIDENCE[id] ?? []) {
        expect(code(read(evidence.file)), `${evidence.file}: ${evidence.text}`).toContain(evidence.text);
      }
    });
  }
});

describe("辞書", () => {
  type ShortcutDict = { section: Record<string, string>; item: Record<string, string> };
  const dicts = {
    ja: ja.settings.shortcuts as unknown as ShortcutDict,
    en: en.settings.shortcuts as unknown as ShortcutDict,
  };

  for (const [lang, dict] of Object.entries(dicts)) {
    it(`${lang}: 区分けと項目の鍵が一覧と同じ集合（欠けも残りも無い）`, () => {
      expect(Object.keys(dict.section).sort()).toEqual([...sectionIds].sort());
      expect(Object.keys(dict.item).sort()).toEqual([...itemIds].sort());
    });
  }
});

describe("配線", () => {
  const app = code(read("../App.vue"));
  const settings = code(read("../components/SettingsDialog.vue"));

  it("「?」は「/」と同じ条件でだけ奪い、ショートカット一覧のページを開く", () => {
    const handler = between(app, "function onShortcutsKey", "\n}");
    expect(handler).toContain("if (!isShortcutsKey(event)) return;");
    expect(handler).toContain('if (view.value !== "village" || showTour.value) return;');
    expect(handler).toContain("if (isEditableElement(document.activeElement)) return;");
    expect(handler).toContain("if (hasOpenOverlay(document)) return;");
    expect(handler).toContain('openSettings("shortcuts");');
    expect(app).toContain('window.addEventListener("keydown", onShortcutsKey);');
    expect(app).toContain('window.removeEventListener("keydown", onShortcutsKey);');
  });

  it("開くページは設定ダイアログへ渡り、タイトルバーからは目録の先頭で開く", () => {
    expect(app).toContain(':initial-page="settingsPage"');
    expect(app).toContain('@open-settings="openSettings()"');
    expect(settings).toContain('const page = ref<Page>(props.initialPage ?? "user");');
  });

  it("設定のユーザーインターフェースにページがあり、案内は最下段のまま", () => {
    expect(settings).toContain("selectPage('shortcuts')");
    expect(settings).toContain("page === 'shortcuts'");
    expect(settings).toContain('v-for="section in SHORTCUT_SECTIONS"');
    expect(settings).toContain("keyLabel(token, keyPlatform)");
    // 案内は利用者裁定で「ユーザーインターフェースの一番下」。後ろに項目を足さない。
    const nav = between(settings, "<nav", "</nav>");
    const tour = nav.indexOf("selectPage('tour')");
    expect(nav.indexOf("selectPage('shortcuts')")).toBeLessThan(tour);
    expect(nav.indexOf("selectPage(", tour + 1)).toBe(-1);
  });
});
