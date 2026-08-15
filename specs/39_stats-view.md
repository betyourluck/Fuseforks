# Spec: 統計画面 — ターンの記録を `sessions.redb` の 4 種別目に留め、全画面で読む

**ID**: 39
**Date**: 2026-08-16
**Status**: rev2 承認（2026-08-16。利用者査読 9 点 → 採用 5 / 訂正して採用 3 / 反証 1）→ P0 着手
**Branch**: P0（契約の凍結）は rev 承認後に main 直コミット。P1 以降は着手日の
`YYYYMMDD_stats` へ積み、P1/P2 のテスト合格をゲートに合流（Spec 38 と同じ 2 段階。
**フロントの全画面切り替えは P3 で初めて画面に触る**ので、P1/P2 は画面を 1 px も変えない）

## Goal

**この村がいくら払ったかを、画面で読めるようにする。** 単位はトークン（素の
prompt / cached / 出力 / 思考 と、Spec 11 の重みで畳んだ**実効トークン**）で、
**通貨には換算しない**（2026-08-15 利用者裁定）。軸は 会話（セッション）× 個体 ×
ターンの終わり方（完走 / 失敗:CODE / 割り込み / 予算）× 時間。

**画面は 3 ペインの上に重ねるダイアログではなく、3 ペインを丸ごと差し替える
全画面**（2026-08-16 利用者裁定 —「統計画面は全ページを切り替えたほうがいい。
3 ペインじゃなくなる感じ」）。参照した見せ方は promptfoo の eval 結果ビュー
（プロバイダごとの見出しに Requests / Total Tokens / Avg Tokens / Avg Latency が並ぶ形）。

**前提は #103**（2026-08-16 に塞いだ）— 失敗したターンの払いが計上されない状態で
統計を出すと、失敗が多い個体ほど安く見える。**先に計器を直してから画面を作る**順序で
ここまで来ている。

**これは拡張か収縮か**: 拡張（`Record` の variant が 1 つ増え、IPC が増え、画面が
1 枚増える）。収縮側の選択肢は D0 に置く（「`fuseforks.log` を grep すれば足りる」）。

**この Spec は評価基盤の「費用の層」だけを扱う**（otari の側）。「払った価値が
あったか」（promptfoo の assert / pass rate / LLM-judge の側）は**扱わない** —
合格条件を持たないこの村に pass の欄を作ると空の欄になる（Spec 15 D7 の規律）。
その材料は Notes に置き、起票は別 Spec。

## 現況（実測 2026-08-16。参照は関数名）

| | 事実 |
|---|---|
| `Record`（`session_store.rs`） | **3 種別で閉じている**（`message` / `exchange` / `summary`）。`exchange` は `agentId / sent / replied` の 3 欄で、**tokens / モデル名 / rounds / 経過時間 / stop が 1 つも無い**。`Message` の `AgentMessage` にだけ時刻がある |
| `turn:` 行（`turn.rs`） | 欄は `agent / hop / rounds=実績/上限 / waves / stop / prompt / cached / total / reasoning / backend`。**`completion` 欄は無い**（`total − prompt` で引く。AgentCard も同じ引き算）。**モデル名も無い**（`backend` はワイヤ名。#104 が「`turn:` の `prompt` はラウンドの合計・モデル名も無い」と書いたのはこれ）。失敗は `stop=failed:{CODE}` で同じ並び（#103） |
| 使用量の行の出口 | 4 つ（完走 `turn:` / 失敗 `turn:` / 割り込み `turn interrupted:` / 予算 `turn budget exhausted:`）。**後ろ 2 つは行の形が違う**（`prompt= cached= total=` のみ・`rounds` も `backend` も無い） |
| `TurnSpend` | `tokens / cached / prompt / reasoning / rounds / hop`。**経過時間は無い**（割り込み経路だけが `turn.requested_at` から `elapsed` を作る）。ターン終了で捨てられる |
| 累計の器 | `AgentRecord` の `total_tokens / prompt_tokens / cached_tokens / accumulated_uptime_secs` の 4 欄だけ。**turn 数も失敗数もモデル別も無い**。`CoreEvent::AgentStatsUpdated` は**稼働中の個体にしか飛ばない**（`spawn_stats_ticker`） |
| 村全体の合計 | **どの画面にも無い**（`totalTokens` を跨いで足している箇所がフロントに 1 つも無い） |
| 過去のターンを持つ器 | **無い**。残るのは累計 4 欄と `fuseforks.log` の行だけ。ログは 8MB で 1 世代しか回さない |
| `SessionStore::records()` | コアには在る（全レコード・seq つき）が **IPC に露出していない** |
| 全画面の切り替え | **無い**。唯一「グリッドを覆う層」は `!state.ready` の初期化オーバーレイ（`fixed inset-0 z-50`）。TitleBar の入口 6 つは全部 `ref<boolean>` + `v-if` のダイアログ |
| チャート | フロントの依存にチャートライブラリは**無い**（`v-network-graph` は SVG） |
| 予算の残額 | 因果ごとの `Arc<BudgetPool>` で**プロセス寿命**。セッションにもディスクにも無い |

**帰結**: 過去のセッションについて統計は**作れない**（数字が保存されていない）。
`fuseforks.log` から遡るのも不可（回転で消える + テキストの再解析は書式の 2 実装目）。
**この Spec を入れた版から先のターンだけが数えられる。** 画面はそれを正直に出す（D6）。

## 設計の核

- **数字の原本を 1 つ増やし、それは `turn:` 行と同じ構造体から書く。** `TurnSpend` を
  `TurnRecord` へ写す関数を 1 実装にし、`turn:` 行もレコードもそこから出す。
  ログとレコードが食い違う形を構造で作らない
- **集計はコアの純関数**（`budget.rs` と同じ形 — 入力は `(session_id, TurnRecord)` の
  列と `SessionSummary` の一覧、出力は集計 DTO。時計を読まず I/O をしない。
  redb を読んで列を集めるのは IPC 側の別の 1 段）。フロントは描くだけ。理由は 3 つ — 実効トークンの重みは
  `budget.rs` に 1 実装で在り、フロントで再計算すると 2 箇所目になる / 50,000 件を
  IPC で運ぶより DTO 1 つが軽い / テストが Rust の単体で書ける
- **画面は全画面。ただし TitleBar と StatusBar は残す**（戻る導線と時計・版番号は
  統計を見ている間も要る）。差し替わるのは `App.vue` のグリッドだけ

## 決めること

### D0. そもそも器を足すか

| 選択肢 | 中身 | 判定 |
|---|---|---|
| (1) `Record::Turn` を足す | 4 種別目。ターンの出口 4 つで 1 件ずつ書く | **推し** |
| (2) 足さない | `fuseforks.log` の `turn:` 行を利用者が grep する（今の状態） | 生きた選択肢。**過去の運用はこれで回っていた** |
| (3) 別テーブル | `sessions.redb` に `turns` テーブルを増やす | 却下 — セッションの単位（村）と seq の採番規律が `records` に在り、別テーブルは fork / delete / export の 4 経路に**もう 1 本ずつ**手が要る |
| (4) `AgentRecord` の累計欄を増やす | turn 数・失敗数を `world.json` に足す | 却下 — 累計は「いつ」を持たず、会話をまたぐ。**セッション別・時系列が原理的に出ない** |

**(2) を推さない理由**: `turn:` 行は 8MB で 1 世代しか回らず、**失敗の内訳を数える
（`stop=failed:` の CODE 別）にはログを読む以外の手が無い**。#103 を塞いだ動機が
「統計画面の前提」だったので、ここで (2) に倒すと #103 の変更は計器のためだけに残る。
ただし (2) は**画面を作らずとも `turn:` 行があれば足りる利用者**には正しく、
D0=(2) なら本 Spec は Notes の材料を残して閉じる。

### D1. `Record::Turn` の欄

```text
turn : { agentId, tsMs, hop, rounds, waves, stop, prompt, cached, completion,
         reasoning, model, backend, elapsedMs }
```

| 欄 | 出自 | 注 |
|---|---|---|
| `agentId` | ターンの主 | |
| `tsMs` | ターン開始の壁時計 | `turn.requested_at` を epoch ms へ。**終了時刻ではなく開始** — 並べ替えの鍵は「いつ頼んだか」 |
| `hop` | `TurnSpend.hop` | 0 = 因果の根（利用者 / 予定 / 外部）。委譲されたターンは ≥ 1 |
| `rounds` | `TurnSpend.rounds` | 上限（`max_tool_iterations`）は**入れない** — 設定であって観測ではない |
| `waves` | `plan_wave` | |
| `stop` | 閉じた列挙 | 下の表 |
| `prompt` / `cached` / `completion` / `reasoning` | `TurnSpend` | **`completion` はレコードで初めて生まれる欄** — `TurnSpend` が持つのは `tokens`（= total）で、`settle_turn` が **`completion = tokens − prompt`** を 1 箇所で計算する（rev2 で明記。利用者査読 2 — 「引き算を要求しない」のは*読む側*の話で、*書く側*は引く）。ログ行は従来どおり `total=tokens`。`reasoning` は `completion` の内数（Spec 32 D2）— D3 の実効計算で二重に足さない |
| `model` | `template.model` | **`turn:` 行に無かった欄**。#104 の「帯も機種も見えない」を閉じる。同時にログ行の**末尾へ `model=` を足す**（D2）。**`Option` にしない** — テンプレートは `handle_message` の段 1（`world.template(...)?`）で 4 出口のどれより先に解決され、引けなければターン自体が始まらない（`observability_rule` の「バックエンドの解決より前に落ちたターンは `turn` 行を持たない」と同じ位置）。rev2 で利用者査読 2 の `Option<String>` 案を**コードで反証** |
| `backend` | `backend.name()` | ワイヤ名 |
| `elapsedMs` | `requested_at.elapsed()` | 割り込み経路だけが持っていたものを 4 出口へ |

**`stop` の閉じた列挙 7 値と、出口 4 つの対応**（rev2 で表にした。利用者査読の細目 1）:

| `stop` | 出口（呼ぶ関数） | 既存のログ行 | 付随する欄 |
|---|---|---|---|
| `completed` | 完走（`run_turn` の末尾） | `turn: … stop=-` | — |
| `repeat` | 完走（RepeatGuard がその周のツールを全部止めた） | `turn: … stop=repeat:{tool}` | `tool: String` |
| `tool_limit` | 完走（`max_tool_iterations` 到達） | `turn: … stop=tool_limit` | — |
| `failed` | `settle_failed_turn` | `turn: … stop=failed:{CODE}` | `code: String`（`CoreError` のコード） |
| `interrupted` | `record_interrupted_turn` | `turn interrupted: …` | — |
| `budget_exhausted` | `finish_budget_exhausted`（残額 0） | `turn budget exhausted: …` | — |
| `reserve_short` | `finish_budget_exhausted`（残額 > 0 だが見積もり未満） | `turn budget exhausted: …`（理由は直前の `budget stop: … reason=reserve_short`） | — |

**`budget_exhausted` / `reserve_short` は同じ関数の同じ行から出る。** 分けるのは
Spec 38 D3 の帰結（「使い切った」と「次の 1 呼び出しぶんを確保できなかった」は
利用者の次の手が違う）で、**今は `budget stop:` の `reason=` にしか無い区別を
レコードへ運ぶ** — `finish_budget_exhausted` に理由を渡す引数が 1 つ増える
（現状はその手前の `note!` で分岐しているだけで、関数は理由を知らない）。
ログ行 `turn budget exhausted:` の書式は変えない（D2）。

**入れない欄**: 予算の残額（因果ごと・プロセス寿命 — セッションの記録に置くと
「どの因果の」が消える）/ 依頼文・本文（`exchange` が持つ。数字の記録に本文を混ぜない —
`observability_rule` と同じ規律）/ セッション id（キーが持つ）。

### D2. 書く場所は 4 出口・書く関数は 1 つ

`settle_turn(shared, spend, outcome) -> ()` を新設し、**`turn:` 行（と後ろ 2 出口の
行）とレコードの両方をここから出す**。4 出口（`run_turn` の完走 / `settle_failed_turn` /
`record_interrupted_turn` / `finish_budget_exhausted`）はこの 1 関数を呼ぶだけになる。

- **ログ行の規則は「接頭辞と既存の欄の並びを変えない。増える欄は末尾へ `model=` の
  1 つだけ」**（rev2 で明文化。利用者査読 1 — rev1 の「後ろ 2 出口の書式は変えない」は
  D1 の「行にも `model=` を足す」と衝突していた）。4 行とも同じ規則で、
  `turn interrupted:` / `turn budget exhausted:` にも**末尾に `model=` だけ**足す。
  `backend=` / `elapsed_ms=` は後ろ 2 行には**足さない**（欄を揃えたければレコードを
  読む。行の書式を 4 本とも揃え直す変更は既存の grep 資産と引き換えで、いま得るものが無い）。
  **レコードは常に全欄**（行とレコードの欄集合は一致させない — 一致させるのは
  *出自の構造体*であって*出力の欄*ではない）
- **失敗経路も書く。** `settle_failed_turn` は `exchange` を書かない（本文が無い）が、
  `turn` は書く — #103 の処方がそのまま伸びる
- **`persist` の失敗は WARN 1 行で続行**（Spec 12 の規律。統計が欠けてもターンは通す）
- **`Record::Turn` は `restore_histories` の入力にならない**（履歴ではない）。
  `tail_messages` / `fork_points` の候補にもならない。**`fork_session` は seq を含めて
  複製する**（inclusive）ので、分岐先にも分岐点までのターンが残る — 分岐先の統計に
  「分岐前に払ったぶん」が載るのは**正しい**（その会話の文脈を作った費用）。
  区別は D3 の `scopeMeta.forkedFrom`（**セッションの属性であってターンの属性ではない** —
  `SessionMeta.parent_id` / `forked_at_seq` が既に持っている。rev2 で置き場を訂正。
  利用者査読 3）

### D3. 集計はコアの純関数 + IPC 1 本

**スコープは閉じた列挙 2 値**（rev2 で独立のブロックにした。利用者査読 6）:

```text
Scope = { kind: "session", sessionId: String }   // 1 会話（既定 = 今開いている会話）
      | { kind: "all" }                          // この村の全会話
```

**「全会話」の読み方**: `sessions.redb` は**村に 1 ファイル**で、`sessions` / `records`
の 2 テーブルが全会話を持つ（`data_contract` の `session_store`。会話ごとに
ファイルが分かれてはいない — 査読の前提を訂正）。だから `all` は
`list_sessions()` → 各 id で `records(id)` を読み、`Record::Turn` だけを
`(session_id, TurnRecord)` に集めて `aggregate` へ渡す。**I/O はここまでで、
`aggregate` は純関数**（引数は集めた列と `SessionMeta` の一覧。時計を読まない）。

`stats.rs`（新設・純機構）:

```text
aggregate(turns: &[(SessionId, TurnRecord)], sessions: &[SessionSummary], scope) -> StatsReport
```

IPC は `session_stats(scope) -> StatsReport` の 1 本。集める → `aggregate` の 2 段。

```text
StatsReport {
  scope,
  scopeMeta: {                              // rev2 で新設（利用者査読 3）
    recordedSince: Option<tsMs>,            // 最初の turn の開始時刻。無ければ null（D6）
    sessions: [{ sessionId, title, forkedFrom: Option<sessionId>,
                 turns, effective }],       // session スコープでは 1 件、all では会話ごとの合計表
  },
  totals:  Slice,                           // スコープ全体
  byAgent: [{ agentId, model, ...Slice }],
  byStop:  [{ stop, code?, count }],        // failed は CODE ごとに分ける
  series:  Option<{ points: [{ tsMs, agentId, effective, prompt, completion, stop }],
                    dropped: u32 }>,        // session スコープのみ。all では null
}
Slice { turns, failed, prompt, cached, completion, reasoning, effective,
        cacheRate, outputShare, avgElapsedMs, avgTokensPerTurn }
```

- **`effective` は `budget.rs` の重み関数を呼ぶ**（(prompt − cached) ×1 + cached ×0.1 +
  completion ×4。1 実装）。**`reasoning` は `completion` の内数なので足さない**
  （rev2 で明記）。フロントで再計算しない
- **`series` は session スコープだけ**。末尾 N = 500 件（`MESSAGE_LIMIT` と同じ数）で、
  落とした件数は `dropped` に出す（**溢れは数える** — #72 / MASC の同結論）。
  **all では出さない** — 会話をまたいだ 500 件は古い会話がまるごと消えた列になり、
  棒を見て「その会話は払っていない」と読める（D6 と同じ誤読）。all の主役は
  `scopeMeta.sessions` の会話ごとの合計表（rev2。利用者査読 6）
- **平均は算術平均のみ**（中央値・分位は出さない — 出す根拠になる利用がまだ無い）
- `cacheRate = prompt > 0 ? cached / prompt : 0`（AgentCard と同じ分母。**0 除算の
  ガードを定義に含める** — rev2、利用者査読の細目 2）、
  `outputShare = (prompt + completion) > 0 ? completion / (prompt + completion) : 0`

### D4. 全画面の切り替え

`App.vue` に `view: ref<"village" | "stats">` を 1 つ。TitleBar に 7 つ目の入口
（SVG アイコン + 「統計」）。`view === "stats"` のとき **3 ペインのグリッドを
`v-show` で隠し（DOM に残す）、`StatsView.vue` を `v-if` で足す**。TitleBar と
StatusBar は残る。（rev2 で「`v-if` / `v-else` で差し替える」の 1 文を削った —
次の箇条書きと矛盾していた。利用者査読 5）

- **保存しない**（`bottomTab` と同じ。起動は必ず村から — 「開いただけで統計が出る」
  形にすると、次に来た人が村の状態を見る前に数字を見る）
- **3 ペインの状態は保つ。** `v-if` で外すと `ChatInput` の入力途中・スクロール位置・
  選択中の個体が捨てられる — だから `v-show`。`TopologyMap` は非表示中に
  `ResizeObserver` が発火しない（Spec 21 の罠「合成されていないページ」と同族）ので、
  **戻ったときに自動フィットが 1 回走る**ことを検収で見る
- **更新は CoreEvent `TurnRecorded { agentId, sessionId }` を新設して、統計を
  開いている間だけ取り直す**（rev2 で反転。利用者査読 5）。rev1 は「全ターンで
  イベントを撒くのは払いすぎ」と書いて 10 秒 pull にしたが、**その見積もりが誤り** —
  `AgentStatsUpdated` は稼働中の個体ごとに**毎秒**飛んでおり、ターンの終わりに 1 通
  足すのはその 1/数十以下。S1「1 目で読める」と 10 秒遅延は両立しない。
  **イベントは薄く**（id 2 つだけ・数字を運ばない — 数字は `session_stats` が
  `aggregate` から出す 1 経路に留め、イベントで運ぶと 2 経路目になる）。
  フロントは `view === "stats"` のときだけ `session_stats` を叩き直し、村の
  表示中はイベントを読み捨てる。**`core://event` の variant 一覧（`data_contract`）と
  加算的変更の回帰テスト（Spec 08 の形）に 1 行ずつ足す**。手動更新のボタンは
  置かない（イベントで足りる。足りない実例が出たら別途）

### D5. 通貨に換算しない（裁定済み・凍結）

価格表を持たない。理由は 3 つ — 各社の改定に追従できない / 同じモデルでも
キャッシュの帯で単価が変わる（#104: `gemini-3.7-flash` は 19K 未満で 1.63 倍）/
**通貨を出すと「合っている」と読まれる**（実効トークンは「予算がそう数えた」という
事実で、通貨は推定）。**利用者が自分の単価で掛けるための数字を出す**のが本 Spec の範囲。

### D6. 遡れないことを画面で言う

`recordedSince` が null のスコープ（この Spec より前に作られた会話）は、0 の表を
出さずに**「この会話にはターンの記録がありません（記録はこの版から）」**の 1 行だけを
出す。0 は「払っていない」と読まれる（#68 の裏返し — 存在しない量を 0 と表示しない）。
`fuseforks.log` からの埋め戻しは**しない**（D0 の帰結）。

### D7. カードの累計との関係

`AgentRecord.total_tokens` はプロセスをまたぐ**生涯累計**で、会話をまたぐ。
統計の「全会話」の個体別合計と**一致するのは、この Spec より後に作られた村だけ**。
既存の村では累計 > 統計になり、それは欠陥ではない。**カードの数字は触らない**
（Spec 11 の実効ではなく素の合計を出している現状も据え置き — 別の問い）。

### D8. 描画はテーブル + 1 本の SVG。依存は足さない

promptfoo の 3 チャート（pass rate 棒 / スコア分布 / 散布図）は**pass の概念が
無いので出せない**。出すのは (a) 見出しの合計タイル (b) 個体別の表 (c) 終わり方の
内訳 (d) **ターンごとの実効トークンの棒（時系列 1 本・SVG 手描き）**。
チャートライブラリは足さない — 色は `style.css` の CSS 変数から引く規律
（`v-network-graph` を選んだ理由と同じ）で、SVG なら破れない。

## Stories

- S1. 利用者は、**今の会話で村がいくら払ったか**（実効 / prompt / cached / 出力 /
  思考）と、**そのうち失敗で払ったぶん**を、統計画面の 1 目で読める
- S2. 個体ごとに ターン数 / 失敗数 / 合計 / 平均 / キャッシュ率 / 平均所要 が並び、
  **「どの個体が高いか」がモデル名つきで読める**（#104 の帯の判断がここでできる）
- S3. 終わり方の内訳で `failed:LLM_OUTPUT_TRUNCATED` が何回かが読める
  （#103 の対照 — 塞ぐ前は原理的に 0 だった数字）
- S4. 全会話へ切り替えると、会話をまたいだ合計と、会話ごとの合計が読める
- S5. 統計を見ている間も村は止まらず、戻ると 3 ペインは離れたときのまま
- S6. 旧い会話を開くと「記録がありません」と出て、0 と誤読しない

## Tasks

- **P0**（main 直・rev 承認後）: `data_contract` — `session_store` の Record を
  **4 種別で閉じる**へ改訂（`turn` の欄・`stop` の列挙 7 値と出口の対応・
  `completion = tokens − prompt` は書く側が 1 箇所で引く・**新しい版は旧い
  `sessions.redb` をそのまま読める（既存 3 種別は不変）/ 旧い版で新しい村を開くのは
  非サポート**）/ `observability_rule` に 4 行の末尾 `model=`（D2 の規則）/
  `core://event` に `TurnRecorded` / `stats_contract` 新設（`Scope` 2 値・DTO の形・
  `effective` は `budget.rs` の 1 実装で `reasoning` を足さない・`series` は session
  のみで上限 500 と `dropped`・`cacheRate` の 0 除算・遡れないことの表示規則）。
  `settings_contract` は触らない（保存する設定が増えない）
- **P1**（ブランチ）: `TurnRecord` + `Record::Turn` + `settle_turn` の 1 実装 +
  4 出口の付け替え（`finish_budget_exhausted` に理由の引数）+ 4 行の末尾 `model=` +
  `TurnRecorded` の emit。**結合 5 本** = `stop` の値ごとに 1 件書かれる
  （完走 / `LLM_OUTPUT_TRUNCATED` / `interrupt_turn` / 小天井で `budget_exhausted` /
  **`reserve_short`** — 最後は `tests/failed_turn_settlement.rs` の場面（残額 495 で
  2 周目が通らない）がそのまま使える。rev2 で 4 → 5、利用者査読の細目 3）。
  **ミューテーション**: `settle_turn` からレコードの書き込みを外す → 5 本とも赤 /
  失敗出口だけ付け替えを戻す → 失敗の 1 本だけ赤 / 理由の引数を無視して
  `budget_exhausted` 固定にする → `reserve_short` の 1 本だけ赤（**5 本が別々の
  値を守っている**ことを見る）。`export_session` の JSONL に `kind: "turn"` が
  出ることを 1 本、`restore_histories` が `Turn` を無視することを 1 本
- **P2**（ブランチ）: `stats.rs` の `aggregate` + 単体（実効の重みが `budget.rs` と
  一致し `reasoning` で二重加算しない / `series` の切り詰めと `dropped` / all で
  `series = null` / 空入力で `recordedSince = null` / `byStop` が `failed` の CODE を
  分ける / `prompt = 0` で `cacheRate = 0` / `scopeMeta.sessions` の `forkedFrom` が
  `SessionMeta.parent_id` から来る）+ IPC `session_stats`（集める → `aggregate`）+ `ipc.ts`
- **P3**（ブランチ）: `view` の切り替え + TitleBar の入口 + `StatsView.vue`
  （タイル / 個体別表 / 終わり方 / SVG の棒 1 本（session のみ）/ スコープ切り替え /
  all では会話ごとの合計表 / `TurnRecorded` で取り直し / 記録なしの 1 行）+
  辞書 ja/en + `statsView.test.ts`（**判定が `recordedSince` を見ているか** /
  **表示が `v-show` で村を残しているか** / **`TurnRecorded` を村の表示中は読み捨てるか**
  の 3 点）+ 起動テストの IPC モック（`session_stats` を足さないと赤くなる網）
- **P4**: 台帳 — README 日英（画面の構成表 + 「何ができるか」）/ DETAIL 日英
  （画面の節 + workspace 木は変わらない）/ CLAUDE.md / `failures.md` は出たら
- **P5**: 実機 — 下の検収 6 件

## 検収（P5。**書く前に「その画面がその値を引いているか」を数えた** — #68）

1. `maxOutputTokens` を小さくした個体で `LLM_OUTPUT_TRUNCATED` を 1 回踏む →
   統計の終わり方に `failed:LLM_OUTPUT_TRUNCATED = 1` が出て、**その個体の合計が
   `fuseforks.log` の同時刻の `turn: … stop=failed:LLM_OUTPUT_TRUNCATED` の
   `prompt` / `total` と一致**する（レコードと行が同じ構造体から出ている証拠）
2. アプリを再起動して同じ会話を開くと、1 の数字が**そのまま残る**（redb 由来。
   `AgentStatsUpdated` では出ない数字であることの対照 = 停止中の個体でも出る）
3. この Spec より前に作った会話を開くと「記録がありません」の 1 行だけ（0 の表が出ない）
4. 統計を開いたまま別の個体へ依頼を 1 通投げ、ターンが終わった直後に表が更新される
   （`TurnRecorded` の対照: 依頼の前後で `turns` が +1。**村の表示中に同じ依頼を
   投げても `session_stats` が呼ばれない**ことを dev の IPC ログで見る）
5. 統計 → 村へ戻ると、入力欄の途中の文・選択中の個体・絆の地図の視点が離れたときと同じ
   （`v-show` の検収。**地図は自動フィットが 1 回走ってよい**）
6. 委譲を含む依頼（進行役 → 2 体）の後、`byAgent` の 3 行の `turns` の和が
   `totals.turns` と一致し、進行役の行に `hop=0`、ワーカーに `hop=1` のターンが
   ある（`series` の `agentId` で読む）

## Notes

1. **promptfoo からのチェリーピック**（2026-08-16 に読んだ 2 記事 —
   dev.to「LLM アプリケーションテスト完全ガイド」/ note「推しのプロンプト実験管理
   ツール『promptfoo』を解説」— と結果ビューの実物 2 枚）。**本 Spec が摘んだのは
   費用の層だけ**:

   | promptfoo | この村での対応 | 本 Spec |
   |---|---|---|
   | プロバイダごとの見出し（Requests / Total Tokens / Avg Tokens / Avg Latency / Tokens/Sec） | 個体別の表（turns / 合計 / 平均 / 平均所要）。**Tokens/Sec は出さない**（ストリーミングしないので意味を持たない） | **摘む** |
   | `Cached` の明示 | `cached` と `cacheRate` | 摘む（既にカードにある） |
   | `assert: cost` / `latency` の閾値 | 天井（Spec 11）が cost の閾値の役。latency の閾値は無い | 天井は既にある。latency は**作らない** |
   | pass rate / `llm-rubric` / 回帰の差分ビュー | 合格条件を持たない | **別 Spec の材料**。摘むなら順序は「正解が機械的に決まる軸」から（`reply: to=` の述語 / `run` の exit / TLA・テスト）、LLM 判定は最後 |
   | `~/.promptfoo/promptfoo.db`（SQLite・ローカル） | `sessions.redb`（ローカル） | 同じ形が既にある |
   | 過去履歴のドロップダウン | 会話一覧（Spec 12）+ スコープ切り替え | 摘む |
   | Red-team / 有害性スコア | 個人利用では対象外 | 見送り |

2. **MASC から取れる 2 案のうち、本 Spec に入れたのは「溢れを数える」だけ**
   （`seriesDropped`）。「version カウンタで並行読み手のドリフト検出」は
   `StatsReport` が pull で取り直される以上、古い表を持っている時間は最長 10 秒で、
   検出しても次の手が「更新を押す」しか無い。**入れない**
3. **`turn:` 行に `model=` を足す変更は、この Spec の外でも価値がある**
   （#104 の帯の判断がログだけでできるようになる）。P1 に含めるが、
   D0=(2) に倒れた場合でも**この 1 欄だけは入れる**
4. **予算の残額を出さない**のは D1 で決めた。「今の因果があといくら払えるか」は
   統計ではなく**進行中の状態**で、置くなら `PlanWavePane`（作業状況）の側
5. **カードの累計を実効に揃えるか**は D7 で据え置いた。統計が実効を出すと、
   カードの素の合計と 2 つの数字が並ぶ。混乱が実機で出たら別途
6. **互換の向きは 2 つあり、壊れるのは片方だけ。** (a) **新しい版が旧い
   `sessions.redb` を読む** — 既存 3 種別のシリアライズは 1 バイトも変えないので
   **そのまま読める**（variant を足しても、既知の `kind` しか入っていないデータの
   復元は変わらない。旧い村では `Turn` が 0 件で D6 の 1 行になるだけ）。
   (b) **旧い版が新しい村を読む** — `kind: "turn"` が未知の variant で `records()` が
   落ちる。配布は単一バイナリで、新しい村を古い版で開く運用は想定していない —
   `summary` を足したときと同じ位置づけ。**rev2 で利用者査読 4 を部分反証** —
   「新版が旧データを読む前方互換も壊れる」は成り立たない（(a)）。また
   「D0 (3) の理由（fork / delete / export に手が要る）は (1) にも掛かる」も
   成り立たない — (1) は `records` テーブルの 1 行なので、**fork（`at_seq` 込みの
   複製）/ delete / export は 1 行も変えずに `Turn` を運ぶ**。それが (3) を退けた
   理由そのもの。**採用したのは書き方** — 向きを 2 つに分けて `data_contract` に
   書く（P0）。未知 `kind` を WARN で飛ばす読み取り経路は**作らない**（読めない
   レコードを黙って落とすと、統計が「払っていない」側へ嘘をつく — #72 の形）

## 改訂履歴

- rev1（2026-08-16）: 起票。裁定済みを 3 つ取り込んだ — 全画面（本日）/
  通貨建てにしない（8/15）/ 器は `sessions.redb` の 4 種別目（8/15）
- rev2（2026-08-16・利用者査読 6 点 + 細目 3 点）: **採用 5** =
  1（D1/D2 の衝突 → ログ行の規則「接頭辞と既存欄は不変、末尾に `model=` のみ」）/
  3（`forkedFrom` は Turn ではなく Session の属性 → `scopeMeta` 新設）/
  5（`v-if` / `v-show` の矛盾 → `v-show` が正。**更新は `TurnRecorded` イベントへ反転** —
  rev1 の「払いすぎ」の見積もりが誤り）/ 細目 1（`stop` 7 値と出口 4 つの対応表）/
  細目 2（`cacheRate` の 0 除算）/ 細目 3（結合 4 → 5 本、`reserve_short`）。
  **訂正して採用 3** = 2（`completion = tokens − prompt` を書く側の 1 箇所で引くと明記。
  **`model: Option` はコードで反証** — テンプレートは段 1 で 4 出口より先に解決される）/
  6（`Scope` を独立ブロックへ・all は `scopeMeta.sessions` の表が主役で `series` は
  session のみ。**「会話ごとに 1 ファイル」の前提は反証** — `sessions.redb` は村に
  1 ファイルで 2 テーブル。集める I/O と純関数 `aggregate` の 2 段に割った）/
  細目 1 の後半（`reserve_short` は `finish_budget_exhausted` の同じ行 —
  理由を引数で運ぶ）。**反証 1** = 4（新版が旧データを読む向きは壊れない・
  fork / delete / export は `records` テーブルの 1 行なので手が要らない。
  採ったのは「向きを 2 つに分けて契約に書く」の書き方だけ）
