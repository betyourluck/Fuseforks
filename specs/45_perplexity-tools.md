# Spec: Perplexity のツール — 7 本目のワイヤと固有スキル 4 本

- 起票: 2026-08-27
- 状態: **rev1（査読待ち）**
- 起点: 利用者 —「Perplexity には `web search` だけではなく `Fetch URL`、
  `People Search`、`Finance Search` が使える。とくに Finance Search は
  xAI の live search 並に金融系の情報収集には人気のツールであるため
  これをぜひ入れたい」（2026-08-27）。**案 A（7 本目のワイヤ）は同日に
  利用者裁定済み**で、本 Spec の査読対象は骨格ではなく各判断の詰め

## Goal

Perplexity の Agent API が持つサーバー側ツール 3 本（`finance_search` /
`people_search` / `fetch_url`）を、モデル登録の**固有スキル**（Spec 31 の器）
として使えるようにする。あわせて、相乗りでは捨てられていた**出典**
（`search_results` ほか `*_results` 系 output item）を画面の出典表示へ写す。

**やらないこと**: `sandbox`（コード実行）— 遠隔サンドボックスは同梱ツール層と
競合する既存の裁定（Code Interpreter を見送った棚）のまま。Perplexity 側の
`mcp` / `connectors` ツールも対象外（村の MCP 機構と競合）。
`usage.cost`（USD 実額）の統計画面への写しも**範囲外**（D10）。

## 起票時の実測（2026-08-27。probe 11 発・費用 ≈ $0.02。予測を先に書いてから観測）

送信形はアプリの OpenAI Responses ワイヤの実物
（`store: false` / `reasoning.summary+context` / `include` / `max_output_tokens`）
をそのまま使い、`tools` だけ差し替えた。宛先は
`POST https://api.perplexity.ai/v1/responses`（`/v1/agent` の OpenAI 互換
エイリアス）、モデルは現用の `perplexity/deepseek-v4-flash-0731`。

| # | probe | 結果 |
|---|---|---|
| 1 | `{"type":"finance_search"}` | 200。**ただし `max_steps` 無しでは `skill_loaded` item だけ返して実行されない**（モデルが「ツール呼び出しが無効」と自己申告する黙った空振り）。`max_steps: 5` を足すと完走 — `finance_results` に AAPL $313.45 / MSFT $496.37 の実データ |
| 2 | `{"type":"people_search"}` | 200。`max_steps` **不要**。`people_search_results {queries, results[{id,url,title,snippet,source,date?,last_updated?}]}` |
| 3 | `{"type":"fetch_url"}` | 200。`max_steps` **不要**。`fetch_url_results {contents[{url,title,snippet}]}`（llms.txt 実物 42KB を取得） |
| 4 | 誤った型 `bogus_search` | **400 named**（`unknown discriminator value: bogus_search`）。**受理集合は列挙しない** — OpenAI（全列挙）とも xAI（無言の 422）とも違う**第 3 の様式**。名指しはするので「送って教わる」は 1 欄ずつなら効く |
| 5 | `usage.cost` | **全応答に USD 実額**（`{currency, input_cost, output_cost, tool_calls_cost, tool_calls_cost_details, total_cost}`）。ツール課金の実測は文書と一致 — finance / people **$0.005/回**、fetch_url **$0.0005/回** |
| 6 | 課金 detail の内部名 | **`people_search` の課金 detail 鍵は `search_people`** — tool 型と揃っていない（xAI の `custom_tool_call` / `web_search_call` 非対称の同族） |
| 7 | `annotations` | **全 probe で 0 件**。出典は `*_results` output item 側にしか無い（2026-08-19 の `search_results` 観察と一貫） |
| 8 | `finance_results` の中身 | `{categories, results[{category, content, sources, tickers}], tickers}`。`content` は markdown の表（price / marketCap / …）、**`sources` は実 URL**（`https://www.perplexity.ai/finance/AAPL`） |
| 9 | carries: 画像 | **✓**（`input_image` + data URL。`google/gemini-3-flash-preview` 経由で 1×1 赤 PNG に「赤」= 内容照合）。**`deepseek-v4-flash` の 400 `invalid request` はモデル層の拒否** — 「ワイヤが運べるか」と「モデルが受理するか」の層の分離（Spec 36）の実例がまた 1 つ |
| 10 | carries: 音声・動画・PDF | **3 つとも ✗**。`invalid type "input_audio"` / `"input_video"` / `"input_file"` と**型ごと名指しで拒否**。**本家 OpenAI Responses は PDF ✓ なので carries が本家と違う** — 相乗りのままだと表に従って送って必ず 400 |
| 11 | 既知の再確認 | `reasoning_tokens` 0 / キャッシュは効く（`cached_tokens` 3968 を観測。2026-08-19 の 0 は初回だったため） |

2026-08-19 の実測（CLAUDE.md「Perplexity を繋いだ」）も前提に含む:
互換の口 `/v1/chat/completions` は **404（存在しない）** / アプリの Responses
ワイヤが送る欄そのまま（`store` / `reasoning` / `include` / 関数ツールと
`web_search` の混在）で 200 / 出典は `annotations` ではなく `search_results`。

## Design

### D1: 7 本目のワイヤ `Provider::PerplexityResponses`（裁定済み）

利用者裁定（2026-08-27・案 A）。根拠は 4 つ、すべて実測:

1. **ゲートの正直さ** — 固有スキルの述語は「フラグ AND provider 一致」
   （`xai_web_search_active` の形）。相乗りのままフラグを足すと provider では
   本物の OpenAI と区別できず、**有効にすると 400 で個体が落ちるトグル**が
   OpenAI のテンプレートに生える。base URL での分岐は既存の禁じ手
   （「URL から provider を逆引きする経路を足すと最初に壊れる」）
2. **出典の decode** — `search_results` ほかは Perplexity 方言の output item。
   相乗りのままでは `dropped content blocks` へ落ち続け、画面は
   「参照元は返ってきていません」のまま
3. **carries が本家と違う**（実測 10）— 相乗りは PDF を送って必ず 400
4. **`usage.cost`** — 単価表を当てずに実額が取れる唯一の口。ワイヤが
   分かれていれば将来 1 欄で写せる（本 Spec では D10 = 範囲外）

xAI（Spec 31）/ Meta（Spec 37）に続く**ベンダー方言 Responses の 3 例目**で、
前例の形にそのまま乗る。

### D2: 要求のトップレベルは分け、input の encode は共有する

Spec 34 の判断「同じエンドポイントだから同じ型、は送る側で成立しない
（`include` で必ず割れる）」がそのまま当たる — 今回は **`max_steps` で割れる**
（D4）。要求のトップレベルは Perplexity 固有の型を持つ。

**input（メッセージ列 → `ResponsesInputItem`）の encode は Spec 37 の
関数ポインタ形（`encode(messages, part_for)`）で共有する。** `part_for` は
carries 表を読まない（読ませると `adapters_match_the_carries_table` が
同語反復になる — Spec 37 の網の設計をそのまま維持）。

応答側の型（`Responses*`）は Spec 34 で共有と実測済み。Perplexity 固有の
output item（`search_results` / `finance_results` / `people_search_results` /
`fetch_url_results` / `skill_loaded`）の variant をどこに足すか
（共有 enum へ加算か、xAI のように専用型か）は **P1 で既存の共有度を
数えてから決め、P1 実装記録に書く**。

### D3: トグルは 4 本。ベンダー接頭辞は `perplexity_`

`perplexity_web_search` / `perplexity_finance_search` /
`perplexity_people_search` / `perplexity_fetch_url`。

- **別トグルの根拠は xAI の前例と同じ** — 別ツール・別課金・別 output 種別
  （1 つに畳むと web だけ欲しい村が people_search の課金面まで開ける）
- 判定は各 `*_active()`（フラグ AND `provider == PerplexityResponses`）。
  フラグ単独で判定しない
- **`openai_web_search` からの自動移行は作らない。** 相乗り運用の既存
  テンプレートが provider を `perplexity_responses` へ切り替えたとき、
  `openai_web_search` は述語が偽になって死に、`perplexity_web_search` を
  人が入れ直す。フラグの自動写しは「切り替えただけで検索の課金面が開く」
  経路になるので作らない（既定 OFF の規律と同じ向き）
- 既定は 4 本とも **OFF**（検索は入力を桁で膨らませる — Spec 37 の実測。
  finance / people は 1 回 $0.005 の呼び出し課金も持つ）
- `people_search` の**パラメータ（`max_tokens` / `max_tokens_per_page`）と
  `fetch_url` の `max_urls` は送らない** — 送れる形が無いほうが強い（#77）。
  必要が実測で出たら欄を足す

### D4: `max_steps` は finance が ON のときだけ送る。値は定数 5

- **必要なのは `finance_search` だけ**（実測 1〜3。finance は `skill_loaded` を
  挟む skill 族で、step 予算が無いと内部ツール `finance_quotes` を呼べない）
- 送らないと **200 のまま黙って空振りする**（エラーにならない最悪の形）ので、
  フラグと欄はコアが対で管理する。利用者に `max_steps` を見せない
- 値は **5**（文書の推奨帯 5〜10 の下限。step は課金と遅延を持つ）。
  コード定数で設定にしない（Spec 11 の重みと同じ扱い）
- 他のツールには送らない — probe で送っても無害と実測済みだが、
  要らない欄を送らないほうが 400 の面が狭い

### D5: 出典は `GroundingEngine::Perplexity` の 1 値で写す

CLAUDE.md が既に指している形（「写すなら `GroundingEngine` に 1 値
（Spec 31 の形）」）。写す元は 4 種:

| output item | 出典の在り処 |
|---|---|
| `search_results` | `results[{url, title, snippet, date?, last_updated?}]` |
| `people_search_results` | `results[{url, title, snippet, source}]` |
| `fetch_url_results` | `contents[{url, title, snippet}]` |
| `finance_results` | `results[].sources`（実 URL の配列） |

- Spec 05 の規律（検索した事実と出典が返らない事実を分ける）はそのまま —
  ツールが走って出典 0 件なら 0 件と出す
- `skill_loaded` は出典を持たない（`{name, type}` だけ）。**捨てるが数える**
  （#72 — `dropped` の計器に載せる。無言で握り潰さない）

### D6: 計器は `pplx tools:` の 1 行

前例（`xai search:` / `meta search:` — 行名は engine ごとに分ける）に乗る。

```text
pplx tools: finance=N people=N fetch=N web=N sources=N cost_usd=X.XXXXXX
```

- 内訳はツール別（課金単価が違うので合計 1 個では読めない）。数える元は
  `usage.tool_calls_details`（**課金 detail の内部名 `search_people` を
  数える** — 実測 6。tool 型の名前で grep すると 0 件になる罠を計器側が吸収）
- `cost_usd` は `usage.cost.total_cost` の生の値。**単位の推論が要らない**
  （xAI の ticks は単位を推論した — こちらは currency 欄つきの USD）。
  値だけで本文は書かない（#71）

### D7: carries は画像のみ ✓

| ワイヤ | 画像 | 音声 | 動画 | PDF |
|---|---|---|---|---|
| **Perplexity Responses** | ✓ | ✗ | ✗ | ✗ |

実測 9〜10（画像は内容照合済み・他 3 種は型ごと名指し拒否）。
`Provider::carries` は provider ごと 1 腕の配列リテラル（Spec 37 の形 —
variant を足すとコンパイラが実際に指すことを確認済みの書き方）。
`tests/carries_table.rs` の逐語凍結と TS 側 `carriesTable.test.ts` の
突き合わせに 1 行ずつ足す。

### D8: base URL の既定と検出。`ALSO_SERVES_COMPAT` には入れない

- `DEFAULT_BASE_URL.perplexity_responses = "https://api.perplexity.ai/v1"`
- `Provider::detect` に `api.perplexity.ai` を足す（未指定テンプレートの判定）
- **`ALSO_SERVES_COMPAT` には入れない** — Perplexity は互換の口を持たない
  （`/v1/chat/completions` は 404・本文なし。実測 2026-08-19）。xAI / Meta /
  Gemini と**逆**で、互換のまま api.perplexity.ai を指す構成は正当ではなく
  `baseUrlMismatch` が指摘してよい

### D9: TS 側の手数え 5 箇所

Spec 37 の教訓（union と `Record` で網羅性が割れる）をそのまま適用。
コンパイラが指すのは `carries.ts` の `Record<Provider,…>` だけで、
**手で数えるのは 5 箇所** — `DEFAULT_BASE_URL` / `ALSO_SERVES_COMPAT`
（今回は「足さない」を選ぶ箇所として数える）/ `providerSkills` の判定と
戻り値 / `anyOffered` / プロトコルの選択肢。**`anyOffered` が最も静か**
（足さなくても型は通り、トグルは描かれるのに見出しの区切りだけが消える）。

### D10: `usage.cost` の統計への写しは範囲外

実額が取れる唯一の口だが、統計画面への写しは Spec 41 の横に 1 欄を足す
別の話（推定 `≈ $` と実額の 2 列をどう並べるかという画面の未決を持つ）。
本 Spec は D6 の計器でログに残すところまで。**捨てはしない・写しもしない。**

### D11: モデル層の拒否には何もしない

`deepseek-v4-flash` は画像をモデル層で拒む（実測 9）。「ワイヤが運べるか」と
「モデルが受理するか」は別の層（Spec 36 で凍結済みの区別）で、後者は
利用者のモデル選択の問題。既存の 400 処理（`fatal` 判定）のまま。

## Phases

- **P0: 契約の凍結** — `data_contract.yaml` へ `perplexity_tools_contract`
  （D1〜D11 の凍結・carries の行・`ModelTemplate` の 4 フラグ・計器の書式）。
  `settings_contract` / 既存の固有スキル関連の節に続報。
  probe は本 Spec の実測欄が P0 相当を先取りしており、追加の probe は不要
- **P1: コア** — `Provider::PerplexityResponses` + `perplexity_responses.rs`
  （encode 共有 + 固有トップレベル + decode + Grounding + 計器）+
  `ModelTemplate` の 4 フラグと `*_active()` 述語 + carries（Rust 側の表と
  逐語凍結）。golden（要求の形。`max_steps` の有無が finance フラグで割れる
  ことを 2 本で）+ ミューテーションで赤確認
- **P2: フロント** — TS 5 箇所の手数え + `ModelTemplateDialog` の固有スキル
  欄（トグル 4 本。課金の注意書き — finance / people は 1 回 $0.005）+
  辞書 ja/en + `carriesTable.test.ts` / `providerSkills.test.ts`
- **P3: 台帳** — README 日英（対応プロバイダ表 / 固有スキル）/ DETAIL 日英
  （ワイヤ層 6 → 7 本）/ CLAUDE.md「Perplexity を繋いだ」節へ続報
  （相乗りから 7 本目へ・`search_results` の宿題を回収）
- **P4: 実機検収**

## 検収項目（各項目に到達経路を書く — Spec 43/44 の教訓）

1. **finance**: `perplexity_responses` + `perplexity_finance_search` ON の
   個体に金融の質問（例「AAPL の今日の株価は」）→ 答えに実データが入り、
   出典表示に `perplexity.ai/finance/…` が出て、`fuseforks.log` に
   `pplx tools: finance=1 … cost_usd=` の行が出る。
   到達経路: ツールを提示すればモデルが選ぶ（probe 1 で同型の質問に発火済み）
2. **finance の空振りが存在しないこと**: 同じ個体・同じ質問で
   `skill_loaded` だけ返って答えが「取得できません」になる形が**出ない**
   （D4 の `max_steps` 自動付与が効いている証拠。probe 1 の空振りが対照）
3. **people**: `perplexity_people_search` ON の個体に人物の質問 →
   `pplx tools: people=1`。**計器が課金 detail 名 `search_people` を
   正しく数えている**ことをこの行の存在で読む（tool 型名で数える実装だと
   この行の people= が 0 になる — 到達可能な負の形）
4. **fetch_url**: ON の個体に URL 入りの依頼 → `pplx tools: fetch=1` と
   本文にそのページ由来の内容
5. **OFF の対照**: 4 本とも OFF の同じ個体に同じ質問 → `pplx tools:` 行が
   出ない・ツール課金 0（提示していないものは呼ばれない）
6. **相乗りの回帰**: 既存の `open_ai_responses` + api.perplexity.ai の
   テンプレートが**このまま動き続ける**（provider を切り替えなくても退行なし。
   到達経路: 既存の村の Perplexity 個体に普通の依頼を 1 本）
7. **出典の回収**: web 検索（`perplexity_web_search` ON）で出典が
   「参照元は返ってきていません」ではなく実 URL の一覧になる
   （2026-08-19 の観察の裏返し。`search_results` が Grounding へ写った証拠）

## Notes

1. **`skill_loaded` という output item は文書に無い**（実測 1 で発見）。
   finance だけが「skill」族で、内部ツール（`finance_quotes`）を持つ。
   文書と実装の乖離の系譜（WebP / legacy 410 / xAI PDF / Meta 2 件）に足す
2. **`deepseek-v4-flash` は文書の対応モデル欄に無いのに 3 ツールとも動いた**
   （乖離のもう 1 件。文書を根拠に「対応モデルのみ」と絞らなくてよかった）
3. 検索結果の本文がプロンプトへ入る攻撃面は web_search と同じ
   （Spec 26 の無害化が envelope 層を守る。X 検索のときの整理のまま）。
   **people_search は人物情報を扱う**ので、README の固有スキルの説明には
   課金と合わせて「何を検索するツールか」を 1 行で明示する
4. probe の生ログは scratchpad（使い捨て）。数字は本 Spec の実測欄が正
