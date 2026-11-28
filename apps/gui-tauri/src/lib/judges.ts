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
import type { AgentGroup, AgentId, GroupId, JudgeSpec, JudgeStatus, JudgeView } from "../types";
import { isHidden, sectionKeyOf, UNASSIGNED } from "./agentGroups";

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
    case "disabled":
      return { key: "judges.status.disabled", params: {}, active: false };
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

// ---- 非表示に連動する（2026-11-29 利用者要望） ------------------------------------------

/**
 * 連動先のグループ。**村に居るグループのときだけ id を返し、引けない id は `null`**（連動しない）—
 * グループを消しても判断役の `hideWithGroup` には触らない（`group_contract` 凍結 3 と同じ）ので、
 * 引けない id は「なし」として描き、次に判断役を保存する操作で `null` へ正規化する。
 * 判定はサーヴァントの所属と同じ `sectionKeyOf` を通す（規則を 2 つ書かない）。
 */
export function linkedGroupOf(
  judge: Pick<JudgeView, "hideWithGroup">,
  groups: readonly AgentGroup[],
): GroupId | null {
  const key = sectionKeyOf({ groupId: judge.hideWithGroup }, groups);
  return key === UNASSIGNED ? null : key;
}

/**
 * 地図で隠すか。連動先が村に居て、かつ非表示の集合に入っているときだけ。**表示だけ** —
 * コアの評価・配送はこの欄を読まない。述語はサーヴァントの `isHidden` をそのまま使う
 * （連動先を所属に見立てる。引けない id は隠れない、まで同じ規則）。
 */
export function isJudgeHidden(
  judge: Pick<JudgeView, "hideWithGroup">,
  groups: readonly AgentGroup[],
  hidden: readonly GroupId[],
): boolean {
  return isHidden({ groupId: judge.hideWithGroup }, groups, hidden);
}

/** 地図に出す判断役（並びは受け取った順のまま）。 */
export function shownJudges<T extends Pick<JudgeView, "hideWithGroup">>(
  judges: readonly T[],
  groups: readonly AgentGroup[],
  hidden: readonly GroupId[],
): T[] {
  return judges.filter((judge) => !isJudgeHidden(judge, groups, hidden));
}

/**
 * 一覧の姿から更新の形を組む（`update_judge` は欄ごと差し替える — 写し忘れた欄は既定へ戻る）。
 * 連動先は `linkedGroupOf` で正規化して送る（引けない id はここで `null` になる）。
 * `patch` は変える欄だけ。
 */
export function judgeSpecOf(
  view: JudgeView,
  groups: readonly AgentGroup[],
  patch: Partial<JudgeSpec> = {},
): JudgeSpec {
  return {
    id: view.id,
    name: view.name,
    order: view.order,
    enabled: view.enabled,
    hideWithGroup: linkedGroupOf(view, groups),
    ...patch,
  };
}
