/**
 * どのワイヤがどの種別の添付を運べるか — **画面の警告用の写し**（Spec 36 D12）。
 *
 * **判定の本体はコア側**（`Provider::carries`）で、送信入口が断る。ここは
 * 「貼った時点で分かる」ようにするためだけの表で、**送信を止めない**。
 *
 * 判定を 2 箇所に持つと、表の同期が切れた瞬間に「画面は通したのにコアが拒否」
 * （またはその逆）が起き、どちらが正か画面から読めなくなる。警告なら食い違いの
 * 被害は「警告が出ない / 余計に出る」で済む。
 *
 * **同期は機械で留める** — `carriesTable.test.ts` が Rust の凍結表
 * （`tests/carries_table.rs` の観測値）を読んで、この表と突き合わせる
 * （`defaultEnabledTools.test.ts` / `toolLabel.test.ts` と同じ網）。
 */
import type { Provider } from "../types";

/** 添付の種別（コアの `AttachmentKind` と同じ閉じた列挙）。 */
export type AttachmentKind = "image" | "audio" | "video" | "pdf";

/**
 * 種別ごとの上限（bytes）。コアの `ATTACHMENT_{KIND}_MAX_BYTES` と同じ値。
 *
 * **画像だけ意味が違う** — あちらは「変換後」の上限で、元ファイルの門は
 * `MAX_SOURCE_BYTES` が別に持つ（Spec 23 D5）。音声・動画・PDF は無変換なので
 * 元ファイルにそのまま掛かる。
 */
export const KIND_MAX_BYTES: Record<AttachmentKind, number> = {
  image: 2 * 1024 * 1024,
  audio: 10 * 1024 * 1024,
  video: 12 * 1024 * 1024,
  pdf: 10 * 1024 * 1024,
};

/**
 * carries の表（P0 の probe 18 発で観測。2026-08-12）。
 *
 * 並びは `[image, audio, video, pdf]`。**Rust の凍結表と同じ順序**で書く —
 * 同期テストがこの順序で突き合わせる。
 */
const TABLE: Record<Provider, readonly [boolean, boolean, boolean, boolean]> = {
  open_ai_compat: [true, true, false, true],
  anthropic: [true, false, false, true],
  gemini: [true, true, true, true],
  // xAI の PDF は公式文書に記述が無いのに通った（文書と実装の乖離 2 例目）。
  xai_responses: [true, false, false, true],
  open_ai_responses: [true, false, false, true],
};

/** 表の列の順序。`TABLE` の並びと 1 対 1。 */
export const KIND_ORDER: readonly AttachmentKind[] = [
  "image",
  "audio",
  "video",
  "pdf",
];

/** そのワイヤがその種別を運べるか。 */
export function carries(provider: Provider, kind: AttachmentKind): boolean {
  return TABLE[provider][KIND_ORDER.indexOf(kind)];
}

/** その種別を運べるワイヤの一覧（案内文の材料）。 */
export function carriersOf(kind: AttachmentKind): Provider[] {
  return (Object.keys(TABLE) as Provider[]).filter((provider) =>
    carries(provider, kind),
  );
}

/**
 * テンプレートの実効ワイヤ（コアの `effective_provider` と同じ規則）。
 *
 * **未設定なら base URL から推定する** — 判定できないホストは互換へ倒す。
 * ここを別の規則にすると、画面の警告とコアの門が違うワイヤを見ることになる。
 */
export function effectiveProvider(
  provider: Provider | null | undefined,
  baseUrl: string,
): Provider {
  if (provider) return provider;
  return baseUrl.includes("api.anthropic.com") ? "anthropic" : "open_ai_compat";
}
