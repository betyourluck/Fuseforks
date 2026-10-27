/**
 * サーヴァント一覧の絞り込み（2026-10-27 利用者裁定）。
 *
 * **照合するのは表示名・id・役職名・モデル名の 4 つ。** どれかに部分一致すれば当たり。
 * 空白で区切った語は**全部**が当たる個体だけを残す（`gpt 調査` = gpt を使う調査役）。
 *
 * **NFKC + 小文字で比べる。** 大文字小文字に加えて全角半角も畳む（`ＧＰＴ` / `gpt` /
 * 半角カナ）。比べた結果を出すだけで位置は使わないので、畳んで長さが変わっても困らない。
 *
 * 役職名は `roleLabel` で引く（`role_contract` 凍結 5 — 表示の解決は 1 実装）。
 * 引けない役職は照合の対象に入らない（画面にバッジが出ないので、出ていない語で当てない）。
 */

import { roleLabel } from "./roleLabel";
import type { AgentId, Role, RoleId } from "../types";

/** 照合に要る欄だけ。`AgentSnapshot` をそのまま渡せる。 */
export interface SearchableAgent {
  id: AgentId;
  name: string;
  model: string;
  roleId: RoleId | null;
}

function fold(text: string): string {
  return text.normalize("NFKC").toLowerCase();
}

/** 問いを語に割る。空白だけの問いは語 0 個 = 絞り込んでいない。 */
export function searchTokens(query: string): string[] {
  return fold(query)
    .split(/\s+/)
    .filter((token) => token.length > 0);
}

/** その個体が全部の語に当たるか。語が 0 個なら常に真。 */
export function matchesAgent(agent: SearchableAgent, roles: Role[], tokens: string[]): boolean {
  if (tokens.length === 0) return true;
  const fields = [agent.name, agent.id, agent.model, roleLabel(agent.roleId, roles) ?? ""].map(
    fold,
  );
  return tokens.every((token) => fields.some((field) => field.includes(token)));
}

/** 当たった個体だけを、渡された順のまま返す。問いが空なら全員。 */
export function filterAgents<T extends SearchableAgent>(
  agents: readonly T[],
  roles: Role[],
  query: string,
): T[] {
  const tokens = searchTokens(query);
  if (tokens.length === 0) return [...agents];
  return agents.filter((agent) => matchesAgent(agent, roles, tokens));
}
