# Spec: Gemini の固有スキル 2 本 — `thinkingLevel` の腕と URL context

- 起票: 2026-09-03
- 状態: **rev2 承認（2026-09-03。査読 2 系統 13 点 → 採用 6 / 訂正して採用 4 /
  反証 2 / 変更なし 1。記録は Notes 5。承認査読の追加 2 点 = golden の変更を「意図した
  削減」と明記 / (f) を f-1・f-2 に割る、も取り込み）→ P0 完了（probe 7 発 +
  `data_contract` 凍結 3 箇所。記録は「P0 実測記録」）→ P1 完了（コア。記録は
  「P1 実装記録」）**。残は P2〜P4
- 起点: 利用者 —「Google が gemini-3.8-flash を発表した。API の固有スキルが
  目を見張るものがあった」→ 村の事前調査を接地（probe 21 発 + 公式文書 9 ページ +
  `fuseforks.log`。記録は CLAUDE.md「Gemini 3.8 Flash と Interactions API の接地
  （2026-09-03）」）→「とりあえず両方、Gemini の固有スキルの追加として起票する？」
  （同日）。**両方 = 下の 2 本**。Interactions API のワイヤと agentic video は
  **起票しない**（範囲外の節）

## Goal

Gemini ネイティブワイヤ（`Provider::Gemini` = `generateContent`）に、
**接地で確定した 2 つの口**を通す。

1. **思考段階の Gemini 腕** — 既存の「思考段階」設定（`ModelTemplate.effort`）を
   `generationConfig.thinkingConfig.thinkingLevel` へ写す。**新しいトグルは生えない**。
   `ChatRequest` は `model` と `effort` を**既に持っている**が、`gemini::encode` が
   **どちらも読んでいない**（`openai_compat` / `openai_responses` は読む）。だから
   3.8 / 3.7 は既定の `medium` で常時思考している。8 日実測で思考が実効の 17% を
   占めた村で、**制御の口が無い唯一のワイヤ**
2. **URL context の固有スキル 1 トグル**（`geminiUrlContext`）— 依頼文に書かれた URL を
   Gemini 側で取得して答えに使う。Perplexity の `fetch_url`（Spec 45）と同型の
   「ワイヤの形が変わる」側。**前提として 2 つの穴を同じ Spec で塞ぐ** —
   (a) 取得本文は `usageMetadata.toolUsePromptTokenCount` として入力課金されるのに
   現行 decode がその欄を読まない（払っているのに `prompt` に出ない = #103 / Spec 40 の形）
   (b) Gemini の decode に `dropped content blocks:` の計器が無く、未知の part が無音で消える

**やらないこと（範囲外）**: Interactions API の 8 本目のワイヤ（generateContent で
3.8 の機能が全部届く。唯一の例外 agentic video は短尺で費用 2.1 倍・時間 2.6 倍で、
効くのは長尺だけ — 仕事が出るまで起票しない）/ code execution（Code Interpreter を
見送った棚。2026-08-09 裁定）/ File Search（村の `rag` と競合し、ファイルが Google 側に
無期限保存 — Meta の `/v1/files` と同じ「利用者が負う条件」。`google_search` /
`url_context` と同時使用不可）/ Maps grounding（用途が無い）/ Computer use
（2026-08-09 裁定のまま。Preview）/ 明示の context caching（Spec 40 で「作らない」）/
implicit caching の passive バッジ（「働きでないものを見せない」の規律の外）/
`thinkingBudget`（Notes 4）。

## 起票時の実測（2026-09-03。probe 21 発。`gemini-3.8-flash`・`GEMINI_API_KEY`）

**probe 1〜5 はすべて `thinkingLevel: "low"` で撃った。** `low` では思考が起きない回があり、
そのとき `thoughtsTokenCount` は**欄ごと省かれる**（probe 1 の 1 回目 = 欄なし・candidates 1、
2 回目 = `thoughtsTokenCount: 34`。同じ要求）。欄なし = 0 として読む（wire は `#[serde(default)]`）。

| # | probe | 結果 |
|---|---|---|
| 1 | `thinkingConfig: {thinkingLevel: "low", includeThoughts: true}` | 200。`includeThoughts` と併存できる。応答の text part に `thoughtSignature` が付く |
| 2 | `thinkingLevel: "minimal"` | 400 `Thinking level MINIMAL is not supported for this model. Please retry with other thinking level.`。**受理集合は列挙されない**（Spec 34 の「1 つ撃てば全列挙」は Gemini では効かない） |
| 3 | `thinkingBudget: 512` | 200。整数の欄も 3.8 で生きている（互換にしない判断は可能だが「使えない」ではない） |
| 4 | `temperature: 0.2` | 200。警告欄も無い（changelog 2026-07-21 の非推奨は 3.6 / 3.5-lite 宛） |
| 5 | `tools: [{urlContext: {}}]` + 本文に URL（**関数宣言なし・`toolConfig` なし**） | 200。`candidates[0].urlContextMetadata.urlMetadata[{retrievedUrl, urlRetrievalStatus: "URL_RETRIEVAL_STATUS_SUCCESS"}]` + **`groundingMetadata.groundingChunks[].web.{uri,title}` に実 URL**（検索の redirect ではない）+ `groundingSupports`。usage = `promptTokenCount` 32 / `candidatesTokenCount` 76 / **`toolUsePromptTokenCount` 8,999** / `thoughtsTokenCount` **欄なし** / `totalTokenCount` 9,107 = 32 + 76 + 0 + 8,999 |
| 6 | `codeExecution` / `googleSearch` + `functionDeclarations`（`includeServerSideToolInvocations` 無し） | 400 `Please enable tool_config.include_server_side_tool_invocations to use Built-in tools with Function calling.`（**2 つとも同じ文面**。文面どおり「組み込み × 関数宣言」の併用に掛かる門で、probe 5 の**関数宣言なし**は `toolConfig` 無しで通っている） |
| 7 | `googleSearch` + FN + `includeServerSideToolInvocations: true` | 200。parts = `[toolCall + thoughtSignature] [toolResponse + thoughtSignature] [text + thoughtSignature]`。**`toolResponse` は現行 decode が無音で捨てる part**。usage = 121 / 128 / thoughts 213 / total 462（= 121 + 128 + 213。tool-use の欄は無い） |
| 8 | `googleSearch` 単独 | `groundingChunks` 3 件（**URI は `vertexaisearch.cloud.google.com/grounding-api-redirect/…`**、title はドメインのみ）。Spec 05 の「sources 空」は 3.8 で解消しているが実 URL ではない。検索の注入分は usage のどの欄にも出ない（課金は 1 クエリ単位: 月 5,000 無料 → $14 / 1,000） |
| 9 | 村の実走 | `fuseforks.log` に `model=gemini-3.8-flash` が 35 ターン・`stop=failed` 0・`rounds=6/12` 完走 — **今のワイヤは 3.8 でそのまま動く**（`thoughtSignature` の持ち回りが通っている） |

Interactions API 側の 8 発（`POST /v1beta/interactions`・ステートレスでは `thought` step の
signature を逐語で送り返さないと 400・`processing: "agentic"` は Interactions 限定で
generateContent にはスキーマ上の欄が無い）は CLAUDE.md の同節が正。本 Spec は使わない。

## Design

### D1: `thinkingLevel` は `Effort` から写す。新しい設定は作らない

写像は `gemini::thinking_level(model: &str, effort: Option<Effort>) -> Option<&'static str>`
（純関数。`openai_compat::reasoning_effort` と同じ置き方）。**入力は `req.model` と
`req.effort`** — `encode` の引数を増やすのではなく、`ChatRequest` に既にある 2 欄を読む:

| `Effort` | 送る値 |
|---|---|
| `None`（未指定） | **送らない**（プロバイダ既定 = `medium`。「既定を勝手に補わない」— `data_contract` の `Effort` note と同じ線） |
| `Low` / `Medium` / `High` | そのまま |
| `XHigh` / `Max` | **`high`**（天井への写像。Meta の `max → xhigh` と同じ規律 — 丸めではなく「その口の天井」） |

`minimal` は **`Effort` に無いので構造的に出ない**（送信前バリデーションを書く必要が無い。
村の報告が求めた「minimal は送信前に拒否」は型で成立している）。

**`includeThoughts: true` はそのまま常送**（Spec 33 D3）。probe 1 で併存を確認済み。

### D2: モデル名で門を掛ける（同じワイヤ内の方言の吸収）

`thinkingLevel` は Gemini 3 系の欄で、2.5 系は `thinkingBudget` しか持たない
（公式 reference。**P0 (b) で確定: `gemini-2.5-flash-lite` は 400
`Thinking level is not supported for this model.`**、`gemini-2.5-flash` は 404 で
提供終了）。門は **`model.starts_with("gemini-3")`** の 1 条件。`reasoning_effort` が `gpt-5` / `grok-4.x` /
o 系で分けているのと同じ「同じワイヤ内の方言の吸収」であって、Spec 31 D2 が禁じた
「ワイヤの選択に名前を使う」には当たらない（`data_contract` の Gemini 思考の節に
既に同じ但し書きがある）。

**バイト等価の範囲**（golden で留める）: (a) `effort: None` のテンプレートは**全モデルで**
送る JSON がバイト等価 (b) **門の外（2.5 以前）は全 `Effort` でバイト等価**（何も送らない）。
変わるのは「3 系 × `effort: Some`」の組み合わせだけ。

### D3: URL context はトグル 1 本。`includeServerSideToolInvocations` は「組み込み × 関数宣言」の門に合わせる（rev2 で改稿 — 査読 B-2）

- `ModelTemplate.gemini_url_context: bool`（`#[serde(default)]`。TS は `geminiUrlContext`）。
  接頭辞は `google_search` / `xai_*` / `perplexity_*` と同じ理由（素の `url_context` は
  Perplexity の `fetch_url` と紛れる）
- 判定は **`gemini_url_context_active() = gemini_url_context AND effective_provider() == Gemini`**
  （`grounding_active` と同型の AND 述語。フラグ単独を判定に使わない）。
  **URL context にモデル名の門は掛けない** — P0 (e) で `gemini-2.5-flash-lite` が
  `urlContext` 単独で 200（`toolUsePromptTokenCount` 6,541）。拒む相手が居ない
  （rev2 で条件付きにし、P0 で確定 — 査読 A-2）
- `encode` は `tools` に `{"urlContext": {}}` を**別要素で** push（`google_search` と同じ形）
- **`toolConfig.includeServerSideToolInvocations: true` の条件は
  「組み込みツールが 1 つでも ON **かつ** 関数宣言が 1 つ以上ある」**。
  probe 6 の 400 文面（`to use Built-in tools with Function calling`）が名指しする
  併用条件そのもので、probe 5（urlContext 単独・関数宣言なし・`toolConfig` なし）が
  200 で通っている。**現行は `google_search` 単独で真にしており**（`gemini.rs:139` と
  単体 `…:607` が「宣言が無くても `Some(true)`」を主張している）、`data_contract` の
  「接地を使うときだけ送る」も関数宣言を数えていない。**関数宣言の無い周に送っても
  害は無い**（P0 (f-1)(f-2): 3 通りとも 200）。**ただし送ると応答の形が変わる** —
  `toolCall` / `toolResponse` の part が付き、送らなければ `text` だけ。現行の decode は
  `toolCall.args.queries` から検索語を拾っているが、**`groundingMetadata.webSearchQueries`
  からも同じ検索語を拾っている**ので、フラグを落としても `Grounding.queries` は空に
  ならない（P1 の結合で留める）。文書と 400 文面が示す条件に揃え、既存の単体 1 本の
  期待値を反転する（P1）。**`google_search` ON のテンプレートの golden は変わる** —
  `toolConfig` が「関数宣言の無い周」で消える。**これは意図した削減**で、加算的変更の
  約束（D2）は `effort` の側だけに掛かる（承認査読の追加指摘）
- **`toolCall` / `toolResponse` は履歴へ返さない（現行のまま）。** P0 (d2) で 3 形を
  撃った — (a) 両方落とす（= 現行 decode）→ 200・**モデルは同じ URL を再取得**
  （`toolUsePromptTokenCount` 9,047 がもう 1 度乗る）(b) `toolCall` だけ返す → 同じく
  再取得 (c) 両方返すが `thoughtSignature` を剥ぐ → 400
  `Tool call part is missing thought_signature`。**返せば取得本文が次の周の `prompt`
  に 9,050 で乗り、返さなければ再取得の `toolUsePrompt` に 9,047 で乗る** — どちらも
  入力単価で、周あたりの払いはほぼ同額。返す形は `ChatMessage` に不透明な part の席を
  作る変更（`data_contract` が「確かめる前に席を作らない」と凍結していた側）で、
  払いが同額なら席を作る理由が無い。**`data_contract` の「未解決: toolCall /
  toolResponse を履歴へ返す必要があるか」はここで閉じた**（返さなくてよい。返すなら
  signature ごと逐語）
- **手順の文（システムプロンプト）は変えない。** `grounding_active` が
  「Google 検索で裏取りしてから答えます」を注入するのは、検索が「持っていない情報を
  埋める」処方だから。URL context は**依頼文に URL があるときだけ**モデルが自分で
  使う道具で、告知が無くても発火する（probe 5 は告知なしで取得した）。
  告知を足すと URL の無い依頼にも毎ターン乗る固定費になる
- 出典は**既存の経路がそのまま埋める** — `groundingChunks[].web` に実 URL が載るので
  `Grounding.sources` に入り、`GroundingEngine::Google` のまま。**新しい engine 値は
  足さない。** URI の形で区別すること自体は可能（URL 取得 = 実 URL / 検索 = Google の
  redirect ホスト。probe 5 と 8）だが、(1) `engine` は**応答 1 つに 1 値**で、検索と
  URL 取得が同じターンに混ざると chunk 単位の出自が要る (2) redirect ホスト名の
  文字列照合は「除外リストは必ずもう一度落ちる」側の検査で、Google がホストを変えた
  瞬間に黙って誤分類する。**得るのは表示の 2 系統化だけ**なので統一する
  （rev2 で理由を書き直し — 査読 A-4。rev1 の「Google が出自を返さない」は engine の
  話と chunk の話を混ぜていた）
- `urlContextMetadata` は **計器 `gemini tools:` の 1 行**へ（D5）。画面には出さない —
  「取得に失敗した URL」は本文でモデルが言う。表示層に載せるのは失敗の頻度を見てから

### D4: `toolUsePromptTokenCount` は `prompt` へ**内数として畳む**（Spec 40 D1 と同じ側）

probe 5 / 7 の恒等式 **`totalTokenCount = promptTokenCount + candidatesTokenCount +
thoughtsTokenCount + toolUsePromptTokenCount`**（欄なしは 0。probe 5 = 32 + 76 + 0 + 8,999 /
probe 7 = 121 + 128 + 213 + 0）。**`data_contract` が凍結している「completion = candidates +
thoughts。この数え方でのみ totalTokenCount と一致する」は、URL context を使った瞬間に
8,999 ずれて嘘になる**。畳めば恒等式が保たれ、それが検定になる（P1 の単体は
**fixture 2 つ** — probe 5 の実物で tool-use 側、probe 7 の実物で thoughts 側を留める。
rev2 — 査読 A-1）。

- **課金の実体は入力単価**（文書「retrieved URL content is counted as part of the
  input tokens」）。畳めば `pricing.rs` は 1 行も変えずに正しい金額になる
- **`cache_read` とは重ならない**（取得本文はキャッシュから読まれない）。畳んだ後の
  **「素の未キャッシュ」`prompt − cache_read − cache_write` には取得本文が含まれる** —
  これは定義であって副作用ではない。取得本文は入力単価で未キャッシュとして課金される
  ので、Spec 40 の予算式（未キャッシュ ×1.0）にそのまま乗るのが正しい（rev2 で明記 —
  査読 A-5）
- **`cache:` 行と `turn:` 行は同じ値になる** — どちらも decode 後の `Usage.prompt` を
  出している（`turn.rs` の `note!("cache: …", usage.prompt, …)`）。生の
  `promptTokenCount` はどの行にも出ていないので、畳みで 2 行がずれることは無い
  （査読 B-3 は実装で反証。ただし検収 4 はこの一致を見る）
- **代償は予算の見積もり**（Spec 38 の「直前実測」）が取得本文ぶん過大になること。
  **P0 (d) で確定: 乗るのは取得が起きた周だけ**（履歴へ返さない = D3 なので、次の周に
  取得本文は残らない。モデルが再取得すればその周にまた乗る）。過大は「取得の次の
  1 呼び出し」で、`reserve_short` の側に倒れる（安全側）。**ただし報告が揺れる** —
  同じ要求（URL 取得 + 関数呼び出しが 1 周に同居）で、(d) は `toolUsePromptTokenCount`
  が**欄なし**、(d2) は 9,030。取得は両方で起きている（`urlContextMetadata` あり）。
  **払っているのに usage に出ない回が実在する**（1 対の観測。機構は作らない —
  `gemini tools:` 行が `urlContextMetadata` を別に数えるので、欄なしの回は
  「取得 1 / tool_use_prompt 0」として後から数えられる）。新しい欄を `Usage` / `Record::Turn` /
  `TurnSpend` / `turn:` 行 / `budget.rs` / `pricing.rs` の 6 箇所へ通す案（Spec 40 が
  `cache_write` でやった形）は、この代償を消すためだけには重い。**頻度を見てから**
  （#47）。畳んだ後も生の値は `gemini tools:` 行に残るので、後から欄に昇格させても
  遡って読める（rev2 で条件付きに — 査読 A-3 / B-5）
- **検索の注入分は畳まない** — probe 8 でどの欄にも出ない（課金がトークン建てでない）。
  ここで「無い」と書けるのは probe 8 が対照だから（#93 の規律）

### D5: 計器 2 行（Gemini の decode に無かった側）

1. **`dropped content blocks:`** — Anthropic / Meta と同じ形で、decode が本文にも
   `tool_calls` にも `grounding` にも写さなかった part の種別と数を 1 行出す。
   今日の実物では `toolResponse`（probe 7）と、将来の `executableCode` /
   `codeExecutionResult` / `inlineData`。**`toolCall` は数えない**（`queries` を
   拾っているので「捨てた」ではない）。`google_search` を使う既存の村では
   **毎ターン出る**ことになるが、それは「今まで無音で捨てていた」の可視化であって
   新しい事象ではない。うるさければ頻度を見て種別を除外する（除外は
   「捨てても失うものが無い」と確かめた種別だけ）
2. **`gemini tools:`** — `pplx tools:` と同じ棚。
   `gemini tools: agent=… url_context={retrieved}/{requested} statuses=… tool_use_prompt={toolUsePromptTokenCount} search_queries={n}`。
   URL context が OFF で検索も無い周では**出さない**（`grep include:` と同じ
   「使った呼び出しのときだけ」）

### D6: TS 側の手数え（Spec 45 D9 と同じ列挙。**`Record` ではない箇所を名指し**）

`types.ts`（`ModelTemplate.geminiUrlContext`）/ `providerSkills.ts`（`providerSkills()` の
戻り値へ `geminiUrlContext` + **`anyOffered` に足す** — 足さなくても型は通り、
トグルは描かれるのに**見出しの区切りだけ**が消える）/ `ModelTemplateDialog.vue`
（チェックボックス + **`draft` 初期化 2 箇所**（`:255` / `:435` 相当）+ `strandedRows`
の 1 行）/ `locales/{ja,en}.json`（`geminiUrlContext` / `geminiUrlContextHint` /
stranded 用）。`providerSkills.test.ts` に `anyOffered` の名指し検査を 1 本
（Spec 45 P2 でミューテーション 1 本だけ赤になった形を写す）。

### D7: 単価表は触らない

`prices.json` に `gemini-3.8-flash` = 0.75 / 3.75 / 0.075 は利用者が 2026-09-03 に登録済み。
**2027-01-01 に倍になる崖**（$1.50 / $7.50 / $0.15）は `pricingAsOf` の運用で追う
（日付で切り替わる単価を機構で持つのは Spec 41 の範囲外）。

## Phases

- **P0 — probe 7 発 + `data_contract` の凍結（完了。記録は下の「P0 実測記録」）**
- **P1 — コア**: `wire.rs`（`GeminiThinkingConfig.thinking_level: Option<&'static str>` /
  `GeminiTool.url_context` / `GeminiUsageMetadata.tool_use_prompt_token_count` /
  `GeminiCandidate.url_context_metadata`）/ `gemini.rs`（`thinking_level(&req.model, req.effort)`
  純関数 — **`encode` の引数は増やさない**。組み込みツールのフラグは
  `encode(req, &GeminiSkills { google_search, url_context })` の struct 1 つで受け、
  bool を 2 本並べない / decode の畳みと計器 2 行）/ `model.rs`（欄 +
  `gemini_url_context_active`）/ `client.rs`（`LlmConfig` + `from_template` + `encode` の
  呼び出し）。
  テスト: 写像表の単体（`None` → 欄なし / `Max` → `high` / 2.5 系 → 欄なし）/
  D2 のバイト等価 2 系（`effort: None` / 門の外）の golden / usage の恒等式（fixture 2 つ =
  probe 5 と 7 の実物）/ `url_context` ON + 関数宣言ありで `tools` と `toolConfig` の
  両方が出る / 関数宣言なしなら `toolConfig` が出ない（**既存の `…:607` の期待値を
  反転** — 「宣言が無いなら `include_server_side_tool_invocations` も送らない」）/
  OFF で `urlContext` が出ない / dropped 計器の結合 1 本。**ミューテーション 2 回**
  （写像を全部 `high` に → 表の単体だけ赤 / 畳みを外す → 恒等式だけ赤）
- **P2 — GUI**: D6 の全箇所 + vitest
- **P3 — 台帳**: README 3 言語（グラウンディングの行に URL 取得を 1 語）/ DETAIL 日英
  （固有スキルの表 + Gemini の節）/ CLAUDE.md（接地の節へ「→ Spec 48 で着地」の続報）/
  `data_contract` の回収。**ランディングページと Qiita 記事は grep 網の外** —
  固有スキルの列挙が載っていれば別に数える
- **P4 — 実機検収**（下）

## P0 実測記録（2026-09-03。probe 7 発 + d2 の 3 形。予測を先に書いてから観測）

| # | probe | 予測 | 結果 |
|---|---|---|---|
| (a) | `gemini-3.6-flash` / `gemini-3.5-flash-lite` に `thinkingLevel: low` | 200 | **200 / 200**（3.6 は `thoughtsTokenCount` 41、3.5-lite は欄なし） |
| (b) | `gemini-2.5-flash` に `thinkingLevel` | 400 | **404** `no longer available to new users. Please update your code to use models/gemini-3.6-flash … We recommend you to use the Interactions API.` — 提供終了。**サーバー自身が Interactions API を推す文面を返す** |
| (b') | `gemini-2.5-flash-lite` に `thinkingLevel` | 400 | **400** `Thinking level is not supported for this model.` — **D2 の門の根拠**（2.5-lite は提供中） |
| (c) | 3.8 `urlContext` + 関数宣言・`toolConfig` なし | 400 | **400**（probe 6 と同じ文面）— D3 の「組み込み × 関数宣言」 |
| (d) | 3.8 `urlContext` + 関数宣言 + フラグ、2 周 | 取得の周だけ | 周 1 = parts `[toolCall, functionCall, toolResponse]`・**`toolUsePromptTokenCount` 欄なし**・`urlContextMetadata` あり / 周 2（`toolResponse` を逐語で返した）= `prompt` 9,050・再取得なし |
| (d2) | (d) の周 1 をもう 1 度 + 履歴の 3 形 | — | 周 1 = **`toolUsePromptTokenCount` 9,030**（同じ要求で (d) と報告が違う）。(a) 両方落とす → 200・再取得 9,047 / (b) `toolCall` だけ → 200・再取得 9,067 / (c) signature を剥ぐ → **400 `Tool call part is missing thought_signature`** |
| (e) | `gemini-2.5-flash-lite` に `urlContext` 単独 | 400 | **200**（`toolUsePromptTokenCount` 6,541）— **門は要らない**（予測が外れた） |
| (f-1) | 3.8 `urlContext` 単独 + フラグ true | 200 | **200**。parts に `toolCall` / `toolResponse` が付く（フラグ無しの probe 5 は `text` だけ） |
| (f-2) | 3.8 `googleSearch` 単独 × フラグ true / なし | 200 / 200 | **200 / 200**。true なら `[toolCall, toolResponse, text]`、なしなら `[text]`。**現行の「単独で真」は害が無かった** |

**予測を外したのは (e)** — 「2.5 は 3 系の欄を拒む」を `thinkingLevel` から `urlContext` へ
写した形で、URL context は 2.5 世代（2025 年）の機能なので拒む理由が無かった。
「同じ世代の欄は同じ門」は成り立たない — **欄ごとに撃つ**（Spec 36 の carries 表を
20 マス全部撃ったのと同じ規律）。

**凍結 3 箇所**（`data_contract.yaml`）: `llm_wire.gemini_native.invariants` — 恒等式を
4 項へ・`includeServerSideToolInvocations` を「組み込み × 関数宣言」へ・`thinkingLevel` の
写像表と門・「未解決: toolCall / toolResponse」を閉じる・計器 2 行 / `ModelTemplate` の
欄の列挙へ `geminiUrlContext` / `grounding_active` の隣に `gemini_url_context_active`。

## P1 実装記録（2026-09-03）

lib 643 → 全 test binary 緑・clippy 警告ゼロ・ワークスペース `cargo check --all-targets`
警告ゼロ。**ミューテーション 2 回とも予測どおり 1 本だけ赤** — 写像を全部 `high` へ →
`thinking_level_maps_effort_and_gates_on_gemini_3` だけ / 畳みを外す →
`tool_use_prompt_tokens_fold_into_prompt_and_keep_the_total_identity` だけ。復元は sed で
（可逆な編集だけを使う — 2026-08-30 の `git checkout --` の事故を踏まない）。

- **`encode(req, skills: GeminiSkills)`** — rev2 の `&GeminiSkills` ではなく **値渡し**
  （`Copy` の 2 bool。参照にする理由が無い）。`GeminiSkills::any()` が「組み込みが
  1 つでも ON」の 1 実装。`thinking_level(&req.model, req.effort)` は `encode` の中で
  呼び、引数は増やしていない（B-4 の訂正どおり）
- **`gemini tools:` 行に `agent=` は無い** — decode 層は個体を知らない（`pplx tools:` /
  `dropped content blocks:` と同じ棚で、同じ制約）。突き合わせは直後の `turn:` 行の
  時刻で行う。D5 の書式から `agent=` を落とした
- **`dropped_kind` は「読む枝がある part」を先に除外する** — `text` / `functionCall` /
  `toolCall` のどれかを持つ part は数えない。`toolResponse` → `inlineData` →
  `functionResponse` の順で名指しし、未知のキーだけの part（serde が全欄 `None` で
  受ける = `executableCode` 等）は `unknown`。**思考の part は `text` を持つので
  数えない**（`thought: true` + `text`）
- **`tool_use_prompt` は `usage` を組む前に取り出す** — `Usage` へ畳んだ後は
  生の値が消えるので、`gemini tools:` 行の `tool_use_prompt=` は畳む前の値
- **既存テスト 1 本の期待値を反転** — `google_search_alone_still_sends_tools` →
  `google_search_alone_sends_tools_but_no_tool_config`（関数宣言が無ければ `toolConfig`
  ごと送らない）。**`google_search` ON のテンプレートの送信 JSON はここで意図して
  変わる**（承認査読の追加指摘。バイト等価の約束は `effort` の側だけ）
- **`ipc_contract.rs` の `wire_field_sets_are_frozen` が赤になった** — `ModelTemplate` の
  欄集合を凍結している側で、`geminiUrlContext` を足した。**このテストの doc は
  「まず `types.ts` を直せ」** — P2 の 1 手目はそこ（P1 では Rust 側の期待値だけ更新し、
  TS は P2 で足す。P1 と P2 の間は TS が 1 欄古い状態）
- 単体の新設 9 本: 写像表 / バイト等価 (a)(b) / 3 系で送る / URL context ON の
  `tools` + `toolConfig` / OFF で痕跡なし / 恒等式（probe 5・7 の実物）/ `dropped_kind` /
  `url_context_report` / `urlContextMetadata` の decode。`model.rs` に AND 述語の単体 1 本
- **触っていないもの**: `Usage` / `Record::Turn` / `TurnSpend` / `budget.rs` /
  `pricing.rs` / `turn:` 行の書式（D4 の「畳む」がこの 6 箇所を触らない理由そのもの）

## 検収項目（各項目に到達経路を書く — Spec 43/44 の教訓）

| # | 何を見るか | 到達経路 |
|---|---|---|
| 1 | 同じ 3.8 の個体・同じ依頼で、思考段階 `low` と `high` の `turn: … reasoning=` が桁で違う | テンプレートの「思考段階」を変えて 2 回依頼。`fuseforks.log` の `turn:` 行 |
| 2 | 思考段階を未設定にした個体の送信 JSON に `thinkingLevel` が無い | 機械（golden）。実機では「変わらない」しか見えないので項目にしない（#68） |
| 3 | URL を貼った依頼で `gemini tools: url_context=1/1` が出て、出典に**実 URL**（redirect ではない）が並ぶ | `geminiUrlContext` ON の 3.8 個体へ `https://…` を含む依頼。ログ + 会話ペインの出典 |
| 4 | 同じターンの `turn: prompt=` が取得本文ぶん（数千）膨らみ、同じ周の `cache:` 行の `prompt` と**同じ値**である（どちらも畳んだ後の `Usage.prompt`） | 3 と同じ走行。`total == prompt + completion` は機械側（P1）で留める |
| 5 | `google_search` ON の既存個体で `dropped content blocks: kinds=toolResponse` が出る | 何も設定を変えずに検索が走る依頼を 1 本。**今まで無音だった行が出る**ことが検収 |
| 6 | `geminiUrlContext` ON のまま provider を互換へ切り替えて保存すると、stranded 行に「Gemini ネイティブ」の持ち主名と「オフにする」が出る | モデル登録ダイアログ。Spec 45 と同じ操作 |
| 7 | URL の取得に失敗したとき（存在しない URL）`gemini tools: … statuses=URL_RETRIEVAL_STATUS_ERROR` が出て、ターンは落ちない | 存在しないドメインの URL を貼る |

**検収から外したもの**: 2.5 系で 400 が出ない（村に 2.5 の個体が無い。P0 (b') の probe と
D2 の単体で留める）。

## Notes

1. **村の事前調査は 6 リンク中 3 本が 404、事実の誤りが 1 件（Agentic Video は 3.8 非対応
   → 実測で対応）、射程の広げすぎが 1 件（temperature 非推奨は 3.6 / 3.5-lite 宛）。**
   骨格（3 点の優先）は合っていた。**「文書の URL が実在するか」は本文の正しさと別に
   数える**（LP の `href` を別に数えた 2026-08-29 と同じ規律が、調査報告にも掛かる）
2. **Interactions API に移る動機は「新機能が先に来る」の 1 点だけで、今は無い。**
   Meta / Perplexity のときは Responses しか口が無かった。ここは違う。
   切るときの形は `Provider::GeminiInteractions`（ステートレス・`thought` step の
   signature を逐語で往復 = Responses 4 本と同じ骨格。probe 8 発で確定済み）
3. **3.8 の `cache:` 行は prompt 10,754〜14,162 で `cached=0` が 8 本中 6 本**。
   #104（3.7 は 19,000 未満でキャッシュを返さない）の同族の可能性が高いが、
   19K 超の対照が無い。**本 Spec の範囲外で、本 Spec の変更とは独立** — 思考段階が
   動かすのは `thoughtsTokenCount`（`completion` 側）で `prompt` ではなく、
   キャッシュの閾値は `prompt` に掛かる（rev1 の「思考段階を下げて prompt が縮む」は
   トークンの種別を取り違えていた — 査読 B-1）。検収 1 の走行で `cached=` も読むのは、
   同じ個体の 2 ターンが対照になるからで、因果を疑ってではない
4. **`thinkingBudget` は足さない。** 3.8 で生きているが（probe 3）、`Effort` は段階の
   列挙で整数の予算ではない。2 つの口を持つと「どちらが勝つか」の規則がもう 1 つ要る
5. **査読の反映（rev2・2026-09-03。2 系統 13 点）**

   | 系統-# | 指摘 | 扱い | 根拠 |
   |---|---|---|---|
   | A-1 | probe 5 の恒等式に thoughts が無い | **訂正して採用** | probe 5 は `low` で撃ち `thoughtsTokenCount` が欄ごと無かった（= 0）。実測表に明記し、P1 の fixture を probe 5 + 7 の 2 つに |
   | A-2 | urlContext にモデル門が無い | 採用 | P0 (e) を足し D3 を条件付きに |
   | A-3 / B-5 | D4 の「次の 1 呼び出しだけ」は P0 (d) 依存 | 採用 | 条件付きに書き換え |
   | A-4 | 出自の説明が逆（URI で区別できる） | **訂正して採用** | 区別は可能。分けない理由を「engine は応答 1 値」+「ホスト名照合は除外リスト」へ書き直し |
   | A-5 | 畳んだ後の「素の未キャッシュ」の定義 | 採用 | 取得本文を含む、と定義として明記（予算式 ×1.0 に乗る） |
   | A-6a | `gemini-3.6-flash` / `3.5-flash-lite` は実在 ID か | **反証** | 村のログに 248 / 92 ターン |
   | A-6b | バイト等価の範囲 | 採用 | D2 に 2 系で明記 |
   | A-6c | `anyOffered` の記述 | 変更なし | 「区切りのみ」の現行で可、と査読自身が結論 |
   | B-1 | Note 3 のトークン種別の混同 | 採用 | 書き直し（思考段階は `prompt` を動かさない） |
   | B-2 | `includeServerSideToolInvocations` は「組み込み × 関数宣言」 | **訂正して採用** | probe 5 / 6 と文面に一致。**現行実装（`google_search` 単独で真）と単体 1 本の期待値が変わる**ので P0 (f) で害の有無も測る |
   | B-3 | `cache:` 行が生の `promptTokenCount` を出していると畳みでずれる | **反証** | `turn.rs:586` は decode 後の `usage.prompt`。生の値はどの行にも出ていない。検収 4 に「同じ値」と明記 |
   | B-4 | `encode` へ `effort` / `model` をどう渡すか未記述 | **訂正して採用** | `ChatRequest` が両方持つ（`canonical.rs:335,350`）。Goal 1 の「引数が無い」を「読んでいない」へ訂正し、D1 / P1 に `thinking_level(&req.model, req.effort)` と明記 |
