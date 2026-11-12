/**
 * 会話の単位で画面を絞る**配線**を機械で留める（Spec 75 P2）。
 *
 * 会話 ID は 4 つの場所を手で揃えて運ぶ — コアのイベント（`event.rs`）→ `types.ts` →
 * `useOrchestrator` の投影 → 部品（会話ペイン・承認画面・作業状況タブ・通知）。
 * 型検査は**会話 ID を読まない配線でも通る**（欄が届いても誰も見なければ、別の会話の
 * 入力中のバブルやツール行が開いている会話に出るだけで、何も落ちない）。
 * 規則そのもの（`isOpenSession`）は `sessionScope.test.ts` が留める。ここが見るのは
 * 画面がそれを読んでいることだけ。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import en from "../locales/en.json";
import ja from "../locales/ja.json";
import zh from "../locales/zh-Hans.json";

function read(rel: string): string {
  return readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf8");
}

const types = read("../types.ts");
const eventRs = read("../../../../crates/fuseforks-core/src/event.rs");
const orchestrator = read("../composables/useOrchestrator.ts");
const chat = read("../components/ChatPanel.vue");
const toast = read("../components/ToastHost.vue");
const approval = read("../components/CommandApprovalDialog.vue");
const waves = read("../components/PlanWavePane.vue");

/** Rust の variant の本体（`Name {` から閉じ括弧まで）。 */
function rustVariant(name: string): string {
  const found = new RegExp(`\\n {4}${name} \\{[\\s\\S]*?\\n {4}\\},`).exec(eventRs);
  expect(found, `event.rs に ${name} が無い`).not.toBeNull();
  return found![0];
}

describe("会話の単位で絞る配線（Spec 75 P2）", () => {
  it("コアが会話 ID を載せるイベントは、types.ts でも sessionId を持つ", () => {
    for (const [rust, front] of [
      ["AgentTyping", "agentTyping"],
      ["ToolInvoked", "toolInvoked"],
      ["ApprovalStopped", "approvalStopped"],
      ["SessionUpdated", "sessionUpdated"],
      ["PlanWaveStarted", "planWaveStarted"],
      ["PlanWaveProposed", "planWaveProposed"],
    ] as const) {
      expect(rustVariant(rust), `${rust} に session_id が無い`).toMatch(/\bsession_id: String,/);
      const line = new RegExp(`type: "${front}";[\\s\\S]*?\\}`).exec(types);
      expect(line, `types.ts に ${front} が無い`).not.toBeNull();
      expect(line![0], `types.ts の ${front} に sessionId が無い`).toMatch(/\bsessionId: string/);
    }
  });

  it("入力中のバブルは開いている会話のターンだけ、全ターン停止は村全体で押せる", () => {
    expect(chat).toContain("isOpenSession(state.typingSession[a.id], state.currentSessionId)");
    // バブルで絞った数で停止ボタンを殺すと、別の会話のターンを止める手段が消える。
    expect(chat).toContain(':disabled="!busyAgents.size"');
    expect(chat).not.toContain(':disabled="!typingAgents.length"');
  });

  it("投影はツール行を開いている会話で絞り、根の会話を立てて消す", () => {
    expect(orchestrator).toContain("if (isOpenSession(event.sessionId, state.currentSessionId)) {");
    expect(orchestrator).toContain("state.typingSession[event.agentId] = event.sessionId;");
    // 消す側は 2 箇所（agentTyping の終了 / 稼働でなくなったとき）。
    expect(orchestrator.match(/delete state\.typingSession\[event\.agentId\];/g) ?? []).toHaveLength(2);
  });

  it("開いている会話が変わったら知らせを閉じ、承認画面の投影を取り直す", () => {
    const at = orchestrator.indexOf('case "sessionSwitched":');
    const next = orchestrator.indexOf("case ", at + 1);
    const body = orchestrator.slice(at, next);
    expect(body).toContain("dismissSessionNotice(event.sessionId);");
    expect(body).toContain("refreshCommandRequests()");
  });

  it("会話を消したら、その会話を指す知らせを閉じる", () => {
    const at = orchestrator.indexOf("async deleteSession(sessionId: string)");
    expect(at, "useOrchestrator に deleteSession が無い").toBeGreaterThan(0);
    // 行コメントを落としてから見る — コメントにした呼び出しも同じ字面を持つので、落とさないと
    // 呼び出しを消した状態でも緑のまま残る（変異で実際に踏んだ）。
    const body = orchestrator
      .slice(at, orchestrator.indexOf("\n    },", at))
      .replace(/\/\/.*$/gm, "");
    // 成功の枝の内側にあること（断られた会話の知らせまで消すと、答えの在りかが分からなくなる）。
    expect(body).toMatch(/if \(succeeded\(done\)\) \{[\s\S]*?dismissSessionNotice\(sessionId\);/);
  });

  it("開いていない会話への書き込みは一覧を引き直し、サーヴァントの発話だけ知らせる", () => {
    const at = orchestrator.indexOf('case "sessionUpdated":');
    expect(at, "useOrchestrator に sessionUpdated の受け手が無い").toBeGreaterThan(0);
    const body = orchestrator.slice(at, orchestrator.indexOf("break;", at));
    expect(body).toContain("ipc.listSessions()");
    // 知らせの呼び出しが送り手の判定の内側にあること（字面が別々に在るだけでは足りない）。
    expect(body).toContain(
      'if (event.from.kind === "agent") noticeAnswerElsewhere(event.sessionId, event.from.id);',
    );
  });

  it("通知の「開く」はその会話を開く", () => {
    expect(toast).toContain('v-if="toast.openSessionId"');
    expect(toast).toContain("orchestrator.openNoticedSession(toast)");
  });

  it("承認画面は別の会話の印を 1 行で出し、そこには「続けさせる」を置かない", () => {
    const at = approval.indexOf('v-for="sessionId in view.stoppedElsewhere ?? []"');
    expect(at, "承認画面に別の会話の印の行が無い").toBeGreaterThan(0);
    const row = approval.slice(at, approval.indexOf("</div>", at));
    expect(row).toContain("commandApproval.stoppedElsewhere");
    expect(row).toContain("openElsewhere(sessionId)");
    // 続けさせた答えは止まった会話へ書かれる — その会話を見ている状態で押させる。
    expect(row).not.toContain("resume(");
  });

  it("作業状況タブは全部の波を出し、別の会話の波に印を付ける", () => {
    expect(waves).toContain("isOpenSession(wave.sessionId, state.currentSessionId)");
    expect(waves).toContain("data-wave-other-session");
    expect(waves).toContain("data-plan-other-session");
    // 波そのものを会話で絞らない（村の作業の一覧。確認待ちのバッジも全部を数える）。
    expect(waves).not.toMatch(/planWaves\.filter\([^)]*sessionId/);
  });

  it("辞書: 知らせの鍵は実行時に組むので 3 言語に在ることをここで見る。旧い拒否の鍵は消えた", () => {
    for (const [lang, dict] of [["ja", ja], ["en", en], ["zh-Hans", zh]] as const) {
      const d = dict as unknown as {
        orchestrator: Record<string, string>;
        errors: Record<string, string>;
      };
      expect(d.orchestrator.answerElsewhere, `${lang}: answerElsewhere`).toBeTruthy();
      expect(d.orchestrator.answerElsewhereMany, `${lang}: answerElsewhereMany`).toBeTruthy();
      expect(d.errors.SESSION_BUSY, `${lang}: SESSION_BUSY`).toBeTruthy();
      expect(d.errors.SESSION_SWITCH_BLOCKED, `${lang}: 撤去した鍵が残っている`).toBeUndefined();
    }
  });
});
