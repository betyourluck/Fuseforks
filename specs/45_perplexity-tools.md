# Spec: Perplexity のツール — 7 本目のワイヤと固有スキル 4 本

- 起票: 2026-08-27
- 状態: **rev2 承認（2026-08-27）→ P0〜P1 完了**（P1 実装記録は下。
  workspace 全 32 スイート緑・vitest 441 緑・clippy 警告ゼロ・
  ミューテーション 2 回で赤確認）。P0 =（`data_contract.yaml` へ
  `perplexity_responses` ブロック新設 + carries 表の 7 行目 +
  `GroundingEngine` values + `ModelTemplate` の 4 欄 +
  `variant_addition_sites` の続報。**P0 で既存の追従漏れを 2 件回収した** —
  `GroundingEngine` の values が `[google, xai]` のままで `open_ai` /
  `meta` が漏れていた（Spec 34 / 37）/ `ModelTemplate` の契約欄の列挙に
  `metaWebSearch` が無かった（Spec 37）。どちらも #51 の「腐るのはその機能の
  節ではなく隣の節」— 契約を書く前に隣の記述を読んだので出た、の再演）。
  査読の反映は 採用 7 / 訂正して採用 3 / 反証 1（記録は Notes 5）。
  rev1 は査読待ちの初版
- 起点: 利用者 —「Perplexity には `web search` だけではなく `Fetch URL`、
  `People Search`、`Finance Search` が使える。とくに Finance Search は
  xAI の live search 並に金融系の情報収集には人気のツールであるため
  これをぜひ入れたい」（2026-08-27）。**案 A（7 本目のワイヤ）は同日に
  利用者裁定済み**で、査読対象は骨格ではなく各判断の詰め

## Goal

Perplexity の Agent API のサーバー側ツールを、モデル登録の**固有スキル 4 本**
として使えるようにする — **新設 3 本**（`finance_search` / `people_search` /
`fetch_url`）と、**移設 1 本**（web 検索。相乗り構成では `openai_web_search`
フラグで動いていたものを、新ワイヤでは `perplexity_web_search` として持ち直す）。
あわせて、相乗りでは捨てられていた**出典**（`search_results` ほか `*_results`
系 output item）を画面の出典表示へ写す。

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
| 5 | `usage.cost` | **全応答に USD 実額**（`{currency, input_cost, output_cost, tool_calls_cost, tool_calls_cost_details, total_cost}`）。**回数は別の欄** `usage.tool_calls_details`（`{invocation: N}`）に載る — 金額と回数で 2 つの欄が並ぶ。ツール課金の実測は文書と一致 — finance / people **$0.005/回**、fetch_url **$0.0005/回** |
| 6 | 課金・回数の内部名 | **`people_search` は `usage.cost.tool_calls_cost_details` / `usage.tool_calls_details` の両方で鍵が `search_people`** — tool 型と揃っていない（xAI の `custom_tool_call` / `web_search_call` 非対称の同族）。finance / fetch_url は tool 型と一致 |
| 7 | `annotations` | **全 probe で 0 件**。出典は `*_results` output item 側にしか無い（2026-08-19 の `search_results` 観察と一貫） |
| 8 | `finance_results` の中身 | `{categories, results[{category, content, sources, tickers}], tickers}`。`content` は markdown の表（price / marketCap / …）、**`sources` は実 URL の文字列配列**（`https://www.perplexity.ai/finance/AAPL`）。**`url` / `title` / `snippet` のオブジェクト形ではない**（D5 の注記） |
| 9 | carries: 画像 | **✓**（`input_image` + data URL。`google/gemini-3-flash-preview` 経由で 1×1 赤 PNG に「赤」= 内容照合）。**`deepseek-v4-flash` の 400 `invalid request` はモデル層の拒否** — 「ワイヤが運べるか」と「モデルが受理するか」の層の分離（Spec 36）の実例がまた 1 つ |
| 10 | carries: 音声・動画・PDF | **3 つとも ✗**。`invalid type "input_audio"` / `"input_video"` / `"input_file"` と**型ごと名指しで拒否**。**本家 OpenAI Responses は PDF ✓ なので carries が本家と違う** — 相乗りのままだと表に従って送って必ず 400 |
| 11 | 既知の再確認 | `reasoning_tokens` 0 / キャッシュは効く（`cached_tokens` 3968 を観測。2026-08-19 の 0 は初回だったため） |

2026-08-19 の実測（CLAUDE.md「Perplexity を繋いだ」）も前提に含む:
Chat Completions の口（`/v1/chat/completions`）は **404（存在しない）** /
アプリの Responses ワイヤが送る欄そのまま（`store` / `reasoning` / `include` /
関数ツールと `web_search` の混在）で 200 / 出典は `annotations` ではなく
`search_results`。

## Design

### D1: 7 本目のワイヤ `Provider::PerplexityResponses`（裁定済み）

利用者裁定（2026-08-27・案 A）。根拠は 4 つ、すべて実測:

1. **ゲートの正直さ** — 固有スキルの述語は「フラグ AND provider 一致」
   （`xai_web_search_active` の形）。相乗りのままフラグを足すと provider では
   本物の OpenAI と区別できず、**有効にすると 400 で個体が落ちるトグル**が
   OpenAI のテンプレートに生える。**禁じ手なのは、能力・スキル・encode の
   判定を base URL で分岐させること**（「URL から provider を逆引きする経路を
   足すと最初に壊れる」）。`Provider::detect` — provider **未指定**の
   テンプレートの推定 — はこれに当たらない（Spec 31 / 34 / 37 で各ホストを
   足してきた前例どおり。rev2 で精密化）
2. **出典の decode** — `search_results` ほかは Perplexity 方言の output item。
   相乗りのままでは `dropped content blocks` へ落ち続け、画面は
   「参照元は返ってきていません」のまま
3. **carries が本家と違う**（実測 10）— 相乗りは PDF を送って必ず 400
4. **`usage.cost`** — 単価表を当てずに実額が取れる唯一の口。ワイヤが
   分かれていれば将来 1 欄で写せる（本 Spec では D10 = 範囲外）

xAI（Spec 31）/ Meta（Spec 37）に続く**ベンダー方言 Responses の 3 例目**で、
前例の形にそのまま乗る。

### D2: 要求のトップレベル型は共有し、`max_steps: Option<u32>` を加算する

**rev1 の「`max_steps` で割れるからトップレベルを分ける」は撤回**（査読 A-4）。
`Option` + `skip_serializing_if` なら **`None` のとき欄ごとワイヤに出ない**ので、
共有しても OpenAI / Perplexity 双方の golden はバイト等価のまま —
「割れる」は成立していなかった。分けているのは `Provider`（= decode と
能力の判定）であって、要求の構造体を 2 つに分ける必要は無い。

- OpenAI 経路では常に `None`（既存 golden が 1 バイトも動かないことを
  変更後の緑で証明する — 加算の機械証明。Spec 35 の手順）
- Perplexity 経路では D4 の規則で `Some(5)` / `None` を出し分ける

**input（メッセージ列 → `ResponsesInputItem`）の encode は Spec 37 の
関数ポインタ形（`encode(messages, part_for)`）で共有する。** `part_for` は
carries 表を読まない（読ませると `adapters_match_the_carries_table` が
同語反復になる — Spec 37 の網の設計をそのまま維持）。

応答側の型（`Responses*`）は Spec 34 で共有と実測済み。Perplexity 固有の
output item（`search_results` / `finance_results` / `people_search_results` /
`fetch_url_results` / `skill_loaded`）の variant をどこに足すか
（共有 enum へ加算か、xAI のように専用型か）は **P1 で既存の共有度を
数えてから決め、P1 実装記録に書く**（P0 との順序は Phases の節 — 査読 A-3）。

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
  経路になるので作らない（既定 OFF の規律と同じ向き）。
  **ただしサイレントな機能喪失にはしない** — ダイアログで provider を
  `perplexity_responses` へ切り替えたとき `openai_web_search` が ON なら、
  その場に注意文を 1 行出す（P2。**表示条件だけで書けるので「1 回だけ
  警告」のような状態の記憶は持たない** — 査読 A の軽微 i を形を変えて採用）
- 既定は 4 本とも **OFF**（検索は入力を桁で膨らませる — Spec 37 の実測。
  finance / people は 1 回 $0.005 の呼び出し課金も持つ）
- `people_search` の**パラメータ（`max_tokens` / `max_tokens_per_page`）と
  `fetch_url` の `max_urls` は送らない** — 送れる形が無いほうが強い（#77）。
  必要が実測で出たら欄を足す

### D4: `max_steps` は finance が ON のときだけ `Some(5)`

- **必要なのは `finance_search` だけ**（実測 1〜3。finance は `skill_loaded` を
  挟む skill 族で、step 予算が無いと内部ツール `finance_quotes` を呼べない）
- 送らないと **200 のまま黙って空振りする**（エラーにならない最悪の形）ので、
  フラグと欄はコアが対で管理する。利用者に `max_steps` を見せない
- 値は **5**（文書の推奨帯 5〜10 の下限。step は課金と遅延を持つ）。
  コード定数で設定にしない（Spec 11 の重みと同じ扱い）
- 他のツールだけのときは `None` — probe で送っても無害と実測済みだが、
  要らない欄を送らないほうが 400 の面が狭い（欄の存在は D2 のとおり
  型を割る理由にはならない。**出し分けの根拠は面の狭さだけ**）

### D5: 出典は `GroundingEngine::Perplexity` の 1 値で写す

CLAUDE.md が既に指している形（「写すなら `GroundingEngine` に 1 値
（Spec 31 の形）」）。写す元は 4 種:

| output item | 出典の在り処 | 形 |
|---|---|---|
| `search_results` | `results[{url, title, snippet, date?, last_updated?}]` | オブジェクト配列 |
| `people_search_results` | `results[{url, title, snippet, source}]` | オブジェクト配列 |
| `fetch_url_results` | `contents[{url, title, snippet}]` | オブジェクト配列 |
| `finance_results` | `results[].sources` | **URL 文字列の二重配列。平坦化が要り、`title` / `snippet` は存在しない**（実測 8。査読 B-4） |

- **4 種の形は 2 族に割れる** — 上 3 つは url + title 持ちのオブジェクト、
  finance だけ URL の裸の文字列。写し先の `Grounding` は title 無しの
  source を許す構造か **P1 の冒頭で現物を確認**し、許さないなら
  finance の title は空で埋めずに URL をそのまま表示名に使う
  （**捏造しない** — title が無いことを無いまま見せる）
- Spec 05 の規律（検索した事実と出典が返らない事実を分ける）はそのまま —
  ツールが走って出典 0 件なら 0 件と出す
- `skill_loaded` は出典を持たない（`{name, type}` だけ）。**捨てるが数える**
  （#72 — `dropped` の計器に載せる。無言で握り潰さない）

### D6: 計器は `pplx tools:` の 1 行（rev2 で全面改稿 — 査読 A-6 / B-2）

前例（`xai search:` / `meta search:` — 行名は engine ごとに分ける）に乗る。

```text
pplx tools: finance=N people=N fetch=N web=N sources=N invocations=k:v,... tool_cost_usd=X.XXXXXX
```

- **`finance=` / `people=` / `fetch=` / `web=` の N は output item の数**
  （`finance_results` / `people_search_results` / `fetch_url_results` /
  `search_results` を decode が数える）。**`usage` の鍵名から数えない** —
  鍵名は tool 型と揃わない例が実在し（`search_people`・実測 6）、
  web_search が detail に現れるかも未実測。decode が確実に持っている
  item 数のほうが、他所の命名規約に依存しない
- **`invocations=` は `usage.tool_calls_details` の鍵と `invocation` を
  `鍵:回数` でそのまま列挙**（生の写し。鍵名を enum で固定しないので、
  `search_people` が将来 `people_search` に直っても、未知のツールが
  増えても、**サイレント欠損にならない** — 査読 A-6c の恒久形）。
  item 数と invocations の食い違いは、そのままこの 1 行から読める
- **`tool_cost_usd=` は `usage.cost.tool_calls_cost`**（ツール課金だけ。
  rev1 の `total_cost` は入出力トークン込みの合計で、行名 `pplx tools:` の
  下に置くと誤読する — 査読 A-6a。総額は D10 = 統計の層の話）
- **`sources=` は出典として写した URL の総数**（finance の
  `results[].sources` は**平坦化してから数える** — 査読 A-6b）
- 値だけで本文は書かない（#71）

### D7: carries は画像のみ ✓

| ワイヤ | 画像 | 音声 | 動画 | PDF |
|---|---|---|---|---|
| **Perplexity Responses** | ✓ | ✗ | ✗ | ✗ |

実測 9〜10（画像は内容照合済み・他 3 種は型ごと名指し拒否）。
`Provider::carries` は provider ごと 1 腕の配列リテラル（Spec 37 の形 —
variant を足すとコンパイラが実際に指すことを確認済みの書き方）。
`tests/carries_table.rs` の逐語凍結と TS 側 `carriesTable.test.ts` の
突き合わせに 1 行ずつ足す。**凍結コメントに「画像はワイヤが運べるが
`deepseek-v4-flash` はモデル層で拒否する（実測 2026-08-27）」を残す**
（D11 と対 — 査読 A-5）。

### D8: base URL の既定と検出。免除は `ALSO_SERVES_RESPONSES` を新設して受ける（rev2 で改稿）

- `DEFAULT_BASE_URL.perplexity_responses = "https://api.perplexity.ai/v1"`
- `Provider::detect` に `api.perplexity.ai` を足す（provider **未指定**の
  テンプレートの推定。D1 の精密化のとおり禁じ手には当たらない。
  なお未指定 + api.perplexity.ai は現状 `OpenAiCompat` へ落ちて
  404 の `/chat/completions` を叩く**既に壊れている構成**なので、
  detect の追加は直す向きにしか働かない）
- **`ALSO_SERVES_COMPAT` には入れない** — あの表は「Chat Completions の口も
  持つ他社ホスト」の免除で、Perplexity はその口を持たない（404 実測）。
  **rev1 の「`baseUrlMismatch` が指摘してよい」は撤回**（査読 A-1 / B-1）—
  既存の相乗り（`open_ai_responses` + api.perplexity.ai）は **2026-08-19 から
  現に動いている正当な構成**で、そこに警告を出すのは誤検知
  （`providerSkills.ts` の doc 自身が「これを持たない実装は誤検知する」と
  書いている形そのもの）。矛盾の実体は「免除の表が `open_ai_compat` にしか
  無い」ことなので、**対になる表 `ALSO_SERVES_RESPONSES`（OpenAI Responses の
  口も持つ他社ホスト）を新設**し、`provider === "open_ai_responses"` かつ
  この表に載る baseUrl を `baseUrlMismatch` の免除にする。初期値は
  api.perplexity.ai の 1 件（`/v1/responses` エイリアスの実在は実測済み）。
  これで検収 6 が**警告ゼロ**の形で成立する

### D9: TS 側の手数え（rev2 でファイル名列挙 — 査読 A の軽微 ii）

Spec 37 の教訓（union と `Record` で網羅性が割れる）をそのまま適用。
コンパイラが指すのは `carries.ts` の `Record<Provider,…>` だけ。手で数えるのは:

| ファイル | 箇所 |
|---|---|
| `lib/providerSkills.ts` | `DEFAULT_BASE_URL`（足す）/ `ALSO_SERVES_COMPAT`（**足さない**と決めた箇所として数える）/ **`ALSO_SERVES_RESPONSES`（新設）** / スキルの判定（`visibility` の呼び出し群）/ `anyOffered` の合成 |
| `components/ModelTemplateDialog.vue` | プロトコルの選択肢 / 固有スキル欄のトグル 4 本 / D3 の切り替え注意文 |
| `lib/carries.ts` | `Record<Provider,…>`（コンパイラが指す側） |

**`anyOffered` が最も静か**（足さなくても型は通り、トグルは描かれるのに
見出しの区切りだけが消える）— P2 の検査で名指しする。

### D10: `usage.cost` の統計への写しは範囲外

実額が取れる唯一の口だが、統計画面への写しは Spec 41 の横に 1 欄を足す
別の話（推定 `≈ $` と実額の 2 列をどう並べるかという画面の未決を持つ）。
本 Spec は D6 の計器でログに残すところまで。**捨てはしない・写しもしない。**

### D11: モデル層の拒否には何もしない。ただし既知の拒否は書き残す（rev2 で但し書き — 査読 A-5）

`deepseek-v4-flash` は画像をモデル層で拒む（実測 9）。「ワイヤが運べるか」と
「モデルが受理するか」は別の層（Spec 36 で凍結済みの区別）で、carries の ✓ は
ワイヤ能力として正しく、モデル拒否は既存の 400 処理のまま（`fatal` なら
個体が落ち、`agent reaped:` → ON で戻る経路 — Meta の `effort=max` と同じ形）。
**機構は足さないが、沈黙にはしない**:

- D7 のとおり `carries_table.rs` の凍結コメントに既知の拒否を明記
- P3 で README の固有スキル説明に「画像はモデル依存（`deepseek-v4-flash` は
  受けない — 実測）」の 1 行

これは「トグルを ON にすると落ちる」（D1 が塞いだ形）とは別 — 添付 ×
モデル選択の組は Gemini ネイティブ以外の全ワイヤに元からある構造で、
本 Spec で新しく開く面ではない。

## Phases

- **P0: 契約の凍結（確定している範囲だけ — 査読 A-3）** —
  `data_contract.yaml` へ `perplexity_tools_contract`: D1 / D3（4 フラグと
  述語）/ D4（`max_steps` の規則と定数 5）/ D6（計器の書式と数え方の定義）/
  D7（carries の行）/ D8（`ALSO_SERVES_RESPONSES` の新設と免除規則）/
  D10 / D11。`settings_contract` ほか関連節へ続報。
  **output item の型の置き場（D2 末尾）はここでは凍結しない**
- **P1: コア** — `Provider::PerplexityResponses` + `perplexity_responses.rs`
  （encode 共有 + `max_steps: Option<u32>` 加算 + decode + Grounding + 計器）+
  `ModelTemplate` の 4 フラグと `*_active()` 述語 + carries（Rust 側の表と
  逐語凍結・既知のモデル拒否コメント）。golden は 3 本 — OpenAI 側が
  1 バイトも動かない（加算の証明）/ finance ON で `max_steps:5` が出る /
  finance OFF で欄ごと出ない。ミューテーションで赤確認。
  **Tasks に含める: output item の型の置き場を決め、`data_contract` へ
  追補する**（「次の Phase で書く」は Tasks に書かないと落ちる —
  Spec 28 P4 の教訓）。`Grounding` が title 無し source を許すかの現物確認
  （D5）もここ
- **P2: フロント** — D9 の表の全箇所 + `ModelTemplateDialog` の固有スキル
  欄（トグル 4 本。課金の注意書き — finance / people は 1 回 $0.005）+
  D3 の切り替え注意文 + 辞書 ja/en + `carriesTable.test.ts` /
  `providerSkills.test.ts`（**`anyOffered` を名指しで検査**）
- **P3: 台帳** — README 日英（対応プロバイダ表 / 固有スキル。people_search が
  何を検索するツールかの 1 行と、画像のモデル依存の 1 行を含む）/
  DETAIL 日英（ワイヤ層 6 → 7 本）/ CLAUDE.md「Perplexity を繋いだ」節へ
  続報（相乗りから 7 本目へ・`search_results` の宿題を回収）
- **P4: 実機検収**

## 検収項目（各項目に到達経路を書く — Spec 43/44 の教訓）

1. **finance**: `perplexity_responses` + `perplexity_finance_search` ON の
   個体に金融の質問（例「AAPL の今日の株価は」）→ 答えに実データが入り、
   出典表示に `perplexity.ai/finance/…` が出て、`fuseforks.log` に
   `pplx tools: finance=1 … tool_cost_usd=` の行が出る。
   到達経路: ツールを提示すればモデルが選ぶ（probe 1 で同型の質問に発火済み）
2. **finance の空振りが存在しないこと**: 同じ個体・同じ質問で
   `skill_loaded` だけ返って答えが「取得できません」になる形が**出ない**
   （D4 の `max_steps` 自動付与が効いている証拠。probe 1 の空振りが対照）
3. **people**: `perplexity_people_search` ON の個体に人物の質問 →
   `pplx tools: people=1` かつ **`invocations=search_people:1`**。
   前者は item 数・後者は `usage` の生の鍵で、**2 つが別の経路から
   同じ 1 回を指す**ことがこの行 1 本で読める（D6 の二重化の検収）
4. **fetch_url**: ON の個体に URL 入りの依頼 → `pplx tools: fetch=1` と
   本文にそのページ由来の内容
5. **OFF の対照**: 4 本とも OFF の同じ個体に同じ質問 → `pplx tools:` 行が
   出ない・ツール課金 0（提示していないものは呼ばれない）
6. **相乗りの回帰**: 既存の `open_ai_responses` + api.perplexity.ai の
   テンプレートが**警告ゼロのまま動き続ける** — `baseUrlMismatch` の警告文が
   ダイアログに**出ず**（D8 の `ALSO_SERVES_RESPONSES` が効いている証拠。
   rev2 で「警告ゼロ」を明文化 — D8 を新設しない実装ではここが赤になる）、
   普通の依頼が完走する
7. **出典の回収**: web 検索（`perplexity_web_search` ON）で出典が
   「参照元は返ってきていません」ではなく実 URL の一覧になる
   （2026-08-19 の観察の裏返し。`search_results` が Grounding へ写った証拠）

## P1 実装記録（2026-08-27）

- **着地**: `Provider::PerplexityResponses`（7 値目）+
  `llm/perplexity_responses.rs` 新設（encode / decode / `Tools` 構造体 /
  golden 3 本 + decode fixture 1 本 + part 1 本）+ `ModelTemplate` の
  4 フラグと `*_active()` 4 本 + carries の 1 腕と凍結表 + TS の網の維持
  （`types.ts` の union / `carries.ts` の `Record` /
  `carriesTable.test.ts` のマス数 24 → 28 と variant 写像）
- **output item の型の置き場（D2 末尾の宿題）**: 共有 `ResponsesOutputItem`
  へ Option 欄を加算（`queries` / `results` / `contents`）。専用型に
  しなかったのは、あの構造体が enum ではなく**全欄 Option + kind 文字列
  分岐**で、xAI 専用の `input` 欄が既に同じ形で載っている前例のため。
  `data_contract` の `perplexity_responses` へ追補済み
- **`encode` の引数は bool 4 連ではなく `Tools` 構造体** — 同型 bool の並びは
  呼び出し側で取り違えてもコンパイラが指さない
- **`usage.tool_calls_details` は `BTreeMap`** — HashMap だと `invocations=`
  の列挙順が走行ごとに揺れ、同じ応答のログが毎回違う
  （`probe_approvals` の保存順を固定したのと同じ理由）
- **凍結の網が 2 つ、意図どおり鳴った**: `ipc_contract` の
  `wire_field_sets_are_frozen`（`ModelTemplate` の欄が増えた = 期待値へ
  4 欄を足して受けた）/ `carriesTable.test.ts`（Rust の凍結表に 7 行目が
  増えた = TS の表・variant 写像・マス数を追従）。**後者の variant 写像
  （`VARIANT_TO_PROVIDER`）は D9 の手数え表に無かった 6 箇所目** —
  P2 で表へ足す
- **ミューテーション 2 回、予測どおりの赤**: (a) `max_steps` の対を
  `None` 固定 → finance ON の golden **1 本だけ**赤（OFF 側 golden は緑 =
  2 本が別々の仕事をしている）(b) carries の PDF を ✓ → 逐語凍結と
  adapter 一致の **2 本**が赤（両方を同時に変えると通る穴を逐語凍結が
  塞いでいる形がそのまま出た）
- **golden の `base_request` は `tool_choice: Auto` へ上げる必要があった** —
  `ChatRequest::plain` の既定は `ToolChoice::None`（ツールを一切送らない
  要求）で、そのままだとサーバー側ツールも `max_steps` も出ない。
  最初の golden 2 本はこれで赤になり、`openai_responses` の golden と
  同じ形へ直した
- **P2 へ送る宿題**: D9 の表 + `VARIANT_TO_PROVIDER`（6 箇所目）/
  `ModelTemplateDialog` のトグル 4 本と切り替え注意文 / 辞書 ja/en /
  `providerSkills.test.ts`（`anyOffered` の名指し検査）

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
5. **査読の反映記録（rev2・2026-08-27。2 系統 = A 6 点 + 軽微 2 / B 4 点。
   A-1≈B-1・A-6≈B-2 は同一指摘）**:
   - **A-1 / B-1（D8 × 検収 6）: 訂正して採用。** 矛盾は実在した（rev1 の
     まま実装すると、`perplexity_responses` の既定値が `KNOWN_DEFAULTS` に
     入った瞬間、既存相乗りテンプレートに `baseUrlMismatch` の警告が出る）。
     ただし提示された修正案 2 つ（deprecated 警告化 / 検収の書き換え）は
     どちらも採らない — 相乗りは deprecated ではなく**正当な現用構成**
     （B-1 の「互換の定義の混同」の指摘が実体を言い当てている:
     `ALSO_SERVES_COMPAT` は Chat Completions の口の免除で、相乗りが
     使っているのは Responses の口。**免除の表が `open_ai_compat` にしか
     無いことが矛盾の本体**）。処方は対になる表 `ALSO_SERVES_RESPONSES` の
     新設（D8）で、検収 6 は「警告ゼロ」へ強めた
   - **A-2（detect は逆引きの禁じ手では）: 採用。** D1 を精密化 — 禁じ手は
     能力・スキル・encode の分岐で、未指定テンプレートの推定（`detect`）は
     Spec 31/34/37 の前例どおり。detect 追加で挙動が変わる既存構成は
     「未指定 + api.perplexity.ai」= 404 を叩く既に壊れた構成だけで、
     直す向きにしか働かないことも D8 に明記
   - **A-3（P0 で凍結できない項がある）: 採用。** P0 を確定範囲に限定し、
     output item の型の置き場は **P1 の Tasks に「決めて data_contract へ
     追補」を明記**（Spec 28 P4 の教訓の適用）
   - **A-4（`max_steps` で割れるは崩れている）: 訂正して採用。**
     「割れる」は撤回し、共有型 + `Option` + `skip_serializing_if` へ（D2）。
     **ただし「D1 の 2 番目の根拠が弱くなる」は反証** — D1 の根拠 4 つに
     `max_steps` は入っていない（2 番目は出典の decode）。弱かったのは
     D2 の記述であって D1 の骨格ではない
   - **A-5（deepseek 個体が落ちる組み合わせ）: 採用。** D11 に但し書き +
     D7 の凍結コメント + P3 の README 1 行。機構は足さない（添付 ×
     モデル選択の 400 は全ワイヤに元からある構造で、本 Spec が開く面ではない）
   - **A-6 / B-2（計器の定義）: 採用（D6 を全面改稿）。** (a) `total_cost` →
     `tool_calls_cost`（`tool_cost_usd=`）(b) `sources=` は平坦化後の URL 数と
     定義 (c) 別名フォールバックは鍵の固定をやめて**生の列挙**（`invocations=`）
     へ — 将来 `search_people` が直っても未知ツールが増えても欠損しない。
     B-2 の指摘 2 点も同時に解消 — 回数の元は金額ではなく
     `usage.tool_calls_details` と item 数の二重化 / `web=N` は item 数なので
     `usage` に web の detail が無くても数えられる。B-2-1 の「パスの食い違い」は
     **両方実在する 2 つの欄**（`usage.tool_calls_details` = 回数 /
     `usage.cost.tool_calls_cost_details` = 金額）で、rev1 が混同していた —
     実測 5 / 6 に 2 欄あることを明記した
   - **B-3（Goal の 3 本 / 4 本の揺れ）: 採用。** Goal を「新設 3 + 移設 1 =
     4 本」に書き分けた
   - **B-4（finance の出典の型差）: 採用。** D5 に形の 2 族と平坦化・
     title 不在の扱い（捏造しない）を明記
   - **軽微 A-i（切り替え時のサイレント喪失）: 訂正して採用。**
     「1 回だけ警告」は状態の記憶が要るので採らず、ダイアログ内の
     表示条件（切り替え時に `openai_web_search` ON なら注意文）= 記憶なしの
     純表示へ（D3）
   - **軽微 A-ii（TS 5 箇所の数え方）: 採用。** D9 をファイル名の表へ
