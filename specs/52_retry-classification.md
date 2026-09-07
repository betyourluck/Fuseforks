# Spec: LLM 再試行の分類と待ち時間 — 閉じた分類・サーバーの明示値・jitter・計器

- 起票: 2026-09-07
- 状態: **rev1（査読待ち）**
- 起点: 利用者 —「breaker.rs の純関数を参考にしましょう」（2026-09-07。busbar =
  github.com/GetBusbar/busbar `4f7e9b0` の実読。CLAUDE.md「先行実装の調査」の
  8 実装目）。前史は 2026-08-25 の実測（CLAUDE.md「波の fan-out はプロバイダの
  レート制限を人数ぶん速く食う」）— `chat_with_backoff` の天井 5 秒はサーバーが
  明示する `retryDelay: "56s"` より短く、**その 429 は再試行しても構造的に必ず失敗
  する**と書いたうえで「機構は作らない・頻度を見てから」と裁定していた。
  今回はその裁定を利用者が覆した（Notes 2）

## Goal

`llm/client.rs` の再試行を、**status の範囲**（429 / 5xx）で決める形から
**閉じた分類**で決める形へ変え、サーバーが明示した待ち時間を下限として尊重し、
再試行そのものを計器に出す。

1. **分類** — 429 でも `insufficient_quota`（OpenAI は 429 で返す）は再試行しない。
   401 / 403 は今も止まるが、分類名を持たないので計器に「なぜ止めたか」が出ない
2. **待ち時間の下限** — `Retry-After` ヘッダ（全ワイヤ）と Gemini の本文
   `google.rpc.RetryInfo.retryDelay` を読み、指数バックオフとの大きいほうを待つ。
   **天井を持ち、天井を超える要求は再試行せず理由を本文に書く**（D3）
3. **jitter** — 波で同時に落ちた個体が同じ瞬間に再送しないよう待ちを散らす
4. **計器** — `llm retry:` の 1 行。**今は再試行がどのログ行にも出ない**

**やらないこと（範囲外）**: フェイルオーバー（村は個体 = 1 ワイヤ 1 モデルで、
別レーンへ逃がす先が無い）/ ブレーカーの状態機械（Open / HalfOpen。待機中の
個体は課金されないので「落ちているレーンを避ける」問題が村に無い）/
`error_map` の設定化（busbar は catalog の YAML で持つが、村は閉じた列挙を
コードに置く — 増やすときはコミットが記録になる。`refusal.rs` の語彙表と同じ判断）/
`max_retries` の意味変更（既定 3 のまま）/ ヘルスプローブ。

## 起票時の実測（2026-09-07。コードとログを読んだ）

**現行の再試行**（`crates/fuseforks-core/src/llm/client.rs:536-555`）:
`attempts = max_retries.max(1)`（既定 3 = `model.rs:794`）、`is_transient()` が真なら
`200ms × 2^attempt` を 5 秒でクランプして sleep。**既定では 200 + 400 ms = 合計 0.6 秒しか
待たない。** `is_transient`（`error.rs:140-149`）= HTTP 障害（timeout / connect / request）
/ `Api` の 429 と 5xx / `EmptyResponse`。408 は 4xx なので非一過性。

**ヘッダは捨てている**: `client.rs:444-454` は `response.status()` だけ読んで
`LlmError::Api { status, body }` を作る。`Retry-After` は crate 全体で 0 ヒット。
`LlmError::Api` のパターンは **2 ファイル 5 箇所**（`client.rs` / `error.rs`）で、
欄を足す変更は小さい。

**計器はゼロ**: `fuseforks.log`（14,450 行・2026-08-09〜09-07）で `retry` に当たる
4 行は**全部プロバイダの本文の引用**。2026-08-11 12:32 の 529 は `tool:` の 16.5 秒後に
`turn failed` だが、**その間に何回試したかはログから読めない**。

**頻度**（同じログ）: `code=LLM_API` の失敗 9 件 = fatal 6（401 ×3 / 404 ×2 / 400 ×1）+
非 fatal 3（**429 ×2 = Gemini 無料枠** / **529 ×1 = Anthropic overloaded**）。`LLM_HTTP` 3。
2026-08-25 から数字は動いていない（頻度ゲートの側は変わらず 3 件）。

**Gemini の 429 本文**（実物）: `error.status = "RESOURCE_EXHAUSTED"`、`error.details[]` に
`"@type": "type.googleapis.com/google.rpc.RetryInfo"` + `"retryDelay": "56s"`（もう 1 件は
`"51s"`）。同じ配列に `google.rpc.QuotaFailure`（`quotaId:
GenerateRequestsPerMinutePerProjectPerModel-FreeTier`）と `google.rpc.Help`。
**`Retry-After` ヘッダが付いていたかは記録が無いので分からない** — P0 で計器を先に足す理由。

**busbar 側（scratchpad の実読）**:

| 関数 | 場所 | 中身 |
|---|---|---|
| `StatusClass` 9 値 / `Disposition` 4 値 / `classify` | `crates/busbar/src/breaker.rs:22-56, 99-110` | 網羅 match（`_ =>` 禁止）。RateLimit / Overloaded / ServerError / Timeout / Network → Transient、Auth / Billing → HardDown、ClientError → ClientFault、ContextLength → 別枠 |
| `parse_retry_after` | `breaker.rs:132-149` | RFC 9110 の delay-seconds と HTTP-date の**両方**。過去の日付は 0（「今すぐ」であって「とても長い」ではない） |
| `normalize_raw_error` | `breaker.rs:153-266` | プロバイダの JSON `code` / `type` を `error_map` で分類 → `context_length_exceeded` は **400 / 413 のときだけ** → status 既定（401 / 403 Auth・429 RateLimit・408 Timeout・**529 Overloaded**・5xx Server・他 4xx Client・2xx / 3xx が来たら Client 扱いで記録しない） |
| `compute_cooldown_with_retry_after` | `crates/busbar/src/store/in_memory/breaker.rs:281-390` | `base × 2^n` を u128 で飽和計算 → `max` でクランプ → **±10% の jitter（帯は最低 1 秒）** → `[d/2 (最低 1), max]` へクランプ → `Retry-After` を**下限**として `max`、ただし天井 `max_honored_retry_after_secs`（既定 86,400）でクランプ |

**busbar も本文の `retryDelay` は読まない**（`retryDelay` は crate 全体で 0 ヒット。
各 proto reader は `retry_after: None`）。読むのは HTTP ヘッダだけ。**村の実機で
観測した 2 件はどちらも本文側**なので、そのまま写すと観測した事象に効かない。

**打ち切りとの関係**: `turn.rs:1429` の `backend.chat(request).await` は `select!` を
持たず、cancel の検査は周回境界（`turn.rs:1357`）。**待ちを伸ばすと打ち切りが
その分遅れる** — `run` に `ToolContext.cancel` を足したとき（Spec 15 P2「葉で 1 箇所
だけ見るのは周回境界の検査を増やすことではない」）と同じ形が LLM 呼び出しにも要る（D4）。

## Design

### D1: 分類は閉じた列挙 `RetryClass`。status 既定は `llm/retry.rs`、プロバイダの code は adapter の純関数

新設 `crates/fuseforks-core/src/llm/retry.rs`（純関数のみ・I/O 無し）:

```rust
pub enum RetryClass {
    RateLimit,      // 429
    Overloaded,     // 529（Anthropic 固有。IANA 未登録だが文書化されている）
    ServerError,    // 5xx
    Timeout,        // 408 / reqwest の is_timeout
    Network,        // reqwest の is_connect / is_request
    Auth,           // 401 / 403
    Billing,        // プロバイダ code（下の表）
    ClientError,    // 上記以外の 4xx・2xx / 3xx がエラー経路へ来た形
    ContextLength,  // 400 / 413 かつプロバイダ code が文脈超過
}
pub enum Verdict { Retry, Stop }
pub fn classify(status: u16, signal: Option<&ErrorSignal>) -> RetryClass;  // 網羅 match
pub fn verdict(class: RetryClass) -> Verdict;                               // 網羅 match
```

| `RetryClass` | `Verdict` | 今との差 |
|---|---|---|
| RateLimit / Overloaded / ServerError / Network | Retry | 同じ（529 は今 5xx として拾っている） |
| Timeout（408） | Retry | **変わる**（今は 4xx として止める） |
| Auth / ClientError / ContextLength | Stop | 同じ結果。**分類名が計器に出る**ようになる |
| **Billing** | **Stop** | **変わる**（今は 429 なら再試行していた。OpenAI の `insufficient_quota` は 429） |

**status 以外の材料は adapter が出す**（`client` は wire の中身を見ない —
`data_contract` の `llm_wire.layers`）。各 adapter に純関数を 1 本:

```rust
pub struct ErrorSignal { pub code: Option<String>, pub retry_after: Option<Duration> }
pub fn error_signal(body: &str) -> ErrorSignal;   // openai_compat / anthropic / gemini
```

| ワイヤ | `code` の出所 | Billing と読む値 | ContextLength と読む値 |
|---|---|---|---|
| OpenAI 互換・Responses 4 本 | `error.code` → 無ければ `error.type` | `insufficient_quota` | `context_length_exceeded` |
| Anthropic | `error.type` | `billing_error`（**未確認** — Notes 4） | `invalid_request_error` は文脈超過に限らないので**読まない** |
| Gemini | `error.status` | 無し（`RESOURCE_EXHAUSTED` は RateLimit。busbar と同じ） | 無し |

**閉じた列挙で、表に無い code は status 既定へ落ちる**（busbar の「error_map に無ければ
HTTP 分類へフォールスルー」と同じ向き。未知の code で止めない）。

### D2: 待ち時間の下限はサーバーの明示値。出所は 2 つ

- **(a) `Retry-After` ヘッダ** — 全ワイヤ。`client.rs:445` の `response` から取れる
  （client の責務 = 「URL・ヘッダ・タイムアウト・再試行」）。delay-seconds と
  HTTP-date の両方を読む。過去の日付は 0
- **(b) Gemini の本文 `error.details[]`** — `@type` が
  `type.googleapis.com/google.rpc.RetryInfo` の要素の `retryDelay`。protobuf `Duration` の
  JSON 形（`"56s"` / `"0.5s"`）。adapter の純関数（`gemini::error_signal`）が読む

`LlmError::Api` へ `retry_after: Option<Duration>` を 1 欄足す。**(a) と (b) の両方が
あれば大きいほう**。

### D3: 待ち = max(指数バックオフ + jitter, 明示値)。天井 60 秒。天井を超える明示値は再試行しない

```rust
pub const MAX_HONORED_RETRY_AFTER: Duration = Duration::from_secs(60);
pub enum WaitPlan { Wait(Duration), StopHintTooLong(Duration) }
pub fn plan_wait(attempt: u32, hint: Option<Duration>, jitter_unit: f64) -> WaitPlan;
```

- **指数部は据え置き**（`200ms × 2^attempt`、5 秒でクランプ）
- **jitter は ±10%、帯の最低幅 20 ms**（200 ms の 10% が 20 ms。busbar の「帯 ≥ 1 秒」は
  秒建ての cooldown 向けで、ミリ秒建ての村ではそのまま写すと帯が本体より大きくなる）。
  `jitter_unit ∈ [-1, 1]` は**引数で受ける**（`schedule.rs` と同じ「内部で時計や乱数を
  読まない」規律。本番は `SystemTime` のナノ秒 + attempt の FNV-1a で作る = busbar と
  同じ形。**`rand` を core に足さない**）
- **明示値は下限**（`max`）。busbar と同じ
- **天井 60 秒で、明示値がそれを超えたら `StopHintTooLong` = 再試行せず止める。**
  **ここが busbar と違う**（あちらは cooldown をクランプして次の要求へ進む）。理由:
  村の待ちは**飛行中のターンの中**で起きる。「3 時間後に再試行せよ」（日次の quota）を
  60 秒でクランプして再送しても必ず失敗し、失敗するまでの 60 秒を利用者が払う。
  止めて本文に「プロバイダは N 秒後の再試行を求めています」と書けば、次の手が人に渡る
  （fail-closed。Spec 46 の「判定不能は宛先を持たない」と同じ向き）。60 秒の根拠 =
  実機の 2 件が 56 / 51 秒（分単位の quota 窓）で、これを通す最小の切り
- **天井は code constant**（Spec 13「`OrchestratorConfig` を出さない」・`budget.rs` の
  重みと同じ扱い）。設定にするなら頻度を見てから

### D4: 待ちは打ち切りで切れる

長い待ちを入れる以上、**打ち切り（Spec 10）が待ちの終わりまで遅れる**のは退行。
`LlmBackend::chat` に cancel を渡す口を足す:

```rust
async fn chat(&self, req: ChatRequest, cancel: Option<CancellationToken>) -> Result<ChatResponse, LlmError>;
```

- `HttpLlmBackend` は sleep を `select!` で token と競わせ、切れたら **`LlmError` の
  新 variant を作らず**、最後のエラー（`last_error`）をそのまま返す。ターンループは
  周回境界で `is_cancelled()` を見て `interrupted` へ落とすので、**再試行の中で
  切られたことに固有の名前は要らない**（打ち切りの分類は 1 箇所 = `turn.rs:1357`）
- `ChatRequest` には載せない — `PartialEq` を導出しており `CancellationToken` は
  比較できない。adapter の純関数の入力に token が混ざるのも層が違う
- **実装は 10 箇所**（`HttpLlmBackend` / `EchoBackend` / 結合テストの 8 バックエンド）。
  全部 `_cancel` で受けるだけの機械的変更。`EchoBackend` は待たないので使わない
- **採らない形**: 天井 60 秒だけで済ませる（打ち切りが最長 60 秒遅れる。Spec 10 の
  実機「要求から 0.0 秒」を 1 箇所だけ破る）。**利用者裁定の対象**（Notes 5）

### D5: 計器 `llm retry:` — 再試行するときと、分類で止めるときの 2 形

```text
llm retry: model=gemini-3.5-flash-lite attempt=1/3 status=429 class=rate_limit hint=56s src=body wait=56000ms
llm retry stop: model=gpt-5.6-terra status=429 class=billing code=insufficient_quota hint=- src=-
llm retry stop: model=… status=429 class=rate_limit hint=10800s src=header reason=hint_too_long
```

- `src=header|body|both|-` — **P0 でこれだけ先に出す**（ヘッダの有無が今は読めない）
- `code=` はプロバイダ code の先頭 40 字。本文は出さない（#71 の規律 — エラー本文は
  `turn failed` 行が既に運んでいる）
- `attempt` は 1 始まり・分母は `max_retries`。`turn failed` 行と突き合わせれば
  「何回試して落ちたか」が読める

### D6: `is_transient` は残し、`Api` の腕だけ分類へ委ねる

`is_transient` は `CoreError::is_retryable` / UI の分岐が読んでおり（`data_contract` の
`error_contract`）、名前も呼び出し元も変えない。中身の `Api` の腕を
`verdict(classify(status, signal)) == Retry` へ差し替える。**`Http` と `EmptyResponse` の
腕は据え置き。**

### D7: 触る台帳

- `data_contract.yaml` — `llm_wire.invariants` の「再試行対象は is_transient のみ（HTTP
  障害 / 429 / 5xx / 推論の空応答）」を分類表へ書き換え / 新設 `retry_contract`
  （分類 9 値と verdict・明示値の出所 2 つ・天井・jitter 帯・打ち切り）/
  `observability_rule` へ `llm retry:` 行
- `error.rs` のモジュール doc 3 項「`is_transient` が再試行の唯一の判断軸」
- DETAIL 日英 — 再試行の記述（`grep 再試行 DETAIL.md`）と**「利用者が負う条件」**:
  429 の直後にターンが最長 60 秒静かになることがある（画面は「入力中」のまま）
- CLAUDE.md — 「先行実装の調査」に busbar の節 / 「波の fan-out」の節の
  「機構は作らない」を取り消し線で覆す / 現在地
- README は触らない（設定もトグルも増えない）。**ランディングページと Qiita は
  再試行に触れていない**（grep 網の外だが嘘にならない）

## Stories

- **S1** Gemini 無料枠の 429（`retryDelay: "56s"`）→ 56 秒 ± jitter 待って 1 回再送、
  `llm retry:` に `hint=56s src=body` が出る。2 回目も 429 なら 3 回目は無い（既定 3）
- **S2** OpenAI の `insufficient_quota`（429）→ 再送せず `llm retry stop: class=billing`。
  `turn failed` は今と同じ 1 本
- **S3** Anthropic 529 → 今と同じく再送。`Retry-After` があれば `src=header`
- **S4** 待ちの最中に「■ 停止」→ 1 秒以内に `turn interrupted`（D4 採用時）
- **S5** 5 体の波が同時に 429 → 5 本の `wait=` が全部違う値（jitter）
- **S6** 明示値が 60 秒超 → 再送せず `reason=hint_too_long`、本文に秒数が載る

## Phases

- **P0 計器と契約**: `llm retry:` 行を**現行の再試行に**先に足す（分類も待ちも変えない。
  `src=` はヘッダを読むだけ）→ 実機で 1 件でも 429 を踏めば D2 の (a) の有無が決まる。
  `data_contract` の `retry_contract` を凍結
- **P1 コア**: `retry.rs`（`classify` / `verdict` / `plan_wait` / `parse_retry_after`）+
  adapter の `error_signal` 3 系統 + `LlmError::Api.retry_after` + `client.rs` の配線。
  単体: 9 値 × verdict の網羅 / 過去日付の HTTP-date は 0 / `"0.5s"` の parse /
  jitter の帯 / 天井超えは Stop。**結合はループバック HTTP スタブ**
  （`tests/attachment_fallback.rs` と同じ作り）: 429 + `Retry-After: 1` → 2 回目で 200
  （受信本文の件数 = 2）/ 429 + `insufficient_quota` → 1 回で止まる（件数 = 1）/
  429 + Gemini 形の `RetryInfo` 本文 → 待ちの下限が効く / 529 → 再送。
  **ミューテーション 2 回**（`verdict` を全部 Retry へ → billing の 1 本だけ赤 /
  `max` を外す → 下限の 1 本だけ赤）
- **P2 打ち切り**: D4 の trait 変更 + 10 実装 + 結合 1 本（待ち中に cancel →
  200 ms 以内に返る）
- **P3 台帳**: D7
- **P4 実機**: 検収項目

## 検収項目（各項目に到達経路を書く）

1. **無料枠の Gemini 鍵で 2 体へ波を撒く**（2026-08-25 の再現）→ `llm retry:` に
   `status=429 class=rate_limit hint=NNs src=body|both` が出る。経路: Gemini 429 →
   `gemini::error_signal` が `RetryInfo` を読む → `plan_wait` → sleep
2. 同じ走行で `wait=` が `hint` 以上・60,000 ms 以下
3. 同じ走行で 2 体の `wait=` が異なる（jitter）。**1 体だけでは判定にならない**
4. **待ちの最中に「■ 停止」** → `turn interrupted` が 1 秒以内（P2 採用時）。経路:
   `select!` が token を拾う → `last_error` を返す → 周回境界で `interrupted`
5. `insufficient_quota` は**実機で踏めない**（残高がある鍵しか無い）→ P1 の結合テストが
   代替（`attachment_fallback` と同じ判断: 踏めない経路をテスト無しで残さない）
6. 529 は Anthropic の混雑時にしか出ない → **狙わない**。出たら `src=` を読む
7. **既存の走行が変わらないこと**: 429 も 5xx も出ない通常のターンで `llm retry:` が
   1 本も出ない（負の対照）

## Notes

1. **busbar から持ち込まないもの**: ブレーカー状態（Closed / Open / HalfOpen）と
   cooldown の永続 — 村の「レーン」は個体 1 体で、落ちている間に避ける先が無い /
   failover の walk / `error_map` の YAML / `ContextLength` を「別の大きいモデルへ
   逃がす」分類（村は同じ個体で再送しない。分類名だけ計器のために持つ）/
   ヘルスプローブ（`user "ping" max_tokens 1` は安いが、待機中の個体は課金されないので
   「黙って死んでいるレーン」が村に無い）
2. **2026-08-25 の「機構は作らない」を覆す理由**: 利用者裁定（2026-09-07）+ 参照実装で
   実装の値段が純関数 1 本に下がった。頻度は今も 3 件。**効き目は「失敗が減る」ではなく
   「サーバーが教えた値を無視しない」と「再試行が見える」の 2 つ** — 2026-08-25 の
   「`retryDelay` を読んで待っても RPM 15 は 2 体 × 10 周には足りない」は今も正しい。
   S1 で 1 呼び出しは通るが、周を回せば次の 429 でまた 56 秒待つ。無人の予定なら
   遅くても完走し、対話なら利用者が止める — どちらへ倒すかは D3 の天井が決めている
3. **写すのは構造で、コードは書き直す**（Apache-2.0 → MPL-2.0 は互換だが、逐語で
   持ち込まないので NOTICE も不要。doc コメントに出典を 1 行書く）
4. **Anthropic の `billing_error` は未確認**（文書にも実機にも当てていない。Anthropic の
   `error.type` で実機に出たのは `overloaded_error` だけ）。P1 で表へ入れるなら
   **入れた根拠を書く**か、入れずに status 既定へ落とす。**busbar の `error_map` も
   Anthropic には空**（`providers.yaml:23-26`）— あちらも持っていない
5. **利用者裁定が要るもの**: D3 の天井 60 秒と「超えたら止める」/ D4 を P2 でやるか
   天井だけで済ませるか / D1 の 408 を Retry へ動かすか（実機で 408 は 0 件）
6. **計器を先に出す順序（P0）が本体より重い判断**: D2 の (a) が実際に付くかは
   ヘッダを記録しないと永遠に分からず、`src=` の実測が無いまま (a) を実装すると
   「入れたのに効いているか分からない機構」になる（#47 の規律）
