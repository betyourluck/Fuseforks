# Spec 71: Anthropic の web 検索 — サーバー側ツールと、1 ターンの中で応答のブロックを送り返す欄

- 起票: 2026-10-25
- 状態: **Draft rev1**（P0 未着手。未決 2）
- 起点: 利用者が公式文書（`platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool`）を
  持ち込んだ → 公式文書 3 ページと Anthropic ワイヤの実装を突き合わせた（同日）→
  「Spec 71（Anthropic の web 検索）として起票して。P0 の probe 項目も Spec に入れて」

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
  `toolUsePrompt` に乗る — どちらも同額）だった。Anthropic の「村のツールと同じ周」の形では、
  ブロックを落とすと**取り直しではなく、検索そのものが走らない**（文書。実際の挙動は P0 (c)）。
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
  形）。D3 の席が固まってからの別の段。P0 (j) で形だけ記録する
- **置き場は tools の末尾**。並びを固定するのはキャッシュの前方一致のため（トグルを切り替えた
  1 回だけ tools の層が書き直しになる）
- **名前の衝突は起きない** — 村のツール名は同梱の固定名 / MCP の `{サーバー}__{ツール}` /
  合成の `ask_*` `transfer_to_*` `plan` などで、素の `web_search` を名乗るものが無い。
  P1 の単体で「提示集合に `web_search` という名前の関数ツールが居たら送らない」を留める
  （黙って 400 にしない）
- `max_uses` の値は未決 1

### D2: 判定は AND 述語 1 本。モデル名の門は掛けない

`ModelTemplate.anthropic_web_search: bool`（`#[serde(default)]`。TS は `anthropicWebSearch`）。
判定は **`anthropic_web_search_active() = anthropic_web_search AND effective_provider() == Anthropic`**
（`gemini_url_context_active` と同型。フラグ単独を判定に使わない）。`LlmConfig` へ判定済みの値を
写し、`anthropic::encode(req, AnthropicSkills { web_search })` の値渡しで受ける
（Gemini の `GeminiSkills` と同じ形。将来 web fetch を足すときに bool を並べない）。

- 3 モデル（`claude-opus-5-5` / `claude-sonnet-5-5` / `claude-fable-5-1`）で通ることは P0 (b) で確かめる
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
- **写しを作るのは、応答にサーバー側ツールのブロックが 1 つでも在るときだけ**。それ以外は
  `None` で、encode は今のまま組み直す。**検索を使わない要求の送信バイトは 1 バイトも変わらない**
  （golden で留める）
- 写しが在る assistant の発話は、encode が**写しのブロックを受け取った順に並べ直さずに送る**
  （組み直さない）。`tool_use` の ID は写しの中の ID そのものなので、続く `tool_result` と対になる
- **#29 の網に 1 条件足す** — 写しを持つ発話は「空」ではない
- **写しに思考のブロックを入れるかは P0 で決める**（(c3) / (e2)）。推しは**入れない** —
  今の村は思考のブロックを 1 度も送っていない。送り始めると、5 世代のモデルが思考のブロックの
  手前の履歴が変わっていないかを確かめる検査（preserved thinking）がターンの中に入ってくる。
  入れなくても 400 にならなければ、入れない
- キャッシュの境界（`place_message_breakpoint`）は最後の発話の最後のブロックに打つ。写しの
  ブロック（`server_tool_use` / `web_search_tool_result`）に `cache_control` を付けてよいかは
  P0 (f)。拒まれたら、その発話では境界を手前の普通のブロックへ下げる

### D4: `pause_turn` は周を 1 つ足して続ける

- canonical の `Finish` に **`Paused`** を足す。作るのは Anthropic の decode だけで、他のワイヤは
  出さない
- 周回ループ: **`finish == Paused` かつツール呼び出しが 0 本**なら、その応答を写しつきの
  assistant の発話として積み、**利用者の発話を足さずに**次の周を回す（「続けて」を足すと
  文書の言うとおり別の発話として扱われる）。止まった周の本文は最終回答として配らない
- **続きの周は普通の周として数える** — `max_tool_iterations` が上限になり、新しい定数は作らない。
  予算の予約・打ち切りの検査・`cache:` 行も普通の周と同じに掛かる（払いが周ごとに見える）
- 5 世代のモデルは assistant の発話で終わる要求（prefill）を 400 で拒む。`pause_turn` の続きは
  文書が示す例外のはずだが、**`claude-opus-5-5` で実際に通るかは P0 (e)**
- 最後の周で止まった場合は既存の「まとめ呼び出し」（`tool_choice: none`）へ進む。待っている
  `server_tool_use` があるまま `none` を送ってよいかは P0 (g)

### D5: 村のツールと同じ周は、写しを送って `tool_result` だけを続ける

- 応答に結果の無い `server_tool_use` が在り `stop_reason: "tool_use"` のとき、写しつきの
  assistant の発話を積み、村のツールを実行して結果を積む（今の手順のまま）。次の応答の先頭に
  検索結果が来て、写しはその周の分に置き換わる
- **周の途中で `tool_result` 以外の user のブロックを積む経路が無いこと**を P1 の単体で留める。
  1 つでも足すと、文書の言う 400（結果の無いサーバー側ツール）になる。可変の文脈（広場ログ・
  入退室など。#45）はターンの最初の発話に畳まれるので、周の途中には入らない
- Anthropic ワイヤは `Role::Tool` を 1 発話 1 つの `tool_result` として user の発話に載せる
  （並列の呼び出しでは user の発話が続く）。これで通ることは今の村の運用が示しているが、
  **検索の保留が挟まったときも通るか**は P0 (c1)

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
- **`agent=` は無い** — decode 層は個体を知らない（`gemini tools:` / `pplx tools:` と同じ制約）。
  突き合わせは直後の `turn:` 行の時刻で行う

### D7: 失敗の形

- **組織で無効** → 400 `invalid_request_error`。Spec 52 の分類では止める側で、今の個体は
  400 で停止状態になる（Spec 37 P5 で `effort=max` の 400 が個体を降ろしたのと同じ）。
  文面は接続先の本文がそのまま出る。**新しい分類は作らない** — トグルを切れば直る失敗で、
  利用者の次の手は 1 つしかない。実際の文面は未実測（組織の設定を変える操作は利用者の側。P0 (i)）
- **検索の失敗**（200 の中のオブジェクト）→ D6 の計器に出し、ターンは続ける。モデルは結果の
  無い検索として扱う

### D8: web fetch は範囲外

web fetch ツール（`web_fetch_20250910` など。会話に既に出ている URL だけを取得する）は、Gemini の
URL context（Spec 48）や Perplexity の `fetch_url`（Spec 45）に当たる。**同じ `server_tool_use` の
形で返り、D3 の写しにそのまま乗る**ので、足すときはトグル 1 本と decode の 1 種で済む。
需要が出てから起票する（未決 2）。

### D9: 費用

- **検索料（1,000 検索 $10）は予算の外** — 予算はトークン建てで、ドル建ての料金を数えない。
  xAI の `ticks` / Perplexity の `tool_cost_usd` と同じ扱いで、計器の `requests=` から後で数えられる。
  統計・金額表示（Spec 39 / 41）への写しは範囲外。リリースノートの「利用者が負う条件」に入れる
- **検索結果のトークンは `input_tokens` に入る**（公式。P0 (a) で量を確かめる）ので、予算と統計には
  今の経路で乗る
- ターンの中では写しが周ごとに送り直されるので、検索結果は周ごとに入力に乗る。最後の発話に打つ
  キャッシュの境界が効けば、2 周目以降はキャッシュ読み（0.1×）で乗る
- ターンをまたいでは持ち越さない（D3）ので、公式の使い方より安い

### D10: PRIVACY

`PRIVACY.md` 4-2 は「利用者が該当する機能を有効にした場合、モデルの提供事業者が代行して web 検索や
X の検索を行います。検索語は提供事業者へ送信されます」と事業者を名指しせずに書いてあり、
Anthropic は既に LLM の接続先。**送り先は増えない**ので改訂しない見込み。P3 で日英を読み直す。

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

### P0 — 測ってから凍結する

鍵は利用者の環境変数 `ANTHROPIC_API_KEY`（値はスクリプトにもログにも出さない）。スクリプトは
scratchpad に置いて使い捨て、数字だけをこの Spec の「P0 実測記録」に残す（Spec 34 / 48 と同じ扱い）。
**撃つ前に予測を表に書く。** 費用の見積もりは 20〜30 要求・検索 60 回前後で **3 ドル前後**
（`claude-opus-5-5` の $4 / $20 と、1 要求 30,000 入力トークンを仮定した概算。未実測）。

| # | 要求 | 予測 | 決めること |
|---|---|---|---|
| (a) | `claude-opus-5-5` + `web_search_20250305` 単独。答えに検索が要る日本語の問い | 200。`server_tool_use` → `web_search_tool_result` → `citations` つきの `text`。`web_search_requests` ≥ 1 | ブロックの実物（欄・件数・`encrypted_content` の字数・`page_age` の形・`cited_text` の字数）/ `input_tokens` に検索結果が何トークン乗るか / 所要時間（既定の 120 秒に収まるか） |
| (b) | (a) と同じ要求を `claude-sonnet-5-5` / `claude-fable-5-1` で | 200 / 200 | D2 の「モデル名の門を掛けない」 |
| (c1) | 村のツール 1 本（例: 時刻を返す関数）と検索を同じ周で呼ばせる依頼 → 保留の応答を**そのまま**送り返し、`tool_result` だけを続ける | 1 回目 = `stop_reason: tool_use` + 結果の無い `server_tool_use`。2 回目 = 先頭に `web_search_tool_result` | D5 の手順。同じ周で呼ばせるには依頼の書き方を何度か変える必要がありうる（試した回数も記録する） |
| (c2) | (c1) の保留を、今の encode と同じく**本文 + `tool_use` だけ**に組み直して送る | 200 で検索が消える、または 400 | 「採らなかった形」の裏付け。200 なら 2 回目にモデルが検索を呼び直すかも見る |
| (c3) | (c1) の保留を、**思考のブロックだけ除いて**送る | 200 | D3 の「写しに思考を入れない」 |
| (d1) | 検索が終わった組（`server_tool_use` と結果）を含み、村のツールも呼んだ応答の次の周で、検索の組を**落として**送る | 200。モデルが検索を呼び直すことがある | 前例（Gemini の取り直し）と同じ形か。呼び直した回数と `requests` |
| (d2) | (d1) を写しのまま送る | 200。呼び直さない | (d1) との入力トークンの差 |
| (e1) | 検索を多く要する依頼（`max_uses` なし）で `pause_turn` を起こす → そのまま送り返す | `pause_turn` → 続きの応答 | D4 の続け方 / **5 世代の prefill 禁止に掛からないか**。数回で起きなければ「再現せず」と書き、D4 は文書だけに接地していると明記する |
| (e2) | (e1) を思考のブロックを除いて送る | 200 | D3 |
| (f) | 写しの最後のブロック（`server_tool_use` / `web_search_tool_result`）に `cache_control` を付ける | 200 | D3 のキャッシュの境界 |
| (g) | 検索を含む `tools` で `tool_choice: none`。保留中の `server_tool_use` があるときと無いとき | 無い = 200・検索なし / 有る = 不明 | D4 の「最後の周で止まったとき」 |
| (h) | `max_uses: 1` で 2 回以上の検索を要する依頼 | 200 の中に `error_code: max_uses_exceeded` | D6 の型（配列とオブジェクト）/ ターンが続くか |
| (i) | 組織で web 検索を無効にした状態で (a) | 400 | D7 の文面。**組織の設定を変えるのは利用者の操作**なので、撃つかは利用者が決める。撃たなければ「未実測」のまま |
| (j) | `web_search_20260209` を `allowed_callers` なしで / `["direct"]` で | なし = 応答にコード実行のブロック / direct = (a) と同じ形 | D1 の「基本版を採る」の裏付け（形の記録だけ） |
| (k) | 関数ツールに `web_search` という名前のものを並べて送る | 400 | D1 の衝突の検査を入れる理由 |

**凍結先**（`data_contract.yaml`）:
- `llm_wire` に **`anthropic_server_tools`** の節を新設（D1 の版と定義 / D3 の写しの規律と寿命 /
  D4 / D5 / D6 の decode と計器 / 送信バイトが変わる条件）
- `ModelTemplate` の欄の列挙へ `anthropicWebSearch`
- `GroundingEngine` の値へ `anthropic`
- `Finish` を列挙している箇所があれば `paused`（無ければ D4 の節に書く）

### P1 — コア

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
  queries・sources・計器の値を読む / 失敗のオブジェクトを読む / `pause_turn` が `Paused`
- 結合（スタブの HTTP で 2 周回す）: 保留の応答の次の要求に写しがそのまま載り、続く発話が
  `tool_result` だけ / `Paused` の次の周に利用者の発話が足されない / 写しが保存されない
  （`sessions.redb` の記録に出ない）
- **ミューテーション**（予測を先に書く）: 写しを使わず組み直す / `Paused` を `Other` に戻す /
  `without_provider_skills` から欄を外す — それぞれ狙った 1 本だけ赤

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

1. **`max_uses` を固定で送るか。** 推しは**定数 5**（`ANTHROPIC_WEB_SEARCH_MAX_USES`）。理由: 1 回の
   要求で払うドル建ての料金に天井ができる（$0.05）/ サーバー側のループが長くならず `pause_turn` が
   起きにくくなる / 足りなければ次の周でまた検索できる（周の上限は `max_tool_iterations`）。
   送らない場合はサーバーの裁量で、比較の依頼では 10 回以上になると文書が書いている
2. **web fetch を同じ Spec に入れるか。** 推しは**入れない**（D8）。D3 の写しができれば後から
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
