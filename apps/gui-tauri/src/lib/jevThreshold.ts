/**
 * ツール結果の即時圧縮の閾値（Spec 59 D6 / `tool_prune_contract`）。
 *
 * **画面はラベル、ログは数値。** 利用者が選ぶのは「どれくらい落とすか」で、
 * 0.2 と 0.22 の違いに意味があるという読みを作らないために離散 4 値にしてある
 * （`chatZoom` と同じ形で、集合に無い値は既定へ落とす）。
 *
 * **値の表は Rust 側（`jev_settings.rs` の `THRESHOLDS` / `DEFAULT_THRESHOLD`）が
 * 正で、ここはその写し。** 走査テスト（`jevSettingsWiring.test.ts`）が 2 つを
 * 突き合わせる — ずれると「画面で選んだ強さと実際に落ちる量が違う」になり、
 * 型検査にも lint にも掛からない。
 */
export const JEV_THRESHOLDS = [0.1, 0.2, 0.3, 0.5] as const;

/** 閾値の型。 */
export type JevThreshold = (typeof JEV_THRESHOLDS)[number];

/**
 * 既定。**P0 の要否判定で確定した値**（帯 0.15〜0.28 の 46 段落を依頼文と
 * 突き合わせ、誤りの合計が最小なのは 0.22 だったが、**誤って落とす側のコストが
 * 高い**ので 0.2 を採った）。
 */
export const JEV_DEFAULT_THRESHOLD = 0.2;

/**
 * ラベルの辞書鍵。**訳語ではなく鍵を返す** — 純関数が i18n を知らない規律
 * （`blackboardLanes` / `batchLabel` と同じ形）。
 *
 * 控えめ = 0.1 / 標準 = 0.2 / 強め = 0.3 / 最大 = 0.5。
 */
export function jevThresholdKey(threshold: number): string {
  return `settings.jev.threshold.${String(threshold).replace(".", "_")}`;
}

/** 集合に無い値は既定へ落とす（手編集・旧版・小数の誤差を落とす）。 */
export function normalizeJevThreshold(value: unknown): number {
  if (typeof value !== "number") return JEV_DEFAULT_THRESHOLD;
  const hit = JEV_THRESHOLDS.find((t) => Math.abs(t - value) < 1e-6);
  return hit ?? JEV_DEFAULT_THRESHOLD;
}
