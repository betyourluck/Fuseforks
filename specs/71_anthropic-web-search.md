# Spec 71: Anthropic の web 検索 — サーバー側ツールと、1 ターンの中で応答のブロックを送り返す欄

- 起票: 2026-10-25
- 状態: **Draft rev2**（2026-10-25。査読 10 点を実装と文書に当てて反映 — 採用 4 / 前提を訂正して採用 3 /
  一部採用 1 / 不採用 1 / 反証 1。表は Notes 2）→ **P0 完了**（同日。25 要求・検索 47 回・定価換算 約 $3.33。
  記録は「P0 実測記録」。**予測が 2 つ外れて設計が 2 か所動いた** — 保留を落としても検索は消えずモデルが出し直す
  （「覆す判断」の理由を書き直した）/ 保留 × `tool_choice: none` は 200 で検索まで走る（D5 の保守的な規則を外した）。
  `pause_turn` は 2 回とも再現せず。`data_contract` の `anthropic_server_tools` へ凍結）→ **未決 0**（同日、利用者が
  推しのまま裁定 — `max_uses` は定数 5 / web fetch は別の Spec）→ **P1 完了**（同日。コア。記録は「P1 実装記録」。
  単体 12 + 結合 3・コア 1,325 本全緑・ミューテーション 4 回とも予測どおり）
- 起点: 利用者が公式文書（`platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool`）を
  持ち込んだ → 公式文書 3 ページと Anthropic ワイヤの実装を突き合わせた（同日）→
  「Spec 71（Anthropic の web 検索）として起票して。P0 の probe 項目も Spec に入れて」

## rev1 からの変更（査読 10 点の反映。詳細は Notes 2）

- **最後の周で保留や停止が起きたときの扱いを決めた**（D4 / D5）— まとめ呼び出しへ保留を持ち込まない。
  rev1 は「P0 (g) で決める」としか書いておらず、assistant の発話で終わる要求が 5 世代では 400（prefill の
  禁止）になる点を数えていなかった
- **写しを作る条件に `pause_turn` を足した**（D3）— 止まった応答にサーバー側ツールのブロックが無い場合も、
  そのまま送り返す必要がある
- **キャッシュの境界を写しへ打つのは adapter だと書いた**（D3）— `place_message_breakpoint` は
  `anthropic::encode` の中にあり、写しはそこで生のブロックとして扱える
- **写しがあるとき `encode_message` は組み立てを丸ごと飛ばし、`tool_calls` から `tool_use` を二重に作らない**、
  を不変条件として書いた（D3）
- **`max_uses` の天井は 1 要求の天井で、1 ターンでは `max_uses × max_tool_iterations` だと書いた**（未決 1）
- 名前の衝突（D1）・計器の 2 つの数（D6）・Spec 68 で検出できない失敗（D7）・web fetch の規模（D8）・
  PRIVACY の読み直しの観点（D10）の文面を直した。P0 の (g) を (g1)（保留）と (g2)（停止）に割り、(c1) / (h) に `calls` と `requests` の差の記録を足した

## Goal

Anthropic ワイヤ（`Provider::Anthropic` = `/v1/messages`）に固有スキルのトグル
**`anthropicWebSearch`** を 1 本足す。ON の個体は Anthropic 側で web 検索を行い、本文の出典
（実 URL）がグラウンディングの注記に出る。xAI（Spec 31）/ OpenAI（Spec 34）/ Meta（Spec 37）/
Perplexity（Spec 45）/ Gemini（Spec 05・48）に続く 6 社目の検索の口。

**前提として 3 つを同じ Spec で塞ぐ**:

1. **検索が途中で止まる 2 つの形を、1 ターンの中で続けられるようにする。**
   (a) `stop_reason: "pause_turn"` — サーバー側の検索ループが長くなると止まり、assistant の
   content をそのまま送り返すと続く (b) 村のツール（`file` / `grep` / MCP など）と同じ周で
   検索を呼ぶと、**検索は実行されずに** `stop_reason: "tool_use"` で返り、次の要求で
   `server_tool_use` ブロックが送り返されたときに初めて走る。**どちらも、受け取ったブロックを
   次の要求に載せる欄が今の canonical に無い**（現況の節）
2. **decode が新しいブロックを読む** — `server_tool_use` / `web_search_tool_result` /
   本文の `citations`。今は 3 つとも `Other` として落ち、`dropped content blocks: kinds=unknown`
   の 1 行が出るだけ。検索の回数・結果の件数・引用・検索の失敗を計器 1 行へ
3. **固有スキルを外す関数 `ModelTemplate::without_provider_skills` に欄を足す** — Spec 63
   （AI で下書き）と Spec 68（接続テスト）がこの関数でテンプレートの固有スキルを外している。
   足し忘れると、接続テストと下書き補助の 1 回ごとに検索が走りうる

**やらないこと（範囲外）**: web fetch ツール（D8）/ `web_search_20260209` 以降の dynamic filtering
（D1）/ `allowed_domains`・`blocked_domains`・`user_location` / 検索料（1,000 検索 $10）を予算・
統計・金額へ入れること（D9）/ Anthropic ワイヤの思考段階（`output_config.effort`。Notes 1 の別件）/
ストリーミング（この村は非ストリーミングのみ）/ ターンをまたいで検索結果を持ち越すこと（D3）。

## 公式文書の要点（2026-10-25 に読んだ。web search tool / server tools / tool reference の 3 ページ）

| 項目 | 内容 |
|---|---|
| ツールの版 | `web_search_20250305`（基本）/ `web_search_20260209`（dynamic filtering を追加）/ `web_search_20260318`（`response_inclusion` を追加）。ベータヘッダーは不要 |
| 定義 | `{"type": "...", "name": "web_search", "max_uses"?, "allowed_domains"? \| "blocked_domains"?, "user_location"?, "allowed_callers"?}` |
| dynamic filtering | 20260209 以降は `allowed_callers` の既定が `["code_execution_20260120"]` になり、サーバーが内部でコード実行を走らせて結果を絞る。応答にコード実行のブロックが混ざり、入れ子の `server_tool_use` に `caller` が付く。`["direct"]` で切れる。ZDR の対象は基本版と `direct` のときだけ |
| 応答のブロック | `server_tool_use`（`id` は `srvtoolu_` 始まり・`input.query`）→ `web_search_tool_result`（`tool_use_id`・`content` は `web_search_result` の**配列**、各要素に `url` / `title` / `page_age` / `encrypted_content`）→ 本文の `text` に `citations`（`web_search_result_location`: `url` / `title` / `cited_text`（150 字まで）/ `encrypted_index`） |
| 引用のトークン | `cited_text` / `title` / `url` は入力にも出力にも数えない |
| 往復 | 検索結果を含む会話を続けるときは assistant の content を受け取ったまま送り返す。`encrypted_content` が欠けるか変わっていると 400 |
| 検索の失敗 | HTTP は 200。`web_search_tool_result.content` が配列ではなく**オブジェクト** `{"type": "web_search_tool_result_error", "error_code": ...}` になる。`error_code` は `too_many_requests` / `invalid_tool_input` / `max_uses_exceeded` / `query_too_long` / `request_too_large` / `unavailable` の 6 つ。失敗した検索は課金されない。一致 0 件は空の配列で、失敗ではない |
| `pause_turn` | 長い検索のターンで返る。続けるには止まった assistant の content を**そのまま**送り返し、同じ `tools` を付ける（待っているツールが `tools` に無いと検証エラー）。続けた先でまた止まりうる。上限 10 周という数は SDK 側の参照資料にあり、公式ページは数を書いていない |
| 村のツールと同じ周 | 検索は実行されず `stop_reason: "tool_use"`。`content` に結果の無い `server_tool_use` と村の `tool_use` が並ぶ。続けるには、その応答を assistant として、**`tool_result` ブロックだけ**の user の発話を続けて送る。次の応答の先頭に検索結果が来る。`tool_result` の後ろに本文などを足すと 400（「`web_search` tool use with id ... was found without a corresponding ... result block」） |
| 組織の設定 | 管理者が Console で無効にしていると、ツールを含む要求が 400 `invalid_request_error`（「web search is not enabled」の趣旨）。ドメイン制限も組織側で掛けられる |
| 費用 | 1,000 検索あたり $10 + 検索結果の入力トークン。公式の使い方（毎ターン content を送り返す）では、取った結果は**以後のターンでも**入力として課金される |
| 使用量 | `usage.server_tool_use.web_search_requests` |

## 現況（実装を数えた。2026-10-25）

- **`encode` の tools は関数ツールだけ** — `wire::AnthropicTool { name, description, input_schema }`
  で、`type` の欄が無い。サーバー側ツールの定義を組めない
- **`encode_message` は assistant の発話を組み直す** — 添付 + `text` + `tool_use` の順で
  canonical の `content` と `tool_calls` から作る（`anthropic.rs` の `encode_message`）。
  受け取ったブロックを残す経路が無い
- **`decode` は 5 種しか知らない** — `Text` / `ToolUse` / `Thinking` / `RedactedThinking` / `Other`
  （`#[serde(other)]`）。`server_tool_use` と `web_search_tool_result` は `Other` に落ちて
  `dropped content blocks: kinds=unknown` を出す。`text` の `citations` は欄が無いので serde が
  読み捨てる。`stop_reason: "pause_turn"` は `Finish::Other`
- **周回ループは `finish` を見ていない** — `turn.rs` は「ツール呼び出しが 0 本 = 最終出力」で
  抜ける。**`pause_turn` で止まった周の途中の本文は、そのまま最終回答として配られる**
- **assistant の発話は `ChatMessage::assistant_tool_calls(text, calls)` で積む** — 本文と
  呼び出しだけ。canonical の `ChatMessage` に不透明なブロックの席は無い（`ToolCall::extra` は
  呼び出し 1 本ごとの欄で、ブロックの並びは持てない）
- **`encode` は本文も呼び出しも添付も無い発話を落とす**（`failures.md` #29 の網）。検索の
  ブロックだけを持つ assistant の発話は、席を作っても今の述語では落ちる
- **思考のブロックは往復していない** — `Thinking` は要約として受け取り、送り返さない
  （Spec 33）。5 世代の個体には毎回 `thinking: {adaptive, summarized}` を送っている
- **前例はどれも「送り返さない」** — Responses 系 4 ワイヤは `reasoning` などの item を
  送り返さずに 200 で通っている（`data_contract` の `intentionally_unread`）。Gemini は
  `toolCall` / `toolResponse` を送り返さず、代わりにモデルが URL を取り直す（Spec 48 P0 (d2)。
  払いが同額なので席を作らないと判断）
- **村の個体**: `claude-opus-5-5` のテンプレートを 1 体が使っている。`claude-fable-5-1` と
  `claude-sonnet-5-5` のテンプレートは登録済みで未使用
- **要求の時間切れの既定は 120 秒**（`ModelTemplate.request_timeout_secs`）。検索のターンが
  何秒かかるかは測っていない

## 覆す判断

- **Spec 48 D3「`ChatMessage` に不透明な part の席を作らない」を、Anthropic ワイヤについて覆す。**
  Spec 48 の根拠は払いの比較（返せば取得本文が次の周の `prompt` に乗り、返さなければ取り直しの
  `toolUsePrompt` に乗る — どちらも同額）だった。**Anthropic では同額にならず、まとめの中身も変わる**
  （P0 で書き直した。rev1・rev2 の理由「落とすと検索そのものが走らない」は (c2) で反証された —
  保留を落とすと、モデルが続きの応答の中で検索を出し直した）:
  - **払い**: 落とすとモデルが検索をやり直す — 検索が終わった組を落とした (d1) は入力 11,673・検索 1 回、
    写しのまま (d2) は入力 9,609・検索 0 回。保留を落とした (c2) は 13,910、写しのまま (c1) は 10,901
  - **まとめの中身**: 写しのままのまとめ呼び出し (g3) は検索結果を引用して答え（出典 3 件）、組み直した
    まとめ (g3c) は出典なしでモデルの記憶から答えた（`failures.md` #82 の「答えられないことに気づけない」形）
  比べる土台が違うので、Gemini の凍結（`data_contract` の `gemini_native`）は変えない。
  席の寿命は 1 ターン（D3）で、`sessions.redb` にも滑る窓にも入らない

## Design

### D1: 版は `web_search_20250305`。tools の末尾に置く

```json
{"type": "web_search_20250305", "name": "web_search", "max_uses": N}
```

- **基本版を採る理由**: 全モデルで使える / ZDR の対象 / `allowed_callers` の既定が `["direct"]` /
  応答にコード実行のブロックが混ざらない。**モデル名で版を選ぶ門が要らない**（Spec 48 D2 の
  ような方言の吸収をしなくて済む）
- **20260209 以降を採らない理由**: dynamic filtering は検索結果を絞って入力トークンを減らす
  （Spec 59 の Jev による圧縮のプロバイダ版）が、応答にコード実行のブロックが入り、
  村のツールと同じ周では**`container` の ID まで送り返す**必要がある（programmatic tool calling の
  形）。D3 の席が固まってからの別の段。**P0 (j) の 1 回では、この問いについて費用でも負けた** —
  既定（dynamic filtering）は検索 3 回・入力 36,675・23 秒で、**本文に引用が 1 つも付かなかった**。
  基本版の (a) は検索 1 回・入力 11,943・9 秒・引用 5 件。`allowed_callers: ["direct"]` は基本版と同じ形
  （検索 2 回・入力 25,995・引用 6 件）。トークンを減らす機能が常に減らすとは限らない（1 回の観測）
- **置き場は tools の末尾**。並びを固定するのはキャッシュの前方一致のため（トグルを切り替えた
  1 回だけ tools の層が書き直しになる）
- **今の命名規則では名前は衝突しない** — 村のツール名は同梱の固定名 / 合成の `ask_*`
  `transfer_to_*` `plan` など / MCP は `mcp::qualified_tool_name` が**必ず** `{サーバー}__{ツール}` の
  接頭辞を付ける（MCP サーバーが素の `web_search` を出しても、村での名前は `x__web_search` になる）。
  それでも P1 の単体で「提示集合（MCP 由来を含む）に `web_search` という名前の関数ツールが居たら
  サーバー側ツールを送らず、`anthropic search:` ではなく 1 行の WARN を出す」を留める。
  **命名規則が変わったときの網**で、黙って 400 にしないため（rev2 で断定の文面を直した — 査読 5）。
  衝突したときの実際の応答は 400 `tools: Tool names must be unique.`（P0 (k)）
- `max_uses` の値は未決 1

### D2: 判定は AND 述語 1 本。モデル名の門は掛けない

`ModelTemplate.anthropic_web_search: bool`（`#[serde(default)]`。TS は `anthropicWebSearch`）。
判定は **`anthropic_web_search_active() = anthropic_web_search AND effective_provider() == Anthropic`**
（`gemini_url_context_active` と同型。フラグ単独を判定に使わない）。`LlmConfig` へ判定済みの値を
写し、`anthropic::encode(req, AnthropicSkills { web_search })` の値渡しで受ける
（Gemini の `GeminiSkills` と同じ形。将来 web fetch を足すときに bool を並べない）。

- 3 モデル（`claude-opus-5-5` / `claude-sonnet-5-5` / `claude-fable-5-1`）とも基本版で 200（P0 (a)(b)）。
  ブロックの並びはモデルで違う — Opus と Sonnet は `server_tool_use` → 結果 → 思考 → 本文、Fable は思考が先頭。
  写しは受け取った順のまま送るので、並びの違いは設計に影響しない
- base URL は見ない（他の固有スキルと同じ）。Anthropic 互換を名乗る別の接続先がサーバー側
  ツールを拒めば 400 になり、Spec 52 の分類どおり止まる
- **`without_provider_skills` に `anthropic_web_search: false` を足す**（Goal の 3）。足し忘れを
  単体で留める — 全部の `*_active()` が偽になることを列挙で検査する

### D3: 席は「応答のブロックの写し」1 つ。寿命は 1 ターン

- canonical に**不透明な写し** `replay: Option<serde_json::Value>` を 2 か所へ足す —
  `ChatResponse.replay`（adapter が decode で書く）と `ChatMessage.replay`（ターンの層が写す）。
  **中身を読むのは書いた adapter だけ**（`ToolCall::extra` と同じ規律）
- **`ChatMessage.replay` は `#[serde(skip)]`** — 保存にも golden にも出ない。履歴は今までどおり
  本文の文字列で積むので、**ターンをまたいで検索結果を持ち越さない**。公式の使い方では取った
  結果が以後のターンでも入力に乗るが、この村は乗らない（滑る窓 8 往復の再送を避ける、
  添付の D1 と同じ判断）
- **写しを作るのは、応答にサーバー側ツールのブロックが 1 つでも在るとき、または
  `stop_reason` が `pause_turn` のとき**（rev2 で後者を足した — 査読 4。文書は止まった応答が
  未実行の `server_tool_use` で終わること「が**ある**」と書いており、無い場合も「そのまま送り返す」
  ことに変わりはない）。それ以外は `None` で、encode は今のまま組み直す。
  **検索を使わない要求の送信バイトは 1 バイトも変わらない**（golden で留める）
- 写しが在る assistant の発話は、`encode_message` が**添付・本文・`tool_calls` からの組み立てを
  丸ごと飛ばし、写しのブロックだけを受け取った順に並べる**（rev2 で明記 — 査読 6。今の組み立ては
  「添付 → 本文 → `tool_use`」の固定順で、`server_tool_use` の位置を保てない）。
  **`tool_use` を二重に作らない** — 写しの中に `tool_use` ブロックが既にあり、canonical の
  `tool_calls` はターンの層（実行・RepeatGuard・計器）のために残るが encode は読まない。
  不変条件として「写しの中の `tool_use` の ID の集合 = `tool_calls` の ID の集合」を単体で留める
  （ずれると、続く `tool_result` が対を失って 400 になる）
- **#29 の網に 1 条件足す** — 写しを持つ発話は「空」ではない
- **写しに思考のブロックは入れない**（P0 (c3) で確定 — 思考を除いた写しで 200、検索結果が先頭に届いた）。
  今の村は思考のブロックを 1 度も送っていない。送り始めると、5 世代のモデルが思考のブロックの
  手前の履歴が変わっていないかを確かめる検査（preserved thinking）がターンの中に入ってくる
- 写しに入る本文のブロックには、**本文が 0 字で `citations` だけを持つもの**がある（P0 (e0) で 7 件）。
  写しはそのまま送るので害は無い。decode で本文を連結するときは 0 字を足すだけになる
- **キャッシュの境界を写しへ打つのは adapter**（rev2 で明記 — 査読 2）。`place_message_breakpoint` は
  `anthropic::encode` の中で、ターンの層が組んだ canonical をワイヤの発話へ写した**後**に呼ばれる
  （ターンの層は境界を知らない）。写しはワイヤの側で生のブロックの variant（`AnthropicRequestBlock::Raw`）
  になり、境界を打つ関数はそのオブジェクトへ `cache_control` の鍵を書き足す。**判定と注入が同じ関数に
  あるので、写しの側に「打ってよいか」の印を持たせない**。写しのブロック（`server_tool_use` /
  `web_search_tool_result`）にも `cache_control` を付けてよい（P0 (f) で 2 種とも 200。1h の書き込みが立った —
  `web_search_tool_result` に打つと 9,548、`server_tool_use` に打つと 2,977）。境界を下げる分岐は要らない

### D4: `pause_turn` は周を 1 つ足して続ける

- canonical の `Finish` に **`Paused`** を足す。作るのは Anthropic の decode だけで、他のワイヤは
  出さない
- 周回ループ: **`finish == Paused` かつツール呼び出しが 0 本**なら、その応答を写しつきの
  assistant の発話として積み、**利用者の発話を足さずに**次の周を回す（「続けて」を足すと
  文書の言うとおり別の発話として扱われる）。止まった周の本文は最終回答として配らない
- **続きの周は普通の周として数える** — `max_tool_iterations` が上限になり、新しい定数は作らない。
  予算の予約・打ち切りの検査・`cache:` 行も普通の周と同じに掛かる（払いが周ごとに見える）
- 5 世代のモデルは assistant の発話で終わる要求（prefill）を 400 で拒む。`pause_turn` の続きは
  文書が示す例外のはずだが、**`claude-opus-5-5` で実際に通るかは確かめられていない** —
  **P0 (e0) は 2 回とも `pause_turn` を再現しなかった**。15 社を 1 社ずつ検索させる依頼で、モデルは
  検索を 1 回の推論にまとめて並列に投げ（15 本 + 2 本 / 15 本）、サーバー側のループの回数が伸びなかった。
  **D4 は文書だけに接地している**。結合テストで続きの形（利用者の発話を足さない・写しのまま積む）を留め、
  実機で起きたら `paused=1` の行で読む
- **最後の周で止まった場合は、止まった応答を積まずにまとめ呼び出しへ進む**（rev2 で決めた — 査読 3）。
  まとめ呼び出しは `tools` を残したまま `tool_choice: none` と system の 1 文（「ツール実行の上限に
  達しました…」）を足して呼ぶ（`turn.rs`。`tools` を残すのは `failures.md` #36 の処方）。
  Anthropic ワイヤは system を最上位の `system` へ持ち上げるので、**止まった応答を積むと要求が
  assistant の発話で終わり**、5 世代では prefill の 400 になりうる。そこへ未実行の `server_tool_use` と
  `none` が重なる組み合わせは文書のどこにも書かれていない。**止まった周の検索料は払われるが、
  その結果はまとめに入らない**（ここまでの周の結果は入る）。上限の到達として扱い
  （`tool_limit_hit`。`ToolLimitReached` を出す）、`anthropic search:` の `paused=1` で後から数えられる。
  **P0 (g2) は `pause_turn` が再現しなかったので撃てていない** — この保守的な規則はそのまま残す
  - 査読 3 の前提「`none` を送ると `web_search` の定義が無くなる」は実装と違う — まとめ呼び出しは
    定義を残す。危険の実体は「assistant で終わる」と「保留 × `none`」の 2 つ

### D5: 村のツールと同じ周は、写しを送って `tool_result` だけを続ける

- 応答に結果の無い `server_tool_use` が在り `stop_reason: "tool_use"` のとき、写しつきの
  assistant の発話を積み、村のツールを実行して結果を積む（今の手順のまま）。次の応答の先頭に
  検索結果が来て、写しはその周の分に置き換わる
- **周の途中で `tool_result` 以外の user のブロックを積む経路が無いこと**を P1 の単体で留める。
  1 つでも足すと、文書の言う 400（結果の無いサーバー側ツール）になる。可変の文脈（広場ログ・
  入退室など。#45）はターンの最初の発話に畳まれるので、周の途中には入らない
- Anthropic ワイヤは `Role::Tool` を 1 発話 1 つの `tool_result` として user の発話に載せる
  （並列の呼び出しでは user の発話が続く。encode は同じ役割の発話を結合しない）。これで通ることは
  今の村の運用が示しているが、**検索の保留が挟まったときも通るか**は P0 (c1)
- **最後の周で保留が残っても、まとめ呼び出しは写しのまま送る**（P0 で確定。rev2 は「組み直す」としていた）。
  (g1) で、保留の写し + `tool_result` + `tool_choice: none` は 200 で、**保留の検索が実行され**、その結果を
  引用して答えた（出典 5 件・検索 1 回）。検索の済んだ組を含むまとめの形 (g3。system に 1 文 + `none`) も
  200 で、検索結果を引用して答えた。組み直すと出典の無い答えになる（「覆す判断」の (g3c)）ので、
  組み直す理由が無い
- **`none` × 検索の定義 × 検索が要る問い × まだ結果が無い会話は、本文の無い応答になった**（P0 (g0)。
  `content: []`・出力 1 トークン。1 回の観測）。対照の 3 本 — 検索の定義なしの同じ問い / 検索の定義ありで
  検索の要らない問い / ツールなしの同じ問い — はどれも普通に答えた。まとめ呼び出しは周を回した後に呼ぶので
  この形にはなりにくいが、なったときは既存の「まとめが空」の処理（`summary_error` と最終文言）に落ちる。
  機構は足さない（頻度を見てから）

### D6: decode — 新しいブロックを読み、計器 1 行

- `server_tool_use`（`name == "web_search"` の `input.query`）→ `Grounding.queries`
- 本文の `citations`（`web_search_result_location`）→ `Grounding.sources`（`url` で重複を落とす。
  `title` が空なら `url`）。**出典は引用されたものだけ**で、検索結果の一覧（`web_search_result`）は
  件数だけ数える — 他の 5 社と同じく「答えの根拠として示されたもの」を注記に出す
- `GroundingEngine` に **`Anthropic`** を足す（ワイヤ値 `anthropic`）。TS の辞書の鍵は
  `groundingEngineKeys.test.ts` が Rust の列挙と突き合わせる（`failures.md` #141）
- `web_search_tool_result.content` は**配列とオブジェクトの両方を受ける**型にする。オブジェクトなら
  `error_code` を数え、ターンは止めない
- 既知のブロックは `dropped` に入れない。`dropped content blocks:` に `unknown` が出るのは本当に
  未知のブロックだけにする
- 計器（`xai search:` / `openai search:` / `meta search:` と同じく行名を engine ごとに分ける）:

  ```text
  anthropic search: calls=2 results=10 cited=3 requests=2 errors=- deferred=0 paused=0
  ```

  `calls` = `server_tool_use` の数 / `results` = 検索結果の件数 / `cited` = 引用の件数 /
  `requests` = `usage.server_tool_use.web_search_requests` / `errors` = `error_code` の列挙 /
  `deferred` = 結果の無い `server_tool_use` の数 / `paused` = `pause_turn` なら 1。
  **検索を含まない応答では出さない**。本文と検索語は書かない（#71）
- **`calls` と `requests` は 1 対 1 にならない**ので両方持つ（rev2 で理由を書いた — 査読の追加 3 は反証）。
  ずれる形が構造として 2 つある — (1) **保留**: 村のツールと同じ周の応答は `calls=1 requests=0`
  （まだ実行していない）、次の応答は検索結果が先頭に来るが `server_tool_use` は繰り返されないので
  `calls=0 requests=1` (2) **失敗**: 失敗した検索は課金されない（文書）ので、`max_uses_exceeded` などは
  `calls` に数えて `requests` に数えない。**P0 で 2 つとも実物を見た** — (c0) は `server_tool_use` 1 本で
  `usage` に `server_tool_use` の欄ごと無く、(c1) は `server_tool_use` 0 本で `web_search_requests: 1`。
  (h) は `max_uses: 1` で 2 本を並列に出し、2 本目が `max_uses_exceeded`・`web_search_requests: 1`
- **`usage.server_tool_use` は検索を実行しなかった応答では欄ごと無い**（(c0) / (d2) / (g3)）。無ければ 0 と読む。
  欄には `web_fetch_requests` も並ぶ（web fetch を使わなければ 0）
- 引用の `cited_text` は **0 字のことがある**（(h) / (c3) / (e0) で計 5 件）。出典は `url` と `title` だけを写すので
  影響しない。150 字を超える引用は末尾が `...` の 153 字で返る
- **`agent=` は無い** — decode 層は個体を知らない（`gemini tools:` / `pplx tools:` と同じ制約）。
  突き合わせは直後の `turn:` 行の時刻で行う

### D7: 失敗の形

- **組織で無効** → 400 `invalid_request_error`。Spec 52 の分類では止める側で、今の個体は
  400 で停止状態になる（Spec 37 P5 で `effort=max` の 400 が個体を降ろしたのと同じ）。
  文面は接続先の本文がそのまま出る。**新しい分類は作らない** — トグルを切れば直る失敗で、
  利用者の次の手は 1 つしかない。実際の文面は未実測（組織の設定を変える操作は利用者の側。P0 (i)）
- **接続テスト（Spec 68）はこの失敗を検出できない** — 接続テストは `without_provider_skills` で固有スキルを
  外して撃つ（Spec 68 の裁定）ので、検索を拒む接続先（組織で無効 / サーバー側ツールを持たない
  Anthropic 互換の中継）でも「応答した」になる。**検索を外して送り直す仕組みは作らない**（rev2 で
  不採用の理由を書いた — 査読の追加 1）: (a) 利用者が ON にした機能を黙って外すことになり、
  トグルの意味が消える (b) どの 400 が検索のせいかを見分けるには接続先の本文の文字列で照合する
  ことになり、照合の語を並べる検査は接続先が文面を変えた日に外れる。リリースノートの
  「利用者が負う条件」に「接続テストは固有スキルを確かめない」を入れる
- **検索の失敗**（200 の中のオブジェクト）→ D6 の計器に出し、ターンは続ける。モデルは結果の
  無い検索として扱う

### D8: web fetch は範囲外

web fetch ツール（`web_fetch_20250910` など。会話に既に出ている URL だけを取得する）は、Gemini の
URL context（Spec 48）や Perplexity の `fetch_url`（Spec 45）に当たる。**同じ `server_tool_use` の
形で返り、写し（D3）・続き（D4 / D5）・計器の行は共有できる**。足すときに要るのは 3 つ —
トグル 1 本（`ModelTemplate` の欄と `AnthropicSkills` の欄）/ ツール定義（`web_fetch_…` の版・
`max_uses`・`max_content_tokens`・`citations` の選択）/ decode の 1 種（`web_fetch_tool_result`。中身は
`document` ブロック）。rev1 の「トグル 1 本と decode の 1 種で済む」はツール定義を数えていなかった
（rev2 — 査読の追加 2）。需要が出てから起票する（未決 2）。

### D9: 費用

- **検索料（1,000 検索 $10）は予算の外** — 予算はトークン建てで、ドル建ての料金を数えない。
  xAI の `ticks` / Perplexity の `tool_cost_usd` と同じ扱いで、計器の `requests=` から後で数えられる。
  統計・金額表示（Spec 39 / 41）への写しは範囲外。リリースノートの「利用者が負う条件」に入れる
- **検索結果のトークンは `input_tokens` に入る**ので、予算と統計には今の経路で乗る。**量は 1 検索で
  約 10,000〜12,000 トークン**（P0 (a) — 短い問い 1 つに検索 1 回で入力 11,943。(c0) の検索前の入力は 2,930）。
  **`max_uses` を送らないと 1 要求で桁が変わる** — (e0) は 15 社の依頼で検索 17 回・入力 236,999・55 秒・
  定価換算 $1.23 の 1 要求になった（未決 1 の材料）
- ターンの中では写しが周ごとに送り直されるので、検索結果は周ごとに入力に乗る。最後の発話に打つ
  キャッシュの境界が効けば、2 周目以降はキャッシュ読み（0.1×）で乗る
- ターンをまたいでは持ち越さない（D3）ので、公式の使い方より安い

### D10: PRIVACY

`PRIVACY.md` 4-2 は「利用者が該当する機能を有効にした場合、モデルの提供事業者が代行して web 検索や
X の検索を行います。検索語は提供事業者へ送信されます」と事業者を名指しせずに書いてあり、
Anthropic は既に LLM の接続先。**送り先は増えない**ので改訂しない見込み。P3 で日英を読み直すときに
見るのは 1 点 — **4-2 の「モデルの提供事業者」に Anthropic が含まれると読めるか**（他社の検索の固有スキルと
同じ読み方ができるか）。読めなければ 4-2 を改訂する（rev2 で観点を書いた — 査読 7）。

### D11: 画面

`types.ts`（`ModelTemplate.anthropicWebSearch`）/ `providerSkills.ts`（`SkillKey` に足す・
`providerSkills()` の `visibility(draft.anthropicWebSearch, onAnthropic)`・**`anyOffered` に足す** —
足さなくても型は通り、トグルは描かれるのに見出しの区切りだけが消える。Anthropic は今パッシブの
バッジ（プロンプトキャッシュ）だけなので、見出しの描き方が変わる）/ `ModelTemplateDialog.vue`
（チェックボックス・`draft` の初期化・`strandedRows` の 1 行）/ `locales/{ja,en}.json` /
`tests/ipc_contract.rs` の `wire_field_sets_are_frozen`。stranded の行は持ち主のワイヤ名を併記する
既存の形（2026-08-30）に乗る。**他社の web 検索のフラグからの自動の写しは作らない**
（Spec 45 と同じ — 切り替えただけで課金の面が開く経路にしない）。

## 採らなかった形

- **写しを作らず、ブロックを落としたままにする**（前例どおり）— `pause_turn` は途中の本文が最終回答に
  なり、同じ周の保留は検索が走らない。どちらも画面からは「検索したのに答えが薄い」としか見えず、
  沈黙を作らない規律に反する。P0 (c2) / (d1) で落としたときの実際の挙動は記録する
- **`LlmBackend::chat` の中で `pause_turn` を続ける**（周回ループを触らない）— 周 1 つぶんの払いが
  `cache:` 行にも予算の予約にも現れず、打ち切りの検査も挟まらない。払った呼び出しは周として見せる
- **dynamic filtering の版を最初から採る** — D1
- **検索結果の一覧（`web_search_result`）を出典にする** — D6。取ったものと根拠にしたものを混ぜない

## Tasks

### P0 — 測ってから凍結する（**完了 2026-10-25**。下の「P0 実測記録」）

鍵は利用者の環境変数 `ANTHROPIC_API_KEY`（値はスクリプトにもログにも出さない）。
**→ 実際は環境変数の鍵が 401（`invalid x-api-key`）だったので、利用者の裁定で、村のテンプレート
`claude-opus-5-5` として資格情報ストアに登録済みの鍵を probe のスクリプトが直接読んだ**（値は表示も保存もしていない）。スクリプトは
scratchpad に置いて使い捨て、数字だけをこの Spec の「P0 実測記録」に残す（Spec 34 / 48 と同じ扱い）。
**撃つ前に予測を表に書く。** 費用の見積もりは 20〜30 要求・検索 60 回前後で **3 ドル前後**
（`claude-opus-5-5` の $4 / $20 と、1 要求 30,000 入力トークンを仮定した概算。未実測）。

| # | 要求 | 予測 | 決めること |
|---|---|---|---|
| (a) | `claude-opus-5-5` + `web_search_20250305` 単独。答えに検索が要る日本語の問い | 200。`server_tool_use` → `web_search_tool_result` → `citations` つきの `text`。`web_search_requests` ≥ 1 | ブロックの実物（欄・件数・`encrypted_content` の字数・`page_age` の形・`cited_text` の字数）/ `input_tokens` に検索結果が何トークン乗るか / 所要時間（既定の 120 秒に収まるか） |
| (b) | (a) と同じ要求を `claude-sonnet-5-5` / `claude-fable-5-1` で | 200 / 200 | D2 の「モデル名の門を掛けない」 |
| (c1) | 村のツール 1 本（例: 時刻を返す関数）と検索を同じ周で呼ばせる依頼 → 保留の応答を**そのまま**送り返し、`tool_result` だけを続ける | 1 回目 = `stop_reason: tool_use` + 結果の無い `server_tool_use`・`web_search_requests` 0。2 回目 = 先頭に `web_search_tool_result`・`server_tool_use` は繰り返されない・`web_search_requests` 1 | D5 の手順 / D6 の `calls` と `requests` のずれ / 保留の検索が 2 回目の `max_uses` に数えられるか。同じ周で呼ばせるには依頼の書き方を何度か変える必要がありうる（試した回数も記録する） |
| (c2) | (c1) の保留を、今の encode と同じく**本文 + `tool_use` だけ**に組み直して送る | 200 で検索が消える、または 400 | 「採らなかった形」の裏付け。200 なら 2 回目にモデルが検索を呼び直すかも見る |
| (c3) | (c1) の保留を、**思考のブロックだけ除いて**送る | 200 | D3 の「写しに思考を入れない」 |
| (d1) | 検索が終わった組（`server_tool_use` と結果）を含み、村のツールも呼んだ応答の次の周で、検索の組を**落として**送る | 200。モデルが検索を呼び直すことがある | 前例（Gemini の取り直し）と同じ形か。呼び直した回数と `requests` |
| (d2) | (d1) を写しのまま送る | 200。呼び直さない | (d1) との入力トークンの差 |
| (e1) | 検索を多く要する依頼（`max_uses` なし）で `pause_turn` を起こす → そのまま送り返す | `pause_turn` → 続きの応答 | D4 の続け方 / **5 世代の prefill 禁止に掛からないか** / 止まった応答の末尾が未実行の `server_tool_use` かどうか（D3 の条件）。数回で起きなければ「再現せず」と書き、D4 は文書だけに接地していると明記する |
| (e2) | (e1) を思考のブロックを除いて送る | 200 | D3 |
| (f) | 写しの最後のブロック（`server_tool_use` / `web_search_tool_result`）に `cache_control` を付ける | 200 | D3 のキャッシュの境界 |
| (g1) | 検索を含む `tools` で `tool_choice: none`。(c1) の保留の写し + `tool_result` の後で / 保留の無い会話で | 無い = 200・検索なし / 保留あり = 不明 | D5 の「最後の周で保留が残ったとき」— 200 なら写しのまま送る側へ緩める |
| (g2) | (e1) の止まった応答を写しのまま積み、system に 1 文足して `tool_choice: none` で送る（= まとめ呼び出しの形） | 400（prefill の禁止）か、不明 | D4 の「最後の周で止まったとき」— 200 なら積む側へ緩める |
| (h) | `max_uses: 1` で 2 回以上の検索を要する依頼 | 200 の中に `error_code: max_uses_exceeded`・`web_search_requests` 1（失敗した検索は数えない） | D6 の型（配列とオブジェクト）/ D6 の `calls` と `requests` のずれ / ターンが続くか |
| (i) | 組織で web 検索を無効にした状態で (a) | 400 | D7 の文面。**組織の設定を変えるのは利用者の操作**なので、撃つかは利用者が決める。撃たなければ「未実測」のまま |
| (j) | `web_search_20260209` を `allowed_callers` なしで / `["direct"]` で | なし = 応答にコード実行のブロック / direct = (a) と同じ形 | D1 の「基本版を採る」の裏付け（形の記録だけ） |
| (k) | 関数ツールに `web_search` という名前のものを並べて送る | 400 | D1 の衝突の検査を入れる理由 |

**凍結先**（`data_contract.yaml`）:
- `llm_wire` に **`anthropic_server_tools`** の節を新設（D1 の版と定義 / D3 の写しの規律と寿命 /
  D4 / D5 / D6 の decode と計器 / 送信バイトが変わる条件）
- `ModelTemplate` の欄の列挙へ `anthropicWebSearch`
- `GroundingEngine` の値へ `anthropic`
- `Finish` を列挙している箇所があれば `paused`（無ければ D4 の節に書く）
  **→ `data_contract` に `Finish` の列挙は無かった**ので、`anthropic_server_tools` の節に書いた

### P0 実測記録（2026-10-25。`claude-opus-5-5` 中心・非ストリーミング・村と同じヘッダと `thinking: {adaptive, summarized}`）

スクリプトは scratchpad に置いて使い捨て、数字はここが正。要求 25 本（401 の 1 本と 400 の 1 本を除く）・
検索 47 回・**定価換算 約 $3.33**（入力 $4 / 出力 $20 / 検索 $0.01。Sonnet と Fable はそれぞれの定価。見積もり 3 ドル前後）。

| # | 結果 | 予測 |
|---|---|---|
| (a) | 200・9.2 秒。ブロック = `server_tool_use`（`input.query`）→ `web_search_tool_result`（9 件。欄は `url` / `title` / `page_age` / `encrypted_content` / `type`。`page_age` は「36 days ago」の相対形、`encrypted_content` は 784〜2,868 字）→ 思考 → 本文 11 ブロック（引用 5 件・別の URL 4 つ。引用の欄は `url` / `title` / `cited_text` / `encrypted_index`（200 字）/ `type`、`cited_text` は 153 字 = 150 字 + `...`）。入力 11,943・出力 711・`web_search_requests` 1。応答の最上位に `container: null` が付く | 当たり |
| (b) | Sonnet 5.5 = 200・8.2 秒・入力 12,055 / Fable 5.1 = 200・11.6 秒・入力 13,008（思考が先頭） | 当たり |
| (c0) | 1 回目で保留を得た。ブロック = `tool_use`（村のツール）→ `server_tool_use`（結果なし）・`stop_reason: tool_use`・**`usage` に `server_tool_use` の欄ごと無い**・入力 2,930 | 当たり |
| (c1) | 写しのまま + `tool_result` だけ → 200。**先頭が `web_search_tool_result`**（保留の ID）で `server_tool_use` は繰り返されない。入力 10,901・`web_search_requests` 1 | 当たり |
| (c2) | 組み直し（本文 + `tool_use`、`server_tool_use` を落とす）→ **200。モデルが同じ検索語で `server_tool_use` を出し直した**。入力 13,910・`web_search_requests` 1 | **外れ**（「検索が消える、または 400」）→ 「覆す判断」の理由を書き直した |
| (c3) | 思考を除いた写し → 200。(c1) と同じ形・入力 10,901 | 当たり → D3 で確定 |
| (d0) | 1 回目で得た。`server_tool_use` → 結果 → `tool_use`（calc）・`stop_reason: tool_use`・入力 12,465 | — |
| (d1) | 検索の組を落とす → 200。**モデルが検索をやり直した**（検索 1 回・入力 11,673） | 当たり |
| (d2) | 写しのまま → 200。検索 0 回・入力 9,609 | 当たり |
| (e0) | **`pause_turn` は 2 回とも再現せず**（15 社の依頼）。1 回目 = 検索 17 回（15 本を並列で出した後に 2 本）・入力 236,999・出力 5,440・54.9 秒・`end_turn` / 2 回目 = 検索 15 回・入力 105,634・37.2 秒・`end_turn`。本文が 0 字で引用だけを持つブロックが 7 件 | 再現しない側に外れた → D4 は文書だけに接地 |
| (e1)(e2)(g2) | 撃てていない（`pause_turn` が起きなかった） | — |
| (f) | `cache_control` を `web_search_tool_result` に打つ → 200・1h の書き込み 9,548 / `server_tool_use` に打つ → 200・書き込み 2,977 | 当たり |
| (g0) | 保留なし × `none` × 検索の要る問い → **200 だが `content: []`・出力 1 トークン**。対照 3 本（検索の定義なし / 検索の要らない問い / ツールなし）はどれも普通に答えた | 予測外（D5 に記録） |
| (g1) | 保留の写し + `tool_result` + `none` → **200。保留の検索が実行され**、引用 5 件で答えた・`web_search_requests` 1 | 「不明」→ D5 の保守的な規則を外した |
| (g3) | まとめの形（写し + system に 1 文 + `none`）→ 200・引用 3 件 / (g3b) 組み直し + 検索の定義を外す → 200・引用 0 件 / (g3c) 組み直し + 定義は残す → 200・引用 0 件 | — |
| (h) | `max_uses: 1` → 2 本を並列に出し、2 本目が `{"type": "web_search_tool_result_error", "error_code": "max_uses_exceeded"}`・`web_search_requests` 1・`end_turn`。`cited_text` が 0 字の引用 1 件 | 当たり |
| (i) | 撃っていない（組織の設定は利用者の操作） | — |
| (j) | 既定 = コード実行のブロック（`server_tool_use` の `name: code_execution` / 入れ子の `server_tool_use` に `caller: {type: code_execution_20260120, tool_id}` / `code_execution_tool_result`）・検索 3 回・入力 36,675・23.3 秒・**引用 0 件** / `["direct"]` = 基本版と同じ形・検索 2 回・入力 25,995・引用 6 件 | 当たり（引用 0 件は予測外） |
| (k) | 関数ツール `web_search` と並べる → 400 `tools: Tool names must be unique.` | 当たり |

**踏んだもの**: 環境変数の鍵が古かった（上記）/ 端末の表示が cp932 で日本語の検索語が化けた
（保存した JSON は UTF-8 で正しい。数字の読み取りには影響なし）。

### P1 — コア（**完了 2026-10-25**。下の「P1 実装記録」）

`wire.rs`（サーバー側ツールの定義 / `ServerToolUse`・`WebSearchToolResult` のブロック / `text` の
`citations` / `usage.server_tool_use`）/ `anthropic.rs`（`encode(req, AnthropicSkills)` / 写しの
送り出し / decode / 計器）/ `canonical.rs`（`ChatResponse.replay`・`ChatMessage.replay`・
`Finish::Paused`・`GroundingEngine::Anthropic`）/ `orchestrator/turn.rs`（写しを積む 2 か所と
`Paused` の続き）/ `model.rs`（欄・`anthropic_web_search_active`・`without_provider_skills`）/
`client.rs`（`LlmConfig` と `from_template`）。

テスト（予定）:
- **golden**: トグル OFF・写しなしの要求は送信バイトが変わらない
- 単体: tools の末尾にサーバー側ツールが付く / 関数ツールに `web_search` が居たら付けない /
  `without_provider_skills` の後は全部の `*_active()` が偽 / decode が P0 の実物 fixture から
  queries・sources・計器の値を読む / 失敗のオブジェクトを読む / `pause_turn` が `Paused` で、
  サーバー側ツールのブロックが無くても写しが付く / 写しがある発話の encode に `tool_calls` 由来の
  `tool_use` が足されない（写しの中の `tool_use` の ID 集合 = `tool_calls` の ID 集合）/
  `place_message_breakpoint` が `Raw` のブロックへ `cache_control` を書き足す（P0 (f) が拒否を示したら、
  手前の普通のブロックへ下がる）
- 結合（スタブの HTTP で 2 周回す）: 保留の応答の次の要求に写しがそのまま載り、続く発話が
  `tool_result` だけ / `Paused` の次の周に利用者の発話が足されない / 写しが保存されない
  （`sessions.redb` の記録に出ない）/ **最後の周で止まったとき、まとめ呼び出しの要求が assistant の
  発話で終わらない** / **最後の周で保留が残ったとき、まとめ呼び出しの要求に写しがそのまま載る**
  （P0 (g1) で確定。rev2 の「載らない」から反転）
- **ミューテーション**（予測を先に書く）: 写しを使わず組み直す / `Paused` を `Other` に戻す /
  `without_provider_skills` から欄を外す / まとめ呼び出しに止まった応答を積む — それぞれ狙った 1 本だけ赤

### P1 実装記録（2026-10-25）

- **canonical**: `ChatMessage.replay`（`#[serde(skip)]`）+ `with_replay` / `ChatResponse.replay` /
  `Finish::Paused` / `GroundingEngine::Anthropic`。`ChatResponse` の構造体リテラル **119 か所**（テストを含む
  42 ファイル）へ `replay: None` を機械で足した — `-> ChatResponse {`（関数の戻り型の直後の本体）に当てない
  ように、直前の字が `->` の一致を除いた（構造体リテラルの置換が戻り型にも当たる罠。2026-09-27 の自己観察）
- **wire**: `AnthropicRequest.tools` を `Vec<AnthropicToolDef>`（タグなしの `Function` / `WebSearch`。関数ツールの
  送る形は 1 バイトも変わらない）/ `AnthropicRequestBlock::Raw(Value)`（その variant だけ `#[serde(untagged)]`）/
  応答の各ブロックを `AnthropicResponseBlock { block, raw }` にして、**型に読んだ形と生のオブジェクトの両方を
  持つ**（`Deserialize` を手で書いた。型に読み直してから組むと `encrypted_content` などの欄が落ちるため）/
  `ServerToolUse` / `WebSearchToolResult` / `Text.citations` / `AnthropicCitation` / `usage.server_tool_use`
- **anthropic.rs**: `encode(req, AnthropicSkills)`（値渡し。Gemini の `GeminiSkills` と同じ形）/
  `ANTHROPIC_WEB_SEARCH_MAX_USES = 5` / 名前の衝突は送らずに `anthropic web search skipped: reason=name_clash`
  の 1 行 / 写しがある発話は `encode_message` が組み立てを飛ばして写しのブロックだけを並べる / #29 の網に
  `replay.is_some()` / `place_message_breakpoint` が `Raw` へ `cache_control` を書き足す / decode は
  `SearchTally`（検索語・引用からの出典・計器の数・保留の判定）と写し（思考を除く）を返す / 計器
  `anthropic search: calls= results= cited= requests= errors= deferred= paused=`
- **turn.rs**: 呼び出しを積むときに写しを一緒に積む / `Finish::Paused` かつ呼び出し 0 本なら、最後の周でなければ
  写しつきの assistant を積んで `continue`、最後の周なら積まずに本文を空にし `ToolLimitReached` → まとめ呼び出し
- **model.rs / client.rs**: `anthropic_web_search` の欄 / `anthropic_web_search_active()` / `without_provider_skills` /
  `LlmConfig.anthropic_web_search` / `from_template`
- **template_probe.rs**: `ProbeFinish::of` に `Finish::Paused` の腕（`Other` へ畳む。接続テストは固有スキルを外して
  撃つので来ないはずの形 — `ProbeFinish` の値は増やさない）
- **P2 から前倒しした 3 行**: `types.ts` の `GroundingEngine` に `"anthropic"` / 辞書 ja・en の
  `grounding.engine.anthropic`。`groundingEngineKeys.test.ts` が Rust の列挙と辞書を突き合わせるので、P1 の時点で
  vitest を赤にしないため（`failures.md` #141 の網）。`ModelTemplate.anthropicWebSearch` の TS 側と画面は P2
- **テスト**: 単体 12（anthropic 10・canonical 1・model 1）+ 結合 3（`tests/anthropic_web_search_turn.rs`）。
  **結合は HTTP のスタブではなく、実物の `anthropic::decode` / `anthropic::encode` を通すスクリプトのバックエンド
  にした**（Tasks の「スタブの HTTP」から変えた）— 見たいのは decode → ターンの層 → encode の往復で、HTTP の層は
  写しの規則に関わらない。「周の途中で `tool_result` 以外の user のブロックを積まない」は結合の保留の 1 本が
  続く発話のブロックの種別まで留めている。golden（`encoding_without_attachments_matches_the_golden_bytes` ほか）は
  `skills` 既定のまま無改変で緑 = OFF の送信バイトは変わらない
- コア 66 バイナリ・1,325 本全緑・clippy 0・vitest 893 全緑・`cargo check --workspace --all-targets` 0
- **ミューテーション 4 回とも予測どおり**（予測を先に書いた。戻したあと SHA-256 の一致を確かめた）:
  M1 写しを使わず組み直す → 3 本（単体の「写しをそのまま送る」・結合の保留・止まった応答の続き）/
  M2 `pause_turn` を `Other` へ → 3 本（単体の `Paused`・結合の続き・最後の周）/
  M3 `without_provider_skills` から欄を抜く → 2 本（新しい単体と、既存の `every_provider_skill_is_turned_off` —
  固有スキルの欄が全部外れることを列挙で確かめる既存の網が効いた）/
  M4 最後の周の止まった応答も積む → 1 本（結合の最後の周）
- **踏んだもの**: 全体のテストで `link.exe` の 1104（`OLDNAMES.lib` を開けない。ディスクの空きは 38 GB）—
  `-j 4` で通った（CLAUDE.md の既知の処方）/ 1 回目の集計が `passed= failed=`（空）だった = 1 本も走っていない。
  件数を見ずに緑と読まない（`failures.md` #132）

### P2 — 画面

D11 の全箇所 + vitest（`anyOffered` の名指し検査・辞書の鍵）。

### P3 — 台帳

DETAIL 3 言語（固有スキルの表・Anthropic の段落・ディレクトリ木）/ README 3 言語（グラウンディングの
行に 1 語）/ PRIVACY 日英の読み直し（D10）/ CLAUDE.md / `data_contract` の回収。
**ランディングページと Qiita 記事は grep 網の外** — 固有スキルの列挙が載っていれば別に数える。

### P4 — 実機

1. ON の個体（`claude-opus-5-5`）に最新の情報を訊く → `anthropic search: calls≥1 cited≥1` と
   注記の実 URL
2. 村のツールと検索を同じ周で呼ぶ依頼 → `deferred=1` の行の次の周で結果が届き、ターンが完走する
3. `pause_turn` — 起きれば `paused=1` の次の周で完走。起きなければ結合テストへ預ける
4. 別のプロバイダへ切り替えると stranded の行に出て、「オフにする」で消える
5. 接続テスト（Spec 68）で `anthropic search:` の行が出ない
6. `max_uses` を超える依頼で `errors=max_uses_exceeded` が出て、ターンが完走する

## 未決

**2 つとも 2026-10-25 に利用者が推しのまま裁定した**（「未決は推しのまま（max_uses は定数 5、web fetch は別の Spec）」）。
以下は判断の材料の記録。

1. **`max_uses` を固定で送るか。** → **定数 5 で送る**（`ANTHROPIC_WEB_SEARCH_MAX_USES`）。推しは**定数 5**。
   **これは 1 要求の天井であって、1 ターンの天井ではない**（rev2 で書き直した — 査読 1）。
   - 1 要求: 5 検索 = $0.05
   - 1 ターン: `5 × max_tool_iterations`。村の既定 12 周なら 60 検索 = $0.60、36 周の個体なら
     180 検索 = $1.80（`pause_turn` の続きも 1 周に数える。まとめ呼び出しは `tool_choice: none` なので
     検索しない）
   - 送る理由: 1 要求の料金に天井ができる / サーバー側のループが長くならず `pause_turn` が起きにくくなる /
     足りなければ次の周でまた検索できる。送らない場合はサーバーの裁量で、比較の依頼では 10 回以上に
     なると文書が書いている
   - **P0 の実測がこの推しを強めた** — `max_uses` なしの (e0) は 1 要求で検索 17 回・入力 236,999・55 秒・
     $1.23。検索 1 回の結果が約 1 万トークンなので、**`max_uses` は検索料だけでなく入力トークンと所要時間
     （要求の時間切れの既定 120 秒）の天井でもある**。5 なら 1 要求の入力はおよそ 6 万トークンまで。
     (h) で見たとおり、超えた検索は 200 の中の `max_uses_exceeded` になり、ターンは続く
   - **ターン単位のドル建ての天井は作らない** — 作る形は「数えて、超えたら周の途中で `tools` から
     検索を外す」になるが、(a) 保留中の `server_tool_use` があるまま定義を外すと検証エラー（文書）
     (b) `tools` が変わると tools の層からキャッシュが書き直し (c) 予算はトークン建てという Spec 11 の
     形に、ドル建ての 2 本目の天井を足すことになる。**ターンの上限は `max_tool_iterations` が担い、
     検索料は「利用者が負う条件」と計器の `requests=` で受ける**
2. **web fetch を同じ Spec に入れるか。** → **入れない（別の Spec）**。推しは**入れない**（D8）。D3 の写しができれば後から
   トグル 1 本で足せる

## Notes

### 1. 起票時に見つけた別件（本 Spec の範囲外）

- **Anthropic ワイヤは思考段階（`output_config.effort`）を送っていない** — `data_contract` が
  「画面で段階を選んでもワイヤは 1 バイトも変わらない（2026-08-13 実測）」と書いている。
  `claude-opus-5-5` は思考を切れず、段階の既定が `medium`（`claude-opus-5` の `high` から 1 段下がった）
  ので、画面の「思考段階」が効く口が無い。別の Spec の材料
- **5 世代の思考のブロックを送り返していない**ことは、今は 400 になっていない。preserved thinking の
  検査は 2026-08-31 以降に作られたアカウントで強制される、と参照資料にある。この村の鍵の
  アカウントがどちらかは確かめていない。D3 で思考を写しに入れない判断は、この検査をターンの中に
  持ち込まないためでもある

### 2. rev1 の査読の反映（2026-10-25。10 点 → 採用 4 / 前提を訂正して採用 3 / 一部採用 1 / 不採用 1 / 反証 1）

各点を実装（`turn.rs` のまとめ呼び出し・`anthropic.rs` の `encode` と `place_message_breakpoint`・
`mcp::qualified_tool_name`・`OrchestratorConfig` の既定）と公式文書に当ててから判定した。

| # | 指摘 | 判定 | 反映先 |
|---|---|---|---|
| 1 | `max_uses = 5` は 1 要求の天井で、ターンでは天井にならない | **一部採用** — 文面は直した（1 ターンは `5 × max_tool_iterations`。既定 12 周で $0.60、36 周で $1.80）。ターン単位のドル建ての天井は作らない（周の途中で定義を外すと保留が検証エラー・キャッシュの書き直し・予算はトークン建て） | 未決 1 |
| 2 | 写しは不透明なのに、キャッシュの境界を打つ `turn.rs` がどう注入するか未定義 | **前提を訂正して採用** — 境界を打つのは `turn.rs` ではなく `anthropic::encode` の `place_message_breakpoint`。写しはそこでワイヤの生のブロックになり、同じ関数が鍵を書き足す。`inject_cache` のような印は要らない（判定と注入が同じ関数にある） | D3 |
| 3 | `pause_turn` 中に `tool_choice: none` を送ると `web_search` の定義が無くなり 400 | **前提を訂正して採用** — まとめ呼び出しは `tools` を残して `none` で縛る（`failures.md` #36 の処方）ので定義は消えない。ただし別の危険が 2 つある: 止まった応答を積むと要求が assistant で終わる（5 世代の prefill 禁止）/ 保留 × `none` は文書に無い。**まとめ呼び出しへ保留を持ち込まない**と決め、P0 (g1)(g2) で緩められるようにした | D4 / D5 / P0 |
| 4 | `pause_turn` の応答にサーバー側ツールのブロックが無いと写しが作られない | **採用** — 写しを作る条件に `finish == Paused` を足した | D3 |
| 5 | 「衝突は起きない」と断定しながら P1 で衝突の検査を置くのは矛盾。MCP が素の `web_search` を出すと衝突する | **前提を訂正して採用** — MCP のツール名は `qualified_tool_name` が必ず `{サーバー}__{ツール}` にするので、素の `web_search` は村に現れない。断定の文面だけ「今の命名規則では起きない。検査は命名規則が変わったときの網」に直した | D1 |
| 6 | `encode_message` の固定順の組み立てでは `server_tool_use` の位置が保てない | **採用** — 写しがあるときは組み立てを丸ごと飛ばす、と明記。あわせて**`tool_calls` から `tool_use` を二重に作らない**不変条件（ID 集合の一致）を足した — 査読に無かった穴 | D3 / P1 |
| 7 | PRIVACY を「改訂しない見込み」と言い切りつつ P3 で読み直すのは弱めるべき | **採用** — 読み直しの観点（4-2 の「モデルの提供事業者」に Anthropic が含まれると読めるか）を書いた | D10 |
| 追加 1 | base URL を見ないので、検索を拒む中継で個体が止まり続ける。接続テストでも止まらない。400 時のフォールバックを | **不採用** — ON にした機能を黙って外すことになる / どの 400 が検索のせいかは本文の文字列照合でしか分からない。接続テストが固有スキルを確かめないことは「利用者が負う条件」へ | D7 |
| 追加 2 | web fetch は「トグル 1 本と decode 1 種」では済まない | **採用** — ツール定義が要る。写し・続き・計器は共有 | D8 |
| 追加 3 | `calls` と `requests` は 1 対 1 のはずなので、差が出る条件（リトライ）を確かめる | **反証** — ずれは構造で起きる（保留は `calls=1 requests=0` → 次の応答で `calls=0 requests=1` / 失敗した検索は課金されない）。リトライは関係しない。両方持つ理由を書き、P0 (c1)(h) で実物の数を記録する | D6 / P0 |

**一般化**: 査読 2 と 3 は、どちらも「境界を打つ・まとめを呼ぶのは誰か」を実装と違う層に置いて
立てた指摘だった。結論の向き（未定義を潰す・保留をまとめへ持ち込まない）は正しく、前提だけが
違った。**前提を実装で確かめると、指摘より強い理由が見つかることがある**（3 は prefill の禁止）
