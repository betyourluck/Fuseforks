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
 * **[`baseUrlMismatch`] は指摘せず、[`presetBaseUrlFor`] は書き換えない。**
 * 用途が 2 つあるのが要点 — 一方は「他社の設定が残っている」の判定（否定）、
 * もう一方は「上書きしてよい既定値か」の判定（肯定）。同じ免除が両方に要る。
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
 *
 * **互換の口も持つホストは、互換へ切り替えても書き換えない。**
 * `api.x.ai` を選んだまま互換へ戻すのは正当な構成（この村の Grok 個体が
 * 現にそう動いていた）で、書き換えると**xAI の鍵を持って OpenAI へ送る**
 * 設定が黙って出来上がる。実機で 401
 * `Incorrect API key provided: xai-…` を踏んだ（2026-08-10）。
 */
export function presetBaseUrlFor(
  provider: string | null,
  currentBaseUrl: string,
): string | null {
  if (!provider) return null;
  const preset = DEFAULT_BASE_URL[provider];
  if (!preset || !KNOWN_DEFAULTS.includes(currentBaseUrl)) return null;
  if (provider === "open_ai_compat" && ALSO_SERVES_COMPAT.includes(currentBaseUrl)) {
    return null;
  }
  return preset === currentBaseUrl ? null : preset;
}

/**
 * そのワイヤで**常に効く**（設定を持たない）固有スキル。返すのは**辞書キー
 * そのもの**で、ラベルが `modelTemplate.{key}`、説明が `modelTemplate.{key}Hint`。
 * テンプレート側で組み立てないのは、鍵の作り方が 2 箇所に散ると
 * 辞書の鍵集合テストが拾えない綴りずれが生まれるため。
 *
 * **チェックボックスで見せてはいけない。** 操作できないものを操作の形で
 * 出すのは、「押しても効かないチェックを見せない」の裏返しの嘘になる
 * （無効化したチェックも同じ — 何かすれば有効にできる、と読める）。
 *
 * **載せるのは「このアプリがそのプロバイダ固有の機構を実際に使っている」もの
 * だけ。** サーバー側が勝手にやる最適化は載せない — OpenAI 互換 / Gemini /
 * xAI はいずれも `cached_tokens` を**読んでいるだけ**で、こちらは何も送って
 * いない。それを固有スキルとして並べると、アプリの働きではないものを
 * 働きとして見せることになる。
 */
export function passiveSkills(provider: ModelTemplate["provider"]): string[] {
  // Anthropic だけが cache_control を実際に組み立てている
  // （`anthropic::place_message_breakpoint` / `build_system_blocks`）。
  return provider === "anthropic" ? ["passivePromptCache"] : [];
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
  /** 設定を持たない固有スキルの辞書キー。 */
  passive: string[];
  /**
   * 見出しを描くか。**パッシブだけの provider（Anthropic）でも描く** —
   * 見出しは「ここから先はこのプロバイダにしか無い能力」の区切りで、
   * 操作できるかどうかとは別。
   */
  anyOffered: boolean;
} {
  const onGemini = draft.provider === "gemini";
  const onXai = draft.provider === "xai_responses";

  const google = visibility(draft.googleSearch, onGemini);
  const xaiWeb = visibility(draft.xaiWebSearch, onXai);
  const xaiX = visibility(draft.xaiXSearch, onXai);

  const passive = passiveSkills(draft.provider);

  return {
    google,
    xaiWeb,
    xaiX,
    passive,
    anyOffered:
      google.offered || xaiWeb.offered || xaiX.offered || passive.length > 0,
  };
}
