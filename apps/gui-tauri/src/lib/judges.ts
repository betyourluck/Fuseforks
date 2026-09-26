/**
 * 判断役（Spec 62）の画面側の規則。純関数だけを置く。
 *
 * - **id は 2 つの一覧をまたいで導く**（`deriveId`）。サーヴァントと判断役は同じ名前空間
 *   （`judge_contract`）で、片方の一覧だけ見ると、コアが `DUPLICATE_AGENT` で拒む名前を
 *   画面が作る
 * - **有効の述語はコアが 1 つだけ持つ**（`JudgeStatus`）。画面は `kind` を読むだけで、
 *   有効かどうかを自分で判定し直さない — 別々に判定すると「ツールは生えないのに線は
 *   描かれる」が生まれる
 */
import type { AgentId, JudgeStatus, JudgeView } from "../types";

/**
 * ID を名前から機械的に導く。`taken` と衝突したら `_2`, `_3` … を足す。
 *
 * **`taken` にはサーヴァントと判断役の両方の id を渡す**（名前空間は 1 つ）。
 * 英数字が 1 字も残らない名前（日本語だけ等）は `fallback` を土台にする。
 */
export function deriveId(name: string, taken: Iterable<AgentId>, fallback = "agent"): AgentId {
  const used = new Set(taken);
  const base =
    name
      .trim()
      .toLowerCase()
      .replace(/[^a-z0-9_-]+/g, "_")
      .replace(/^_+|_+$/g, "") || fallback;

  if (!used.has(base)) return base;
  let n = 2;
  while (used.has(`${base}_${n}`)) n += 1;
  return `${base}_${n}`;
}

/** 判断役の並び（`order` の昇順。同じなら id）。 */
export function judgesInOrder(judges: readonly JudgeView[]): JudgeView[] {
  return [...judges].sort((a, b) => a.order - b.order || a.id.localeCompare(b.id));
}

/** 新しく作る判断役の `order`（末尾）。 */
export function nextJudgeOrder(judges: readonly JudgeView[]): number {
  return judges.reduce((max, j) => Math.max(max, j.order + 1), 0);
}

/** 状態の表示（辞書の鍵と差し込む値）。**判定はしない** — `kind` を写すだけ。 */
export interface JudgeStatusText {
  key: string;
  params: Record<string, string>;
  /** 使える状態か（色分けだけに使う）。 */
  active: boolean;
}

export function judgeStatusText(status: JudgeStatus): JudgeStatusText {
  switch (status.kind) {
    case "active":
      return { key: "judges.status.active", params: {}, active: true };
    case "noFile":
      return { key: "judges.status.noFile", params: {}, active: false };
    case "invalid":
      return {
        key: "judges.status.invalid",
        params: { location: status.location, message: status.message },
        active: false,
      };
    case "missingTargets":
      return {
        key: "judges.status.missingTargets",
        params: { targets: status.targets.join(", ") },
        active: false,
      };
    case "noJudgeModel":
      return { key: "judges.status.noJudgeModel", params: {}, active: false };
  }
}

/** 地図の破線（判断役 → サーヴァント）。 */
export interface JudgeEdge {
  from: AgentId;
  to: AgentId;
}

/**
 * 判断役 → サーヴァントの線（Spec 62 D9）。**正本は `judge.toml` の `to`** で、
 * 保存しない — 描画のたびにここで合成する。**有効な判断役からしか描かない**
 * （ツールが生えない判断役から線が出ていると、画面の行き先と実際の配送先がずれる）。
 */
export function judgeEdges(judges: readonly JudgeView[]): JudgeEdge[] {
  return judges
    .filter((j) => j.status.kind === "active")
    .flatMap((j) => [...new Set(j.targets)].map((to) => ({ from: j.id, to })));
}
