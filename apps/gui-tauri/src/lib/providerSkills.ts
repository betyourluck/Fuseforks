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
  // **open_ai_compat と同じ文字列。表は非単射になる**（Spec 34 D7）。
  // gemini / xai_responses は「互換の口も持つ他社ホスト」で鍵が別だったが、
  // ここは値が完全に一致する。**壊れないのは、この表を引くのが常に
  // provider を鍵にした前方参照だから** — URL から provider を逆引きする
  // 経路を足すと、ここが最初に壊れる。
  //
  // **登録しないと壊れる側もある**: anthropic → open_ai_responses の切り替えで
  // preset が undefined になり、base URL が api.anthropic.com のまま残る。
  open_ai_responses: "https://api.openai.com/v1",
  meta_responses: "https://api.meta.ai/v1",
  perplexity_responses: "https://api.perplexity.ai/v1",
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
  // Meta も互換の口を持つ。**この村の muse-spark 個体は現に互換で動いている**
  // ので、`open_ai_compat` のまま api.meta.ai を指すのは正当な構成。
  DEFAULT_BASE_URL.meta_responses,
  // **Perplexity はここに入れない**（Spec 45 D8）— この表は Chat Completions の
  // 口の免除で、api.perplexity.ai はその口を持たない（/v1/chat/completions は
  // 404・本文なし。実測 2026-08-19）。あちらの免除は下の
  // ALSO_SERVES_RESPONSES が担う。
];

/**
 * **OpenAI Responses の口も同時に持つ**ホストの既定 URL（Spec 45 D8）。
 *
 * `ALSO_SERVES_COMPAT` の対になる 2 枚目の免除表。`provider:
 * open_ai_responses` のまま api.perplexity.ai を指す相乗りは **2026-08-19 から
 * 現に動いている正当な構成**（/v1/responses は /v1/agent の互換エイリアス）で、
 * perplexity_responses の既定値が `KNOWN_DEFAULTS` に入った瞬間、この表が
 * 無いと既存の設定に嘘の警告が出る（rev1 の Spec 45 が査読で指摘された矛盾）。
 *
 * 用途は `ALSO_SERVES_COMPAT` と同じ 2 つ — `baseUrlMismatch` の免除（否定）と
 * `presetBaseUrlFor` の据え置き（肯定）。**片方だけに足すと、切り替えた瞬間に
 * Perplexity の鍵を持って OpenAI へ送る設定が黙って出来上がる**
 * （api.x.ai の 401 実機と同じ形）。
 */
const ALSO_SERVES_RESPONSES: readonly string[] = [
  DEFAULT_BASE_URL.perplexity_responses,
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
  if (
    provider === "open_ai_responses" &&
    ALSO_SERVES_RESPONSES.includes(baseUrl)
  ) {
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
  if (
    provider === "open_ai_responses" &&
    ALSO_SERVES_RESPONSES.includes(currentBaseUrl)
  ) {
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
/**
 * このテンプレートは Responses ワイヤへ切り替えると能力が増えるか（Spec 34）。
 *
 * **これは「案内」であって「判定」ではない。** ワイヤの選択は provider だけで
 * 決まり（D2 / D7）、この関数は 1 行の案内を出すかどうかしか決めない。
 * **モデル名を見てよいのはそのため** — 判定に使うと `Provider::detect` と
 * 2 系統になるが、案内は効き目に一切触れない。
 *
 * **要る理由は頻度が 1.0 だから。** 既存の gpt-* テンプレートは 100% が
 * 互換（または自動判定）で、切り替えるまで新しい能力が在ることが画面の
 * どこにも出ない。「頻度を見てから」は**頻度が未知の事象**の規律で、
 * 構造で決まっている穴へ当てると先送りに規律の名前を借りることになる
 * （Spec 34 D8 で自分が踏んだ形）。
 *
 * **base URL を条件に入れるのは偽陽性が高くつくから。** ローカルの互換サーバへ
 * `gpt-5` を名乗るモデルを載せている村に「切り替えれば使える」と言うと、
 * 切り替えた先に `/responses` が無く**毎ターン失敗する**。
 * 取りこぼし（プロキシ経由で OpenAI を使う村に出ない）は手で切り替えれば済む。
 */
export function suggestsResponses(
  draft: Pick<ModelTemplate, "provider" | "model" | "baseUrl">,
): boolean {
  // 既に Responses なら案内しない。他社を明示している村にも出さない。
  if (draft.provider !== null && draft.provider !== "open_ai_compat") return false;
  // 既定の OpenAI ホストのときだけ。自前プロキシ / ローカルには出さない。
  if (draft.baseUrl !== DEFAULT_BASE_URL.open_ai_responses) return false;
  // #76 / #77 が対象にしている家族と同じ範囲に揃える（Rust の
  // `uses_max_completion_tokens` が見ているのも `gpt-5` 前置）。
  return draft.model.startsWith("gpt-5");
}

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
   * **UI からも普通に作れる** — スキルを ON にしたままプロトコルを切り替えて
   * 保存すると残る（保存は draft の全欄をそのまま送る。2026-08-30 に
   * Perplexity のテンプレートで実機観測 — OpenAI Responses 時代の
   * `openaiWebSearch: true` が残っていた）。`world.json` の直接編集でも作れる。
   * 消す UI は stranded 行の「オフにする」ボタン（ModelTemplateDialog）。
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
  | "provider"
  | "googleSearch"
  | "xaiWebSearch"
  | "xaiXSearch"
  | "openaiWebSearch"
  | "openaiReasoningPro"
  | "metaWebSearch"
  | "perplexityWebSearch"
  | "perplexityFinanceSearch"
  | "perplexityPeopleSearch"
  | "perplexityFetchUrl"
>): {
  google: SkillVisibility;
  xaiWeb: SkillVisibility;
  xaiX: SkillVisibility;
  openaiWeb: SkillVisibility;
  /**
   * Pro 推論モード（`reasoning.mode = "pro"`。Spec 34 D4）。
   *
   * **3 値の選択肢ではなくトグル。** 送らないのと `"standard"` は
   * `input_tokens` が完全に一致する（実測 20 / 20）ので、省略 ≡ standard。
   * 器の 3 形目は要らなかった。
   */
  openaiPro: SkillVisibility;
  /**
   * Meta の web 検索（Spec 37 D3）。
   *
   * **トグル 1 つに閉じる。** `search_context_size` という軸は実在する
   * （実測で 200）が、`filters` / `max_results` は 400 で名指しされる —
   * **受理集合が列挙されないワイヤ**なので、器を増やす前に 1 つずつ
   * 名指しさせて確かめる必要がある。
   */
  metaWeb: SkillVisibility;
  /**
   * Perplexity の固有スキル 4 本（Spec 45 D3）。別トグルなのは
   * 別ツール・別課金・別 output 種別（xAI が 2 つに割れたのと同じ根拠）。
   */
  perplexityWeb: SkillVisibility;
  perplexityFinance: SkillVisibility;
  perplexityPeople: SkillVisibility;
  perplexityFetch: SkillVisibility;
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
  const onOpenAi = draft.provider === "open_ai_responses";
  const onMeta = draft.provider === "meta_responses";
  const onPerplexity = draft.provider === "perplexity_responses";

  const google = visibility(draft.googleSearch, onGemini);
  const xaiWeb = visibility(draft.xaiWebSearch, onXai);
  const xaiX = visibility(draft.xaiXSearch, onXai);
  const openaiWeb = visibility(draft.openaiWebSearch, onOpenAi);
  const openaiPro = visibility(draft.openaiReasoningPro, onOpenAi);
  const metaWeb = visibility(draft.metaWebSearch, onMeta);
  const perplexityWeb = visibility(draft.perplexityWebSearch, onPerplexity);
  const perplexityFinance = visibility(draft.perplexityFinanceSearch, onPerplexity);
  const perplexityPeople = visibility(draft.perplexityPeopleSearch, onPerplexity);
  const perplexityFetch = visibility(draft.perplexityFetchUrl, onPerplexity);

  const passive = passiveSkills(draft.provider);

  return {
    google,
    xaiWeb,
    xaiX,
    openaiWeb,
    openaiPro,
    metaWeb,
    perplexityWeb,
    perplexityFinance,
    perplexityPeople,
    perplexityFetch,
    passive,
    anyOffered:
      google.offered ||
      xaiWeb.offered ||
      xaiX.offered ||
      openaiWeb.offered ||
      openaiPro.offered ||
      metaWeb.offered ||
      perplexityWeb.offered ||
      perplexityFinance.offered ||
      perplexityPeople.offered ||
      perplexityFetch.offered ||
      passive.length > 0,
  };
}
