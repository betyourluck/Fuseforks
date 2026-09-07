# Spec: LLM 再試行の分類と待ち時間 — 閉じた分類・サーバーの明示値・jitter・計器

- 起票: 2026-09-07
- 状態: **rev2 承認（2026-09-07。査読 2 系統 16 点 → 採用 12 / 訂正して採用 2 /
  反証 1 / 裁定へ 1。記録は Notes 7。承認時の裁定 = 408 は据え置き Stop / 承認査読の
  追加 1 点 = jitter と天井の順序を Notes 8 へ）→ P0 完了（2026-09-07。記録は
  「P0 実装記録」）→ P1 完了（同日。記録は「P1 実装記録」）→ P2 完了（同日。記録は
  「P2 実装記録」— D4 の形を加算的な既定メソッドへ訂正）→ P3 完了（同日。記録は
  「P3 台帳記録」）→ P4 完了 = Done**（2026-09-08。実機検収 7 件 = 観測 5（1・2・3・
  4・7）/ 結合が代替 1（5）/ 狙わない 1（6）。**検収 4 で D4 の穴が出て同日に塞いだ** —
  待ちは 2 ms で切れたが分類が `failed:LLM_API` だった。記録は「P4 実機記録」。
  **起票から Done まで 2 日**）
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
   **天井を持ち、天井を超える明示値は再試行せず理由を本文に書く**（D3）
3. **jitter** — 波で同時に落ちた個体が同じ瞬間に再送しないよう待ちを散らす
4. **計器** — `llm retry:` の 1 行。**今は再試行がどのログ行にも出ない**

**今と変わる挙動は 2 つだけ**: `Billing` を再試行しなくなること / 明示値を下限として
待つこと。それ以外（401 / 403 / 400 / 404 が止まる・429 / 5xx / 529 が再送される）は
結果が同じで、**分類名が計器に出る**ようになるだけ。408 は据え置き（D1。2026-09-07 裁定で決着）。

**やらないこと（範囲外）**: フェイルオーバー（村は個体 = 1 ワイヤ 1 モデルで、
別レーンへ逃がす先が無い）/ ブレーカーの状態機械（Open / HalfOpen。待機中の
個体は課金されないので「落ちているレーンを避ける」問題が村に無い）/
`error_map` の設定化（busbar は catalog の YAML で持つが、村は閉じた列挙を
コードに置く — 増やすときはコミットが記録になる。`refusal.rs` の語彙表と同じ判断）/
`max_retries` の意味変更（既定 3 のまま）/ ヘルスプローブ / **HTTP 往復そのものの
打ち切り**（D4。払いの記録を失うので Spec 10 の境界のまま）。

## 起票時の実測（2026-09-07。コードとログを読んだ）

**現行の再試行**（`crates/fuseforks-core/src/llm/client.rs:536-555`）:
`attempts = max_retries.max(1)`（既定 3 = `model.rs:794`）、ループ変数 `attempt` は
**0 始まり**（`for attempt in 0..attempts`）。`is_transient()` が真なら
`200ms × 2^attempt` を 5 秒でクランプして sleep — **0 回目の失敗の後が 200 ms、
1 回目の後が 400 ms**、2 回目の失敗で `last_error` を返す。**既定では通算 3 回試行・
待ちは合計 0.6 秒。** `is_transient`（`error.rs:140-149`）= HTTP 障害（timeout / connect /
request）/ `Api` の 429 と 5xx / `EmptyResponse`。408 は 4xx なので非一過性。

**ヘッダは捨てている**: `client.rs:444-454` は `response.status()` だけ読んで
`LlmError::Api { status, body }` を作る。`Retry-After` は crate 全体で 0 ヒット。
`LlmError::Api` のパターンは **2 ファイル 5 箇所**（`client.rs` / `error.rs`）で、
欄を足す変更は小さい。**client は既にプロバイダで分岐している**（`client.rs:352` の
要求の組み立てと `:459` の decode の振り分け）— 「client は wire の中身を見ない」は
**client が JSON を自分で解釈しない**という規律で、adapter の純関数へ委ねることは
decode で毎回やっている（Notes 7 の反証 1）。

**計器はゼロ**: `fuseforks.log`（14,450 行・2026-08-09〜09-07）で `retry` に当たる
4 行は**全部プロバイダの本文の引用**。2026-08-11 12:32 の 529 は `tool:` の 16.5 秒後に
`turn failed` だが、**その間に何回試したかはログから読めない**。

**頻度**（同じログ）: `code=LLM_API` の失敗 9 件 = fatal 6（401 ×3 / 404 ×2 / 400 ×1）+
非 fatal 3（**429 ×2 = Gemini 無料枠** / **529 ×1 = Anthropic overloaded**）。`LLM_HTTP` 3。
408 は 0 件。2026-08-25 から数字は動いていない（頻度ゲートの側は変わらず 3 件）。

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

### D1: 分類は閉じた列挙 `RetryClass`。status 既定は `llm/retry.rs`、プロバイダの code は adapter の純関数。**分類は `LlmError::Api` が持つ**

新設 `crates/fuseforks-core/src/llm/retry.rs`（純関数のみ・I/O 無し・時計も乱数も読まない）:

```rust
pub enum RetryClass {
    RateLimit,      // 429
    Overloaded,     // 529（Anthropic 固有。IANA 未登録だが文書化されている）
    ServerError,    // 5xx
    Timeout,        // reqwest の is_timeout（408 は含めない — 下の表と裁定）
    Network,        // reqwest の is_connect / is_request
    Auth,           // 401 / 403
    Billing,        // プロバイダ code（下の表）
    ClientError,    // 上記以外の 4xx（408 を含む）・2xx / 3xx がエラー経路へ来た形
    ContextLength,  // 400 / 413 かつプロバイダ code が文脈超過
}
pub enum Verdict { Retry, Stop }
pub fn classify(status: u16, code: Option<&str>) -> RetryClass;  // 網羅 match
pub fn verdict(class: RetryClass) -> Verdict;                     // 網羅 match
```

| `RetryClass` | `Verdict` | 今との差 |
|---|---|---|
| RateLimit / Overloaded / ServerError / Timeout / Network | Retry | 同じ（529 は今 5xx として拾っている） |
| Auth / ClientError / ContextLength | Stop | 同じ結果。**分類名が計器に出る**ようになる |
| **Billing** | **Stop** | **変わる**（今は 429 なら再試行していた。OpenAI の `insufficient_quota` は 429） |

**408 は `ClientError`（Stop）に据え置く**（rev2 で表を確定・承認時に裁定で決着）。査読 2 系統が
割れた点で、R1 は据え置き（「変わる挙動は 2 つ」を守る・実機 0 件）、R2 は Retry
（busbar と揃える・網羅 match として自然）。**私は据え置きを推す** — 動かす根拠が
「あちらがそうしている」だけで、村の実測が 0 件。`Timeout` は reqwest の
`is_timeout` 専用にし、408 を Retry へ動かす日は `classify` の 1 行と表の 1 行で済む。

**分類は `LlmError::Api` に載せる**（R2 の 2(b) — `is_transient` は `CoreError::is_retryable`
（`error.rs:400-406`）と UI から**引数無し**で呼ばれるので、判定材料を error 自身が
持たないと 429 の `insufficient_quota` が status 既定の RateLimit へ落ちて再試行され続ける）:

```rust
LlmError::Api {
    status: u16,
    body: String,
    class: RetryClass,                 // classify(status, signal.code) を client が呼んだ結果
    retry_after: Option<Duration>,     // (a) ヘッダと (b) 本文をマージした最終値（D2）
    hint_src: HintSource,              // Header | Body | Both | None（計器用。D5）
}
```

テストで `Api` を組む場所のために **`LlmError::api(status, body)`** を置く（signal 無しで
分類 = status 既定・`retry_after: None`）。既存の 5 箇所はそれへ寄せる。

**status 以外の材料は adapter が出す**（`client` は wire の中身を見ない —
`data_contract` の `llm_wire.layers`。client は decode と同じく `Provider` で振り分ける）。
各 adapter に純関数を 1 本:

```rust
pub struct ErrorSignal { pub code: Option<String>, pub retry_after: Option<Duration> }
pub fn error_signal(body: &str) -> ErrorSignal;   // openai_compat / anthropic / gemini
```

| ワイヤ | `code` の出所 | Billing と読む値 | ContextLength と読む値 |
|---|---|---|---|
| OpenAI 互換・Responses 4 本（xAI / OpenAI / Meta / Perplexity） | `error.code` → 無ければ `error.type` | `insufficient_quota` | `context_length_exceeded` |
| Anthropic | `error.type` | **無し**（`billing_error` は文書にも実機にも当てていない。busbar の `error_map` も Anthropic は空 = `providers.yaml:23-26`。Notes 4） | 無し（`invalid_request_error` は文脈超過に限らない） |
| Gemini | `error.status` | 無し（`RESOURCE_EXHAUSTED` は RateLimit。busbar と同じ。**課金起因の 429 が同じ status で来た場合は RateLimit として再試行するリスクを許容する** — 明示値が付いていれば下限で待つだけで、天井超えなら止まる） | 無し |

**閉じた列挙で、表に無い code は status 既定へ落ちる**（busbar の「error_map に無ければ
HTTP 分類へフォールスルー」と同じ向き。未知の code で止めない）。

### D2: 待ち時間の下限はサーバーの明示値。出所は 2 つ。**マージは client の 1 箇所**

- **(a) `Retry-After` ヘッダ** — 全ワイヤ。`client.rs:445` の `response` から取れる
  （client の責務 = 「URL・ヘッダ・タイムアウト・再試行」）。`retry::parse_retry_after`
  は delay-seconds と HTTP-date の両方を読む。**過去の日付は `Some(0)`**（「今すぐ」。
  `None` = ヘッダ無しと区別する — 計器では `hint=0s src=header` と出る）
- **(b) Gemini の本文 `error.details[]`** — `@type` が
  `type.googleapis.com/google.rpc.RetryInfo` の要素の `retryDelay`。protobuf `Duration` の
  JSON 形（`"56s"` / `"0.5s"`）。adapter の純関数（`gemini::error_signal`）が読む

**マージは `attempt()` の失敗枝ただ 1 箇所**（`client.rs:447` 付近）:

```rust
let header_hint = retry::parse_retry_after(response.headers());   // (a)
let signal = match self.config.provider { Provider::Gemini => gemini::error_signal(&body), … };
let body_hint = signal.retry_after;                                  // (b)
let retry_after = max_opt(header_hint, body_hint);                   // 両方あれば大きいほう
let hint_src = HintSource::from(header_hint.is_some(), body_hint.is_some());
let class = retry::classify(status, signal.code.as_deref());
Err(LlmError::Api { status, body, class, retry_after, hint_src })
```

`plan_wait` にはマージ済みの 1 値だけを渡す。生の 2 値は `hint_src` へ畳んでから捨てる。

### D3: 待ち = max(指数, 明示値) × (1 + 0.1 × u)。天井 60 秒は明示値にだけ掛かり、超えたら再試行しない

```rust
pub const MAX_HONORED_RETRY_AFTER: Duration = Duration::from_secs(60);
pub enum WaitPlan { Wait(Duration), StopHintTooLong(Duration) }
/// attempt は 0 始まり（0 回目の失敗の後 = 200 ms）。u ∈ [0, 1]。
pub fn plan_wait(attempt: u32, hint: Option<Duration>, u: f64) -> WaitPlan;
```

計算の順序（**ここが rev1 の誤りで、査読 2 系統が同じ穴を指した** — Notes 7）:

1. `exp = min(200ms × 2^attempt, 5s)` — **指数部の天井 5 秒は据え置き**
2. `hint > 60s` なら **`StopHintTooLong(hint)` を返して終わり**。`Verdict::Retry` を
   ここで上書きする。**60 秒の天井は明示値にだけ掛かる**（指数部は 5 秒で頭打ちなので
   60 秒を超えるのは明示値だけ）
3. `base = max(exp, hint)` — 明示値は下限
4. `wait = base × (1 + 0.1 × u)` — **jitter は `max` の後に、上向きだけ乗算で掛ける。**
   rev1 の「`max(exp + jitter, hint)`」は明示値が勝つと jitter が消え、5 体の波が
   56,000 ms ちょうどで揃う（S5 と検収 3 が原理的に成立しない）。上向きだけなのは
   **明示値を下回らない**ため（busbar は ±で散らすが、あちらの値は cooldown で
   下限ではない）。乗算なので「帯の最低幅」は要らない（200 ms なら 0〜20 ms）
5. 天井の判定は jitter 前の `hint`。**jitter 後の `wait` は最大 66 秒**

- `u` は**引数で受ける**（`schedule.rs` と同じ「内部で時計や乱数を読まない」規律）。
  本番の生成は `retry.rs` の外（`client.rs`）で、`SystemTime` のナノ秒・`attempt`・
  **`self`（`HttpLlmBackend`）のアドレス**（個体ごとに 1 インスタンス =
  `client.rs:683` で起動時に作られる。busbar の `cell_id` と同じ役）を FNV-1a で畳み、
  `u = (h % 1001) as f64 / 1000.0`。**`rand` を core に足さない**
- **天井を超えたら止める理由（busbar と違う点）**: あちらは cooldown をクランプして
  次の要求へ進む。村の待ちは**飛行中のターンの中**で起きるので、「3 時間後に再試行せよ」
  （日次の quota）を 60 秒に丸めて再送しても必ず失敗し、失敗するまでの 60 秒を利用者が
  払う。止めて本文に秒数を書けば次の手が人に渡る（fail-closed。Spec 46 の「判定不能は
  宛先を持たない」と同じ向き）。60 秒の根拠 = 実機の 2 件が 56 / 51 秒（分単位の
  quota 窓）で、これを通す最小の切り
- **止めたときの本文**: `chat_with_backoff` が `LlmError::Api` の `body` を
  `format!("プロバイダは {secs} 秒後の再試行を求めています。時間を置いて依頼し直して\
  ください。プロバイダの応答: {body}")` へ差し替えて返す — **JPEG フォールバックの
  「この接続先は画像を受け付けません」（`client.rs:636-643`）と同じ形・同じ層**。
  `status` / `class` は元のまま（UI の `formatError` は `LLM_API` として扱う。
  英語 UI では訳語 + 原文併記 = Spec 13 P4 の規律のまま）。`turn failed` 行は今と同じ
  1 本で、直前に `llm retry stop: … reason=hint_too_long` が出る
- **天井は code constant**（Spec 13「`OrchestratorConfig` を出さない」・`budget.rs` の
  重みと同じ扱い）。設定にするなら頻度を見てから

### D4: 待ちは打ち切りで切れる。**切るのは sleep だけで、HTTP 往復は切らない**

長い待ちを入れる以上、**打ち切り（Spec 10）が待ちの終わりまで遅れる**のは退行。
`LlmBackend::chat` に cancel を渡す口を足す:

```rust
async fn chat(&self, req: ChatRequest, cancel: Option<CancellationToken>) -> Result<ChatResponse, LlmError>;
```

- **`select!` に入れるのは sleep だけ。** `attempt()` の HTTP future は入れない —
  reqwest の future を drop すれば接続は切れるが、**プロバイダが生成を始めていれば
  課金は起きており、こちらには `usage` が届かない**（`failures.md` #103 の形 =
  払ったのに `turn:` 行にも予算にも出ない）。sleep は捨てても 1 トークンも払わない。
  **HTTP 往復の途中の打ち切りは今までどおり周回境界まで待つ**（Spec 10 の境界のまま）
- 切れたら **`LlmError` の新 variant を作らず**、`last_error` をそのまま返す。sleep は
  失敗の後にしか無いので `last_error` は必ず `Some`（初回失敗前に sleep は無い）。
  ターンループは周回境界で `is_cancelled()` を見て `interrupted` へ落とすので、
  **再試行の中で切られたことに固有の名前は要らない**（打ち切りの分類は 1 箇所 =
  `turn.rs:1357`）
- `ChatRequest` には載せない — `PartialEq` を導出しており `CancellationToken` は
  比較できない。adapter の純関数の入力に token が混ざるのも層が違う
- **実装は 10 箇所**（`HttpLlmBackend` / `EchoBackend` / 結合テストの 8 バックエンド）。
  全部 `_cancel` で受けるだけの機械的変更
- **帰結として「■ 停止が 1 秒以内」の保証は待ちの最中だけ。** HTTP 往復中
  （`request_timeout_secs` 既定 120 秒）はこれまでと同じ。検収 4 はその範囲で書く

### D5: 計器 `llm retry:` — 再試行するときと、分類で止めるときの 2 形。P0 は縮退版

P1 以降の形:

```text
llm retry: model=gemini-3.5-flash-lite attempt=1/3 status=429 class=rate_limit code=RESOURCE_EXHAUSTED hint=56s src=body wait=58240ms
llm retry stop: model=gpt-5.6-terra attempt=1/3 status=429 class=billing code=insufficient_quota hint=- src=-
llm retry stop: model=… attempt=1/3 status=429 class=rate_limit code=- hint=10800s src=header reason=hint_too_long
```

**P0 の縮退版**（`retry.rs` が無いので `class` / `code` / `src=body` は出せない —
査読 2 系統が同じ点を指した）:

```text
llm retry: model=… attempt=1/3 status=429 hint=-|NNs src=header|- wait=200ms
```

- `attempt=k/M` — **k は失敗した通算の試行番号（1 始まり）、M は `max_retries`**。
  `wait=` はその失敗の後の待ち。ループ変数（0 始まり）とは 1 ずれるので、実装は
  `attempt + 1` を出す
- `status=-` は `Http` / `EmptyResponse` の再試行。`class=` は `Api` と `Http` が
  `RetryClass`、`EmptyResponse` は `empty_response` の固定文字
- `code=` はプロバイダ code の先頭 40 字。本文は出さない（#71 の規律 — エラー本文は
  `turn failed` 行が既に運んでいる）
- `hint=0s src=header` = 「ヘッダは付いていたが過去の日付」。`hint=-` と読み分ける
- `turn failed` 行と突き合わせれば「何回試して落ちたか」が読める

### D6: `is_transient` は残し、`Api` の腕だけ分類へ委ねる

`is_transient` は `CoreError::is_retryable` / UI の分岐が読んでおり（`data_contract` の
`error_contract`）、名前も呼び出し元も変えない。中身の `Api` の腕を
`verdict(*class) == Retry` へ差し替える（材料は D1 で error 自身が持つ）。**`Http` と
`EmptyResponse` の腕は据え置き。**

### D7: 触る台帳

- `data_contract.yaml` — `llm_wire.invariants` の「再試行対象は is_transient のみ（HTTP
  障害 / 429 / 5xx / 推論の空応答）」を分類表へ書き換え / 新設 `retry_contract`
  （分類 9 値と verdict・明示値の出所 2 つとマージ地点・待ちの式と順序・天井・
  打ち切りの範囲）/ `observability_rule` へ `llm retry:` 行
- `error.rs` のモジュール doc 3 項「`is_transient` が再試行の唯一の判断軸」
- DETAIL 日英 — 再試行の記述（`grep 再試行 DETAIL.md`）と**「利用者が負う条件」**:
  429 の直後にターンが最長 66 秒静かになることがある（画面は「入力中」のまま）/
  既定 3 回では**待ちが 2 回起きうる**（S1）
- CLAUDE.md — 「先行実装の調査」に busbar の節 / 「波の fan-out」の節の
  「機構は作らない」を取り消し線で覆す / 現在地
- README は触らない（設定もトグルも増えない）。**ランディングページと Qiita は
  再試行に触れていない**（grep 網の外だが嘘にならない）

## Stories

- **S1** Gemini 無料枠の 429（`retryDelay: "56s"`）→ 56〜61.6 秒待って再送（通算 2 回目）、
  `llm retry:` に `hint=56s src=body` が出る。**通算 2 回目も 429 なら、もう 1 度
  56 秒待って通算 3 回目を送り、それも 429 なら `turn failed`**（既定 3 = 通算 3 回・
  待ち 2 回。最長で約 2 分）。分単位の quota 窓なら 2 回目で通る
- **S2** OpenAI の `insufficient_quota`（429）→ 再送せず `llm retry stop: class=billing`。
  `turn failed` は今と同じ 1 本
- **S3** Anthropic 529 → 今と同じく再送。`Retry-After` があれば `src=header`
- **S4** 待ちの最中に「■ 停止」→ 1 秒以内に `turn interrupted`
- **S5** 5 体の波が同時に 429 → 5 本の `wait=` が全部違う値（jitter は `max` の後）
- **S6** 明示値が 60 秒超 → 再送せず `reason=hint_too_long`、本文に秒数が載る

## Phases

- **P0 計器と契約**: `llm retry:` の縮退版を**現行の再試行に**先に足す（分類も待ちも
  変えない。足すのは `retry::parse_retry_after` と `LlmError::Api.retry_after` /
  `hint_src` の 2 欄だけ）→ 実機で 1 件でも 429 / 529 を踏めば D2 の (a) の有無が決まる。
  `data_contract` の `retry_contract` を凍結
- **P1 コア**: `retry.rs`（`RetryClass` / `classify` / `verdict` / `plan_wait`）+
  adapter の `error_signal` 3 系統 + `LlmError::Api.class` + `LlmError::api()` +
  `client.rs` の配線（マージ・分類・止めたときの本文）。
  単体: 9 値 × verdict の網羅 / 過去日付の HTTP-date は `Some(0)` / `"0.5s"` の parse /
  `plan_wait` の順序（hint 56s・u=1 → 61,600 ms / hint 61s → Stop / hint 無し attempt 0
  u=0 → 200 ms）/ `max` の後の jitter（同じ hint で u が違えば wait が違う）。
  **結合はループバック HTTP スタブ**（`tests/attachment_fallback.rs` と同じ作り）:
  429 + `Retry-After: 1` → 通算 2 回目で 200（受信本文の件数 = 2・間隔 ≥ 1 秒）/
  429 + `insufficient_quota` → 1 回で止まる（件数 = 1）/ 429 + Gemini 形の `RetryInfo`
  本文 → 待ちの下限が効く / 529 → 再送。
  **ミューテーション 2 回**（`verdict` を全部 Retry へ → billing の 1 本だけ赤 /
  `max` を外す → 下限の 1 本だけ赤）
- **P2 打ち切り**: D4 の trait 変更 + 10 実装 + 結合 1 本（`Retry-After: 30` の待ち中に
  cancel → 200 ms 以内に `Err` が返り、受信本文の件数 = 1）
- **P3 台帳**: D7
- **P4 実機**: 検収項目

## P0 実装記録（2026-09-07）

**入れたもの**: `llm/retry.rs`（`HintSource` 4 値 + `parse_retry_after(value, now)`。P0 は
この 2 つだけ）/ `LlmError::Api` に `retry_after` と `hint_src` の 2 欄 + `LlmError::api()` /
`client.rs` の `attempt()` が `Retry-After` を**本文を読む前に**取る（`text()` が response を
消費する）/ `chat_with_backoff` の sleep の直前に `llm retry:` の縮退版 1 行 /
`data_contract` の `retry_contract` 凍結。**判定と待ちは 1 ミリも変えていない**。

**テスト**: 単体 5（delay-seconds のトリム / 未来の HTTP-date は残り秒 / 過去は `Some(0)` /
読めない値 4 種は `None` / `HintSource` の 4 組）+ 結合 1（`tests/retry_hint_log.rs`。
ループバックのスタブが 429 + `retry-after: 7` を返す → 通算 2 回送って落ち、`Api` が
`Some(7s)` と `Header` を運ぶ / ヘッダ無しの 429 → `None` と `None` / 200 → 計器が出ない）。
lib 655 + 結合全緑・clippy 警告ゼロ。**ミューテーション 1 回**: ヘッダの解釈を `None` に
差し替えると結合の「ヘッダの 7 秒」の assert だけが赤。

**確定した 2 点**:
- **chrono の `parse_from_rfc2822` は IMF-fixdate の `GMT` を読む**（RFC 9110 の例文
  `Sun, 06 Nov 1994 08:49:37 GMT` を単体で固定。新しい依存は要らない —
  `httpdate` を足さない）
- **`llm retry:` の `attempt=` はループ変数 + 1**（失敗した通算の試行番号）。
  `max_retries = 2` のスタブで `attempt=1/2` の 1 行だけが出る = 通算 2 回目の失敗では
  再試行しないので行が出ない。**「M 回試行 = M − 1 行」**が読み方

**P0 が答えていないこと**: D2 (a) が実際に付くか。スタブは付けているが、実機のプロバイダが
付けるかは**次に 429 / 529 を踏んだときの `src=` が初めて答える**（Notes 6）。

## P1 実装記録（2026-09-07）

**入れたもの**: `retry.rs` に `RetryClass`（9 値）/ `Verdict` / `classify` / `verdict` /
`plan_wait` / `WaitPlan` / `MAX_HONORED_RETRY_AFTER` / `ErrorSignal` / `parse_proto_duration` /
adapter の `error_signal` 3 系統（`openai_compat` = Responses 4 本と共有 / `anthropic` /
`gemini` — `RetryInfo` を読むのはここだけ）/ `LlmError::Api` に `class` と **`code`**
（先頭 40 字。計器の `code=` に出す）/ `is_transient` の `Api` の腕を `verdict` へ /
`client.rs` の `attempt()` でマージと分類（1 箇所）/ `chat_with_backoff` を `plan_wait` +
計器 2 形へ書き換え / `with_hint_too_long`（本文の前置）/ `jitter_unit`（FNV-1a）。

**テスト**: 単体 +14（`retry.rs` 13 = 9 値の verdict 網羅 / status 既定の表 / code の優先と
400・413 の門 / 指数部が今と同じ / 明示値は下限 / jitter は max の後・上向き・定義域の
クランプ / 天井は jitter 前で 60 秒は通り 61 秒は止まる / proto Duration。adapter 3 = 実機の
429 本文と同じ形で `RESOURCE_EXHAUSTED` + `56s` を読む、ほか）+ 結合 1 本を 7 場面へ
（ヘッダ 1 秒 → 実測 ≥ 1 秒 / ヘッダ無し → 200 ms / 200 → 行が出ない / 課金切れ → 1 回で
止まる / 3,600 秒 → 1 回で止まり本文の先頭に秒数 / Gemini 本文 `1s` → ヘッダ無しでも 1 秒
待つ / 529 → 再送）。行は 6 本（再送 4 + 停止 2）で `class=` / `code=` / `hint=` / `src=` を
逐語で留めた。lib 655 → 669 + 結合全緑・clippy 警告ゼロ。

**ミューテーション 2 回**（どちらも狙った 1 本だけが赤）: `verdict` の Stop を全部 Retry へ →
結合の「再試行しない」（`hits == 1`）と単体の網羅が赤 / `plan_wait` の `max` を外す →
結合の「明示値 1 秒は下限」（`elapsed ≥ 1s`）だけが赤。

**実装で決めた 3 点**:
- **`code` を `Api` に載せた**（rev2 の型には無かった）。載せないと計器の `code=` が
  「分類が code 由来のときだけ語を捏造する」形になり、`overloaded_error` や
  `RESOURCE_EXHAUSTED` のように分類に効かない code が読めない。40 字で切って本文は出さない
- **分類で止めるときの `llm retry stop:` は `Api` だけ**。`Blocked` / `Parse` / `Config` は
  再試行の問いに最初から入っていないので計器に混ぜない
- **最後の試行の失敗は行を出さない**（「M 回試行 = M − 1 行」の読みは P0 のまま）。
  ただし天井超えは最後の試行でも `stop` の行と本文の前置を出す — 秒数が人に渡ることが
  目的で、試行番号は関係ない

**P1 が答えていないこと**: D2 (a) が実機で付くか（P0 と同じ。`src=` が答える）/ 打ち切り
（P2。今の sleep は `select!` を持たず、明示値の待ちは最長 66 秒まで止められない）。

## P2 実装記録（2026-09-07）

**D4 の形を 1 つ訂正した — trait の署名は変えず、既定実装つきのメソッドを足した。**
rev2 は「`chat` に `cancel` を足す。実装は 10 箇所」と書いたが、**実装は 57 箇所**あった
（`EchoBackend` + `HttpLlmBackend` + 結合テストの 55）。起票時の grep を `head` で切って
数えたのが誤りで、実物を数え直したのは P2 の 1 手目。署名を変えると 55 本のテスト
バックエンドが機械的に落ちるだけで何も守らないので、

```rust
async fn chat_cancellable(&self, req, cancel: Option<CancellationToken>) -> … {
    let _ = cancel;      // 既定: token を読まずに chat へ委ねる
    self.chat(req).await
}
```

を **trait の既定メソッド**として足し、`HttpLlmBackend` だけが上書きする。ターンループの
2 箇所（`turn.rs` の本体の呼び出しと、まとめの呼び出し）が `chat_cancellable(request,
Some(turn.token.clone()))` を呼ぶ。要約（`summarize_agents`）は `chat` のまま（ターンの外で、
打ち切りの対象ではない）。**D4 が守るもの（sleep だけ切る・HTTP は切らない・新 variant を
作らない・分類は周回境界の 1 箇所）は 1 つも動いていない** — 変えたのは配線の形だけ。

**入れたもの**: `LlmBackend::chat_cancellable`（既定実装）/ `HttpLlmBackend` の `chat_inner`
（`chat` と `chat_cancellable` の共通部。JPEG フォールバックはここ）/ `chat_with_backoff` の
sleep を `select!` で token と競わせる（切れたら `Err(err)` = いま受けた失敗をそのまま）/
`turn.rs` の 2 箇所。

**テスト**: 結合を 9 場面へ（+2）。**8 = 30 秒の明示値の待ち中に 150 ms で cancel → 2 秒以内に
返り、送ったのは 1 回、返るのは受けた 429 そのもの**（`retry_after = Some(30s)`）。
**9 = 600 ms 黙るスタブへ 100 ms で cancel → 応答が届く**（`elapsed ≥ 600 ms`・本文 `late`）=
HTTP 往復は切れないことの負の対照。ログは 7 本になり、打ち切られた待ちも「待ち始めた
1 行」（`hint=30s wait=30000..33000ms`）は残る — 切れた事実は `turn.rs` の `interrupted` が
書くので、ここに固有の行は作らない。lib 669 + 結合全緑・clippy 警告ゼロ。
**ミューテーション 1 回**: `select!` の token の腕を `pending()` に差し替える → 場面 8 が
**30.5 秒待ってから**「30 秒待たずに返る」の assert で赤（ミューテーションのテストが
30 秒かかるのは、機構が無いと本当に 30 秒待つことの実測でもある）。

**次に触る人が要る 2 点**:
- **`chat` を直に呼ぶ経路は打ち切りの外**。ターンループ以外から `HttpLlmBackend` を呼ぶ
  ときは `chat_cancellable` に token を渡さないと、その待ちは切れない（今それに当たるのは
  要約だけで、意図どおり）
- **HTTP 往復中の打ち切りは今までどおり `request_timeout_secs`（既定 120 秒）まで待つ**。
  検収 4 の「1 秒以内」は待ちの最中だけ

## P3 台帳記録（2026-09-07）

- **DETAIL 日英**: ディレクトリ木に `retry.rs` / `llm_wire` の罠の箇条書きに分類と下限の
  2 項 / **「運用 > 再試行の待ち」を新設**（利用者が負う条件 = 429 の直後にターンが最長
  66 秒静か・既定 3 回で待ちが 2 回・停止は待ちだけ切る・`llm retry:` の読み方）
- **CLAUDE.md**: 「波の fan-out」の「機構は作らない」を取り消し線で覆し Spec 52 を指す /
  「busbar の実読と突き合わせ」を新設（8 実装目の表と、使えたもの・採らないもの）/
  LangGraph の対照表の「ノード再試行」の行に追記 / 「現在地（2026-09-07）」/ Spec の状態
- **`failures.md` #120**: 実装の数を `grep | head` で数えて 10 と書いたら 57 だった
- **`data_contract`**: P0〜P2 で `retry_contract` と `llm_wire.invariants` を更新済み。P3 では
  状態行だけ
- **README は触らない**（設定もトグルも増えない）。ランディングページと Qiita は再試行に
  触れていないので嘘にならない（grep 網の外）
- **数えたのはファイル単位**（#51 (b)）: DETAIL.md / DETAIL_en.md / CLAUDE.md / failures.md /
  data_contract.yaml / この Spec の 6 本

## 検収項目（各項目に到達経路を書く）

1. **無料枠の Gemini 鍵で 2 体へ波を撒く**（2026-08-25 の再現）→ `llm retry:` に
   `status=429 class=rate_limit hint=NNs src=body|both` が出る。経路: Gemini 429 →
   `gemini::error_signal` が `RetryInfo` を読む → `plan_wait` → sleep
2. 同じ走行で `wait=` が `hint` 以上・`hint × 1.1` 以下
3. 同じ走行で 2 体の `wait=` が異なる（jitter）。**1 体だけでは判定にならない**
4. **待ちの最中に「■ 停止」** → `turn interrupted` が 1 秒以内。**HTTP 往復中の停止は
   対象外**（D4。今までどおり周回境界）。経路: `select!` が token を拾う →
   `last_error` を返す → 周回境界で `interrupted`
5. `insufficient_quota` は**実機で踏めない**（残高がある鍵しか無い）→ P1 の結合テストが
   代替（`attachment_fallback` と同じ判断: 踏めない経路をテスト無しで残さない）
6. 529 は Anthropic の混雑時にしか出ない → **狙わない**。出たら `src=` を読む
7. **既存の走行が変わらないこと**: 429 も 5xx も出ない通常のターンで `llm retry:` が
   1 本も出ない（負の対照）

## P4 実機記録（2026-09-07〜。利用者検証）

- **検収 7（負の対照）= 観測**（2026-09-07 23:37〜23:42。開発ビルド `version: app=0.1.0
  profile=debug`、起動 23:19:41）。進行役（claude-sonnet-5）が agent_4 / agent_7
  （`gemini-3.5-flash-lite`・無料枠）へ波を撒き、2 体とも `rounds=3/16 stop=-` で完走、
  agent_9（muse-spark-1.3）が `rounds=7/32` で完走、進行役が `rounds=2/36 stop=-` で束ねた。
  **起動以降の 93 行に `llm retry` は 0 本**。429 が出ていない走行で計器が沈黙している =
  「常に出る実装」ではないことの対照
- **検収 1・2・3 = 観測**（2026-09-08 00:36〜00:37。進行役が agent_2 / agent_4 / agent_5
  （全部 `gemini-3.5-flash-lite`・無料枠）へ `plan wave: tasks=3`）。6 秒後に 3 体が同時に
  429 を受け、`llm retry:` が 3 本:

  ```text
  00:36:19.174 attempt=1/3 status=429 class=rate_limit code=RESOURCE_EXHAUSTED hint=41s src=body wait=43714ms
  00:36:19.376 attempt=1/3 … hint=41s src=body wait=41200ms
  00:36:20.126 attempt=1/3 … hint=40s src=body wait=40080ms
  ```

  - **検収 1**: 行が出た。`code=RESOURCE_EXHAUSTED`・`hint=` は本文の `RetryInfo.retryDelay`
  - **検収 2**: `wait` は `hint` 以上・`hint × 1.1` 以下（43,714 = 41,000 × 1.066 / 41,200 =
    × 1.005 / 40,080 = × 1.002）
  - **検収 3**: 同じ `hint=41s` の 2 体で `wait` が 43,714 と 41,200 = **jitter が `max` の後に
    効いている**（rev1 の式なら 41,000 で揃っていた）
  - **D2 (a) の答え（Gemini）: `Retry-After` ヘッダは付いていない。** 8 本すべて `src=body`。
    ヘッダを読む経路は Gemini では発火せず、(b) だけが効いている。他ワイヤの 429 / 529 は
    未観測（OpenAI / Anthropic がヘッダを付けるかは次に出たときの `src=` が答える）
- **予測を 1 つ外した — `hint=0s` が来る。** 41 秒待った後の 2 回目（00:37:00.5 / 00:37:00.9）も
  429 で、本文の `retryDelay` は **`"0s"`**（`hint=0s src=body wait=404ms` / `429ms`）。
  下限 0 なので指数部の 400 ms + jitter だけ待ち、3 回目も 429 で agent_4 は
  `turn failed`（`rounds=5/- stop=failed:LLM_API`。払いは `turn:` 行に残っている）。
  **サーバーの明示値は下限であって約束ではない**（Gemini は分単位の quota 窓の中で
  「今すぐ」と言うことがある）— D3 が明示値を「置き換え」ではなく「下限」にした判断の
  実物。機構は変えない: 0s を信じて 400 ms で再送し、外れれば `max_retries` で止まる
- **S1 の「待ちが 2 回起きうる」も実物が出た** — 別の個体が 00:37:02.351 に `attempt=1/3
  hint=58s wait=63626ms`、その 1 秒後に別の個体が `attempt=2/3 hint=57s wait=59633ms`
  （前の 43.7 秒の待ちの後の 2 回目）。**同じターンの中で 41 秒 + 60 秒**。無料枠の RPM 15 は
  3 体 × 5 周に足りない、が 2026-08-25 と同じ結論で、変わったのは再試行が正直になった
  こと（**5 秒で諦めていた場所で、サーバーの言う 40〜60 秒を待つ**）
- **待った 2 体は通った。** 63.6 秒 / 59.6 秒の待ちが明けた後（00:38:03〜06）の再送は
  429 にならず（以後 `llm retry` は 0 本）、agent_2 / agent_5 は `rounds=16/16
  stop=tool_limit` まで走って 141 字を依頼主へ返し、束ねは `plan bundle: tasks=3
  elapsed_ms=124671`。**S1 の「分単位の quota 窓なら次で通る」の実物** — 2026-08-25 は
  5 秒で諦めて 2 体が `turn failed` だったのに対し、今回は 3 体のうち 2 体が完走した
  （落ちた 1 体は `hint=0s` を信じた 400 ms の再送が外れた側）
- **検収 4 = 観測。ただし半分だけ合格で、D4 の穴が 1 つ出た**（2026-09-08 01:21）。
  4 体が `attempt=2/3 hint=56〜59s wait=57〜63 秒` の待ちに入った 9 秒後に agent_4 で
  「■ 停止」:

  ```text
  01:21:13.190 interrupt requested: agent=agent_4 seq=4
  01:21:13.192 turn: agent=agent_4 hop=1 rounds=5/- waves=0 stop=failed:LLM_API …
  01:21:13.202 turn failed: agent=agent_4 hop=1 code=LLM_API fatal=false: API エラー (status=429)
  ```

  **待ちは 2 ms で切れた**（機構は効いた。1 秒以内の要件は満たす）が、**ターンは
  `stop=failed:LLM_API` で閉じ、「API エラー」の System 行が出た** — 人が止めたターンが
  失敗を名乗る。D4 の「切れたら失敗をそのまま返し、ターンループが周回境界で
  `is_cancelled()` を見て `interrupted` へ落とす」は、**`Err` の腕には当たっていなかった** —
  `turn.rs` の `Err` の腕は払いを清算してすぐ `Err` で抜け、周回境界（`Ok` の経路にしか無い）
  を通らない。`token_budget.precedence`（cancel が最優先）を 1 経路で破っていた。
  - **処方**（同日）: `Err` の腕で払いの清算の直後に `turn.token.is_cancelled()` を見て
    `finish_interrupted` へ落とす 1 箇所。打ち切りへ落とす判定は**周回境界と `Err` の腕の
    2 箇所、どちらも `finish_interrupted` の 1 実装**。払った分は落とす前に台帳へ入れる
  - **赤 → 緑**: `tests/interrupt_during_retry_wait.rs`（token が切れるまで待って 429 を返す
    バックエンドで、`TurnInterrupted` 1 本・`AgentFailed` 0・ログに `turn interrupted:` が
    あり `stop=failed:` と `turn failed:` が無い）。修正前は `TurnInterrupted` 0 で赤、
    修正後に緑。**ミューテーション**（`Err` の腕の検査を `if false &&` で殺す）で同じ
    assert が赤に戻ることを確認
  - **予測を外した点**: 打ち切りで閉じた出口は `turn interrupted:` の行で、`turn: …
    stop=interrupted` ではない（4 出口の書式は CLAUDE.md「失敗したターンの払い」に
    書いてある。テストの初版は `turn:` 行を探して自分で赤にした）
  - 他の 3 体はそのまま待ち、01:22:09〜10 に 2 体が 3 回目の 429（`hint=50〜51s`）、
    agent_6 は `rounds=9/16 stop=-` で完走
- **検収 4 = 修正後に再観測して合格**（2026-09-08 06:57。起動 06:51:46 の再ビルド）。
  3 体が `attempt=2/3 hint=59s wait=60〜64 秒` の待ちに入った 29 秒後に agent_6 で「■ 停止」:

  ```text
  06:57:30.987 interrupt requested: agent=agent_6 seq=4
  06:57:30.999 turn interrupted: agent=agent_6 seq=4 hop=1 rounds=4 … prompt=17862 model=gemini-3.5-flash-lite
  ```

  **12 ms で切れ、`turn interrupted:` で閉じた。** `turn: … stop=failed:` も `turn failed:` も
  出ていない（01:21 の形との対）。払った分（prompt 17,862）は `turn interrupted:` の行に
  残っている。待ちを続けた残り 2 体（agent_2 / agent_4）は 06:58:07 / 06:58:10 に `stop=-`
  で完走 = 停止は押した個体だけを切り、同じ波の他の待ちには触れない。
  同じ走行で `hint=4s` / `5s` の小さい明示値も出た（`wait=4169〜5123ms` — 下限として
  そのまま効いている）
- **D2 (a) の要否（Gemini）= 不要だが残す**。Gemini の 429 は 17 本すべて `src=body` で、
  ヘッダを読む経路は 1 度も発火していない。**外すのは誤り** — OpenAI / Anthropic が
  `Retry-After` を付けるかは未観測で、読む側は 10 行の純関数。次に 429 / 529 が他ワイヤで
  出たとき `src=` が答える

## Notes

1. **busbar から持ち込まないもの**: ブレーカー状態（Closed / Open / HalfOpen）と
   cooldown の永続 — 村の「レーン」は個体 1 体で、落ちている間に避ける先が無い /
   failover の walk / `error_map` の YAML / `ContextLength` を「別の大きいモデルへ
   逃がす」分類（村は同じ個体で再送しない。分類名だけ計器のために持つ）/
   ヘルスプローブ（`user "ping" max_tokens 1` は安いが、待機中の個体は課金されないので
   「黙って死んでいるレーン」が村に無い）/ ±の jitter（D3 — 明示値を下限に保つため上向きだけ）
2. **2026-08-25 の「機構は作らない」を覆す理由**: 利用者裁定（2026-09-07）+ 参照実装で
   実装の値段が純関数 1 本に下がった。頻度は今も 3 件。**効き目は「失敗が減る」ではなく
   「サーバーが教えた値を無視しない」と「再試行が見える」の 2 つ** — 2026-08-25 の
   「`retryDelay` を読んで待っても RPM 15 は 2 体 × 10 周には足りない」は今も正しい。
   S1 で 1 呼び出しは通るが、周を回せば次の 429 でまた 56 秒待つ。無人の予定なら
   遅くても完走し、対話なら利用者が止める — どちらへ倒すかは D3 の天井が決めている
3. **写すのは構造で、コードは書き直す**（Apache-2.0 → MPL-2.0 は互換だが、逐語で
   持ち込まないので NOTICE も不要。doc コメントに出典を 1 行書く）
4. **Anthropic の `billing_error` は表に入れない**（rev2 で確定）。文書にも実機にも
   当てておらず、busbar の `error_map` も Anthropic は空。入れるなら根拠を書いてから
5. **利用者裁定（2026-09-07 承認時に決着）**: **408 は据え置き Stop**（根拠 = 実機 0 件 /
   「変わる挙動は 2 つ」を守る / `Timeout` は reqwest の `is_timeout` で既に生きており
   408 を足さなくてもタイムアウト系の再試行は覆われている）。D3 の天井 60 秒と
   「超えたら止める」は査読 2 系統とも賛成 / D4 は「待ちだけ切る」に範囲を確定
   （#103 の形を避けるため — HTTP 往復を切ると払いの記録を失う）
6. **計器を先に出す順序（P0）が本体より重い判断**: D2 の (a) が実際に付くかは
   ヘッダを記録しないと永遠に分からず、`src=` の実測が無いまま (a) を実装すると
   「入れたのに効いているか分からない機構」になる（#47 の規律）
7. **rev1 → rev2 の査読記録（2 系統 16 点）**:
   - **採用 12**: R1-2（マージ地点を client の 1 箇所に明記）/ R1-3・R2-4（P0 の計器は
     縮退版 — `class` / `src=body` は P1）/ R1-4（天井は明示値にだけ掛かる・`Verdict` を
     上書き）/ R1-6（`attempt` は 0 始まり・計器は 1 始まり）/ R1-細 3 件（Gemini の
     Billing リスクを表に明記 / `hint=0s` の読み / `u` の正規化式）/ R2-2(b)（`LlmError::Api`
     に `class` を載せる — `is_transient` が引数無しで呼ばれる）/ R2-3（S1 の回数を通算で
     書き直し。待ちは 2 回起きうる）/ R2-5（止めたときの本文は JPEG フォールバックと
     同じ層・同じ形）/ R2-6（jitter の種にインスタンスのアドレス）
   - **訂正して採用 2**: R1-5（D4 の範囲を「待ちだけ」に絞る — 理由は R1 の
     「1 秒保証が破れる」ではなく、**HTTP を切ると払いの記録を失う**こと。結果は同じ）/
     R1-7 + R2-1（jitter の式 — 乗算・**`max` の後**・上向きだけ。R1 は式の固定を求め、
     R2 は「明示値が勝つと jitter が消える」バグを指した。両方を 1 つの式で受けた）
   - **反証 1**: R2-2(a)「client が wire を見ずに adapter の `error_signal` を呼ぶ
     インターフェースが未定義」— client は `client.rs:352` / `:459` で既に `Provider` で
     振り分けて adapter の encode / decode を呼んでいる。規律が禁じるのは client 自身が
     JSON を解釈することで、委譲は毎回やっている
   - **裁定へ 1**: R1-1 と R2-3（408）— 査読同士が逆を向いた。表は据え置き（Stop）で
     書き、裁定で動かす → **据え置きで決着**（Notes 5）
8. **jitter と天井の順序（承認査読の追加 1 点）**: 天井の判定は **jitter を掛ける前の
   `hint`** で行い、`hint ≤ 60s` なら jitter 後の `wait` が 60 秒を超えても（最大 66 秒）
   そのまま待つ。順序を逆にすると `hint = 56s` が `56 × 1.1 = 61.6s` で天井に当たって
   Stop になる。`u` の定義域は `[0, 1]`（rev1 の `[-1, 1]` から変更）。D3 の手順 2 → 4 が
   この順序で、実装はこの順序を単体テストで留める（`hint = 56s, u = 1 → Wait(61,600 ms)`）
