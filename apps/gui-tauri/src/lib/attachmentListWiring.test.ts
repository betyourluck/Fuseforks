/**
 * 添付の列と再添付の配線（Spec 73 P2）。
 *
 * 部品をマウントする土台が無いので、規則は `attachmentList.ts` の単体で、配線はソースの
 * 字面で留める。どれも型検査に掛からず、外れると**黙って**効かなくなる種類:
 * IPC の引数名（Tauri は `attachmentRefs` を `attachment_refs` へ写し、`Option` なので
 * 綴りがずれると再添付が空で送られる）/ 上限の値（コアと食い違うと、画面が通した
 * 発話をコアが断る）/ 実行時に組む辞書キー。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import { attachmentMime } from "./attachment";
import {
  MAX_ATTACHMENTS,
  MAX_MESSAGE_BYTES,
  canReattachFrom,
  type AttachLimit,
} from "./attachmentList";

const read = (path: string) =>
  readFileSync(fileURLToPath(new URL(`../../${path}`, import.meta.url)), "utf8");

/** コメントを落とす（走査テストはコメントも読むので、説明文の字面に当たらないように）。 */
const code = (src: string) =>
  src.replace(/<!--[\s\S]*?-->/g, "").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "");

const input = code(read("src/components/ChatInput.vue"));
const panel = code(read("src/components/ChatPanel.vue"));

describe("上限の値がコアと同じ", () => {
  const core = read("../../crates/fuseforks-core/src/attachment.rs");

  it("件数", () => {
    const found = /pub const ATTACHMENT_MAX_PER_MESSAGE: usize = (\d+);/.exec(core);
    expect(found, "attachment.rs に ATTACHMENT_MAX_PER_MESSAGE が在ること").not.toBeNull();
    expect(MAX_ATTACHMENTS).toBe(Number(found![1]));
  });

  it("合計バイト", () => {
    const found = /pub const ATTACHMENT_MAX_MESSAGE_BYTES: u64 = (\d+) \* 1024 \* 1024;/.exec(core);
    expect(found, "attachment.rs に ATTACHMENT_MAX_MESSAGE_BYTES が在ること").not.toBeNull();
    expect(MAX_MESSAGE_BYTES).toBe(Number(found![1]) * 1024 * 1024);
  });
});

describe("IPC", () => {
  it("引数の綴りが Rust と TS で揃っている", () => {
    expect(read("src-tauri/src/commands.rs")).toMatch(/attachment_refs: Option<Vec<String>>/);
    expect(read("src/lib/ipc.ts")).toMatch(/quoteIds,\s*attachmentRefs,\s*\}\);/);
  });

  it("入力欄 → 会話ペイン → orchestrator まで ID の列が通る", () => {
    // 入力欄は再添付を ID だけに割って 4 つ目の引数で渡す。
    expect(input).toMatch(/const \{ uploads, refIds \} = splitForSend\(failed\.attachments\)/);
    expect(input).toMatch(/props\.submit\(\s*failed\.text,\s*uploads,[\s\S]*?refIds,\s*\)/);
    expect(panel).toMatch(/quoteIds,\s*attachmentRefs,\s*\);/);
    expect(read("src/composables/useOrchestrator.ts")).toMatch(
      /ipc\.sendUserMessage\(agentId, content, undefined, attachments, quoteIds, attachmentRefs\)/,
    );
  });
});

describe("入力欄", () => {
  it("「参照…」は複数選択を受け、貼り付けは全ファイルを取り込む", () => {
    expect(input).toMatch(/type="file"[\s\S]*?multiple[\s\S]*?@change="onFilePicked"/);
    expect(input).toMatch(/const files = Array\.from\(input\.files \?\? \[\]\)/);
    // 1 件目で return する旧い形に戻っていないこと。
    expect(input).toMatch(/if \(item\.kind !== "file"\) continue;/);
  });

  it("件数は変換の前に切り、足すのは admit を通す 1 箇所だけ", () => {
    expect(input).toMatch(/const taken = files\.slice\(0, room\)/);
    expect(input).toMatch(/const refused = admit\(attachments\.value, item\)/);
    // 列へ足す代入は push の中の 1 行だけ（他の経路が admit を素通りしない）。
    expect(input.match(/attachments\.value = \[\.\.\.attachments\.value, /g)).toHaveLength(1);
  });

  it("上限を超えた列では送信できない", () => {
    expect(input).toMatch(/attachmentsOver\.value === null/);
  });

  it("再添付は外へ公開し、会話ペインが呼ぶ", () => {
    expect(input).toMatch(/defineExpose\(\{ fill, addAttachmentRef \}\)/);
    expect(panel).toMatch(/input\.value\?\.addAttachmentRef\(att, attachmentKindOf\(att\)\)/);
  });
});

describe("会話ペインの「入力欄へ」", () => {
  it("渡し直せる発話の、消えていない添付にだけ出す（Spec 76 S76-3 — サーヴァントの発話も）", () => {
    expect(panel).toMatch(
      /v-if="canReattachFrom\(entry\.row\.message\) && attachmentUrl\(att\)"[\s\S]*?data-reattach[\s\S]*?@click="reattach\(att\)"/,
    );
  });

  it("渡し直せる送り手はコアの find_reattachable と同じ集合", () => {
    // 画面だけ広げると「押せるのに断られる」、コアだけ広げると「渡せるのに押せない」になる。
    const runtime = read("../../crates/fuseforks-core/src/orchestrator/runtime.rs");
    const fn = /async fn find_reattachable[\s\S]*?\n {4}\}\n/.exec(runtime);
    expect(fn, "runtime.rs に find_reattachable が在ること").not.toBeNull();
    expect(fn![0]).toContain("matches!(message.from, Endpoint::User | Endpoint::Agent { .. })");
    expect(canReattachFrom({ from: { kind: "user" } })).toBe(true);
    expect(canReattachFrom({ from: { kind: "agent", id: "agent_1" } })).toBe(true);
    expect(canReattachFrom({ from: { kind: "system" } })).toBe(false);
    expect(canReattachFrom({ from: { kind: "external", client: "Neo" } })).toBe(false);
  });
});

describe("保存してある形式で描く（Spec 76 S76-1）", () => {
  it("会話ペインと入力欄のチップは MIME を形式から引く", () => {
    expect(panel).toContain("{ type: attachmentMime(att.format) }");
    expect(input).toContain("data:${attachmentMime(ref.format)};base64,");
    // 生成画像の写しは JPEG のことがある — WebP の決め打ちに戻さない。
    expect(panel).not.toContain('{ type: "image/webp" }');
  });

  it("形式ごとの MIME", () => {
    expect(attachmentMime("jpeg")).toBe("image/jpeg");
    expect(attachmentMime("webp")).toBe("image/webp");
    expect(attachmentMime(undefined)).toBe("image/webp");
    expect(attachmentMime("pdf")).toBe("application/pdf");
  });
});

describe("実行時に組む辞書キー", () => {
  const limits: AttachLimit[] = ["count", "bytes", "duplicate"];
  const over: Exclude<AttachLimit, "duplicate">[] = ["count", "bytes"];

  for (const [name, dict] of [["ja", ja], ["en", en]] as const) {
    it(`${name}: attachLimit / attachOver の全種別が在る`, () => {
      const chatInput = dict.chatInput as Record<string, unknown>;
      const limitTable = chatInput.attachLimit as Record<string, string>;
      const overTable = chatInput.attachOver as Record<string, string>;
      for (const key of limits) expect(limitTable[key], key).toBeTruthy();
      for (const key of over) expect(overTable[key], key).toBeTruthy();
      expect(Object.keys(limitTable).sort()).toEqual([...limits].sort());
      expect(Object.keys(overTable).sort()).toEqual([...over].sort());
    });
  }
});
