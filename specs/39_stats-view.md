# Spec: 統計画面 — ターンの記録を `sessions.redb` の 4 種別目に留め、全画面で読む

**ID**: 39
**Date**: 2026-08-16
**Status**: **Done**（2026-08-16。起票から Done まで 1 日。rev3 = 実機で入口の位置を裁定・#107 の退行を回収）→ P0 完了（2026-08-16。`data_contract` の `session_store`（4 種別・
互換の向き 2 つ）/ `core://event` の `turnRecorded` / `observability_rule` の末尾 `model=` /
`stats_contract` 新設。CLAUDE.md と Spec 12 の「3 種別」に続報）→ **P1 完了**（同日・ブランチ `20260816_stats`。結合 5 + 単体 2・ミューテーション 3 回）→ **P2 完了**（同日。`stats.rs` + IPC `session_stats`。単体 8 + 結合 1）→ **P3 完了**（同日。全画面 + `StatsView.vue` + 辞書。vitest 378）→ **P4 完了**（同日。README 日英 / DETAIL 日英 / CLAUDE.md）→ 合流 → **rev3 の実機修正**（入口を StatusBar へ / #107 の退行を回収）→ **P5 完了 = Done**（2026-08-16。**検収 6 件すべて観測**）
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
| `tsMs` | ターン開始の壁時計 | `TurnContext.started_ts_ms`（**P1 で訂正**。上と同じ）。**終了時刻ではなく開始** — 並べ替えの鍵は「いつ頼んだか」 |
| `hop` | `TurnSpend.hop` | 0 = 因果の根（利用者 / 予定 / 外部）。委譲されたターンは ≥ 1 |
| `rounds` | `TurnSpend.rounds` | 上限（`max_tool_iterations`）は**入れない** — 設定であって観測ではない |
| `waves` | `plan_wave` | |
| `stop` | 閉じた列挙 | 下の表 |
| `prompt` / `cached` / `completion` / `reasoning` | `TurnSpend` | **`completion` はレコードで初めて生まれる欄** — `TurnSpend` が持つのは `tokens`（= total）で、`settle_turn` が **`completion = tokens − prompt`** を 1 箇所で計算する（rev2 で明記。利用者査読 2 — 「引き算を要求しない」のは*読む側*の話で、*書く側*は引く）。ログ行は従来どおり `total=tokens`。`reasoning` は `completion` の内数（Spec 32 D2）— D3 の実効計算で二重に足さない |
| `model` | `template.model` | **`turn:` 行に無かった欄**。#104 の「帯も機種も見えない」を閉じる。同時にログ行の**末尾へ `model=` を足す**（D2）。**`Option` にしない** — テンプレートは `handle_message` の段 1（`world.template(...)?`）で 4 出口のどれより先に解決され、引けなければターン自体が始まらない（`observability_rule` の「バックエンドの解決より前に落ちたターンは `turn` 行を持たない」と同じ位置）。rev2 で利用者査読 2 の `Option<String>` 案を**コードで反証** |
| `backend` | `backend.name()` | ワイヤ名 |
| `elapsedMs` | `TurnContext.started.elapsed()` | **P1 で訂正** — 起票時に書いた `requested_at` は*割り込みを要求された時刻*で、ターンの開始時刻はどこにも無かった。`run_turn` の入口で `TurnContext` を作って 4 出口へ渡す |

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
  byAgent: [{ agentId, model, ...Slice }],   // 鍵は (agentId, model)。rev4 で訂正
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

`App.vue` に `view: ref<"village" | "stats">` を 1 つ。`view === "stats"` のとき
**3 ペインのグリッドを `v-show` で隠し（DOM に残す）、`StatsView.vue` を `v-if` で
足す**。TitleBar と StatusBar は残る。（rev2 で「`v-if` / `v-else` で差し替える」の
1 文を削った — 次の箇条書きと矛盾していた。利用者査読 5）

**入口は StatusBar（フッター）にアイコンだけで置く**（rev3・2026-08-16 実機裁定。
P3 では TitleBar の 7 つ目に置いていた）— **あの列はダイアログの入口だけの列**で、
そこへ「面ごと差し替える 1 つ」を混ぜると、同じ見た目で振る舞いが違う。
時計の左・`--color-run` の緑で発光。**この帯で唯一の操作**なので字を持たない。
**「村へ戻る」も字から出口の図形へ**（この面は表と数字ばかりで絵が少ない）。

**統計を見ている間、TitleBar のダイアログ入口 6 つは `disabled`**（同裁定）。
**窓操作 3 つは塞がない** — 最小化・最大化・閉じるはどの面でも要る。

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

### D5. 通貨に換算しない（裁定済み・凍結 → **2026-08-18 に覆した**）

> **[Spec 41](41_model-pricing.md) が覆した**（利用者裁定 —「おおよその合計コスト
> (金額)みたいなのは出したいね」）。**3 つの理由のうち 2 つが 3 日で消えた** —
> 追従は「固定して取り込む」で選択になり、キャッシュの帯は Spec 40 の分解で
> **実測の内訳が帯の結果を含む**ようになった。**残る 1 つ（通貨は「合っている」と
> 読まれる）は撤回していない** — Spec 41 は「出さない」ではなく
> **`≈` と被覆率で近似だと分かる形にする**ことで受ける。以下は凍結時の原文。

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

## P1 実装記録（2026-08-16・ブランチ `20260816_stats`）

- **D1 の `elapsedMs` の起点を訂正した。** 起票時に「`requested_at.elapsed()` — 割り込み経路
  だけが持っていた」と書いたが、**`TurnHandle.requested_at` は割り込みを要求された時刻**で、
  割り込み経路の「要求から N 秒」の起点。ターンの開始時刻は**どこにも無かった**。
  `TurnContext { started, started_ts_ms, model, backend }` を新設し、`run_turn` の入口
  （バックエンド解決の直後）で 1 度作って 4 出口へ `&` で渡す。`tsMs` / `elapsedMs` は
  ここから。`turn interrupted:` 行の `elapsed_ms=`（要求からの経過）とは**別の量**で、
  欄名が同じでも取り違えないよう `settle_turn` の doc に書いた
- **`settle_turn` の 1 実装** = (1) カードの累計へ積む（**4 出口に散っていた加算を
  ここへ移した** — 出口ごとに積むと片方だけ既定値のまま化ける、#103 の形） (2)
  `TurnRecord` を組んで `persist` (3) 書けたら `TurnRecorded`。ログ行は D2 のとおり
  **`turn:` の 2 出口（完走 / 失敗）はここが書き、後ろ 2 出口は呼び出し側が自分の行を
  書いて末尾に `model=` だけ足す**（数字は戻り値の `TurnRecord` から読む）。
  `completion = tokens − prompt` はここの 1 箇所
- **`persist` が `bool` を返すようになった**（書けたか）。保存先を持たない村・書けなかった
  村では `TurnRecorded` を出さない — 出すとフロントが無い記録を取りに行く
- **`budget_stop_reason` を新設**（`budget stop:` の 1 行 + `TurnStop` の 2 値を同じ判定から
  返す）。以前は予約の 2 箇所（周回境界 / まとめ呼び出しの前）に同じ `note!` が
  複製されており、理由は行にしか無かった。`finish_budget_exhausted` に `stop` の引数が
  増え、`debug_assert!` で 2 値以外を弾く。**`tests/budget_reserve_wiring.rs` の
  ソース走査が赤くなった**（`"budget stop:` のリテラルを 2 で数えていた）→
  「書式は 1 実装 + 呼び出し 2 箇所」へ理由つきで更新
- **読み口は `export_session` の JSONL**（`tests/turn_records.rs`）。redb はバイナリなので、
  人が読める出口が機構の一部（Spec 12）— 5 本がそれを通ることで「JSONL に
  `kind: "turn"` が出る」も同時に留まる。**結合 5 本 = `stop` の 5 値**（完走 /
  `failed:LLM_OUTPUT_TRUNCATED` / `interrupt_turn` / 天井 1,000 で `budget_exhausted` /
  天井 3,000・実費 2,400 で `reserve_short`）。**ミューテーション 3 回とも予測どおり** —
  書き込みを外す → 5 本赤 / 失敗出口だけ戻す → 失敗の 1 本 / 理由を `exhausted` 固定 →
  `reserve_short` の 1 本
- **単体 2 本**（`session_store.rs`）— `Turn` は `restore_histories` / `tail_messages` /
  `fork_points` のどれにも現れず、seq は 4 種別で 1 つの列、fork は seq 込みで複製、
  JSON は `kind: "turn"` + camelCase + `stop: { kind, … }` / `TurnStop::log_label` が
  `turn:` 行の `stop=` と 1 対 1。**観察 1 つ**: `fork_points` の `at_seq` は turn の seq を
  座標として数える（turn は候補にならないが、直前の seq が turn ならそこを指す）— 分岐は
  seq 込み複製なので正しい
- **TS の `CoreEvent` union に `turnRecorded` を足した**（ワイヤ形のミラー）。受け手は
  P3（`useOrchestrator` の `switch` に `default` は無く、未知の型は読み捨てられる）
- ワイヤ層は 1 行も変えていない。fuseforks-core 563 + 結合全緑・clippy 警告ゼロ・
  vitest 365・`vue-tsc` 緑（**`cargo test --workspace` は開発機で `fuseforks.exe` が
  ロックされており走らせていない** — GUI クレートは `cargo check --tests` まで）

## P2 実装記録（2026-08-16・同ブランチ）

- **`stats.rs` 新設**（純機構）— `StatsScope` / `StatsSlice` / `AgentStats`（Slice を
  `#[serde(flatten)]`）/ `StopCount` / `SeriesPoint` / `StatsSeries` / `SessionStats` /
  `StatsScopeMeta` / `StatsReport` と `aggregate(turns, sessions, scope)`。実効は
  `budget::effective_tokens` を呼ぶ（`TurnRecord` → `Usage` へ写して渡す。`reasoning` は
  重み関数の中でも足していない）。**丸めは `budget.rs` の切り上げに従う** — 単体で
  `(10 − 2) ×1 + 2 ×0.1 + 3 ×4 = 20.2` を 20 と書いて赤になり、21 へ直した（重みだけ
  でなく丸めも 1 実装に従うことが、赤で確かめられた形）
- ~~**`by_agent.model` は最後のターンのモデル**~~ → **2026-08-18 に覆した。鍵は
  `(agentId, model)` で、モデルを切り替えた個体は行が増える**（`agentId` は一意でない）。
  旧実装は最後のターンのモデルで上書きしており、**その個体の全ターンがそのモデルの下に
  畳まれていた** — 単価はモデルごとに違うので、外部の価格表を当てると切り替え前の
  ターンまで最後のモデルの単価で計算される。**「いま何で払っているか」は AgentCard が
  既に持っている**（`AgentSnapshot.model`）ので、統計が同じことを言う必要は無かった。
  詳細は下の「D3 改訂（2026-08-18）」。並びは実効の多い順・同点は id 順 → モデル名順。
  `by_stop` は件数の多い順・同点は種別名 → コード順。`scope_meta.sessions` は渡した
  並び（`all` は `list_sessions()` の更新の新しい順）で、**`turns` に無い会話も 0 で並ぶ**
  （表の行として存在する。0 を「払っていない」と読ませないのはフロントの仕事 — D6）
- **`Orchestrator::session_stats(scope)`**（`sessions.rs`）= 集める → `aggregate`。
  `session` は `session_meta` で存在検査（無ければ `SessionNotFound`）、`all` は
  `list_sessions()` → 各 `records()`。**`sessions.redb` は村に 1 ファイル**なので
  `all` でも開くファイルは 1 つ（rev2 の反証どおり）
- IPC `session_stats(scope)`（`commands.rs` + `lib.rs` の登録）/ `ipc.ts` の
  `sessionStats` / `types.ts` に `TurnStop` / `StatsScope` / `StatsSlice` / `AgentStats` /
  `StopCount` / `SeriesPoint` / `StatsSeries` / `SessionStats` / `StatsReport`
- **単体 8 本**（`stats.rs`）: 重みの一致と `reasoning` の二重加算なし / 空入力で
  `recorded_since = None` と比 0 / `prompt = 0` で `cache_rate = 0` / `by_stop` の CODE 分割と
  `failed` の数え / `by_agent` の並びと最後のモデル / `series` の末尾 N と `dropped`・
  逆順入力でも並ぶ・`all` では `None` / `all` の会話ごとの表と `forked_from` /
  ワイヤ形（`kind` タグ・平坦な Slice・`code` は無ければ出ない）。**結合 1 本**
  （`tests/turn_records.rs`）: 2 ターン後に `session` / `all` の両スコープで数字が出て、
  存在しない会話は `SessionNotFound`
- fuseforks-core 全緑・clippy 警告ゼロ・vitest 365・`vue-tsc` 緑・GUI クレート
  `cargo check --tests` 緑

## P3 実装記録（2026-08-16・同ブランチ）

- **`App.vue`**: `view: ref<"village" | "stats">`（保存しない）。3 ペインのグリッドは
  **`v-show="view === 'village'"`** で DOM に残し、`StatsView` を **`v-if`** で足す。
  TitleBar に `statsActive` を渡し `toggle-stats` で往復
- **`TitleBar.vue`**: 7 つ目の入口（棒グラフの SVG + 「統計」）。**他の 6 つと違いトグル**
  なので押している間 `is-on`（`aria-pressed` も出す）— 3 ペインが丸ごと消えるので、
  どこに居るかがタイトルバーから読めないと戻る導線が消える。`data-stats-toggle`
- **`StatsView.vue`**: 見出し行（題 / スコープ「この会話 | 全会話」/ 単位の注記 /
  「村へ戻る」）+ 合計タイル 6 枚（ターン・うち失敗 / 実効 / 入力・キャッシュ率 /
  出力・うち思考 / 平均トークン・出力の割合 / 平均所要）+ **時系列の SVG 棒 1 本**
  （session のみ・実効・色調は `is_failure` の境界）+ 会話ごとの表（all のみ・
  `turns = 0` は「—」）+ 個体別の表（promptfoo のプロバイダ見出しの写し）+ 終わり方の
  内訳。**記録が無い会話は 1 行だけ**（`data-stats-empty`）。取り直しは
  `watch([scope, turnRecordedTick])` で、**古い応答が新しい応答を上書きしない**よう
  `fetchSeq` で最後の 1 本だけ採る
- **`useOrchestrator.ts`**: `state.turnRecordedTick`（受けた回数）。`case "turnRecorded"` は
  **数を進めるだけで IPC を呼ばない** — 取り直すかは統計画面（`v-if` で足される間だけ）
  が決める。これで「村の表示中に `session_stats` が走らない」が構造で決まる
- **`lib/statsView.ts`**（純関数）: `statsNotice`（`recordedSince` だけを見る）/
  `STOP_LABEL_KEYS`（7 値の `Record` — 閉じた列挙を網羅で持つ）/ `stopTone`
  （`is_failure` と同じ境界）/ `seriesBars`（**線形** — 突出こそ見たいもの。全部 0 なら
  高さ 0 の棒）/ `formatPercent` / `formatDuration`
- **辞書 ja/en**: `titleBar.stats` / `titleBar.statsTitle` + `stats.*`（59 鍵ずつ。
  `json` モジュールで組み立て、既存の鍵に差分ゼロ — Spec 28 P3 の教訓）
- **`statsView.test.ts` 13 本**: 判定が `recordedSince` を見ている（`turns = 3` でも
  `null` なら empty / `turns = 0` でも値があれば ready）/ `App.vue` の `v-show` と
  `v-if` と「保存しない」をソース走査 / `turnRecorded` の受け手が IPC を呼ばない /
  `seriesBars` の線形・0・色調 / 7 値の鍵が ja/en に揃う / 書式。
  **ミューテーション**: `v-show` → `v-if` で 1 本だけ赤（予測どおり）
- 描画ライブラリは足していない（`package.json` の依存は不変）。vitest 365 → 378・
  `vue-tsc` 緑・`bun run build` 緑。**画面の目視は実機（P5）** — vite 単体では
  `invoke` が落ちて起動の覆いが外れない（Spec 25 P3 と同じ）
- **起動テストの IPC モックは触っていない** — `initialize()` は `session_stats` を
  呼ばない（統計画面が開いたときだけ）。Tasks に書いた「足さないと赤くなる網」は
  この形では発火しない（初期化が IPC を増やしていないので正しく緑）

## P4 台帳記録（2026-08-16）

- README 日英の「何ができるか」に **📊 統計** の 1 行（通貨に換算しない / 失敗の払いも入る /
  記録はこの版から）。**README は 161 → 162 行**で、CLAUDE.md の「160 行以内」の
  上限を Spec 36/37 の頃から 1 行超えており、本 Spec でさらに 1 行。守りたいのは
  「最初に探すものが埋まっていない」ことで、`bun install` は今も `## ビルド` の下に在る
- DETAIL 日英: ディレクトリ木に `StatsView.vue` / TitleBar の入口列に「統計」/ 画面の
  構成表に **全画面** の行（モーダルではない、を明記）/ ログの節に「4 行の末尾 `model=`」と
  「同じ数字は `sessions.redb` の `turn` レコードにも残る。この版より前の会話には無い」
- CLAUDE.md: Spec の状態を P0〜P4 完了へ + 次に触る人が要る 3 点 / #104 の節の
  「`turn:` にモデル名も無い」に取り消し線と続報
- `data_contract` は P0 で凍結済み。P1〜P3 で契約から外れた点は 1 つ — `tsMs` の注記
  「(requested_at)」が Spec D1 の取り違えを写しており、P4 で「`run_turn` の入口 =
  `TurnContext.started`。`TurnHandle.requested_at` ではない」へ訂正した

## P3 修正記録（2026-08-16 実機・利用者指摘 3 点 → rev3）

**指摘 1 が退行だった**（`failures.md` #107）。統計画面でダイアログを開くと、
その場では何も起きず村へ戻った瞬間に現れる。真因は **`ToastHost` / `ConfirmHost` /
ダイアログ 6 つ / 初期化の覆いがグリッドの内側にあった**こと — `v-show` の
`display: none` の中では **`fixed` でも描画されない**。**利用者が踏んだのは
ダイアログだが、重いのは `ConfirmHost`** で、統計画面では「閉じる前の確認」が
出ず `askConfirm` が解決しない = **窓が閉じられない**。全画面の層をグリッドの
外へ出した。**入口を塞ぐだけでは足りない**（`ToastHost` / `ConfirmHost` の入口は
TitleBar ではない）。

**指摘 2・3 は入口と出口の作法**（上の D4 に反映）。入口を StatusBar へ移したことで、
TitleBar は「ダイアログの入口だけ」に戻り、列の性質が割れなくなった。

**テスト 13 → 26 本**（`statsView.test.ts`）。足したのは (a) 9 つの層がグリッドの
**外**にある（範囲は `<div>` の深さで求める — インデント判定は整形で壊れる）
(b) TitleBar が統計の入口を持たない (c) StatusBar が入口を持ち `--color-run` で
発光する (d) ダイアログ入口 6 つが `disabled` になり窓操作 3 つは残る。
**ミューテーション**: `ConfirmHost` をグリッドの中へ戻すと 1 本だけ赤。

**作業中に自分で踏んだ罠**（#107 の一般化 3）: ブロックを移す編集で閉じタグを
1 つ落としたとき、**`vue-tsc` も vitest 391 本も緑のまま**で、`bun run build` だけが
`Element is missing end tag` で落ちた。**テンプレートを構造ごと動かす編集は
ビルドで確かめる。**

## P5 実機記録（2026-08-16。**検収 6 件すべて観測 = Done**）

観測は 3 回の走行に分かれた。**時刻が近い 2 枚が対で読める形になったのが効いた** —
1 枚だけでは「そういう状態」と読めるものが、2 枚だと「判定が生きている」と読める。

| 検収 | 観測 | 決め手 |
|---|---|---|
| 6 | 4 体・5 ターンの会話（04:28） | `byAgent` の 2+1+1+1 = `totals.turns` 5。**4 体が別々のモデル**で並び、`turn:` 行に無かった `model=` が画面で効いた |
| 2 | 再起動しても数字が残る | **`AgentStatsUpdated` では出ない** — 再起動直後は誰も稼働しておらずイベントは 1 通も飛ばない。`turns` / `failed` / `byStop` は累計 4 欄からは作れないので、**残ること自体が redb 由来の証拠** |
| 3 | 記録の無い会話（05:58:49） | 0 の表を出さず 1 行だけ |
| 4 | **同じ会話**の 85 秒後（06:00:14） | `turns` 0 → 1。`turnRecorded` が届いた証拠であると同時に、**「記録がありません」が固定の状態ではなく `recordedSince` で動的に決まる**ことも読める |
| 5 | 統計 ↔ 村の往復 | 入力途中・選択・視点が残る |
| 1 | `maxOutputTokens` = 64 の個体で切り詰め（06:02:07） | 内訳に **`失敗: LLM_OUTPUT_TRUNCATED 1`**、棒の 2 本目が赤、当該個体の行の `失敗` が 1 |

**検収 1 の走行が #107 の負の対照を兼ねた。** 失敗のトーストが**統計画面の上に出た** —
修正前は `ToastHost` がグリッドの中で `display: none` だったので、**この吹き出しは
原理的に見えなかった**。検収に書いていなかったが、同じ 1 枚が処方の実証になっている。

**数字は 3 回とも内部で閉じた**（表示から逆算）:

```text
04:28 実効 = 164.5×1 + 380.1×0.1 + 22.8×4 = 293.7K   （タイルと一致）
06:02 実効 = ジェミー 43.8K + ミュゼ 9.6K = 53.4K      （個体別の和 = totals）
06:02 ミュゼ = 9.3×1 + 0×0.1 + 0.066×4 = 9.56K → 9.6K
```

**ミュゼの出力が上限 64 の近傍（≈66）**なのが決定的 — 切り詰めで止まったターンの
払いが、そのまま数字として入っている（#103 の処方が画面まで届いた）。
キャッシュ率 **0.0% が警告色**なのも設計どおり（初回呼び出しなのでキャッシュが無い、
という正しい状態を色で言っている）。

**実機でだけ見えた観察 2 つ**（機構は足していない）:

- **棒に縦軸が無く、最大値が読めない**（ホバーの `title` には出る）。突出は形で分かるが
  「何トークンの突出か」は静止画から読めない。**1 本しか無いときは常に高さいっぱい**に
  なる（最大値 = 自分自身）ので、単体の棒は情報を持たない
- **「全会話」の表は当面ほぼ `—` で埋まる**（記録がこの版からなので）。時間が解決するが、
  いま見ると 14 行中 13 行が空

どちらも #49 の形（テストが通る機構でも、人がどう使うかは実機でしか出ない）。
**頻度を見てから決める**ので、今は直さない。

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

## D3 改訂（2026-08-18・rev4）— `by_agent` の鍵を (個体, モデル) へ

**起点は利用者** —「エージェント + モデルでのトークンに分けられるか？ API の単価表を
JSON で持って金額を算出したいので、現在のモデル名で算出されていると金額が狂う」。

**懸念は当たっていた。しかも実装は、その形で間違えていた。** 旧 `aggregate` の鍵は
`agentId` 単独で、`model` は毎ターン上書き（`entry.1 = turn.model.as_str()`）だった。
帰結: sonnet で 100 ターン → opus で 1 ターン走った個体は、**101 ターン全部が opus の
行に載る**。単価を掛けると過大に出る。

- **「機能が無い」より悪い形だった** — 出てくる数字がもっともらしく、**間違っている
  ことが画面から読めない**。器（`model` 欄）は最初からあり、集計だけが潰していた
- **データは失われていない。** `Record::Turn.model` は**ターンごと**で、D1 が
  非 `Option` として凍結済み（テンプレートは段 1 で 4 出口より先に解決される）。
  だから **`v0.1.8` 以降の全ターンを遡って正しく割れる。移行も新しい記録も要らない**
- **旧挙動はテストが「正しい」として凍結していた** —
  `by_agent_orders_by_effective_and_takes_the_latest_model` が
  `assert_eq!(…model, "m2", "最後のターンのモデル")` を主張していた。
  **一般化: 誤った挙動を凍結したテストは、赤くならないので誰も気づかない。**
  期待の側を先に書き換えて Red を出し、その失敗出力（`("a","m2",3,30)` =
  3 ターンが最後のモデルへ畳まれている）を症状の証拠として読んだ
- **`agentId` が一意でなくなる**のが、この変更の唯一の波及。`StatsView.vue` の
  `:key="row.agentId"` が**重複キーになる**ので複合キーへ直した（型検査にも
  vitest にも掛からない種類。#107 と同じ位置の罠）
- **並びの同点処理を 1 段深くした**（実効の多い順 → id 順 → **モデル名順**）。
  同じ個体の複数行が並ぶのは本 Spec で初めて起きるので、その中の順序を固定する
- **旧設計の目的（「いま何で払っているか」）は失われない** — それは
  `AgentSnapshot.model` が持ち、AgentCard が既に出している。**統計が同じことを
  言う必要は無かった**（統計は履歴で、カードは現在。層が違う）
- **`totals` は変わらない**（同じターンを 2 度数えない）。テストで凍結した

**金額の算出には、この変更で埋まらない穴が 2 つ残る**（別件。1 は
[Spec 40](40_cache-write-accounting.md) として同日に起票、2 は未起票）:

1. **キャッシュの書き込みが独立していない。** canonical の `Usage` は
   `prompt` / `cache_read` / `completion` / `reasoning` の 4 つで、
   `anthropic.rs` は `cache_creation_input_tokens` を**読んでいるのに `prompt` へ
   畳んでいる**。**この村は `CACHE_TTL = "1h"` を送っているので書き込みは 2.0×**
   （5 分なら 1.25×）。`uncached = prompt − cached` を基本単価で掛けると
   **書き込みぶんを半分しか数えない**。ずれの大きさは未測定。射程が違う（canonical + 6 ワイヤの decode +
   `TurnRecord` の欄）ので混ぜない
2. **`base_url` を記録していない。** 鍵にできるのは `model` と `backend`（ワイヤ名）
   だけで、**OpenAI 互換の口は複数のベンダーが共有する**。価格表の鍵を `model` 単独に
   するか `backend + model` にするかは、表を作る側で先に決めておく

**`reasoning` は `completion` の内数**なので出力単価で掛けるのが正しく、
足し込む必要は無い（思考も出力単価で課金される）。ここは変更なし。

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

- **rev4（2026-08-18・利用者要望）**: D3 の `by_agent` の鍵を `agentId` 単独から
  **`(agentId, model)`** へ。旧実装は最後のターンのモデルで上書きしており、
  外部の価格表を当てると切り替え前のターンまで最後のモデルの単価で計算された。
  誤った挙動を凍結していたテストを書き換えて Red を出してから直した。
  詳細は「D3 改訂（2026-08-18・rev4）」の節
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
