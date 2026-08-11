/**
 * 入退室の通知の判定（2026-08-08）。
 *
 * **文字列で見ている機構なので、コア側の文言との一致を機械で留める。**
 * `AgentMessage` に種別の欄が無く文末で判定するしかないため、
 * `orchestrator.rs` の `set_status` が組む文言を変えるとここが黙って
 * 効かなくなる — 型検査にも lint にも掛からない（`failures.md` #51 と同じ性質）。
 * `defaultEnabledTools.test.ts` / `toolLabel.test.ts` と同じ形で Rust を読む。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { dirname, resolve } from "node:path";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

import {
  PRESENCE_FAILURE_SUFFIX,
  PRESENCE_SUFFIXES,
  isPresenceNotice,
} from "./presenceNotice";
import type { AgentMessage, Endpoint } from "../types";

const here = dirname(fileURLToPath(import.meta.url));

function message(from: Endpoint, to: Endpoint, content: string): AgentMessage {
  return { id: "m1", from, to, content, tokens: 0, tsMs: 1, hop: 0 };
}

const system: Endpoint = { kind: "system" };
const user: Endpoint = { kind: "user" };
const agent: Endpoint = { kind: "agent", id: "agent_2" };

describe("isPresenceNotice", () => {
  it("定常の入退室を真にする", () => {
    for (const suffix of PRESENCE_SUFFIXES) {
      expect(
        isPresenceNotice(message(system, user, `agent_2（ロボットくん1号）${suffix}`)),
      ).toBe(true);
    }
  });

  it("失敗による停止は隠さない", () => {
    // 負の対照。カードは「いまの状態」しか示さないので、過去に落ちた事実の
    // 置き場が会話ログしか無い。**文末の 1 字（が / り）だけで分かれている**
    // ので、この 1 本が接尾辞の精度そのものを見ている。
    expect(
      isPresenceNotice(
        message(system, user, `agent_2（ロボットくん1号）${PRESENCE_FAILURE_SUFFIX}`),
      ),
    ).toBe(false);
  });

  it("入退室でない System 通知は隠さない", () => {
    expect(
      isPresenceNotice(
        message(system, user, "agent_2（ロボットくん1号）への予定「1 分ごと」を飛ばしました（停止中）"),
      ),
    ).toBe(false);
  });

  it("送り手が System でなければ偽", () => {
    // 発話の本文に同じ文末を書かれても隠さない（本文は攻撃者が書ける）。
    expect(
      isPresenceNotice(message(agent, user, "agent_2（ロボットくん1号）が停止しました")),
    ).toBe(false);
  });

  it("宛先が User でなければ偽", () => {
    // 予定の発火は System → Agent。配送される発話は入退室ではない。
    expect(
      isPresenceNotice(message(system, agent, "agent_2（ロボットくん1号）が停止しました")),
    ).toBe(false);
  });
});

describe("コア側の文言との一致", () => {
  const rustSource = readFileSync(
    resolve(here, "../../../../crates/fuseforks-core/src/orchestrator/mod.rs"),
    "utf-8",
  );

  it("set_status が組む 3 つの文言が実ソースにある", () => {
    // **3 つ揃えて見る。** 隠す 2 つだけを見ると、失敗の文言が変わって
    // 「が停止しました」へ寄ったときに、隠してはいけないものが隠れる。
    for (const suffix of [...PRESENCE_SUFFIXES, PRESENCE_FAILURE_SUFFIX]) {
      expect(rustSource).toContain(`）${suffix}")`);
    }
  });

  it("失敗の文言は隠す接尾辞のどれにも一致しない", () => {
    // 実ソースから組み立てた実物で確かめる（定数どうしの比較では、
    // 両方を同時に書き換えたときに気づけない）。
    const failure = `agent_9（ミュゼ）${PRESENCE_FAILURE_SUFFIX}`;
    expect(PRESENCE_SUFFIXES.some((suffix) => failure.endsWith(suffix))).toBe(false);
  });
});
