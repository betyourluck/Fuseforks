# Spec: 思考トークンを数える — `reasoning` を `completion` から分離する

**ID**: 32
**Date**: 2026-08-10
**Status**: **rev3 → P0〜P4 完了 = Done**（2026-08-10。**起票から Done まで
1 セッション**。起票 `59063bb` / P0 = `bcad504` / P1 = `e4b6204` /
P2 = `2e503e8` / P3 = `146fd22` / P4 = `f437cf5` + 本コミット。
**4 社すべてで思考トークンが取れる。**
実機は 4 社 6 ターン。**検収は 2 度書き直した** — 1 は判定基準が probe の産物
だった（`failures.md` #92）/ 2・5 は Anthropic の判定が誤っていた
（`failures.md` #93。P4 で訂正）。
P0 の作業中に**社を数え落としていた**のが出て改訂。
rev2 は 3 社で数えていたが **Gemini が 4 社目**で、しかも
**思考トークンを既に読んでいる唯一の社**だった。検収 2 の対照も同時に壊れていた。
rev2 = 査読 8 点 → 採用 6 / 訂正して採用 1 / 反証 1。rev1 からの最大の変更は
**範囲の確定** — 本 Spec は「数える」まで。要約本文の受け取りと表示は Spec 33、
OpenAI の Responses ワイヤは Spec 34 へ送る）
**Branch**: なし（main へ直接コミット。Phase ごと）

## Goal

**モデルが思考に使ったトークンを、村が数えられるようにする。**

実測（下記）で `grok-4.5` は**出力の 99% 以上が `reasoning`** で、**本文は 4〜8 字**
だった。いまの村はこれを `Usage.completion` へ畳んでおり、**カードの累計にも
`turn:` 行にも「思考にいくら払ったか」が 1 桁も出ていない**。#72（握り潰した事実を
数えないと後から追えない沈黙になる）と同じ形が、捨てているのではなく**畳んでいる**
側で起きている。

**要約本文の受け取りと表示は本 Spec の対象外**（rev1 から縮小。査読 4 の A 案）。
理由は Goal と Phase の重心が一致していなかったこと — rev1 は Goal を「数える」に
置きながら Phase の 6 割が「見せる」で、しかも Note で「1 段目だけで閉じる選択肢」と
「OpenAI 互換を含めるか」を**両論併記のまま P0 へ送っていた**。決定を残した契約は
実装で分岐する（Spec 20 D10 が「機構 A の結果」と「機構 B の条件」を同居させて
矛盾したのと同型）。

## 接地（2026-08-10 の probe 8 発。使い捨ての example・実鍵・`grok-4.5`）

**予測を先に書いてから観測し、2 つとも外れた。**

### 実装の現状（コードで数えた）

| 社 | 現状 | 出典 |
|---|---|---|
| **xAI** | `"reasoning" => reasoning_count += 1` で**数えるだけ**。`XaiOutputItem` に payload を受ける欄が**無い**。usage の `reasoning_tokens` も読んでいない | `llm/xai_responses.rs` / `llm/wire.rs` |
| **Anthropic** | decode が `Thinking` / `RedactedThinking` を `dropped` へ入れる。**ユニット変種**で本文を deserialize すらしない。~~`budget_tokens` が無い = 要求していないので今は 1 件も返っていない~~ **後半は誤り（P4 で訂正）** — **要求しなくても既定で思考しており、数も返っている** | `llm/anthropic.rs` / `llm/wire.rs` |
| **OpenAI 互換** | 思考は `/v1/chat/completions` に**来ない**。`reject_empty_reasoning` は「思考だけで本文が空」を検出する**別の門**で、受け取りではない | `llm/openai_compat.rs` |
| **Gemini** | **既に読んでいる。** `completion: candidates_token_count + thoughts_token_count` と**足し込んでおり**、`data_contract` に「この数え方でのみ `totalTokenCount` と一致する（実測）」と凍結済み。**分離されていないだけで、数字は手元にある** | `llm/gemini.rs` / `llm/wire.rs` の `thoughts_token_count` / `data_contract.yaml` |

**Spec 31 D7 の「decode の捨てている行は 1 箇所なので、そちらで一括して拾う」は
誤り。** 実測は **4 社 4 形**。P0 で当該行に取り消し線を入れて回収する
（`specs/15:282` の前例）。CLAUDE.md の同じ記述も同時に回収する。

### `reasoning_tokens` は `output_tokens` の内数（8/8。査読 2 の決着）

| 回 | `output_tokens` | `reasoning_tokens` | 差 | 本文 chars |
|---|---|---|---|---|
| 1 | 1,475 | 1,470 | 5 | 4 |
| 2 | 1,892 | 1,889 | 3 | 8 |
| 3 | 1,698 | 1,695 | 3 | 4 |
| 4 | 1,497 | 1,494 | 3 | 4 |
| 5 | 2,161 | 2,157 | 4 | 5 |
| 6 | 3,690 | 3,685 | 5 | 8 |
| 7 | 1,123 | 1,118 | 5 | 8 |
| 8 | 1,637 | 1,634 | 3 | 4 |

**差は本文のトークン数と一致する。** 内数か外数かは推測ではなく**実測で決まった** —
外数なら差が本文と無関係に開く。

### xAI の `reasoning` item は中身を持つ（Spec 33 の入力）

```json
{ "id": "rs_…", "summary": [{ "type": "summary_text", "text": "…" }],
  "type": "reasoning", "status": "completed" }
```

- **欄は 4 つで固定**（8 回とも同じ）。**`encrypted_content` も `signature` も無い**
  → **xAI には往復の要求が無い**
- 要約は **2 形**で来る — 短形（129 字 = 問題文の再掲だけ）と長形（919〜1,367 字）
- **`reasoning: {"summary":"auto"}` は効かない** — 短形/長形の分布が
  **A（既定）2:2 / B（auto）2:2 で一致**した。初回の 1 対 1 で止めていれば
  「auto を送ると要約が痩せる」という**逆の実装**をしていた
  （`failures.md` #91 / 自己観察 `one_sample_generalization_20260810` と同型）
- **要約は生の思考ではない** — `reasoning_tokens` 1,118〜3,685 に対し要約は最大 1,367 字

## 決定（D）

### D1: 本 Spec の範囲は「数える」まで（査読 4 の A 案）

入るのは `Usage` の 1 欄と、それを運ぶ計器・統計・画面の数字だけ。
**要約本文・その表示・送信側のパラメータ追加はすべて対象外**。

**Note で「確認したい」を残さない。** 範囲は本 D で確定しており、
Spec 33 / 34 への引き渡しは末尾の引き継ぎ節に**決定として**書く。

### D2: `reasoning` は `completion` の内数。天井と課金の計算は 1 ミリも動かさない

```rust
/// 出力トークン数。
pub completion: u64,
/// うち思考に使われたぶん。**`completion` の内数**（実測 8/8 で
/// `completion - reasoning` = 本文のトークン数）。取れないプロバイダでは 0。
pub reasoning: u64,
```

- **`Usage::total()` は `prompt + completion` のまま。** `reasoning` を足すと
  **二重計数**になり、トークン天井（Spec 11）の実効値が跳ねる
- **実効トークンの重み（未キャッシュ ×1 / キャッシュ済み ×0.1 / 出力 ×4）も不変。**
  思考ぶんは**既に出力として ×4 で天井に乗っており**、分離しても課金の扱いは
  変わらない。本 Spec が変えるのは**見え方だけ**
- **`Option<u64>` にしない。** 「取れない」と「0 だった」を型で分けると、
  表示側が 2 状態を扱うことになる。D3 の対照検収は**両方 0 で出る**ことを
  要求するので、区別は害になる

### D3: `turn:` 行へ `reasoning=` を**常に**出す（0 も出す）

現行は `turn: agent={} hop={} rounds={}/{} waves={} stop={} prompt={} cached={} total={}`
（`orchestrator`）。**`output` という欄はそもそも無い**ので、そこへ足す形ではなく
`total=` の隣へ 1 語を加える。

**条件付きで出さない**理由は検収 2 にある — 思考を使わないモデルで **0 が出る**ことが
「常に出す実装」との対照になる。**省くと対照が取れず、機構が効いているか読めない**
（Spec 31 で `queries=0` が真因を教えたのと同じ形）。

### D4: 4 社の扱いは非対称。P1 の完了条件は社ごとに違う（査読 3。rev3 で 4 社目を追加）

**「共通」とは呼ばない。**

| 社 | P1 の完了条件 |
|---|---|
| **Gemini** | **`thoughts_token_count` を `Usage.reasoning` へも入れる**（`completion` への足し込みは**そのまま**）。既に読んでいるので**最も安い**。`data_contract` の「この数え方でのみ `totalTokenCount` と一致する」を壊さないことを回帰で留める |
| **xAI** | `usage.output_tokens_details.reasoning_tokens` を読み、`Usage.reasoning` へ入れる |
| **Anthropic** | **usage に同じ数字があるかを数え、結果を Spec へ書く。** 無ければ 0 のまま（要求していないので現状は返らない — D5） |
| **OpenAI 互換** | 同上。**思考が来ない経路なので 0 が正しい**可能性が高い |

**観測結果を書くことが完了条件**で、値が取れることは完了条件ではない。

**rev2 は Gemini を数え落としていた。** 起票時に数えたのは「思考を捨てている
社」だけで、**足し込んでいる社**が網の外にあった。`Thinking` / `reasoning` で
grep すると `llm/` の 3 社が出るが、Gemini の欄名は `thoughts_token_count` で
**同じ語では引けない**（#62 と同型 — 名前で引く網は、別の名前を持つ同じ概念を
拾わない）。**同じ概念に社ごとの呼び名があるときは、概念の側から数える。**

### D5: 送信側には何も足さない

`reasoning: {"summary":"auto"}`（xAI）も `thinking` / `budget_tokens`（Anthropic）も
**本 Spec では送らない**。前者は効果ゼロを 4 対 4 で実測済み、後者は
**要求すると応答の形が変わる**ので Spec 33 の probe（旧 D4）が先。

**D5 は xAI と Anthropic で理由が違う**（査読 5）— 前者は「効かないから送らない」、
後者は「効くが、効いた先を測っていないから送らない」。

### D6: 台帳の `data_contract.yaml` は P0 と P2 で役割が違う（査読 8a）

- **P0 = 形式の凍結**（`Usage` の欄の意味・内数であること・`turn:` 行の形）
- **P2 = 実装への追従回収**（実測値・観測結果・隣の節の漏れ）

## Phases

- **P0**: 契約凍結 — `data_contract.yaml` へ `Usage.reasoning`（内数・0 の意味・
  `total()` 不変）と `turn:` 行の形式。**Spec 31 D7 と CLAUDE.md の
  「decode の 1 箇所」を取り消し線で回収**
- **P1**: 実装 — `Usage` に欄 + **Gemini の `thoughts_token_count` を入れる**（最も安い）+
  `wire.rs` の `XaiUsage` に `output_tokens_details.reasoning_tokens` + xAI decode の
  配線 + `turn:` 行 + カードの累計と村の集計。
  **Anthropic / OpenAI 互換の usage を数えて結果を書く**（D4）
- **P2**: 台帳 — README / DETAIL 日英 + `data_contract` の回収
  （**数えるのはファイル単位** — #51 (b)。台帳は日英 4 ファイル）
- **P3**: 実機

## P1 実装記録（2026-08-10）

lib 455 → 496（+41。うち新規 6）/ 結合 124 / clippy 警告ゼロ。

### D4 の観測結果 — 4 社のうち **3 社で数が取れる**

| 社 | 欄 | 結果 |
|---|---|---|
| **Gemini** | `usageMetadata.thoughtsTokenCount` | **取れる。** `completion` への足し込みは維持し、同じ値を内数として `reasoning` にも入れた |
| **xAI** | `usage.output_tokens_details.reasoning_tokens` | **取れる。** 欄を `wire.rs` へ新設（`XaiOutputTokensDetails`） |
| **OpenAI 互換** | `usage.completion_tokens_details.reasoning_tokens` | **取れる（rev3 の見立てが裏返った）** |
| **Anthropic** | ~~無い~~ `usage.output_tokens_details.thinking_tokens` | ~~構造的に取れない~~ **取り消し（P4）— 取れる。判定を自分の型で下した誤り（`failures.md` #93）** |

**rev3 の「OpenAI 互換は思考が来ない経路なので 0 が正しい可能性が高い」は
半分だけ正しかった。** 来ないのは**本文**で、**数は来る** — 推論モデルは
`completion_tokens_details.reasoning_tokens` を返す。**「受け取れない」と
「数えられない」を同じ語で考えていたのが誤り**で、この 2 つは独立していた。

**ただしこれは仕様に基づく実装で、実機の応答で確かめていない**（テストは合成
JSON）。**空想と実測の境界はここ** — 裏取りは P3 の実機で取る。

**Anthropic だけが本当に取れない。** `AnthropicUsage` は
`input_tokens` / `output_tokens` / `cache_read_input_tokens` /
`cache_creation_input_tokens` の **4 欄しか無く**、思考は `output_tokens` に
畳まれている。**0 は「思考が無かった」ではなく「この経路では数えられない」** —
その旨を decode のコメントに残した（`reasoning: 0` の行が将来
「未実装だから 0」と読まれると、欄を探しに行く無駄が生まれる）。

### 実装で決めた 3 点

- **`normalized_usage`（usage 欠落時のバイト見積もり）では思考ぶんを見積もらない。**
  バイト数から出せるのは受け取った**本文**の量で、思考は本文に現れないので
  推定する材料が無い。保守側へ倒す（大きめに入れる）と、**天井には効かないのに
  計器だけが嘘の桁を運ぶ**（Spec 31 D8 で tick を換算しなかったのと同じ規律）
- **`turn:` 行は `total=` の隣へ 1 語**。`reasoning=` は 0 でも必ず出す（D3）
- **周ごとの加算は既存の 3 つ（`prompt` / `cached` / `tokens`）と同じ 2 箇所**。
  ツールループの周とまとめ呼び出しの周で、片方に足し忘れると
  **まとめ呼び出しの思考だけが消える**

### ミューテーションで赤を確かめた（一発で通ったので）

`Usage::total()` に `reasoning` を足す変異（= 外数の実装）。
**予測「4 本が赤・`budget` は落ちない」を先に書いてから実行し、
1 本も違わなかった**:

- `canonical::reasoning_is_inside_completion_and_never_added_to_total`
- `gemini::function_call_returns_tool_use_despite_stop_finish_reason`
- `openai_compat::reasoning_tokens_are_read_when_the_breakdown_is_present`
- `xai_responses::reasoning_tokens_are_read_as_a_share_of_output`

**`budget` が落ちないことが設計を語っている** — `effective_milli` は
`total()` を経由せず `prompt` / `cache_read` / `completion` を直接読むので、
**合計の定義を壊しても天井は動かない**。逆に言えば、天井を守っているのは
`total()` ではなく `effective_milli` の 3 欄で、そちらは
`reasoning_does_not_change_the_effective_cost` が別に留めている。

## P2 実装記録（2026-08-10。台帳の回収）

**数えたのはファイル単位**（#51 (b)。台帳は日英 4 ファイル）。走査は
**肯定の対照つき**で回した（#90）— 同じクエリで CLAUDE.md 7 件 / DETAIL 日英
各 2 件 / `data_contract` 8 件がヒットすることを見てから残りを読んだ。

- **`data_contract` の `llm_wire.usage`** — P0 で「未観測」と書いた 2 社を
  P1 の結果で置き換えた（OpenAI 互換は**数は来る**／~~Anthropic は欄が無い~~
  **← P4 で取り消し**）。`turn:` 行の形も着地形へ
- **DETAIL 日英** — 思考の節（~~3 社で取れる／Anthropic だけ取れない~~
  **← P4 で 4 社すべてへ訂正**／
  数えた理由 = 出力 1,497 のうち 1,494 が思考で本文は 4 字）とログの節
- **ログの実例は書き換えない。** DETAIL のサンプル行は**改名前に実際に出た行**
  なので、`reasoning=` を後から書き足すと**実物の記録が嘘になる**。
  書式の説明側に「この例より 1 語多い」と注記した
  （CLAUDE.md の「過去の観測記録の `[concordia]` 行は書き換えない」と同じ規律）
- **README 日英は追従不要**。Spec 32 は**画面要素を 1 つも作らない**ので
  「何ができるか」表に行が生えない。**数えて、無いことを確認した**

### 副産物: CLAUDE.md の行数の記述が腐っていた

「README.md — **日英とも 148 行**」が実測 **156 / 155 行**になっていた
（Spec 32 とは無関係の drift。README への直近の編集は `f553453`）。
**正確な行数は編集のたびに腐る**ので、上限（160 行以内）+ 守りたい性質
（新しく来た人が最初に探すものが奥に埋まっていない）へ書き換えた。
**#67 の処方（grep で守れない記述は、grep しなくても壊れない形へ）を数へ適用した形。**

## 検収（書く前に読み口の実在を数える — #68 / #80 の規律）

1. ~~`grok-4.5` のターンの `turn:` 行に `reasoning=` が出て、
   `(total - prompt)` と `reasoning` の差が 3〜5 に収まる~~
   **書き直し（P3 で判定基準の誤りが出た。上の P3 観測結果が正）** —
   3〜5 は probe のプロンプトが作った値であって機構の性質ではない。
   **正しい判定は (a) `reasoning ≤ completion`（内数）(b) 差が同じターンの
   `dropped content blocks:` 行の `text_chars` で説明できること**
2. **`reasoning=0` が出るモデルの同じ行で 0 が出る** — 1 の対照。
   **片方だけでは「常に 0 を出す実装」と区別が付かない**。
   **対照のモデルは実測で選ぶ**。除外が 2 つある（どちらも #68 —
   確かめられない項目は書かない）:
   - **`gemini-3.5-flash-lite` は使わない** — Gemini は `thoughtsTokenCount` を
     返す経路を持つ（golden の実測は `thoughtsTokenCount: 407` 対
     `candidatesTokenCount: 97`）
   - **OpenAI 互換の推論モデル（`gpt-5.6-*`）も使わない** — P1 で
     `completion_tokens_details.reasoning_tokens` を読むようにしたので、
     **0 が出る保証が消えた**（rev3 の候補が実装で無効になった）

   ~~残る候補は Anthropic（欄が無いので構造的に 0）。Anthropic を対照にするのが
   最も強い~~ **取り消し（P4）— Anthropic は非ゼロを返す側になった。**
   **対照は 3 度候補を失っている**（Gemini → OpenAI 互換の推論モデル →
   Anthropic）。いずれも「0 が出る」の根拠が**測る前の思い込み**だった。
   **残るのは実測で 0 だったものだけ** — `gpt-5.6-terra`（#77 で
   ツールを送る周は `reasoning_effort:"none"` を強制するので思考しない）
3. **`reasoning` が非ゼロで出る社が 3 社そろう**（Gemini / xAI / OpenAI 互換）。
   **OpenAI 互換は仕様に基づく実装で実機の応答を見ていない**ので、
   ここが唯一の裏取りになる
4. カードの累計トークンが**変わらない**（D2 の否定的検収。`total()` を触っていない
   ことが画面で読める）
5. ~~Anthropic のターンで `reasoning=0` が出る~~ **書き直し（P4）** —
   **Anthropic のターンで `reasoning` が非ゼロで出る**。
   **P3 で観測した `agent` の `reasoning=0` は、0 を固定していた実装の産物**で、
   対照として無効。**P4 の実機で取り直す**

## P3 観測結果（2026-08-10 19:58〜20:00。4 社 5 ターン）

**走査は肯定の対照つき**（#90）— `turn:` 行 71 本のうち **`reasoning=` を持つのは 5 本**。
古い 66 本に無いことが同じ走査で読める（= 新ビルドのターンだけを見ている証拠）。

| 個体 | プロバイダ | prompt | total | `completion` | `reasoning` | 差 |
|---|---|---|---|---|---|---|
| `agent` | Anthropic `claude-sonnet-5` | 52,909 | 53,166 | 257 | **0** | — |
| `agent_10` | xAI `grok-4.5` | 11,016 | 11,301 | 285 | **216** | 69 |
| `agent_3` | Gemini `gemini-3.6-flash` | 12,585 | 13,082 | 497 | **420** | 77 |
| `agent_8` | OpenAI 互換 `gpt-5.6-terra` | 12,136 | 12,180 | 44 | **0** | — |
| `agent_9` | OpenAI 互換 `muse-spark-1.2` | 14,454 | 15,948 | 1,494 | **1,072** | 422 |

### 検収 1 の判定基準が誤っていた（合格だが、書き方が間違っていた）

**「差が 3〜5 に収まる」は成立しない。** 実測は 69 / 77 / 422 で、全部外れている。

**真因は私が数字を probe の産物から取ったこと。** 3〜5 は
「**結論だけを 1 行で答えよ**」と縛った probe の値で、本文が 4〜8 字だったから
そうなっていた。**差は本文のトークン数**なので、答えが長くなれば差も伸びる。
**答えの長さを固定していない実機で、probe の値を合格条件にした**のが誤り。

**正しい判定は 2 つ** — (a) `reasoning ≤ completion`（内数が成立している）
(b) **差が本文の量で説明できる**。

**(b) は同じ走行で裏が取れた。** xAI の 6 ms 前に出ている行:

```text
19:59:21.721 dropped content blocks: kinds=reasoning:1 count=1 output_tokens=285 text_chars=110 tool_calls=0
19:59:21.727 turn: agent=agent_10 … prompt=11016 cached=640 total=11301 reasoning=216
```

**`output_tokens=285` が `total - prompt = 285` と一致する** — 別々の計器が
同じ数字を独立に出した。差 69 トークンに対し `text_chars=110`（日本語で
約 1.6 字/トークン）で、**差は本文であることが説明できる**。

**一般化（#80 の 3 例目）: 検収に数字を書くとき、その数字が
「測定条件の産物」か「機構の性質」かを分ける。** 3〜5 は probe の
プロンプトが作った値で、機構が保証する値ではなかった。**機構が保証するのは
不等式（内数）であって、等式ではない。**

### 検収 2・5 合格。ただし **2 つの 0 は理由が違う**

- `agent`（Anthropic）= ~~構造的な 0。usage に欄が無い~~
  **取り消し（P4）— これは実装が 0 を固定していたから出た 0** で、
  観測ではなく**自分が書いた定数を読んでいた**。対照としては無効
- `agent_8`（`gpt-5.6-terra`）= **思考していない 0**。gpt-5 系はツールを送る周で
  `reasoning_effort: "none"` を強制している（#77 / `openai_compat.rs`）ので、
  **既存の機構と辻褄が合う**。`completion=44` の小ささもこれで説明できる

**ログからは 2 つを区別できない。** これは計器の穴だが**機構は足さない** —
区別が要る場面がまだ 1 度も来ていない（#47 で L1 / L2 を作らなかったのと同じ判断）。
**区別が要ると分かったときのために、ここに 2 例が並んでいることを記録しておく。**

### 検収 3 合格 — **OpenAI 互換の裏取りが取れた**

`muse-spark-1.2` が **`reasoning=1072`**。P1 は
`completion_tokens_details.reasoning_tokens` を**仕様に基づいて**実装し、
実機の応答では未確認のまま残していた。**その 1 点が実物で埋まった。**

**rev3 の「OpenAI 互換は 0 が正しい可能性が高い」は、実機でも否定された。**
同時に、**同じ OpenAI 互換でも `gpt-5.6-terra` は 0** なので、
**「互換だから来る／来ない」ではなく接続先とモデル次第**であることも読める
（1 サンプルで規則を作らないための、同じ走行に居る対照）。

### 検収 4 合格（利用者が画面で確認。2026-08-10）

カードの累計トークンに変化なし。**ただしこれは弱い観測**で、
前後の数字を並べて比べたわけではない（変化が無いことの確認なので、
比較対象が原理的に取れない）。

**強い保証は型と回帰のほうが持っている** — 累計は `Usage::total()` の
積み上げで、`total()` は `prompt + completion` のまま。
`reasoning_is_inside_completion_and_never_added_to_total` が留めており、
**外数の実装に変えると 4 本が赤になることをミューテーションで確かめてある**。
画面の確認はその裏取りであって、根拠そのものではない。

## P4 実装記録（2026-08-10。**P1 の凍結を訂正した**）

Spec 33 の 1 手目として打った Anthropic の probe（D4 / 引き継ぎ節）が、
**P1 で凍結した契約を 1 つ falsify した**。

### 訂正 1: `budget_tokens` は過去形だった

```text
400 "thinking.type.enabled" is not supported for this model.
    Use "thinking.type.adaptive" and "output_config.effort"
```

**サーバー自身が後継を名指しする** — Spec 31 の `search_parameters` → 410 Gone と
**同じ形**。`output_config.effort` は `canonical.rs` の doc に名前だけあり実装は無い。

### 訂正 2: **「Anthropic は欄が無く構造的に取れない」は誤り**（`failures.md` #93）

`usage.output_tokens_details.thinking_tokens` が**実在した**（5/5 の応答に出た）。
P1 の判定材料は `AnthropicUsage`（当時 4 欄）で、**実際のワイヤはそれより多くの
欄を返していた** — 「無い」の根拠は**自分がまだ書いていないこと**だった。

**同じコミットの中で 3 社は正しくやっている**（ワイヤの欄名を先に確かめてから
型を足した）。**Anthropic だけ手順が逆**で、既にある型を読んで「無い」と結論した。

### 決定的な観測: **claude-sonnet-5 は既定で思考している**

probe D（**`thinking` も `output_config` も送らない = いまの村と同じ形**）:

```text
content ブロック: ["thinking"]      ← text が 1 つも無い
usage: output_tokens=2048  output_tokens_details={"thinking_tokens":2048}
```

**出力 2,048 トークン全部を思考に使い、本文をゼロで終えた**（`max_tokens` 到達）。
**`failures.md` #72 の正体がこれ** — 当時「thinking だけに使ったと読める」と
書いた推測が実物で確認された。**Spec 32 の目的から見て、最も数えるべき社で
0 を出していた。**

### 往復の要求は観測されなかった（Spec 33 の D2 に効く）

C（thinking を落として `tool_result`）が **HTTP 200**、C'（戻す・対照）も 200。
**「戻さないと拒否される」は再現しなかった** — 予測と逆。
**ただし各 1 サンプル**なので、Spec 33 では条件を変えて測り直す。

### Spec 33 に効く制約: **Anthropic は読める思考文を返していない**

`thinking` ブロックは **`signature` が 368〜8,380 字ある一方、`thinking` テキストは
0 字**だった（4 回とも）。**表示するものが無い**可能性が高い。

### ミューテーション

`reasoning` を 0 固定へ戻す変異で、**予測「1 本だけ赤」が的中**
（欄なしのテストは 0 固定でも通るので、対で置いた 2 本のうち 1 本だけが落ちる）。

lib 496 → 498 / clippy 警告ゼロ。

### P4 の実機（2026-08-10。**検収 5' 合格**）

**同じ個体・同じプロバイダ・ほぼ同じプロンプト量で、P4 の前後が並んだ。**

```text
[P4 前 = P1 のビルド]
20:05:38.475 dropped content blocks: kinds=thinking count=1 output_tokens=1743 text_chars=802
20:05:38.479 turn: agent=agent … prompt=23191 cached=20371 total=24934 reasoning=0

[P4 後]
21:15:46.416 dropped content blocks: kinds=thinking count=1 output_tokens=1832 text_chars=1414
21:15:46.421 turn: agent=agent … prompt=23455 cached=0     total=25287 reasoning=515
```

**両方のターンが `kinds=thinking` を落としている** = どちらも思考している。
にもかかわらず前は `reasoning=0` を出していた — **同じログの 2 行が、
その 0 が観測ではなく定数だったことを証明している**。

内数も両方で成立: `total - prompt` = 1,743 / 1,832 が、それぞれ同じターンの
`dropped` 行の `output_tokens` と**一致**（別々の計器が独立に同じ数字を出す）。
P4 後は `reasoning=515 ≤ completion=1832`。

**#72 は例外的な事故ではなく、常時起きていた。** 2 ターンとも思考しており、
Anthropic の思考ぶんは**この村がずっと払い続けていた**。いま数字で読める。

**検収 2 の対照は `gpt-5.6-terra`（`agent_8` の `reasoning=0`）が担う。**
実測で 0 だった唯一の経路で、理由も既存の機構（#77）で説明できる。

## 引き継ぎ（決定。Spec 33 / 34 の骨格）

**Spec 33 = 要約本文の受け取りと表示（xAI → Anthropic）。** 査読で確定した形を
そのまま渡す:

- **履歴に積まない判断は `xAI において` に限定する**（査読 1）。Anthropic は
  署名つきブロックを次ターンへ戻す要求があるかもしれず、**真なら
  「積まない」は成立しないだけでなく #45 の規律（履歴は送った文字列そのもの）まで
  壊れる**。Spec 33 の 1 手目は probe 1 発で、見るのは (a) `signature` の有無
  (b) 戻さないと拒否されるか (c) ツール併用時の変化。**(b) を測らずに実装しない**
- **P2 / P3 の責務分界**（査読 6）— P2 = モデル → `ChatResponse.reasoning_summary`
  → `AgentMessage` の経路と保存（**表示なし**）、P3 = `AgentMessage` → 画面の描画。
  語は「思考の要約」固定、短形も正常系として描画
- **「積んでいない」の検収は構造で取る**（査読 8b を訂正して採用）。提案の
  「リクエスト JSON の `input` 文字列長で測る」は runtime の計測で、
  **長形が出る回を待つ必要がある**。Spec 23 P3 は同じ問いを**型で**閉じた
  （「展開が読むのは受信の参照だけ。履歴は `String` なので画像を持てない」）。
  要約も `AgentMessage` にだけ席を作り `ChatMessage` に作らなければ、
  **単体テスト 1 本で「積めない」ことが証明できる**
- **画面で接地（出典）と思考（要約）を同じ枠に並べるか**は Spec 33 の決めどころ。
  出典は検証できる外部の指し先、要約は検証できない内部の申告で、
  **同じ枠に並べると後者に前者の信用が乗る**

**Spec 34 = OpenAI の Responses ワイヤ**（査読 7）。実質は
`Provider::OpenAiResponses` の新設で **Spec 31 P1 と同規模以上**。
**「Thinking の受け取り」の名前で見積もると必ず外れる**ので独立させる。
CLAUDE.md が記録している「思考の復活と web 検索が同じ 1 本で解ける」の回収先が
ここで、**2 社 4 機能ではなく 1 社 2 機能を先に取る**形になる。

**利用者裁定の順序「xAI → Anthropic → OpenAI 互換」は保たれる** — Spec 33 の
Phase 順と Spec 34 の位置がその順序そのもの。

## Notes

1. **v0.2.0 の残りは本 Spec + Spec 33 / 34 + 多モーダル。** 公表が最後のマイルストーン
2. **本 Spec は新しい画面要素を 1 つも作らない**（数字が 1 つ増えるだけ）。
   CLAUDE.md の「書かない境界」に触れる規模だが Spec を切ったのは、
   **内数か外数か・`total()` を触るか・0 を出すか**が決めどころで、
   どれも間違えると天井の実効値が動くため
3. **probe は使い捨てで削除済み**（`crates/fuseforks-core/examples/probe_reasoning.rs`。
   Spec 31 と同じ流儀）。実 API 呼び出し 8 発、ticks 合計約 9.4 億 —
   **tick の単位は応答に書かれていないので金額へ換算しない**（Spec 31 D8 と同じ規律）
4. **コミットは既存の `Spec NN PX — 説明` 形式**（査読 8c は不採用）。
   直近 12 コミットに conventional commits は 0 件で、`data_contract.yaml` は
   機械にパースされていないため P0 は main を壊せない
