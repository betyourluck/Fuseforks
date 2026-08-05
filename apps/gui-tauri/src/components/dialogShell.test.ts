/**
 * タイトルバーから開くダイアログの**外枠が揃っているか**を機械で確かめる。
 *
 * # なぜ要るか（2026-08-04 実機の指摘）
 *
 * 役職ダイアログだけ、条例 / 共通 MCP / スケジュールと違う形になっていた —
 * 説明を `bg-surface-0` の帯ではなく本文の中に置き、しかもタイトルと同じ語の
 * 見出しをもう 1 つ持っていた。**型検査にもテストにも 1 つも掛からない**ので、
 * 人が画面を並べて見るまで誰にも分からなかった（`failures.md` #52 の一般化 3 と
 * 同型 — 見た目の誤りはテストで落ちない）。
 *
 * ここで留めるのは**外枠だけ**。中身の作りはダイアログごとに違ってよく、
 * 揃っているべきなのは「開いたときに同じ場所に同じものがある」という一点。
 *
 * # 留めないこと
 *
 * 寸法は揃えない（条例と役職は 560×680、MCP とスケジュールは 640×760）。
 * **中身の量で決まるべきもの**まで縛ると、次に作る人が窮屈な枠へ詰め込む。
 */
import { describe, expect, it } from "vitest";
// @ts-expect-error @types/node を入れない方針のため（vite.config.ts と同じ扱い）
import { readFileSync } from "node:fs";
// @ts-expect-error 同上
import { fileURLToPath } from "node:url";

/** タイトルバーから開くダイアログ。**足したらここに書く。** */
const TITLE_BAR_DIALOGS = [
  "OrdinanceDialog",
  "RoleDialog",
  "McpDialog",
  "ScheduleDialog",
] as const;

function source(name: string): string {
  return readFileSync(fileURLToPath(new URL(`./${name}.vue`, import.meta.url)), "utf8");
}

describe("タイトルバーのダイアログの外枠", () => {
  it.each(TITLE_BAR_DIALOGS)("%s: 画面全体を覆う同じ背景を持つ", (name) => {
    // 覆いは `bg-scrim`（2026-08-05。旧 `bg-black/60` は唯一残っていた
    // 生のパレットで、ライトでは黒 60% が重すぎるためトークンへ移した）。
    expect(source(name)).toContain("fixed inset-0 z-40 flex items-center justify-center bg-scrim");
  });

  it.each(TITLE_BAR_DIALOGS)("%s: 見出しの帯が同じ形", (name) => {
    expect(source(name)).toContain(
      'class="flex shrink-0 items-center gap-2 border-b border-line px-3 py-2.5 text-xs"',
    );
  });

  it.each(TITLE_BAR_DIALOGS)("%s: 見出しの直下に説明の帯がある", (name) => {
    // `bg-surface-0` の 1 段。ここに説明を置くのが揃った形で、本文の中へ
    // 書くと役職ダイアログのように 1 つだけ浮く。
    expect(source(name)).toContain(
      "shrink-0 border-b border-line bg-surface-0 px-3 py-2 text-[11px] text-ink-dim",
    );
  });

  it.each(TITLE_BAR_DIALOGS)("%s: 本文は min-h-0 flex-1 で内側にスクロールする", (name) => {
    // これが無いと、中身が伸びたときダイアログごと画面外へはみ出す
    // （App.vue の `minmax(0, 1fr)` と同じ理由）。
    expect(source(name)).toMatch(/class="min-h-0 flex-1[^"]*"/);
  });
});
