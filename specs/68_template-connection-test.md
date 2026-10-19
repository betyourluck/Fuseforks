# Spec 68: モデルテンプレートの接続テスト

- 状態: **Draft rev1**（2026-10-19 起票。査読前。実装は 1 行も無い）
- 起点: 利用者が持ち込んだ外部レビュー（2026-10-19。設定・管理ダイアログの 3 点）の ② —
  「モデルテンプレートを登録しても、繋がるかどうかを確かめる手段が無い」。① と ③ は同日に着地した
  （`2fc3c14`。CLAUDE.md「設定ダイアログを閉じるときの確認と、承認待ちの帯」の節）。② は
  **決めることが 4 つある**（確かめる対象 / 成功の基準 / 送る中身 / 払いの記録）ので Spec を切ると判断し、
  利用者が「② は後で」と保留した。本 Spec はその起票
- 利用者裁定: まだ無い。下の D1〜D4 は推奨で、査読と裁定を待つ。利用者の判断が要るものは「未決」に置いた
- 覆す凍結: 無い。Spec 52 の `RetryClass`（再試行の分類 9 値）は 1 値も変えない — 表示のための分類は
  接続テストの層に別に持つ（D6）

## Goal

1. **モデルテンプレート登録ダイアログ（`ModelTemplateDialog.vue`）から、保存済みのテンプレートで LLM を
   1 回だけ呼び、繋がったかどうかを数秒で見せる。** サーヴァントへ割り当てて起動し、最初の依頼を送って
   失敗するのを待たずに、鍵・base URL・モデル名・プロトコル・思考段階・最大出力トークンの組み合わせが
   接続先に受け付けられるかが分かる
2. **失敗したときは、次の手が分かる分類で見せる**（鍵が違う / モデル名か base URL のパスが無い /
   要求の形が拒否された / レート制限 / 残高不足 / 接続先に届かない / 応答の形が選んだプロトコルと違う）。
   接続先の本文もそのまま見せる
3. **押したときだけ外へ出る。** ダイアログを開いたとき・保存したとき・起動したときには送らない
4. **払ったトークンは少なくともログに 1 行出る**（`failures.md` #103 — 払ったのにどこにも出ない形を
   新しい経路で作らない）。画面にもその場で出す

**やらないこと（範囲外）**: ツール呼び出し・固有スキル・添付・長い入力が通るかの確認（D3 の
「確かめないもの」）/ 確かめた結果の保存と一覧のバッジ / 自動の確認（保存時・起動時）/ `fuseforks-cli check`
への追加。

## 現況（実測 2026-10-19）

### ダイアログ

- `apps/gui-tauri/src/components/ModelTemplateDialog.vue`（1,229 行）にある操作は **保存**
  （`orchestrator.upsertTemplate`）/ **鍵の保存**（`saveSecret` — `upsertTemplate` を先に呼んでから
  `setCredential`）/ **鍵の削除**（`clearCredential`）/ **単価の取得**（`fetchModelPrices`）/ **削除** の 5 つ。
  **疎通を確かめる操作は無い**
- 画面が持つ検査は `checkBaseUrlMismatch`（`lib/providerSkills`）の 1 つだけ — 「他社の既定 base URL が
  そのまま残っている」ときだけ警告する。鍵が正しいか・モデル名が接続先に在るかは、どこにも出ない
- **未保存の判定（dirty）が無い。** 閉じるときの確認も無い（`askConfirm` の呼び出しは鍵の削除とテンプレートの
  削除の 2 箇所だけ）。2026-10-19 に未保存の確認を直したのは `AgentSettingsDialog.vue` で、このダイアログではない
- **鍵はテンプレートより先に保存されない** — `saveSecret` がテンプレートを upsert してから鍵を書くので、
  「鍵だけ未保存」の状態は作れない（D1 の前提）

### 設定の誤りが今どこで表に出るか

サーヴァントにそのテンプレートを割り当て、起動し、依頼を送った最初のターンで出る:

- 鍵が無い（`credential` が `unset`、資格情報ストアに無い）→ GUI のファクトリは
  `HttpBackendFactory::echo_on_failure`（`crates/fuseforks-host/src/boot.rs:255`）なので、
  **会話に「[エコー応答 / 理由]」が返る**（エラーにはならない）
- 鍵が違う・モデル名が無い・パラメータが拒否される → `CoreError::Llm(LlmError::Api)` のトーストと
  `turn failed:` 行。設定不備は `stops_the_agent` が真なので**個体が `Failed` へ降りる**
  （Spec 37 P5 では Meta の `effort=max` の 400 で個体が落ちた）
- 1 つ直すと次の 400 が出る形（`failures.md` #76 / #77 — 400 はパラメータを 1 つずつしか教えない）を、
  個体の起動と依頼の往復で 1 つずつ踏むことになる

### 再利用できる部品（Spec 63 の `assist_draft`）

`crates/fuseforks-core/src/orchestrator/assist.rs:76-89` が、ターンループの外で 1 回だけ LLM を呼ぶ形を
既に持っている:

```text
world.template(&id)?.clone()           // 保存済みの値を読む
  → .without_provider_skills()         // 固有スキル 11 欄を偽に（model.rs:938）
  → self.shared.factory.create(&t)?    // キャッシュ（backend_for）を通さずに組む
  → resolution.degraded_reason が Some なら拒否
```

- **`degraded_reason` の検査が要の 1 行。** GUI のファクトリは設定不備をエコー応答へ退避させるので、
  これを見ないと**鍵の無いテンプレートの接続テストがエコー応答で「成功」する**
- **複製をキャッシュへ入れない**（assist.rs のコメント — id が同じなので、入れると村の個体が
  スキル無しのバックエンドを掴む）。接続テストも同じ理由で `factory.create` を直接呼ぶ
- ツール無しで全ワイヤへ送る前例が 2 つある — `use_tools` が偽のテンプレートの通常のターンと、
  手動要約（`summarize_agents`。「要約の呼び出しにツールは提示しない」）

### 呼び出しの経路と失敗の型

- `LlmBackend::chat` → `chat_inner` → **`chat_with_backoff`**（`llm/client.rs:581`）。再試行は
  `max_retries`（既定 3）回で、429 では明示値を下限に最長 60 秒待つ（Spec 52）。**そのまま呼ぶと、
  接続テストのボタンが最長で数分沈黙しうる**
- タイムアウトは `request_timeout_secs`（既定 120 秒）。最大出力トークンの既定は 8,192
- `LlmError` は 8 値（`llm/error.rs`）。コードは `LLM_CONFIG` / `LLM_HTTP` / `LLM_API` / `LLM_EMPTY_RESPONSE` /
  `LLM_OUTPUT_TRUNCATED` / `LLM_BLOCKED` / `LLM_NO_STRUCTURED_OUTPUT` / `LLM_PARSE`
- **`LlmError::Api` は `class: RetryClass` を持つが、画面へは届かない。** IPC のエラーは `ErrorPayload`
  （`code` / `message` / `detail` / `agentId` / `retryable`）で、**API の失敗は鍵の誤りもモデル名の誤りも
  レート制限も全部 `LLM_API`** になる。分類を画面に出すには、接続テストの結果の型に載せて運ぶ必要がある
- `RetryClass`（`llm/retry.rs:75`）は 9 値 — `RateLimit` / `Overloaded` / `ServerError` / `Timeout` /
  `Network` / `Auth`（401・403）/ `Billing`（code が `insufficient_quota` のときだけ）/ `ClientError`
  （それ以外の 4xx）/ `ContextLength`。**「モデルが無い」に当たる値は無い** — 404 は `ClientError`
- `LlmError::usage()` が `Some` を返すのは `OutputTruncated` だけ。`EmptyResponse` と `Blocked` は
  課金が起きていても使用量を持たない
- 応答の JSON をこのプロトコルの形として解釈できなかったとき（200 だが形が違う）は `LlmError::Parse`
  （`client.rs:496` ほか）
- エラー本文は `MAX_ERROR_BODY`（2,000 バイト）で切る（`client.rs:460`）

### 先例: Jev の「接続を確かめる」（Spec 59・IPC `test_jev`）

- **押したときだけ外へ出る** — `test_jev` の doc が「起動経路・画面遷移・タイマーから呼んではならない」と
  凍結し、`jev_settings.rs` の単体が `state.rs` を走査して `.probe(` が無いことを留める
- **設定が OFF でも押せる**（入れた鍵が通るかを ON にする前に確かめたい）
- **本番と確認で設定の写しを共有する** — `scorer_for` を `apply`（本番へ差し込む）と `test_jev` が共有し、
  「確認だけ通って本番が別の設定で動く」形を作らない
- **200 が返ることと、機能していることを分けて見せる** — 2 段落を送り、点が分かれることまで画面に出す
  （`jev.rs` の `PROBE_*` の doc）
- 失敗は `CoreError::JevProbe { reason }` の新設コード 1 つで、理由は文字列

## Stories

- **S1** テンプレートを登録して鍵を入れ、保存する → 「接続テスト」→ 数秒で
  「繋がりました — 応答『OK』（入力 14 / 出力 2 / 思考 0 トークン・1.4 秒）」。どのサーヴァントにも割り当てる前に分かる
- **S2** 鍵を貼り間違えた → 「認証で拒否されました。API キーを確かめてください」+ 接続先の本文。
  個体を起動して `Failed` に落ちるのを待たない
- **S3** モデル名を 1 文字間違えた → 「モデル名か base URL のパスが接続先にありません（404）」
- **S4** 互換の口で `gpt-6-*` を登録した → 「接続先が要求の形を拒否しました（400）」+ 本文が
  `max_tokens` を名指しする（Spec 63 P0 で実際に踏んだ形 = `failures.md` #76 の再演）
- **S5** 推論モデルで最大出力トークンを小さくした → 「繋がりました。ただし出力上限 N で応答が切れました
  （思考に使い切った可能性）」— **失敗ではなく成功に数える**（接続先は要求を受け付けている）
- **S6** 鍵を入れ忘れた → 「モデル設定に不備があります — API キーが未設定です」。**エコー応答で
  成功と出さない**
- **S7** 未保存の変更がある → ボタンは押せず「保存してから確かめます」と出る

## Design

### D1. 確かめる対象 — 保存済みのテンプレート（推奨）

- **確かめるのは `world.json` に保存済みのテンプレート。** IPC は `templateId` だけを受け取り、コアが
  `world.template(&id)` で読む。**未保存の間（新規の下書き・変更がある間）はボタンを押せない**
- 理由:
  - **下書きを確かめる形にすると、未保存の鍵を IPC で渡すことになる。** 今の鍵の経路は
    `setCredential` → 資格情報ストアの 1 本だけで、「入力欄の値をそのまま LLM 呼び出しに使う」経路は無い。
    確認のために 2 本目を作ると、鍵が UI のメモリに載ったまま往復する経路が増える
  - **確認と本番の写しを 1 つにする**（Spec 59 の `scorer_for` と同じ規律）。下書きで確かめると、
    「確認は通ったが保存した値は別」が作れる。保存済みを確かめれば、確かめたものがそのまま個体の使うもの
  - 鍵は `saveSecret` がテンプレートを先に保存するので、「鍵だけ未保存」は元から起きない
- 代償: 試すたびに保存が要る（「保存 → 接続テスト → 直す → 保存」）。保存は既存のボタン 1 回で、
  保存しても稼働中の個体の挙動は次のターンから変わるだけ
- **未保存の判定を新設する**（このダイアログに初めて入る）— `lib/templateDraft.ts` の純関数
  `isTemplateDirty(draft, saved)`。比較の規則を 1 実装にしてテストで留める（`temperature` の `null` と
  空欄、単価の `null` と未入力が同じ値に揃っているか）。閉じるときの確認には使わない（範囲外。Notes 5）

### D2. 成功の基準 — 接続先が 2xx を返し、選んだプロトコルの応答として解釈できた（推奨）

**「1 トークンだけ返させる」は採らない。** 推論モデルは出力の予算を思考に使うので、最大出力を 1 にすると
本文が空のまま `finish = length` で終わる。最大出力に下限を持つ接続先もある（OpenAI Responses の
`max_output_tokens` に下限があると記憶しているが、この村では未測定 — P0 で撃つ）。どちらも**設定は正しいのに
失敗と出る**。

判定は `LlmError` の網羅 match（`_ =>` を書かない — 9 値目が足されたらここが落ちる）:

| 結果 | 判定 | `outcome` | 画面 |
|---|---|---|---|
| `Ok(response)` | 成功 | `replied` | 応答の先頭 200 字（コードポイント）と使用量 |
| `OutputTruncated { limit, usage }` | **成功** | `truncated` | 「出力上限 {limit} で応答が切れました（思考に使い切った可能性）」+ 使用量 |
| `EmptyResponse` | 成功 | `no_text` | 「接続先は応答しましたが本文がありませんでした」。使用量は分からない |
| `Blocked { reason }` | 成功 | `no_text` | 「接続先が応答をブロックしました（{reason}）」。使用量は分からない |
| `Config(_)` / ファクトリの `Err` / `degraded_reason` | 失敗 | `config` | 理由の文（鍵が未設定 など） |
| `Http`（`is_timeout`） | 失敗 | `timeout` | 「{N} 秒以内に応答がありませんでした」 |
| `Http`（その他） | 失敗 | `network` | 「base URL に届きませんでした」 |
| `Api { class, status, .. }` | 失敗 | D6 の表 | 分類の 1 行 + 接続先の本文 |
| `Parse { .. }` | 失敗 | `unexpected_shape` | 「200 が返りましたが、選んだプロトコルの応答の形ではありません。プロトコルの選択を確かめてください」 |
| `NoStructuredOutput` | 失敗 | `unexpected_shape` | （強制しないので到達しない。網羅のための腕） |

- **`truncated` を成功に数える理由**: 接続先は鍵・モデル名・パラメータを受け付けて 200 を返している。
  切れたのは上限の値の問題で、本番でも同じ上限で切れうることを注記として出す（失敗として出すと
  「繋がらない」と読まれる）
- **`no_text` を成功に数える理由**: 同じく 200 が返っている。注記で「本文が無かった」ことは隠さない
- 成功の文面は「使えます」ではなく**「この設定で接続先が応答しました」**にする（D3 の確かめないものがあるので、
  「使える」と書くと保証の範囲を広げて読ませる）

### D3. 送る中身 — 固有スキルとツールを外し、思考段階と出力上限はテンプレートのまま（推奨）

`ChatRequest` はこう組む:

| 欄 | 値 | 理由 |
|---|---|---|
| テンプレート | `without_provider_skills()` の複製 + **`max_retries = 1`**（D5） | 固有スキルは検索・pro モードの固定費を払う（Spec 63 P0: gpt-6-sol は外すと入力 11,854 → 170 トークン）。スキルが通るかは別の問い |
| `messages` | **利用者発話 1 通だけ**（村の言語の定型文。ja「接続の確認です。『OK』とだけ返してください。」/ en「This is a connection check. Reply with just "OK".」） | システムプロンプトも履歴も送らない。利用者のデータを 1 字も含まない（D10） |
| `tools` / `tool_choice` | 空 / `None` | ツールを使わないテンプレートの通常のターンと同じ形 |
| `effort` | **テンプレートの値** | 思考段階の値が接続先に拒否される形を捕まえる — Spec 37 P5（Meta の `effort=max` が 400 で個体を落とした）/ Spec 48 P0（2.5 系に `thinkingLevel` を送ると 400）。外すと**確認は通るのに本番の最初のターンで落ちる** |
| `max_tokens` | **テンプレートの `max_output_tokens`** | 上限の欄名と値が拒否される形を捕まえる — `failures.md` #76（`max_tokens` を `unsupported_parameter` で拒否）/ Spec 63 P0（gpt-6-sol を互換の口で撃って同じ 400） |
| `temperature` | テンプレートの値 | 同じ理由（Spec 34 — Responses 系は `temperature` を拒否するので欄ごと持たない。送る/送らないの判断を本番と揃える） |
| `cacheable_prefix_len` | 0 | |

- **払いの上限はテンプレートの最大出力トークン**（既定 8,192）。推論モデルが思考に使い切ったときが最悪で、
  定型文への答えなら通常は数十トークン（P0 で 7 ワイヤぶん実測する）
- **確かめないもの**（画面の注記と DETAIL に書く）:
  - **ツールを送ったときだけ出る拒否** — `failures.md` #77（`reasoning_effort` と関数ツールの併用不可。
    今はアダプタが吸収している）のように、tools を送った要求でだけ 400 になる形は捕まえない（未決 2）
  - 固有スキル（Gemini の `includeServerSideToolInvocations` の門など）/ 添付（Spec 36 の carries 表）/
    長い入力（`contextLength`）/ 本番のシステムプロンプトの大きさとキャッシュ
  - レート制限の余裕（波は個体の人数ぶん RPM を食う — 2026-08-25 の実測。1 回の確認では分からない）

### D4. 払いの記録 — ログに 1 行。統計へ入れるかは未決（未決 1）

**`template probe:` 行**（1 回の確認ごとに 1 行。成功でも失敗でも出す）:

```text
template probe: template=<id> wire=<provider> model=<model> outcome=<replied|truncated|no_text|config|auth|…>
  status=<HTTP|-> class=<RetryClass|-> code=<provider code 先頭 40 字|-> elapsed_ms=<n>
  prompt=<n|-> completion=<n|-> reasoning=<n|->
```

- **書かないもの**: 送った文・応答の本文・接続先のエラー本文・API キー・base URL。分類と数だけ
  （`failures.md` #71 の線。エラー本文には接続先が書いた文がそのまま入る）
- 使用量が分からない結果（`no_text` / 失敗）は `-` を書く。**0 を捏造しない**（#72 の規律。`-` は
  「分からない」で、`0` は「払っていない」と読まれる）
- **村の予算（`tokenBudget`）と個体の累計には入れない** — 村の仕事の因果ではなく、人が押した設定の操作
  （Spec 63 D2 と同じ線）
- **画面にもその場で出す**（S1 の「入力 14 / 出力 2 / 思考 0 トークン」）。払いが見える場所が 2 つになる
- **統計画面（`Record`）へ入れるかは未決 1**。推奨は入れない — 理由は Notes 4

### D5. 再試行しない。タイムアウトはテンプレートのまま

- **`max_retries = 1` の複製で呼ぶ。** そのまま呼ぶと `chat_with_backoff` が 429 で最長 60 秒待ってから
  再送し、既定の 3 回で数分沈黙しうる。**429 も 5xx も、接続テストでは結果そのものが情報**
  （レート制限なら設定は通っている可能性が高い）
- 429 で接続先が待ち時間を明示していれば（`LlmError::Api.retry_after`）、その秒数を画面に添える
  （「接続先は {N} 秒後を指定しています」）
- タイムアウトは `request_timeout_secs`（既定 120 秒）のまま — タイムアウトの設定も本番と同じ値で確かめる。
  押した後は「確かめています…（最長 {N} 秒）」と出す
- `llm retry:` 行は出ない（再試行しないので）

### D6. 失敗の分類 — 接続テストの層に表示用の閉じた列挙を置く

`RetryClass` は**再試行の判定**のための 9 値（Spec 52 で凍結・`verdict` が網羅 match）。「モデルが無い」を
足すと再試行の表まで触ることになるので、**表示の分類は別の列挙 `ProbeFailure` に置き、`(LlmError, status,
class, code)` から写す**。

| `ProbeFailure` | 写す元 | 画面の 1 行（辞書 `modelTemplate.probe.failure.*`） |
|---|---|---|
| `config` | `Config` / ファクトリの `Err` / `degraded_reason` | モデル設定に不備があります（理由の文をそのまま） |
| `auth` | `RetryClass::Auth`（401 / 403） | 認証で拒否されました。API キーを確かめてください |
| `not_found` | `ClientError` かつ status 404 | モデル名か base URL のパスが接続先にありません |
| `rejected` | 404 以外の `ClientError` / `ContextLength` | 接続先が要求の形を拒否しました（思考段階・最大出力トークン・温度など）。下の本文が理由を名指しすることがあります |
| `rate_limit` | `RateLimit` | レート制限に当たりました。設定は通っている可能性があります |
| `billing` | `Billing` | 残高かクォータが不足しています |
| `server_error` | `ServerError` / `Overloaded` | 接続先の一時的な障害です |
| `timeout` | `Http`（`is_timeout`）/ `RetryClass::Timeout` | {N} 秒以内に応答がありませんでした |
| `network` | `Http`（その他）/ `RetryClass::Network` | base URL に届きませんでした |
| `unexpected_shape` | `Parse` / `NoStructuredOutput` | 200 が返りましたが、選んだプロトコルの応答の形ではありません |

- **写しは純関数 1 本**（`template_probe.rs` の `classify_failure`）で、`LlmError` と `RetryClass` の両方を
  網羅 match で書く（`_ =>` を書かない）
- **「残高不足」の判定は狭い** — `Billing` になるのは code が `insufficient_quota` のときだけ（Spec 52）。
  Anthropic の「残高が足りない」は 400 の `invalid_request_error` で返ると記憶しており、その場合は
  `rejected` に落ちる（未測定）。**本文を必ず一緒に出すのはこのため** — 分類が外れても接続先の文が理由を言う
- **鍵の誤りが `auth` にならない接続先がある見込み** — Gemini の不正な鍵は 401 ではなく 400 の
  `INVALID_ARGUMENT`（詳細に `API_KEY_INVALID`）で返ると記憶している（未測定）。そうなら `rejected` に落ちる。
  **P0 で 7 ワイヤに不正な鍵とモデル名を撃ち、status と code の表を作ってから**、プロバイダの code で
  `auth` / `not_found` を補う腕を足すかを決める（`error_signal` は既に code を取り出している）
- 接続先の本文は `ErrorPayload`（`CoreError::Llm(err)` から作る）で運び、**画面は `formatError` を通す**
  （他のエラー表示と同じ文面・同じ多言語化の規則）

### D7. コアと IPC

**コア**: `Orchestrator::probe_template(&ModelTemplateId) -> CoreResult<TemplateProbe>`
（`orchestrator/template_probe.rs`）+ 純機構 `crates/fuseforks-core/src/template_probe.rs`
（定型文 ja/en・`classify_failure`・結果の型・応答の先頭 200 字の切り出し）。

```ts
type TemplateProbeUsage = { prompt: number; completion: number; reasoning: number };

type TemplateProbe =
  | { result: "reached";
      outcome: "replied" | "truncated" | "no_text";
      text: string | null;          // 応答の先頭 200 字（replied のときだけ）
      note: string | null;          // truncated の上限 / no_text の理由
      usage: TemplateProbeUsage | null;
      elapsedMs: number }
  | { result: "failed";
      failure: ProbeFailure;        // D6 の 10 値
      status: number | null;        // HTTP ステータス（Api のときだけ）
      retryAfterSecs: number | null;
      error: ErrorPayload;          // formatError に渡す
      elapsedMs: number };

testModelTemplate(templateId: ModelTemplateId): Promise<TemplateProbe>;   // IPC `test_model_template`
```

- **LLM の失敗は `Err` ではなく `result: "failed"` で返す。** `CoreError` で返すと画面に届くのは
  `LLM_API` だけで、D6 の分類が落ちる（現況の「`class` は画面へ届かない」）
- `Err(CoreError)` を返すのは確認を始められないときだけ — テンプレートが無い（`MODEL_TEMPLATE_NOT_FOUND`）
- IPC の名前は先例 `test_jev` に揃える。GUI の `commands.rs` から `state.host.orchestrator.probe_template` を呼ぶ
- **呼び出しは並べない** — 同じテンプレートへの 2 回目は画面の `busy` で塞ぐ（コアには排他を置かない。
  別のテンプレートを同時に確かめる理由も、それを禁じる理由も無い）

### D8. 画面

- ボタンはダイアログのフッターの左端（右に「キャンセル」「保存」）。文言「接続テスト」
- **押せないとき**（`disabled` + 理由を `title` に）: 新規の下書き（`isNewDraft`）/ 未保存の変更がある
  （`isTemplateDirty`）/ 確かめている最中
- 結果はフッターの上に 1 区画。成功は `--color-run`、失敗は `--color-fail`、`truncated` / `no_text` の注記は
  `--color-warn`。応答の本文と接続先のエラー本文は**素のテキスト**で出す（`v-html` を使わない）
- **結果は押したテンプレートに結び付け、選択の切り替え・保存・削除で消す**（別の設定の結果が残って
  読まれる形を作らない）。編集を始めただけでは消さない（結果は「保存済みの設定」についての事実のまま正しい。
  ボタンは押せなくなる）
- ダイアログを閉じても IPC は最後まで走る（打ち切りの口は持たない）。結果は捨てる。ログの 1 行は残る

### D9. 押したときだけ外へ出る

- 呼び出し元は**ボタンの click ハンドラ 1 つだけ**。ダイアログを開いたとき・テンプレートを選んだとき・
  保存したとき・起動したときには送らない（Spec 59 D10 / Spec 41 の `pricing_fetch_freeze` と同じ凍結）
- 走査テストで留める — `ModelTemplateDialog.vue` の中で `testModelTemplate` を呼ぶのが click の 1 箇所だけ /
  `onMounted` と `watch` から呼ばない / `fuseforks-host` の起動経路（`boot.rs` / `state.rs`）が
  `probe_template` を呼ばない

### D10. 外へ送るもの（PRIVACY）

- 送り先は**テンプレートに登録した接続先だけ**（新しい送り先は増えない）
- 送る中身は D3 の定型文 1 通と、接続先が要求する認証情報とモデルの設定（モデル名・思考段階・上限）。
  **利用者の依頼・会話・ファイル・記憶は 1 字も含まない**
- `PRIVACY.md` の 4-1 は既に「登録した接続先へ API キーを送る」を書いているので、**改訂は不要**（Notes 3）

### D11. 言語

定型文（D3）と画面の文言は村の `language` に従う（Spec 35）。D6 の 1 行は辞書 `modelTemplate.probe.*` から
引く。**辞書の鍵を実行時に組む**（`modelTemplate.probe.failure.${failure}`）ので、`ProbeFailure` の 10 値と
ja / en の鍵の突き合わせを走査テストで留める（`blackboardLanesWiring` と同じ形 — 欠けても vue-tsc も
i18n のコンパイル検査も通る）。

## 採らなかった形

- **編集中の下書きを確かめる** — D1。未保存の鍵が IPC を往復する経路が増え、確認と保存の写しが 2 つに割れる
- **1 トークンだけ返させる** — D2。推論モデルと出力の下限を持つ接続先で、正しい設定が失敗と出る
- **固有スキルを付けたまま確かめる** — D3。スキルの固定費を払い（検索 1 回の課金を含む）、
  しかも「スキルが通るか」は接続とは別の問い
- **思考段階を外して確かめる** — D3。確認は通るのに、本番の最初のターンで思考段階の 400 が出る
- **再試行してから結果を出す** — D5
- **自動で確かめる**（保存したとき・ダイアログを開いたとき・起動したとき）— 押していないのに外へ出て
  課金が起きる（D9）
- **確かめた結果を保存し、一覧にバッジを出す** — 結果は押した時点の事実で、鍵の失効・残高・接続先の
  モデル廃止で黙って古くなる。バッジがあると「このテンプレートは使える」と読まれる
- **`RetryClass` に `not_found` を足す** — D6。再試行の判定（Spec 52 の凍結）を表示の都合で動かさない
- **`fuseforks-cli check` に足す** — Spec 64 の `check` は LLM にも MCP にも触れない起動前の静的検査で、
  その性質（何度叩いても課金されない）を変えない。要るなら別のサブコマンドで（頻度を見てから）
- **`CoreError::TemplateProbe` を新設して失敗を `Err` で返す**（`test_jev` の形）— D7。理由が文字列に
  畳まれ、D6 の分類が型で画面へ届かない

## Tasks

### P0 — 測ってから凍結する

**予測を先に書いてから撃つ**（Spec 63 P0 と同じ）。村のテンプレートと資格情報ストアの鍵で、実物のアダプタ
（`HttpBackendFactory::strict`）から D3 の要求を撃つ。プローブは `crates/fuseforks-core/examples/` の使い捨て
（コミットしない。数字は本 Spec が正）。

| 撃つもの | 7 ワイヤ（OpenAI 互換 / Anthropic / Gemini / xAI / OpenAI / Meta / Perplexity の Responses）で |
|---|---|
| 正しい設定 | 結果・所要・使用量（**特に出力と思考のトークン** = 1 回の確認の値段） |
| 鍵を 1 文字変える | status / code（`error_signal`）/ `RetryClass` / D6 の写しが `auth` になるか |
| モデル名を 1 文字変える | status / code / D6 の写しが `not_found` になるか |
| 最大出力を 1 にする（推論モデル 1 本） | `truncated` で返るか、400（下限）で返るか — D2 の根拠 |

- 結果で D6 の表を確定する（プロバイダの code で `auth` / `not_found` を補う腕を足すか）
- **空の `tools` + `ToolChoice::None` で 7 ワイヤとも `tools` / `tool_choice` を送らないこと**を確かめる
  （`use_tools` 偽の経路で動いているはずだが、本番で全ワイヤを踏んだ記録を数えてから書く）
- `data_contract.yaml` へ `template_probe_contract`（D1〜D10 の凍結: 保存済みだけ / 成功の基準 /
  送る中身 / 再試行しない / 閉じた分類 / ログに本文・鍵・base URL を書かない / 予算と個体の累計に入れない /
  キャッシュに入れない / `degraded_reason` を成功にしない / 押したときだけ）と `entities` の
  `TemplateProbe` / `ProbeFailure`

### P1 — コア

- `template_probe.rs`（純機構）: 定型文 ja/en / `classify_failure`（網羅 match）/ 結果の型 / 先頭 200 字
- `orchestrator/template_probe.rs`: `probe_template` — 読む → 複製（スキル無し・`max_retries = 1`）→
  `factory.create` → `degraded_reason` の検査 → 1 回呼ぶ → 判定 → `template probe:` 行
- テスト: スタブのバックエンドで D2 の表の全行（`Ok` / `OutputTruncated` / `EmptyResponse` / `Blocked` /
  `Config` / `Http` 2 種 / `Api` を `ProbeFailure` の 10 値ぶん / `Parse`）・**退避型のファクトリで
  鍵の無いテンプレートが `config` で返り、エコー応答で成功しない**・要求が D3 の表どおり（固有スキル偽・
  tools 空・effort と上限がテンプレートの値・発話 1 通）・`max_retries = 1` で 429 が 1 回で返る・
  ログ行に本文と鍵が出ない・キャッシュ（`backend_for`）に複製が入らない・ワイヤ形の凍結
  （`tests/ipc_contract.rs`）

### P2 — 画面

- IPC `test_model_template` と型 / `lib/templateDraft.ts` の `isTemplateDirty`（単体）/ ボタンと結果の区画 /
  辞書 ja/en（`modelTemplate.probe.*`）
- 走査テスト: 呼び出しが click の 1 箇所だけ（D9）/ `ProbeFailure` と辞書の鍵の突き合わせ（D11）/
  エラー本文を `formatError` に通している / `v-html` を使っていない

### P3 — 台帳

- DETAIL 3 言語（モデル登録の節に接続テストと「確かめないもの」）/ README 3 言語は既存のモデル登録の行を
  延ばす（行を足さない）/ PRIVACY は改訂しない（D10）

### P4 — 実機

1. 保存済みのテンプレートで押す → `template probe: outcome=replied` と画面の使用量が一致する
2. 鍵を 1 文字変えて保存 → 押す → `auth`（P0 の表で `rejected` になるワイヤはその表示）と接続先の本文
3. モデル名を 1 文字変えて保存 → `not_found`
4. 鍵を削除する → `config`（**会話にエコー応答が出ない**ことも見る）
5. 未保存の変更があるとボタンが押せず、保存すると押せる / 別のテンプレートを選ぶと結果が消える
6. ダイアログを開いただけ・保存しただけでは `template probe:` 行が出ない

## 未決

1. **統計画面（`Record`）へ入れるか**（D4）— 推奨は入れない（Notes 4）
2. **`use_tools` が真のテンプレートで、ダミーの関数を 1 本添えるか**（D3）— 添えると「ツールを送ったときだけ
   出る拒否」（#77 の形）も捕まえられるが、モデルがそのツールを呼ぶことがあり、定義の分だけ入力が増える。
   推奨は添えない（確かめないものとして注記する）
3. **確かめる対象を下書きへ広げるか**（D1 の裏返し）— 推奨どおり保存済みにするなら閉じる。利用者の
   「保存してから確かめるのは面倒」が実機で出たら再検討

## Notes

### 1. 先例 `test_jev` との対照

| `test_jev`（Spec 59） | 本 Spec |
|---|---|
| 設定が OFF でも押せる | 割り当てていないテンプレートでも押せる（村の個体の状態に依らない） |
| `scorer_for` を本番と共有 | `factory.create` と `LlmConfig::from_template` を本番と共有（スキル・再試行・ツールだけが違う） |
| 2 段落で「点が分かれる」まで見せる | 応答の先頭と使用量を見せ、「使える」とは書かない（D2・D3） |
| 失敗は `CoreError::JevProbe { reason }` | 失敗は結果の型で返し、分類を運ぶ（D7） |
| 押したときだけ外へ出る（走査で留める） | 同じ（D9） |

### 2. 外部レビュー（2026-10-19）との関係

3 点のうち ① 設定ダイアログの保存の二重構造（`AgentSettingsDialog` の右のファイルの未保存が閉じるときの
確認に入っていなかった）と ③ コマンド承認待ちの帯は `2fc3c14` で着地。② が本 Spec。CLAUDE.md の同日の節は
「決めることが 4 つ = 保存済みか下書きか / 成功の基準（200 が返れば `OutputTruncated` も成功）/
固有スキルとツールを外すか / 払いの記録。部品は Spec 63 の `without_provider_skills` + `factory.create`」と
書いており、D1〜D4 がその 4 つ。

### 3. PRIVACY を改訂しない根拠

- 送り先は利用者がテンプレートに登録した接続先だけで、4-1 が既に「登録した接続先」「認証のための API キー」を
  名指ししている
- 送る中身は固定の 1 文で、4-1 が列挙する利用者のデータ（依頼文・会話・規則・ツール結果・画像）を 1 つも含まない。
  「AI で作成」の段落を 4-1 に足したのは、送る中身（編集中の本文など）が増えたからで、本 Spec には当たらない
- 6（診断ログ）の「記録しないもの」とも矛盾しない（D4 で本文と鍵を書かない）

### 4. 統計へ入れない推奨の理由（未決 1）

- **`Record` に 6 種別目を足すと、旧い版がその会話を読めなくなる**（Spec 63 の `assist` と同じ互換の代償 —
  旧版で開くと履歴なしで始まり、統計の「全会話」が集計ごと失敗する）
- **書き込む会話が無い** — 接続テストはどの会話にも属さない操作。`Record::Assist` は開いている会話へ書いたが、
  接続テストを会話に結び付ける理由は無い
- 払いは画面とログの 2 箇所に出る（D4）。1 回の値段は P0 で測る（定型文への答えで数十トークンの見込み）
- 代償: 統計画面の `≈ $` に接続テストの費用は入らない。リリースノートの「利用者が負う条件」に書く

### 5. 見つけた別件（本 Spec の範囲外）

- **`llm/client.rs:460` の `body.truncate(MAX_ERROR_BODY)` が文字境界を見ていない。** `String::truncate` は
  切る位置が UTF-8 の文字の途中だと panic する。接続先のエラー本文が 2,000 バイトを超え、2,000 バイト目が
  多バイト文字（日本語）の途中に来ると、エラーを返すはずの呼び出しが panic する。本 Spec の画面は
  エラー本文をそのまま出す経路なので、P1 の前に直しておくのが筋（`is_char_boundary` で後ろへ戻す 1 行）
- **このダイアログには閉じるときの確認が無い**（未保存の変更が、✕・背景のクリック・別のテンプレートの選択で
  黙って消える）。D1 で足す `isTemplateDirty` はその確認にも使える形にしておくが、確認そのものは範囲外
- `RetryFacts` の doc が「本文は `turn failed` 行が既に運んでいる」と書いている — 接続先のエラー本文は
  `turn failed:` 行経由で `fuseforks.log` に入っている。PRIVACY の 6 は「プロンプトの本文」「ツール結果の本文」を
  記録しないと書いているが、エラー本文には触れていない。本 Spec の `template probe:` 行は本文を書かない
