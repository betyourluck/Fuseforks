/**
 * AI 下書き補助のパネル（Spec 63）の**純関数**。部品（`AssistPanel.vue`）は描くだけ。
 *
 * - 生成役のテンプレートの選び方（`useTools` が偽は選べない / 前回の選択を端末に覚える）
 * - 反映の前に見せる差（字数と、増えた行・消えた行の数 — D9）
 */
import type { ModelTemplate, ModelTemplateId } from "../types";

/** 前回の選択を覚える棚（`localStorage`）。村には入れない — 端末ごとの好み。 */
export const ASSIST_STORAGE_KEY = "fuseforks.assist.v1";

/** 選択肢 1 つ。`disabled` は `useTools` が偽のもの（下書きをツール呼び出しで受け取るため — D2 / D5）。 */
export interface TemplateChoice {
  id: ModelTemplateId;
  name: string;
  model: string;
  disabled: boolean;
}

/** 並べる選択肢。**偽のものも並べる**（消すと「なぜ無いか」が分からない）が、選べない。 */
export function templateChoices(templates: readonly ModelTemplate[]): TemplateChoice[] {
  return templates.map((t) => ({ id: t.id, name: t.name, model: t.model, disabled: !t.useTools }));
}

/**
 * 最初に選ばれているテンプレート。覚えた id が今も在り `useTools` が真ならそれ、
 * 無ければ選べる最初のもの、1 つも無ければ `null`（パネルは送れない）。
 */
export function pickTemplate(
  templates: readonly ModelTemplate[],
  remembered: ModelTemplateId | null,
): ModelTemplateId | null {
  const usable = templates.filter((t) => t.useTools);
  if (remembered && usable.some((t) => t.id === remembered)) return remembered;
  return usable[0]?.id ?? null;
}

/** 覚えた選択を読む。壊れた値・読めない棚は `null`（パネルは既定へ落ちる）。 */
export function readRemembered(storage: Pick<Storage, "getItem"> | null): ModelTemplateId | null {
  try {
    const raw = storage?.getItem(ASSIST_STORAGE_KEY);
    if (!raw) return null;
    const parsed: unknown = JSON.parse(raw);
    if (parsed && typeof parsed === "object" && typeof (parsed as { templateId?: unknown }).templateId === "string") {
      return (parsed as { templateId: string }).templateId;
    }
    return null;
  } catch {
    return null;
  }
}

/** 選択を覚える。書けなくても黙って続ける（覚えるのは便宜で、失敗しても補助は使える）。 */
export function writeRemembered(storage: Pick<Storage, "setItem"> | null, templateId: ModelTemplateId): void {
  try {
    storage?.setItem(ASSIST_STORAGE_KEY, JSON.stringify({ templateId }));
  } catch {
    // 端末の棚が使えない（プライベートモード等）。覚えないだけ。
  }
}

/** コードポイント数（コアの `chars().count()` と同じ数え方。`length` は UTF-16 の単位）。 */
export function codePoints(text: string): number {
  return [...text].length;
}

/** 反映の前に見せる差（D9）。 */
export interface DraftDelta {
  /** 編集中の本文の字数。 */
  beforeChars: number;
  /** 下書きの字数。 */
  afterChars: number;
  /** 下書きにあって編集中の本文に無い行の数。 */
  added: number;
  /** 編集中の本文にあって下書きに無い行の数。 */
  removed: number;
}

/**
 * 行の差を**多重集合として**数える（並べ替えだけの行は差にしない）。全文の差分表示は未決 1 —
 * ここで欲しいのは「思ったより消えていないか」を 1 行で読めることだけ。
 */
export function draftDelta(before: string, after: string): DraftDelta {
  const count = (text: string) => {
    const map = new Map<string, number>();
    if (text === "") return map;
    for (const line of text.split(/\r?\n/)) map.set(line, (map.get(line) ?? 0) + 1);
    return map;
  };
  const a = count(before);
  const b = count(after);
  let added = 0;
  let removed = 0;
  for (const [line, n] of b) added += Math.max(0, n - (a.get(line) ?? 0));
  for (const [line, n] of a) removed += Math.max(0, n - (b.get(line) ?? 0));
  return { beforeChars: codePoints(before), afterChars: codePoints(after), added, removed };
}

/** 反映の前に確認を出すか（D9 — 編集中の本文が空でなければ置き換えになる）。 */
export function applyNeedsConfirm(current: string): boolean {
  return current.trim() !== "";
}
