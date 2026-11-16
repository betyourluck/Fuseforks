/**
 * 画像生成の設定ページの判断（Spec 76 P2）。**ページの部品は配線と DOM だけ**で、
 * 何を保存するか・いつ押せるか・どの文を出すかはここに置く。
 *
 * 「いま効いているか」は**ここでも決めない** — Rust の `status` の 1 実装が返した
 * `active` / `inactiveReason` を読むだけ（画面で組み直すと「画面では有効なのに提示されない」が作れる）。
 */
import type {
  ImageAspect,
  ImageGenProviderSlot,
  ImageGenSettingsConfig,
  ImageGenSettingsView,
  ImageProvider,
  ImageResolution,
} from "../types";

/** 縦横比の選択肢（Rust の `ImageAspect` と同じ 3 値）。 */
export const IMAGE_ASPECTS: readonly ImageAspect[] = ["square", "landscape", "portrait"];

/** 解像度の選択肢（Rust の `ImageResolution` と同じ 3 段）。 */
export const IMAGE_RESOLUTIONS: readonly ImageResolution[] = ["standard", "high", "highest"];

/** 接続確認の結末の語（閉じた集合 = `ok` + 失敗の 8 区分 + 送らずに断った 2 つ）。 */
export const PROBE_OUTCOMES = [
  "ok",
  "auth",
  "rate_limited",
  "blocked",
  "timeout",
  "network",
  "shape",
  "api",
  "config",
  "no_provider",
  "no_key",
] as const;

/** 接続先の表示名。**訳さない**（社名）。 */
export const PROVIDER_NAMES: Record<ImageProvider, string> = {
  openai: "OpenAI",
  gemini: "Gemini",
  meta: "Meta",
  xai: "xAI",
};

/** タイムアウトの上限（秒）。これより大きい値・整数でない値は「既定」へ落とす。 */
export const MAX_TIMEOUT_SECS = 3600;

/** ページが編集している枠の入力（文字列のまま持つ）。 */
export interface SlotInputs {
  model: string;
  baseUrl: string;
  timeout: string;
}

/** 保存してある枠を入力の形へ。 */
export function inputsFromSlot(slot: ImageGenProviderSlot | undefined): SlotInputs {
  return {
    model: slot?.model ?? "",
    baseUrl: slot?.baseUrl ?? "",
    timeout: slot?.timeoutSecs == null ? "" : String(slot.timeoutSecs),
  };
}

/** タイムアウトの入力を値へ。空・整数でない・範囲外は `null`（= 既定）。 */
export function parseTimeoutInput(raw: string): number | null {
  const text = raw.trim();
  if (!/^\d+$/.test(text)) return null;
  const value = Number(text);
  return value >= 1 && value <= MAX_TIMEOUT_SECS ? value : null;
}

/** 入力を枠へ（前後の空白を落とす）。 */
export function slotFromInputs(inputs: SlotInputs): ImageGenProviderSlot {
  return {
    model: inputs.model.trim(),
    baseUrl: inputs.baseUrl.trim(),
    timeoutSecs: parseTimeoutInput(inputs.timeout),
  };
}

/** いま表示している接続先の行。 */
export function shownProvider(view: ImageGenSettingsView | null) {
  if (!view?.provider) return null;
  return view.providers.find((p) => p.id === view.provider) ?? null;
}

/** 表示している枠の入力が、保存してある値から変わっているか（接続確認を止める判定にも使う）。 */
export function slotDirty(view: ImageGenSettingsView | null, inputs: SlotInputs): boolean {
  const row = shownProvider(view);
  if (!row) return false;
  const saved = slotFromInputs(inputsFromSlot(row.slot));
  const draft = slotFromInputs(inputs);
  return (
    saved.model !== draft.model ||
    saved.baseUrl !== draft.baseUrl ||
    saved.timeoutSecs !== draft.timeoutSecs
  );
}

/**
 * 保存する設定を組む。**4 社の枠を全部送る**（送らなかった枠は Rust 側で消える）。
 * 表示している接続先の枠だけ入力で置き換える — Jev の閾値を変えたときにアカウント ID の
 * 入力ごと保存するのと同じ形で、保存は「いま画面に出ている値」を書く。
 */
export function buildConfig(
  view: ImageGenSettingsView,
  inputs: SlotInputs,
  next: Partial<Pick<ImageGenSettingsConfig, "provider" | "aspect" | "resolution">> = {},
): ImageGenSettingsConfig {
  const perProvider: Partial<Record<ImageProvider, ImageGenProviderSlot>> = {};
  for (const row of view.providers) perProvider[row.id] = { ...row.slot };
  if (view.provider) perProvider[view.provider] = slotFromInputs(inputs);
  return {
    provider: next.provider !== undefined ? next.provider : view.provider,
    perProvider,
    aspect: next.aspect ?? view.aspect,
    resolution: next.resolution ?? view.resolution,
  };
}

/** 接続確認の結末から辞書の鍵を組む。知らない語は `unknown`（鍵名を画面に出さない）。 */
export function probeMessageKey(outcome: string): string {
  const known = (PROBE_OUTCOMES as readonly string[]).includes(outcome);
  return `settings.imageGen.probe.${known ? outcome : "unknown"}`;
}

/** 「接続を確かめる」を押せない理由の辞書鍵。押せるなら `null`。 */
export function probeBlockKey(view: ImageGenSettingsView | null, inputs: SlotInputs): string | null {
  const row = shownProvider(view);
  if (!row) return "settings.imageGen.probeNeedsProvider";
  if (!row.hasKey) return "settings.imageGen.probeNeedsKey";
  if (slotDirty(view, inputs)) return "settings.imageGen.probeNeedsSave";
  return null;
}
