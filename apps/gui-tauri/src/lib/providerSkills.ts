/**
 * モデルテンプレートの「プロトコル」まわりの純関数（Spec 31 P2）。
 *
 * ダイアログの中に書いていた判定をここへ出した。理由は 2 つ:
 * - **固有スキルの出し分けが provider ごとに増える。** 表示条件と
 *   「効かない設定が残っている」の判定が対で要るので、対のまま検査したい
 * - `baseUrlMismatch` に**誤検知があった**（下記）。SFC の computed のままでは
 *   赤で示せない
 */

import type { ModelTemplate } from "../types";

/** プロバイダごとの既定 base URL。プロトコル切り替え時にこの値へ揃える。 */
export const DEFAULT_BASE_URL: Record<string, string> = {
  open_ai_compat: "https://api.openai.com/v1",
  anthropic: "https://api.anthropic.com/v1",
  gemini: "https://generativelanguage.googleapis.com/v1beta",
  xai_responses: "https://api.x.ai/v1",
};

/** 既知の既定値のいずれかであれば、プロトコル変更に追随してよいと判断する。 */
export const KNOWN_DEFAULTS = Object.values(DEFAULT_BASE_URL);

/**
 * **OpenAI 互換の口も同時に持つ**ホストの既定 URL。
 *
 * `provider: open_ai_compat` のまま使うのは正当な運用なので、
 * [`baseUrlMismatch`] は食い違いとして指摘しない。
 *
 * これを持たない実装は誤検知する。Gemini は互換の口も持っており
 * （`Provider::detect` の doc が「generativelanguage.googleapis.com は OpenAI
 * 互換としても動いており」と書いている）、xAI も同じ — この村の Grok 個体は
 * **いま実際に api.x.ai を互換経路で使っている**。
 */
const ALSO_SERVES_COMPAT: readonly string[] = [
  DEFAULT_BASE_URL.gemini,
  DEFAULT_BASE_URL.xai_responses,
];

/**
 * プロトコルと base URL が明らかに食い違っているか。合っていれば `null`。
 *
 * 判定するのは「**他社の既定値がそのまま残っている**」という一点だけ。
 * 自前のプロキシやローカルサーバの URL は正当なので警告してはいけない。
 *
 * ただし**互換の口も持つホストは他社ではない** — 互換を選んだまま
 * api.x.ai や generativelanguage を指すのは、この村で現に動いている構成。
 */
export function baseUrlMismatch(
  provider: ModelTemplate["provider"],
  baseUrl: string,
): string | null {
  if (!provider) return null;

  const expected = DEFAULT_BASE_URL[provider];
  if (!expected || baseUrl === expected) return null;
  if (provider === "open_ai_compat" && ALSO_SERVES_COMPAT.includes(baseUrl)) {
    return null;
  }
  return KNOWN_DEFAULTS.includes(baseUrl) ? expected : null;
}

/**
 * プロトコルを変えたときの base URL の差し替え先。変えないなら `null`。
 *
 * 実際に `provider: anthropic` と `baseUrl: api.openai.com` の組み合わせが
 * 保存され、Anthropic のモデル名で OpenAI へ送る設定ができてしまっていた。
 * ユーザーが手で入れた URL（プロキシ等）は正当なので、既定値のときだけ触る。
 */
export function presetBaseUrlFor(
  provider: string | null,
  currentBaseUrl: string,
): string | null {
  if (!provider) return null;
  const preset = DEFAULT_BASE_URL[provider];
  if (!preset || !KNOWN_DEFAULTS.includes(currentBaseUrl)) return null;
  return preset === currentBaseUrl ? null : preset;
}

/** 固有スキルの 1 つぶんの表示状態。 */
export interface SkillVisibility {
  /** チェックボックスを出すか（そのワイヤを明示選択している）。 */
  offered: boolean;
  /**
   * 効かない設定が真のまま残っているか。
   *
   * **隠すだけだと真のまま見えなくなる**ので、その時だけ理由と直し方を出す。
   * UI からは作れないが `world.json` の直接編集で作れる。
   */
  stranded: boolean;
}

function visibility(enabled: boolean, onItsWire: boolean): SkillVisibility {
  return { offered: onItsWire, stranded: enabled && !onItsWire };
}

/**
 * 固有スキルの出し分け（Spec 31 D3 / D4）。
 *
 * **判定は provider であってモデル名ではない。** モデル名で分けると
 * `Provider::detect` と判定が 2 系統になる（D2）。コア側の
 * `grounding_active` / `xai_*_search_active` と同じ AND の形で、
 * こちらは「見せるか」、あちらは「効かせるか」を担う。
 */
export function providerSkills(draft: Pick<
  ModelTemplate,
  "provider" | "googleSearch" | "xaiWebSearch" | "xaiXSearch"
>): {
  google: SkillVisibility;
  xaiWeb: SkillVisibility;
  xaiX: SkillVisibility;
  /** どれか 1 つでも出るか（カテゴリ見出しを描くかの判定）。 */
  anyOffered: boolean;
} {
  const onGemini = draft.provider === "gemini";
  const onXai = draft.provider === "xai_responses";

  const google = visibility(draft.googleSearch, onGemini);
  const xaiWeb = visibility(draft.xaiWebSearch, onXai);
  const xaiX = visibility(draft.xaiXSearch, onXai);

  return {
    google,
    xaiWeb,
    xaiX,
    anyOffered: google.offered || xaiWeb.offered || xaiX.offered,
  };
}
