# Spec 68: モデルテンプレートの接続テスト

- 状態: **rev2・未決 0 → P0〜P2 完了**（2026-10-19 起票 → 同日、査読 7 点を実コードに当てて rev2。判定は 7 点とも採用 —
  うち点 4 は処方を査読の案から変えた。対応表は Notes 6。2026-10-20 に未決 3 つを利用者が推奨どおりに閉じ、
  同日 P0 = 7 ワイヤへ撃って D6 を確定し `data_contract` の `template_probe_contract` を凍結 / P1 = コア / P2 = IPC と画面。
  残りは P3 台帳と P4 実機）
- 起点: 利用者が持ち込んだ外部レビュー（2026-10-19。設定・管理ダイアログの 3 点）の ② —
  「モデルテンプレートを登録しても、繋がるかどうかを確かめる手段が無い」。① と ③ は同日に着地した
  （`2fc3c14`。CLAUDE.md「設定ダイアログを閉じるときの確認と、承認待ちの帯」の節）。② は
  **決めることが 4 つある**（確かめる対象 / 成功の基準 / 送る中身 / 払いの記録）ので Spec を切ると判断し、
  利用者が「② は後で」と保留した。本 Spec はその起票
- 利用者裁定（2026-10-20）: 未決 3 つを推奨どおりに閉じた — **統計画面へは入れない**（D4）/ **ダミーの関数は
  添えない**（D3）/ **確かめるのは保存済みのテンプレートと登録済みの鍵だけ**（D1）。P0 への着手はまだ指示されていない
- 覆す凍結: 無い。Spec 52 の `RetryClass`（再試行の分類 9 値）は 1 値も変えない — 表示のための分類は
  接続テストの層に別に持つ（D6）。`chat_with_backoff` の再試行の輪にも触らない（接続テストは輪の外の
  1 往復の口を使う。D5）

## rev1 からの変更（査読 7 点の反映。詳細は Notes 6）

- **「鍵だけ未登録」は起きる**（点 1）— rev1 の現況は向きが逆だった。API キーの入力欄 `secretInput` は
  フッターの「保存」と独立しており、テンプレートは保存済みで鍵だけ入力欄に残る形が 2 経路で作れる。
  **ボタンを押せない条件に `secretInput.trim() !== ""` を足した**（D1・D8）
- **結果を消す契機を `reseedDraft` の 1 点にした**（点 2）— 鍵の登録（`saveSecret`）と削除（`clearSecret`）は
  `save()` を通らないので、rev1 の「保存で消す」では漏れていた（D8）
- **成功の分け方を実経路に合わせた**（点 3）— 7 ワイヤの decode は `EmptyResponse` も `Blocked` も作らない。
  「200 だが本文が空」は使用量つきの `Ok` で返るので、`no_text` は `Ok` の空本文から作り、使用量を持つ（D2）
- **再試行の輪を通らない 1 往復の口 `chat_once` を足す**（点 4）— rev1 の「`max_retries = 1` の複製で `chat`」では
  `llm retry stop:` 行と、60 秒超の待ち要求で本文の先頭に書き足される日本語の文が残る（D5）
- **原文の運び方を経路ごとに決めた**（点 5）— `degraded_reason` は包み直さない / `Parse` は受け取った本文の先頭を
  運ぶ / 画面の分類の 1 行は理由の文を含めない（D6・D7）
- **P1 のテストの数え方を直した**（点 6）— `Api` から写るのは 6 値（D6・P1）
- **結果の型を outcome ごとの判別共用体にした**（点 7）— `note: string | null` をやめ、`limit` と `finish` を
  構造で運ぶ。コアは訳した文字列を入れない（D7・D11）

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
  （`orchestrator.upsertTemplate`）/ **鍵の登録**（`saveSecret` — `upsertTemplate` を先に呼んでから
  `setCredential`）/ **鍵の削除**（`clearCredential`）/ **単価の取得**（`fetchModelPrices`）/ **削除** の 5 つ。
  **疎通を確かめる操作は無い**
- 画面が持つ検査は `checkBaseUrlMismatch`（`lib/providerSkills`）の 1 つだけ — 「他社の既定 base URL が
  そのまま残っている」ときだけ警告する。鍵が正しいか・モデル名が接続先に在るかは、どこにも出ない
- **未保存の判定（dirty）が無い。** 閉じるときの確認も無い（`askConfirm` の呼び出しは鍵の削除とテンプレートの
  削除の 2 箇所だけ）。2026-10-19 に未保存の確認を直したのは `AgentSettingsDialog.vue` で、このダイアログではない
- **鍵の入力欄 `secretInput`（`ref("")`、166 行）はテンプレートの下書き `draft` の欄ではなく、フッターの「保存」
  （`save()`）から独立している**（rev2 で訂正。点 1）。`secretInput` を空にするのは `saveSecret` の `finally`
  （203 行）だけで、`save()` も選択の切り替え（`edit()` / `create()`、483〜491 行）も触らない。そのため
  **「テンプレートは保存済み・鍵だけ入力欄に残って未登録」が作れる**:
  - (a) 新規作成（`blank()` の `credential` は `"unset"`）で鍵を打ち、横の「登録」ではなくフッターの「保存」を押す
  - (b) 既存のテンプレートで新しい鍵を打ち、「登録」を押す前に別の操作をする
  - rev1 は「`saveSecret` がテンプレートを先に保存するので『鍵だけ未保存』は起きない」と書いたが、
    それは「鍵だけ保存されてテンプレートが未保存」が起きない、の向きで、逆向きは起きる
- **`saveSecret` と `clearSecret` は `save()` を通らず `reseedDraft(id)` を呼ぶ**（198・225 行）。`reseedDraft` は
  `save()` も呼ぶので、保存済みの値で下書きを作り直す点は 3 経路で 1 つ

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
- **`degraded_reason` は完成した表示文**（rev2 で追記。点 5）— `HttpBackendFactory::create`（`client.rs:927`）が
  `err.to_string()` で作るので、`LlmError` の Display の接頭辞（「LLM 設定エラー: 」、HTTP クライアントの
  組み立てに失敗したときは「HTTP エラー: 」）を既に含む
- **複製をキャッシュへ入れない**（assist.rs のコメント — id が同じなので、入れると村の個体が
  スキル無しのバックエンドを掴む）。接続テストも同じ理由で `factory.create` を直接呼ぶ
- ツール無しで全ワイヤへ送る前例が 2 つある — `use_tools` が偽のテンプレートの通常のターンと、
  手動要約（`summarize_agents`。「要約の呼び出しにツールは提示しない」）

### 呼び出しの経路と失敗の型

- `LlmBackend::chat` → `chat_inner` → **`chat_with_backoff`**（`llm/client.rs:581`）→ 1 往復ごとに `attempt`
  （345 行。要求を組んで送り、ステータスと `error_signal` を読み、decode まで行う）。再試行は
  `max_retries`（既定 3）回で、429 では明示値を下限に最長 60 秒待つ（Spec 52）。**そのまま呼ぶと、
  接続テストのボタンが最長で数分沈黙しうる**
- **`chat_with_backoff` は回数に関わらず 2 つのことをする**（rev2 で追記。点 4）:
  - 非一過性の `Api`（400 / 401 / 403 / 404 / `insufficient_quota`）では、返す前に **`llm retry stop:` 行**を出す（612〜618 行）
  - 待ち時間の判定（`plan_wait`）は回数の判定より先に来る。**429 の明示値が 60 秒
    （`MAX_HONORED_RETRY_AFTER`）を超えると `StopHintTooLong`** になり、`with_hint_too_long`（790 行）が `Api.body` の
    先頭に**日本語固定の文**「プロバイダは N 秒後の再試行を求めています。…」を書き足す（英語の村でも日本語）。
    **→ 2026-10-19 に撤去**（本文は書き換えない。秒数は `LlmError::retry_after_too_long()` で読み、画面は
    `ErrorPayload.retryAfterSecs`）。D5 の `chat_once` の理由のうち残るのは `llm retry stop:` 行の重複だけ
- **空の本文は `Ok` で返る**（rev2 で追記。点 3）。7 ワイヤとも decode の後に `reject_empty_reasoning`
  （`openai_compat.rs:349`、`client.rs:501〜562` の 7 箇所）を通り、**本文が空・ツール呼び出しが無い・`finish = Length`
  の 3 つが揃ったときだけ** `OutputTruncated { limit, usage }` になる。それ以外の空の本文は
  `Ok(ChatResponse { text: None か空白 })` で、**使用量を持つ**
  - Gemini の `finishReason: SAFETY` も `Finish::Other` の `Ok` で返る（`gemini.rs:544`。安全フィルタの名前は残らない）
  - **`LlmError::Blocked` を作る箇所は実経路に 0**（`error.rs` の定義と表示だけ）。`EmptyResponse` は
    `chat_with_backoff` の到達しない末尾（661 行）と、`assist.rs:173`（`Ok` を受けてから `Classified::Empty` で変える）だけ
- タイムアウトは `request_timeout_secs`（既定 120 秒）。最大出力トークンの既定は 8,192
- `LlmError` は 8 値（`llm/error.rs`）。コードは `LLM_CONFIG` / `LLM_HTTP` / `LLM_API` / `LLM_EMPTY_RESPONSE` /
  `LLM_OUTPUT_TRUNCATED` / `LLM_BLOCKED` / `LLM_NO_STRUCTURED_OUTPUT` / `LLM_PARSE`
- **`LlmError::Api` は `class: RetryClass` を持つが、画面へは届かない。** IPC のエラーは `ErrorPayload`
  （`code` / `message` / `detail` / `agentId` / `retryable`）で、**API の失敗は鍵の誤りもモデル名の誤りも
  レート制限も全部 `LLM_API`** になる。分類を画面に出すには、接続テストの結果の型に載せて運ぶ必要がある
- `RetryClass`（`llm/retry.rs:75`）は 9 値 — `RateLimit` / `Overloaded` / `ServerError` / `Timeout` /
  `Network` / `Auth`（401・403）/ `Billing`（code が `insufficient_quota` のときだけ）/ `ClientError`
  （それ以外の 4xx）/ `ContextLength`。**「モデルが無い」に当たる値は無い** — 404 は `ClientError`
- **`Api.class` は `retry::classify(status, code)`（136 行）だけが決め、`Timeout` / `Network` を返さない**
  （rev2 で追記。点 6）。この 2 値は `Http` の計器（`RetryFacts::of`）でだけ使われる
- `LlmError::usage()` が `Some` を返すのは `OutputTruncated` だけ
- 応答の JSON をこのプロトコルの形として解釈できなかったとき（200 だが形が違う）は `LlmError::Parse { source, raw }`
  （`client.rs:496` ほか）。**表示は「構造化出力のパースに失敗しました: {source}」**で、`raw`（受け取った本文）は
  表示にも `ErrorPayload` にも載らない（rev2 で追記。点 5）
- エラー本文は `MAX_ERROR_BODY`（2,000 バイト）で切る（`client.rs:460`。文字境界の問題は Notes 5）

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
- **S6** 鍵を入れ忘れた → 「テンプレートからバックエンドを組めませんでした」+ 理由「API キーが未設定です」。
  **エコー応答で成功と出さない**
- **S7** 未保存の変更がある → ボタンは押せず「保存してから確かめます」と出る
- **S8**（rev2）鍵を入力欄に打ったまま「登録」を押していない → ボタンは押せず「API キーを登録してから確かめます」と出る
- **S9**（rev2）接続テストが通った直後に鍵を削除した → 「繋がりました」の結果が消える

## Design

### D1. 確かめる対象 — 保存済みのテンプレート（2026-10-20 裁定で決定。下書きへは広げない。rev2 で前提を訂正）

- **確かめるのは `world.json` に保存済みのテンプレートと、資格情報ストアに登録済みの鍵。** IPC は `templateId`
  だけを受け取り、コアが `world.template(&id)` で読む。**未保存の間はボタンを押せない** — 未保存とは次の 2 つ:
  - 下書き `draft` が保存済みの値と違う（新規の下書きを含む）
  - **鍵の入力欄 `secretInput` に文字が残っている**（rev2。点 1 — `isTemplateDirty` では検知できない。
    `secretInput` は `draft` の欄ではないため）
- 理由:
  - **下書きを確かめる形にすると、未保存の鍵を IPC で渡すことになる。** 今の鍵の経路は
    `setCredential` → 資格情報ストアの 1 本だけで、「入力欄の値をそのまま LLM 呼び出しに使う」経路は無い。
    確認のために 2 本目を作ると、鍵が UI のメモリに載ったまま往復する経路が増える
  - **確認と本番の写しを 1 つにする**（Spec 59 の `scorer_for` と同じ規律）。下書きで確かめると、
    「確認は通ったが保存した値は別」が作れる。保存済みを確かめれば、確かめたものがそのまま個体の使うもの
  - **入力欄に鍵が残っている状態で確かめると、古い鍵（または鍵なし）の結果が「今打った鍵の結果」に読まれる。**
    rev1 は「`saveSecret` がテンプレートを先に保存するので鍵だけ未保存は起きない」と書いたが、向きが逆だった
    （現況の (a) (b)）
- 代償: 試すたびに保存が要る（「保存 → 接続テスト → 直す → 保存」）。保存は既存のボタン 1 回で、
  保存しても稼働中の個体の挙動は次のターンから変わるだけ
- **未保存の判定を新設する**（このダイアログに初めて入る）— `lib/templateDraft.ts` の純関数
  `isTemplateDirty(draft, saved)`。比較の規則を 1 実装にしてテストで留める（`temperature` の `null` と
  空欄、単価の `null` と未入力が同じ値に揃っているか）。閉じるときの確認には使わない（範囲外。Notes 5）

### D2. 成功の基準 — 接続先が 2xx を返し、選んだプロトコルの応答として解釈できた（推奨。rev2 で表を実経路に合わせた）

**「1 トークンだけ返させる」は採らない。** 推論モデルは出力の予算を思考に使うので、最大出力を 1 にすると
本文が空のまま `finish = length` で終わる。最大出力に下限を持つ接続先もある（OpenAI Responses の
`max_output_tokens` に下限があると記憶しているが、この村では未測定 — P0 で撃つ）。どちらも**設定は正しいのに
失敗と出る**。

判定は `chat_once`（D5）の結果を `LlmError` の網羅 match で振り分ける（`_ =>` を書かない — 9 値目が足されたら
ここが落ちる）:

| `chat_once` の結果 | 判定 | `outcome` | 運ぶもの |
|---|---|---|---|
| `Ok`、本文（trim 後）が空でない | 成功 | `replied` | 本文の先頭 200 字（コードポイント）・使用量 |
| `Ok`、本文が空（`None` か空白） | 成功 | `no_text` | `finish`（`stop` / `length` / `tool_use` / `other`）・**使用量** |
| `Err(OutputTruncated { limit, usage })` | **成功** | `truncated` | `limit`・使用量 |
| `Err(EmptyResponse)` / `Err(Blocked { .. })` | 成功 | `no_text` | `finish: null`・使用量 `null`。**実経路では到達しない腕**（網羅のため。現況） |
| ファクトリの `Err` / `degraded_reason` | 失敗 | `config` | 原文（D7） |
| `Err(Config(_))` | 失敗 | `config` | 原文 |
| `Err(Http { .. })`（`is_timeout`） | 失敗 | `timeout` | 原文 |
| `Err(Http { .. })`（その他） | 失敗 | `network` | 原文 |
| `Err(Api { .. })` | 失敗 | D6 の表 | `status`・`retryAfterSecs`・原文 |
| `Err(Parse { raw, .. })` | 失敗 | `unexpected_shape` | **受け取った本文の先頭**（D7） |
| `Err(NoStructuredOutput)` | 失敗 | `unexpected_shape` | 原文（強制しないので到達しない腕） |

- **rev1 の表は `Ok` を一律 `replied` にし、`no_text` を `EmptyResponse` / `Blocked` から作っていた**（点 3）。
  実経路ではその 2 つは作られず、空の本文は `Ok` で来るので、rev1 のままだと**本文 `null` の `replied` ができ、
  `no_text` は到達不能**だった。`no_text` は使用量を持つ（rev1 の「使用量は分からない」は誤り）
- **`truncated` を成功に数える理由**: 接続先は鍵・モデル名・パラメータを受け付けて 200 を返している。
  切れたのは上限の値の問題で、本番でも同じ上限で切れうることを注記として出す（失敗として出すと
  「繋がらない」と読まれる）
- **`no_text` を成功に数える理由**: 同じく 200 が返っている。注記で「本文が無かった」ことと終了理由は隠さない。
  Gemini の安全フィルタ（`SAFETY`）は decode で `other` に畳まれるので、画面は「安全フィルタ」と名指しできない
- 成功の文面は「使えます」ではなく**「この設定で接続先が応答しました」**にする（D3 の確かめないものがあるので、
  「使える」と書くと保証の範囲を広げて読ませる）

### D3. 送る中身 — 固有スキルとツールを外し、思考段階と出力上限はテンプレートのまま（2026-10-20 裁定で決定。ダミーの関数は添えない）

`ChatRequest` はこう組む:

| 欄 | 値 | 理由 |
|---|---|---|
| テンプレート | `without_provider_skills()` の複製（rev2: `max_retries` は触らない — 再試行の輪を通らないため。D5） | 固有スキルは検索・pro モードの固定費を払う（Spec 63 P0: gpt-6-sol は外すと入力 11,854 → 170 トークン）。スキルが通るかは別の問い |
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
    今はアダプタが吸収している）のように、tools を送った要求でだけ 400 になる形は捕まえない（旧未決 2。2026-10-20 に
    「ダミーの関数を添えない」で決定 — 添えるとモデルがそれを呼ぶことがあり、定義の分だけ入力が増える）
  - 固有スキル（Gemini の `includeServerSideToolInvocations` の門など）/ 添付（Spec 36 の carries 表）/
    長い入力（`contextLength`）/ 本番のシステムプロンプトの大きさとキャッシュ
  - レート制限の余裕（波は個体の人数ぶん RPM を食う — 2026-08-25 の実測。1 回の確認では分からない）
  - 再試行の輪の挙動（D5 — 接続テストは輪を通らない）

### D4. 払いの記録 — ログに 1 行。統計へは入れない（2026-10-20 裁定。旧未決 1。rev2 で `-` の規則を直した）

**`template probe:` 行**（1 回の確認ごとに 1 行。成功でも失敗でも出す）:

```text
template probe: template=<id> wire=<provider> model=<model> outcome=<replied|truncated|no_text|config|auth|…>
  finish=<stop|length|tool_use|other|-> status=<HTTP|-> class=<RetryClass|-> code=<provider code 先頭 40 字|->
  elapsed_ms=<n> prompt=<n|-> completion=<n|-> reasoning=<n|->
```

- **書かないもの**: 送った文・応答の本文・接続先のエラー本文・受け取った本文（`Parse` の `raw`）・API キー・
  base URL。分類と数だけ（`failures.md` #71 の線。エラー本文には接続先が書いた文がそのまま入る）
- **使用量の欄に `-` を書くのは、使用量を持たない結果だけ** — 失敗（`config` / `timeout` / `network` / `Api` 由来 /
  `unexpected_shape`）と、`no_text` のうち到達しない腕（`EmptyResponse` / `Blocked`）。`replied` / `truncated` /
  `Ok` 由来の `no_text` は数を書く（rev2。点 3）。**0 を捏造しない**（#72 の規律。`-` は「分からない」で、
  `0` は「払っていない」と読まれる）
- **同じ確認で他に出る行**: `truncated` のときは `attempt` の中の `reject_empty_reasoning` が `truncated:` 行を先に出す
  （1 呼び出しの計器で、本番のターンと同じ）。`llm retry:` と `llm retry stop:` は出ない（D5）
- **村の予算（`tokenBudget`）と個体の累計には入れない** — 村の仕事の因果ではなく、人が押した設定の操作
  （Spec 63 D2 と同じ線）
- **画面にもその場で出す**（S1 の「入力 14 / 出力 2 / 思考 0 トークン」）。払いが見える場所が 2 つになる
- **統計画面（`Record`）へは入れない**（2026-10-20 裁定。旧未決 1）— 理由は Notes 4。代償として統計の `≈ $` に
  接続テストの払いは入らない（リリースノートの「利用者が負う条件」に書く）

### D5. 再試行の輪を通らない 1 往復の口 `chat_once`（rev2。点 4 — rev1 の「`max_retries = 1` の複製」をやめた）

- **`LlmBackend` に `chat_once(req)` を足す。既定の実装は `self.chat(req)`**、上書きするのは `HttpLlmBackend` だけで、
  中身は `self.attempt(&req)` の 1 回（要求を組んで送り、ステータスを読み、decode と `reject_empty_reasoning` まで）。
  形は Spec 52 の `chat_cancellable` と同じ（既定の実装つきで足し、57 箇所の実装を触らない）
- **rev1 の「`max_retries = 1` の複製で `chat` を呼ぶ」では 2 つが残った**（現況）:
  - 非一過性の `Api` で `llm retry stop:` 行が出て、`template probe:` 行と同じ事象を 2 行で書く
  - 429 の明示値が 60 秒を超えると `with_hint_too_long` が `Api.body` の先頭に日本語固定の文を書き足し、
    画面の「接続先は N 秒後を指定しています」と二重になる（英語 UI でも本文に日本語が混ざる）
- `chat_once` はどちらも起こさない。**輪の外に出るのは待ち・再送・計器 2 行・本文の書き足しだけ**で、要求の組み立てと
  decode は本番の 1 往復と同じ `attempt` を通る（Spec 59 の `scorer_for` の規律 — 確認と本番で写しを割らない）
- **429 も 5xx も、接続テストでは結果そのものが情報**（レート制限なら設定は通っている可能性が高い）。待たずに返す
- 429 で接続先が待ち時間を明示していれば（`LlmError::Api.retry_after`）、**60 秒を超えても値のまま**運び、
  画面に添える（「接続先は {N} 秒後を指定しています」）
- タイムアウトは `request_timeout_secs`（既定 120 秒）のまま — タイムアウトの設定も本番と同じ値で確かめる。
  押した後は「確かめています…（最長 {N} 秒）」と出す
- JPEG の再エンコードの再送（Spec 23 D3。`chat_inner` の中）も通らない — 接続テストは添付を送らないので影響しない

### D6. 失敗の分類 — 接続テストの層に表示用の閉じた列挙を置く（rev2 で写す元の数と 1 行の文を直した）

`RetryClass` は**再試行の判定**のための 9 値（Spec 52 で凍結・`verdict` が網羅 match）。「モデルが無い」を
足すと再試行の表まで触ることになるので、**表示の分類は別の列挙 `ProbeFailure` に置き、`(LlmError, status,
class, code)` から写す**。

| `ProbeFailure` | 写す元 | 画面の 1 行（辞書 `modelTemplate.probe.failure.*`） |
|---|---|---|
| `config` | ファクトリの `Err` / `degraded_reason` / `Config` | テンプレートからバックエンドを組めませんでした。下の理由を直して保存し直してください |
| `auth` | `Api`・`RetryClass::Auth`（401 / 403） | 認証で拒否されました。API キーを確かめてください |
| `not_found` | `Api`・`ClientError` かつ status 404 | モデル名か base URL のパスが接続先にありません |
| `rejected` | `Api`・404 以外の `ClientError` / `ContextLength` | 接続先が要求の形を拒否しました（思考段階・最大出力トークン・温度など）。下の本文が理由を名指しすることがあります |
| `rate_limit` | `Api`・`RateLimit` | レート制限に当たりました。設定は通っている可能性があります |
| `billing` | `Api`・`Billing` | 残高かクォータが不足しています |
| `server_error` | `Api`・`ServerError` / `Overloaded` | 接続先の一時的な障害です |
| `timeout` | `Http`（`is_timeout`） | {N} 秒以内に応答がありませんでした |
| `network` | `Http`（その他） | base URL に届きませんでした |
| `unexpected_shape` | `Parse` / `NoStructuredOutput` | 200 が返りましたが、選んだプロトコルの応答の形ではありません。プロトコルの選択を確かめてください |

- **`Api` から写るのは 6 値**（`auth` / `not_found` / `rejected` / `rate_limit` / `billing` / `server_error`。rev2。点 6）。
  `Api.class` は `classify` だけが決め、`Timeout` / `Network` を返さない（現況）。`Api` の腕の中の `RetryClass` の
  網羅 match には `Timeout` / `Network` の腕も書くが、**型の網羅のための腕で到達しない**（写し先は `timeout` /
  `network`）。残りの 4 値（`config` / `timeout` / `network` / `unexpected_shape`）は `Api` 以外から生まれる
- **画面の 1 行は理由の文を含めない**（rev2。点 5）。理由は D7 の `error` だけが運び、画面は `formatError` を通して
  1 行の下に出す — rev1 は `config` の 1 行に「理由の文をそのまま」と書き、`formatError` と同じ文が 2 回並ぶ形だった
- **写しは純関数 1 本**（`template_probe.rs` の `classify_failure`）で、`LlmError` と `RetryClass` の両方を
  網羅 match で書く（`_ =>` を書かない）
- **「残高不足」の判定は狭い** — `Billing` になるのは code が `insufficient_quota` のときだけ（Spec 52）。
  Anthropic の「残高が足りない」は 400 の `invalid_request_error` で返ると記憶しており、その場合は
  `rejected` に落ちる（未測定）。**本文を必ず一緒に出すのはこのため** — 分類が外れても接続先の文が理由を言う
- ~~**鍵の誤りが `auth` にならない接続先がある見込み** — Gemini の不正な鍵は 401 ではなく 400 の
  `INVALID_ARGUMENT`（詳細に `API_KEY_INVALID`）で返ると記憶している（未測定）。~~ **→ P0 で外れた**: Gemini は 401
  `UNAUTHENTICATED`。鍵の誤りが 400 で返るのは xAI だけ、モデル名の誤りが 400 で返るのはさくらと Perplexity。
  **プロバイダの code で補う腕は足さない**（P0 実測記録。status だけで鍵は 7 中 6、モデル名は 7 中 5 が当たり、外れる 3 つは
  本文が理由を名指しする）

### D7. コアと IPC（rev2 で結果の型と原文の運び方を直した）

**コア**: `Orchestrator::probe_template(&ModelTemplateId) -> CoreResult<TemplateProbe>`
（`orchestrator/template_probe.rs`）+ 純機構 `crates/fuseforks-core/src/template_probe.rs`
（定型文 ja/en・`classify_failure`・結果の型・本文の先頭の切り出し）+ `LlmBackend::chat_once`（D5）。

```ts
type TemplateProbeUsage = { prompt: number; completion: number; reasoning: number };
type ProbeFinish = "stop" | "length" | "tool_use" | "other";

/** outcome ごとに持つ欄が決まる（rev2。点 7 — rev1 の `note: string | null` をやめた）。 */
type Reached =
  | { outcome: "replied";   text: string;  usage: TemplateProbeUsage }        // text は先頭 200 字
  | { outcome: "truncated"; limit: number; usage: TemplateProbeUsage }        // 適用されていた上限
  | { outcome: "no_text";   finish: ProbeFinish | null;                       // 到達しない腕では null
                            usage: TemplateProbeUsage | null };

type TemplateProbe =
  | ({ result: "reached"; elapsedMs: number } & Reached)
  | { result: "failed";
      failure: ProbeFailure;        // D6 の 10 値
      status: number | null;        // Api のときだけ
      retryAfterSecs: number | null;// Api で接続先が明示したときだけ（60 秒を超えても値のまま）
      error: ErrorPayload;          // 原文。画面は formatError に通す
      elapsedMs: number };

testModelTemplate(templateId: ModelTemplateId): Promise<TemplateProbe>;   // IPC `test_model_template`
```

- **LLM の失敗は `Err` ではなく `result: "failed"` で返す。** `CoreError` で返すと画面に届くのは
  `LLM_API` だけで、D6 の分類が落ちる（現況の「`class` は画面へ届かない」）
- `Err(CoreError)` を返すのは確認を始められないときだけ — テンプレートが無い（`MODEL_TEMPLATE_NOT_FOUND`）
- **コアは訳した文字列を結果に入れない**（rev2。点 7）。画面が `outcome` / `failure` / `finish` から辞書の鍵を選び、
  `limit` / `status` / `retryAfterSecs` / 秒数を引数として埋める
- **`error`（原文）の作り方は経路ごとに 1 つ**（rev2。点 5）:

  | 経路 | `error` |
  |---|---|
  | `Api` / `Http` / `Config` / ファクトリの `Err` | `ErrorPayload::from(&CoreError::Llm(err))`（既存の表示と同じ） |
  | `degraded_reason` | `{ code: "LLM_CONFIG", message: degraded_reason }` — **`LlmError::Config` に包み直さない**（`#[error("LLM 設定エラー: {0}")]` が二重に掛かる）。HTTP クライアントの組み立ての失敗も文が「HTTP エラー: 」で始まるだけで、ここに入る |
  | `Parse { raw, .. }` | `{ code: "LLM_PARSE", message: raw の先頭 2,000 字（コードポイントで切る）, detail: serde の理由 }` — **Display の「構造化出力のパースに失敗しました」を使わない**（構造化出力を使わない確認で出ると誤った方向を指す）。受け取った本文（間違った base URL が返した HTML など）がいちばん効く手掛かり |
  | `NoStructuredOutput` | 既存の表示（到達しない腕） |

- IPC の名前は先例 `test_jev` に揃える。GUI の `commands.rs` から `state.host.orchestrator.probe_template` を呼ぶ
- **呼び出しは並べない** — 同じテンプレートへの 2 回目は画面の `busy` で塞ぐ（コアには排他を置かない。
  別のテンプレートを同時に確かめる理由も、それを禁じる理由も無い）

### D8. 画面（rev2 で押せない条件と結果を消す契機を直した）

- ボタンはダイアログのフッターの左端（右に「キャンセル」「保存」）。文言「接続テスト」
- **押せないとき**（`disabled` + 理由を `title` に）:
  - 新規の下書き（`isNewDraft`）/ 未保存の変更がある（`isTemplateDirty`）→「保存してから確かめます」
  - **鍵の入力欄に文字がある（`secretInput.trim() !== ""`）→「API キーを登録してから確かめます」**（rev2。点 1）
  - 確かめている最中
- 結果はフッターの上に 1 区画。成功は `--color-run`、失敗は `--color-fail`、`truncated` / `no_text` の注記は
  `--color-warn`。失敗は D6 の 1 行の下に `formatError(error)` を出す。応答の本文と原文は**素のテキスト**で出す
  （`v-html` を使わない）
- **結果は押したテンプレートに結び付け、次の契機で消す**（rev2。点 2）:
  - **`reseedDraft` の中**（保存済みの値で下書きを作り直す 1 点）— `save()` / `saveSecret`（鍵の登録）/
    `clearSecret`（鍵の削除）の 3 経路がここを通る。rev1 の「保存で消す」では、鍵の登録と削除が `save()` を通らないので
    **接続テストが通った直後に鍵を削除しても「繋がりました」が残った**
  - 選択の切り替え（`edit()` / `create()`）・テンプレートの削除
- 編集を始めただけでは消さない（結果は「保存済みの設定」についての事実のまま正しい。ボタンは押せなくなる）
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

### D11. 言語（rev2 で辞書の鍵を 3 組にした）

定型文（D3）と画面の文言は村の `language` に従う（Spec 35）。画面の文は辞書から引く — 
`modelTemplate.probe.reached.{replied|truncated|no_text}` / `modelTemplate.probe.failure.{D6 の 10 値}` /
`modelTemplate.probe.finish.{stop|length|tool_use|other}`。**辞書の鍵を実行時に組む**ので、3 つの列挙と ja / en の
鍵の突き合わせを走査テストで留める（`blackboardLanesWiring` と同じ形 — 欠けても vue-tsc も i18n の
コンパイル検査も通る）。

## 採らなかった形

- **編集中の下書きを確かめる** — D1。未保存の鍵が IPC を往復する経路が増え、確認と保存の写しが 2 つに割れる
- **1 トークンだけ返させる** — D2。推論モデルと出力の下限を持つ接続先で、正しい設定が失敗と出る
- **固有スキルを付けたまま確かめる** — D3。スキルの固定費を払い（検索 1 回の課金を含む）、
  しかも「スキルが通るか」は接続とは別の問い
- **思考段階を外して確かめる** — D3。確認は通るのに、本番の最初のターンで思考段階の 400 が出る
- **再試行してから結果を出す** — D5
- **`max_retries = 1` の複製で `chat` を呼ぶ**（rev1）— D5。`llm retry stop:` 行と、60 秒超の待ち要求での日本語の
  書き足しが残る（点 4）
- **`with_hint_too_long` の書き足しを接続テストのために直す** — 本番のターンの表示（英語の村でも日本語になる）に
  触ることになり、本 Spec の範囲を越える（Notes 5 の別件）
- **`degraded_reason` を `LlmError::Config` に包み直す** — D7。表示の接頭辞が二重になる（点 5）
- **`EmptyResponse` / `Blocked` に独立した outcome を与える** — D2。実経路に作る箇所が 0 で、到達しない値を
  画面の語彙に増やすことになる。作る decode が足されたら、そのとき分ける
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

### P0 — 測ってから凍結する（**完了 2026-10-20**。下の「P0 実測記録」）

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
- `data_contract.yaml` へ `template_probe_contract`（D1〜D10 の凍結: 保存済みのテンプレートと登録済みの鍵だけ /
  成功の基準 / 送る中身 / 再試行の輪を通らない `chat_once` / 閉じた分類 / 原文の運び方 / ログに本文・鍵・base URL を
  書かない / 予算と個体の累計に入れない / キャッシュに入れない / `degraded_reason` を成功にしない / 押したときだけ）と
  `entities` の `TemplateProbe` / `ProbeFailure` / `ProbeFinish`

### P0 実測記録（2026-10-20）

**予測（撃つ前に書いた）**。テンプレートは村のもの（OpenAI 互換 = Qwen3-VL-30B（さくら）/ Anthropic = claude-opus-5-5（low）/
Gemini = gemini-3.5-flash-lite / xAI = grok-4.5（grok-4.7・medium）/ OpenAI = gpt-6-sol（gpt-6.1-sol・medium）/
Meta = muse-spark-1.2（low）/ Perplexity = perplexity（deepseek-v4-pro））。`max_retries` は 1 にして撃つ（`chat_once` の前の代用）。

| ワイヤ | 正しい設定 | 鍵を 1 文字変える | モデル名を 1 文字変える |
|---|---|---|---|
| OpenAI 互換（さくら） | replied | 401 → auth | 404 → not_found |
| Anthropic | replied（思考 0〜100） | 401 → auth | 404 → not_found |
| Gemini | replied | **400 `INVALID_ARGUMENT`（API_KEY_INVALID）→ rejected** | 404 → not_found |
| xAI | replied（思考あり） | **400 → rejected** | 404 → not_found |
| OpenAI Responses | replied（思考あり） | 401 → auth | **400 `model_not_found` → rejected** |
| Meta | replied | 401 → auth | 400 → rejected |
| Perplexity | replied | 401 → auth | 400 → rejected |

- 最大出力 1（gpt-6.1-sol）: **400（`max_output_tokens` の下限 16）**
- 空の `tools` + `ToolChoice::None`: 7 ワイヤとも `tools` / `tool_choice`（Gemini は `toolConfig`）を送らない

**結果**（`crates/fuseforks-core/examples/template_probe_p0.rs`。使い捨てで、撃った後に消した）。太字が予測の外れ:

| ワイヤ | 正しい設定（所要・入力/出力/思考） | 鍵を 1 文字変える | モデル名を 1 文字変える |
|---|---|---|---|
| OpenAI 互換（さくら） | OK「OK」1.6 秒・22 / 2 / 0 | 401 `Auth` → auth | **400 `ClientError`「This model is not available.」→ rejected** |
| Anthropic | OK 1.4 秒・27 / 4 / 0 | 401 `authentication_error` → auth | 404 `not_found_error` → not_found |
| Gemini | OK 1.2 秒・13 / 1 / 0 | **401 `UNAUTHENTICATED` → auth** | 404 `NOT_FOUND` → not_found |
| xAI | OK 1.6 秒・**1,255（キャッシュ 1,152）/ 83 / 82** | 400 `ClientError`（本文の `code` は `invalid-argument`）→ rejected | 404 → not_found |
| OpenAI Responses | OK 3.0 秒・20 / 5 / 0 | 401 `invalid_api_key` → auth | **404 `model_not_found` → not_found** |
| Meta | OK 3.2 秒・21 / **170 / 160** | 401 `invalid_api_key` → auth | **404 `model_not_found` → not_found** |
| Perplexity | OK 2.2 秒・38 / 12 / 0 | 401 `invalid_api_key` → auth | 400 `invalid_request`（本文がモデル名を名指し）→ rejected |

- 最大出力 1（gpt-6.1-sol）: 400 `integer_below_min_value`「Expected a value >= 16」— **予測どおり**。D2 の「1 トークンだけは採らない」の根拠が実測になった
- オフライン（ループバックへ撃って本文を見た）: 7 ワイヤとも `tools` / `tool_choice` / `toolConfig` / `functionDeclarations` を**送らない** — 予測どおり
- **1 回の確認の値段**: 7 ワイヤ中 5 つは入力 13〜38・出力 1〜12 トークン。高いのは xAI（接続先が入力へ 1,200 トークン余りを足す。大半はキャッシュ）と
  Meta（思考段階 low でも思考 160）。どちらも 1 回で数百トークンの桁で、D4 の「払いはログと画面で見せる」で足りる
- **D6 の決定 — プロバイダの code で `auth` / `not_found` を補う腕は足さない。** status だけで鍵の誤りは 7 中 6（外れは xAI の 400）、
  モデル名の誤りは 7 中 5（外れはさくらと Perplexity の 400）が当たる。外れる 3 つの本文はどれも理由を名指しする
  （「Incorrect API key provided」/「This model is not available.」/「model … is not supported」）ので、本文を必ず一緒に出す D6 の
  処方で読める。補うなら xAI のトップレベルの `code: "invalid-argument"` を見ることになるが、これは鍵以外の誤りにも出る汎用の値で、
  補うと別の 400 を「鍵の誤り」と誤って名指しする
- **観察（範囲外）**: OpenAI は不正な鍵の 401 本文に**鍵の末尾 4 文字**を返す（`sk-proj-****…abcd`）。接続テストは `template probe:` 行に
  本文を書かない（D4）ので、ログには入らない。画面には `formatError` で出る — これは今の会話の失敗のトーストと同じで、利用者自身の
  画面に、利用者が入れた（誤った）鍵の末尾が出るだけ。**ただし通常のターンの失敗は `turn failed:` 行が本文ごと `fuseforks.log` に
  書いている**（Notes 5 の最後の項と同じ観察）

### P1 — コア（rev2 でテストの数え方を直した。点 6）

- `LlmBackend::chat_once`（既定の実装は `chat`。`HttpLlmBackend` は `attempt` の 1 回で上書き）
- `template_probe.rs`（純機構）: 定型文 ja/en / `classify_failure`（網羅 match）/ 結果の型 / 本文の先頭の切り出し
  （コードポイントで切る — Notes 5 の `truncate` の形を作らない）
- `orchestrator/template_probe.rs`: `probe_template` — 読む → 複製（スキル無し）→ `factory.create` →
  `degraded_reason` の検査 → `chat_once` → 判定 → `template probe:` 行
- テスト（D2 の表の行ごと。数えるのは結果の腕で、`ProbeFailure` の値の数ではない）:
  - **成功の腕 5**: `Ok` 本文あり → `replied` / `Ok` 本文空（`finish` が `stop` と `other`）→ `no_text` で使用量あり /
    `OutputTruncated` → `truncated` / `EmptyResponse`・`Blocked` → `no_text` で使用量 `null`（純関数の単体）
  - **`Api` の腕**: `RetryClass` の到達しうる 7 分類 + 404 の分け = 8 行で、写し先は 6 値
    （`Auth`→`auth` / `ClientError` 404→`not_found` / `ClientError` 400→`rejected` / `ContextLength`→`rejected` /
    `RateLimit`→`rate_limit` / `Billing`→`billing` / `ServerError`・`Overloaded`→`server_error`）
  - **`Api` 以外の失敗の腕**: ファクトリの `Err` / `degraded_reason` / `Config` → `config`、`Http` 2 種 → `timeout` /
    `network`、`Parse` / `NoStructuredOutput` → `unexpected_shape`。合わせて `ProbeFailure` の 10 値がすべて現れる
  - **退避型のファクトリで鍵の無いテンプレートが `config` で返り、エコー応答で成功しない**。`error.message` に
    「LLM 設定エラー: 」が 1 回だけ現れる
  - `Parse` の `error.message` が受け取った本文の先頭で、「構造化出力」の語を含まない（多バイト文字の途中で切っても panic しない）
  - **`HttpLlmBackend::chat_once` が輪を通らない** — ループバックの最小 HTTP スタブ（`tests/attachment_fallback.rs` の形）で
    429 + `Retry-After: 120` を返すと、待たずに 1 回で返り、`retry_after` が 120 秒のまま、`body` に書き足しが無く、
    `llm retry` で始まる行が出ない
  - 要求が D3 の表どおり（固有スキル偽・tools 空・effort と上限がテンプレートの値・発話 1 通）
  - ログ行に本文と鍵が出ない・使用量の `-` が D4 の規則どおり・キャッシュ（`backend_for`）に複製が入らない・
    ワイヤ形の凍結（`tests/ipc_contract.rs`）

### P1 実装記録（2026-10-20）

- `LlmBackend::chat_once`（既定 = `chat`。`HttpLlmBackend` だけ `attempt` の 1 回で上書き）/ 純機構
  `crates/fuseforks-core/src/template_probe.rs`（`judge` が `LlmError` と `RetryClass` を `_ =>` なしの網羅 match で振り分ける・
  `factory_failure` / `degraded_failure` / `ProbeFacts` / `log_line`）/ `Orchestrator::probe_template`
  （`orchestrator/template_probe.rs`）
- **2026-10-20 に足された `LlmError::ImagesRejected`**（JPEG フォールバックの日本語固定の修正）も網羅 match に入る。接続テストは
  添付を送らないので到達しない腕で、写し先は `rejected`（status 400）
- テスト: 単体 13（成功の腕 4・`Api` の 8 行・`Api` 以外の失敗・`Parse` の本文の先頭と多バイト・退避の接頭辞 1 回・
  ログ行に本文と鍵の末尾が出ない・ワイヤ形・定型文の言語）/ 結合 6（`tests/template_probe.rs` — D3 の要求・英語の定型文・
  **複製がキャッシュに入らず個体はスキルつきのバックエンドを掴む・確認の払いが個体の累計に入らない**・LLM の失敗は `Err` に
  ならない・テンプレートが無いときだけ `Err`・**退避型のファクトリでエコー応答を成功と出さない**）/ HTTP 1
  （`tests/template_probe_once.rs` — 429 + `Retry-After: 120` で 1 回だけ撃ち、`llm retry` の行を出さず、`retry_after` を 120 の
  まま運び、`template probe:` 行に本文と base URL を書かない）/ ワイヤ形の凍結 1（`tests/ipc_contract.rs`）
- **ミューテーション 4 回とも予測どおり** — `chat_once` を輪つきへ戻す → HTTP の 1 本 / 退避の理由を無視する → 退避の 1 本 /
  固有スキルを外さない → 送る中身とキャッシュの 2 本 / 404 の分けを潰す → 写しの単体と形の 2 本
- **作業の罠**: 変異の戻しに `git checkout --` を使い、**まだ追跡されていない新しいファイルには効かず**変異が残った（SHA の
  照合で気づいて手で戻した）。以後は逐語の置換スクリプトで入れて戻し、SHA で確かめた

### P2 実装記録（2026-10-20）

- IPC `test_model_template`（`commands.rs` / `lib.rs`）と型 `TemplateProbe` / `ProbeFailure` / `ProbeFinish`（`types.ts`）/
  `lib/templateDraft.ts` の `isTemplateDirty`（欄が無いことと `null` を同じに扱う）/ `lib/templateProbe.ts` の
  `probeBlockedKey`・`probeView`・`probeFinishKey`（辞書の鍵を返す純関数）/ `ModelTemplateDialog.vue` のフッター左端の
  「接続テスト」と結果の区画（`data-template-probe`）/ 辞書 `modelTemplate.probe.*`（ja / en。`json` の往復がバイト等価で
  あることを先に確かめてから挿した）
- 結果を消すのは `reseedDraft`（保存・鍵の登録・鍵の削除）/ `edit` / `create` / 削除。確認中に別のテンプレートへ移ったら
  返ってきた結果を捨てる。テンプレートが無いときの `Err` は `formatError` の文で区画に出す
- テスト: 純関数 12（`templateProbe.test.ts`）/ 走査 6（`templateProbeWiring.test.ts` — 呼び出しが click の 1 箇所・起動経路が
  `probe_template` を呼ばない・押せない条件が鍵の入力欄を読む・結果の消去が `reseedDraft` の中・`formatError` を通し `v-html` を
  使わない・3 つの列挙の鍵が ja / en に在る・**TS の 10 値が Rust の `ProbeFailure::as_str` と一致**）
- **ミューテーション 3 回とも予測どおり 1 本だけ赤**（押せない条件から鍵の入力欄を落とす / `reseedDraft` の消去を外す /
  純関数が鍵の入力を無視する）。Rust 1,381 全緑（GUI の crate は除外）・clippy 0（GUI の crate を含む）・vitest 845・build 緑
- **実機は未確認**（P4）

### P2 — 画面（rev2 で押せない条件と消す契機を足した）

- IPC `test_model_template` と型 / `lib/templateDraft.ts` の `isTemplateDirty`（単体）/ ボタンと結果の区画 /
  辞書 ja/en（`modelTemplate.probe.*` の 3 組）
- 押せない条件に `secretInput.trim() !== ""` / 結果を消すのは `reseedDraft` と選択の切り替えと削除
- 走査テスト: 呼び出しが click の 1 箇所だけ（D9）/ 3 つの列挙と辞書の鍵の突き合わせ（D11）/
  エラー本文を `formatError` に通している / `v-html` を使っていない / **`disabled` の条件が `secretInput` を読んでいる** /
  **結果を消す処理が `reseedDraft` の中にある**

### P3 — 台帳

- DETAIL 3 言語（モデル登録の節に接続テストと「確かめないもの」）/ README 3 言語は既存のモデル登録の行を
  延ばす（行を足さない）/ PRIVACY は改訂しない（D10）

### P4 — 実機

1. 保存済みのテンプレートで押す → `template probe: outcome=replied` と画面の使用量が一致する
2. 鍵を 1 文字変えて「登録」→ 押す → `auth`（P0 の表で `rejected` になるワイヤはその表示）と接続先の本文
3. モデル名を 1 文字変えて保存 → `not_found`
4. 鍵を削除する → `config`（**会話にエコー応答が出ない**ことも見る）。理由の文が画面に 1 回だけ出る
5. 未保存の変更があるとボタンが押せず、保存すると押せる / 別のテンプレートを選ぶと結果が消える
6. ダイアログを開いただけ・保存しただけでは `template probe:` 行が出ない
7. （rev2）鍵を入力欄に打って「登録」を押さずにいると押せない / 接続テストが通った直後に鍵を削除すると結果が消える

## 未決

**無い**（2026-10-20 に利用者が 3 つとも推奨どおりに閉じた）。閉じた 3 つと、決めたこと:

1. ~~統計画面（`Record`）へ入れるか~~ → **入れない**（D4・Notes 4）
2. ~~`use_tools` が真のテンプレートで、ダミーの関数を 1 本添えるか~~ → **添えない**（D3。「ツールを送ったときだけ
   出る拒否」（#77 の形）は確かめないものとして画面の注記と DETAIL に書く）
3. ~~確かめる対象を下書きへ広げるか~~ → **広げない**（D1。保存済みのテンプレートと登録済みの鍵だけ。
   「保存してから確かめるのは面倒」が実機で出たら再検討）

rev2 で未決の増減は無かった（査読 7 点はどれも実コードで決着し、利用者の判断が要る点は出なかった）。

## Notes

### 1. 先例 `test_jev` との対照

| `test_jev`（Spec 59） | 本 Spec |
|---|---|
| 設定が OFF でも押せる | 割り当てていないテンプレートでも押せる（村の個体の状態に依らない） |
| `scorer_for` を本番と共有 | `factory.create` と `attempt`（要求の組み立てと decode）を本番と共有。違うのは固有スキル・ツール・再試行の輪だけ（rev2） |
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

### 4. 統計へ入れない理由（旧未決 1。2026-10-20 に推奨どおり決定）

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
- **鍵の入力欄 `secretInput` は選択の切り替えで空にならない**（rev2 で追記。点 1 を確かめる途中で出た）—
  `edit()` / `create()` は `draft` と `selectedId` だけを差し替えるので、テンプレート A で打った鍵が、B を選んだ後も
  入力欄に残り、そのまま「登録」を押すと **B の鍵として保存される**。本 Spec では D8 の押せない条件で接続テスト側だけ
  塞ぐ（B の確認が「API キーを登録してから」で止まる）。入力欄を選択の切り替えで空にするかは別件
- **`with_hint_too_long`（`client.rs:790`）の書き足しは日本語固定**（rev2 で追記。点 4）— 本番のターンでも、
  英語の村のエラー表示に日本語の文が混ざる。Spec 35（二言語化）の網から漏れている。接続テストは `chat_once` で
  ここを通らない。**→ 2026-10-19 に修正**（`with_hint_too_long` を撤去し、文は画面と失敗の注記が言語ごとに組む。
  CLAUDE.md の現在地（2026-10-19 更新）が正）。同じ形の JPEG フォールバックの文（`client.rs` の「この接続先は画像を
  受け付けません」）は未修正。選択の切り替えで鍵の入力欄が残る件（1 つ上）と `client.rs` の `truncate` の件も同日に修正
- `RetryFacts` の doc が「本文は `turn failed` 行が既に運んでいる」と書いている — 接続先のエラー本文は
  `turn failed:` 行（`orchestrator/mod.rs:1557` が `payload.message` をそのまま書く）経由で `fuseforks.log` に入っている。
  PRIVACY の 6 は「プロンプトの本文」「ツール結果の本文」を記録しないと書いているが、エラー本文には触れていない。
  本 Spec の `template probe:` 行は本文を書かない

### 6. rev1 の査読の反映（2026-10-19。7 点 → 採用 7。うち点 4 は処方を変えた）

| # | 指摘（要約） | 確かめた所 | 判定 | 反映 |
|---|---|---|---|---|
| 1 | 「`saveSecret` が先に保存するので鍵だけ未保存は起きない」は逆向き。`secretInput` はフッターの「保存」から独立しており、新規でキーを打って「保存」/ 既存で新しい鍵を打って「登録」前に押す、の 2 つで鍵だけ未登録が起きる。`isTemplateDirty` では検知できない | `ModelTemplateDialog.vue` 166 行（`ref("")`）/ 203 行（空にするのは `saveSecret` の `finally` だけ）/ 451 行（`blank()` の `credential: "unset"`）/ 514〜528 行（`save()` は `secretInput` に触らない）/ 483〜491 行（選択の切り替えも触らない） | **採用** | 現況・D1・D8・S8・P2・P4。**確かめる途中で別件 1 つ**（選択を切り替えても入力欄が残り、別のテンプレートの鍵として登録できる。Notes 5） |
| 2 | 結果を消す契機に `saveSecret` / `clearSecret` が漏れている（どちらも `save()` を通らず `reseedDraft` を呼ぶ） | 同 198 行（`saveSecret` → `reseedDraft`）/ 225 行（`clearSecret` → `reseedDraft`）/ 521 行（`save()` → `reseedDraft`） | **採用** — 処方は査読の 2 案のうち `reseedDraft` の中で消す形（3 経路が通る 1 点に置けば、4 本目の経路を足しても漏れない） | D8・S9・P2・P4 |
| 3 | 7 ワイヤの decode と `reject_empty_reasoning` は `EmptyResponse` も `Blocked` も返さない。空の本文は `Ok` で来るので、`Ok` を一律 `replied` にすると本文 `null` の `replied` ができ、`no_text` は到達不能。`Ok` の空本文を `no_text` にすると使用量はあるので「`-` を書く」と矛盾 | `openai_compat.rs:349-375`（`Length` かつ空のときだけ `OutputTruncated`）/ `client.rs:501〜562`（7 ワイヤ全部が通る）/ `Blocked` の生成は `llm/` の実装に 0 件 / `EmptyResponse` は `client.rs:661`（到達しない末尾）と `assist.rs:173` だけ / `gemini.rs:544`（`SAFETY` → `Finish::Other`） | **採用** | D2 の表・D4 の `-` の規則・D7 の型・P1 |
| 4 | `max_retries = 1` でも `chat_with_backoff` は非一過性の `Api` で `llm retry stop:` 行を出す（D5 と食い違い、`template probe:` 行と重複）。429 で 60 秒超の明示値は `with_hint_too_long` が本文の先頭に日本語固定の文を書き足し、D5 の表示と二重 | `client.rs:597-618`（`stop("")` は回数の判定の前）/ 629-633（`StopHintTooLong` は `attempt + 1 >= attempts` の腕より先）/ 790-813（`with_hint_too_long`） | **採用（処方は変えた）** — 査読は症状の指摘で、処方は `LlmBackend::chat_once` を足して輪ごと通らない形にした（`max_retries` の複製をやめる）。輪を通したまま 2 行と書き足しを受け入れる案は「採らなかった形」へ | D5・D3・D4・P1・Notes 1 |
| 5 | (a) `degraded_reason` は既に「LLM 設定エラー: 」付きで、`LlmError::Config` に包み直すと二重になり、`config` の 1 行でも `formatError` と同じ文が並ぶ。(b) `Parse` の表示は「構造化出力のパースに失敗しました」で `raw` は載らないので、接続先の本文が出ず、構造化出力を使わない確認でその語が出る | (a) `client.rs:927`（`err.to_string()`）・`error.rs` の `#[error("LLM 設定エラー: {0}")]`（`HttpLlmBackend::new` の失敗は `Http` なので「HTTP エラー: 」で始まることもある）(b) `error.rs` の `Parse` の `#[error]` と、`ErrorPayload::from` の `detail` が source 連鎖だけを畳むこと | **採用** | D6（1 行は理由を含めない）・D7（原文の作り方の表）・P1 |
| 6 | 「`Api` を `ProbeFailure` の 10 値ぶん」は数が合わない。`config` と `unexpected_shape` は `Api` から生まれず、`Api` から写るのは最大 8 値（`Timeout`・`Network` を除けば 6 値） | `retry.rs:136-154`（`classify` は `Timeout` / `Network` を返さない）/ `error.rs:138-147`（`LlmError::api` も `classify` を通る） | **採用** — 到達しうるのは 6 値。8 は型の網羅の腕を数えた値 | D6・P1（結果の腕ごとに数える形へ） |
| 7 | `failed` は `status` / `retryAfterSecs` を構造化しているのに、`reached` は `limit` と `Blocked` の理由を `note: string \| null` に押し込んでいる。画面が辞書を引くのかコアが訳すのかが D7 と D11 で食い違う | rev1 の D7 の型と D11 の文面 | **採用** — outcome ごとの判別共用体にし（`limit` / `finish` を構造で運ぶ）、コアは訳した文字列を入れない。`Blocked` は点 3 で到達しない腕になったので outcome を分けない | D7・D11・P2 |

**rev1 で外した形の共通点**: 7 点のうち 5 点（1・3・4・5・6）は、**部品の名前と doc から振る舞いを書き、呼び出しの中身を
1 段下まで読んでいなかった**ことから出た — `saveSecret` の doc（先に保存する）を読んで逆向きを数えなかった /
`LlmError` の variant の一覧から成功の表を作り、どの decode がそれを作るかを数えなかった / `max_retries` の意味から
輪の挙動を書き、輪の中の計器と書き足しを読まなかった / `degraded_reason` と `Parse` の表示の文面を確かめなかった /
`RetryClass` の 9 値を `Api.class` の値域と取り違えた。CLAUDE.md の「doc を読んで実装を読まなかった」（Spec 56 P1）と同じ形。
