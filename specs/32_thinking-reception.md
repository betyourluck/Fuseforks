# Spec: 思考トークンを数える — `reasoning` を `completion` から分離する

**ID**: 32
**Date**: 2026-08-10
**Status**: **rev3**（P0 の作業中に**社を数え落としていた**のが出て改訂。
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
| **Anthropic** | decode が `Thinking` / `RedactedThinking` を `dropped` へ入れる。**ユニット変種**で本文を deserialize すらしない。**`budget_tokens` は実装に 1 つも無い**（`anthropic.rs` の 3 件はテスト 2 + decode 1）= **要求していないので今は 1 件も返っていない** | `llm/anthropic.rs` / `llm/wire.rs` |
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
（`orchestrator.rs`）。**`output` という欄はそもそも無い**ので、そこへ足す形ではなく
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

## 検収（書く前に読み口の実在を数える — #68 / #80 の規律）

1. `grok-4.5` のターンの `turn:` 行に `reasoning=` が出て、
   **`(total - prompt)` と `reasoning` の差が 3〜5 に収まる**
   （`total - prompt` が `completion`。差は本文のトークン数で、実測の範囲。
   外れたら読み口が違う）
2. **`reasoning=0` が出るモデルの同じ行で 0 が出る** — 1 の対照。
   **片方だけでは「常に 0 を出す実装」と区別が付かない**。
   **対照のモデルは実測で選ぶ。`gemini-3.5-flash-lite` は使わない** —
   Gemini は `thoughtsTokenCount` を返す経路を持っており（golden の実測値は
   `thoughtsTokenCount: 407` 対 `candidatesTokenCount: 97`）、**0 が出る保証が無い**。
   候補は OpenAI 互換（構造的に来ない）。**確かめられない項目は書かない**（#68）
3. カードの累計トークンが**変わらない**（D2 の否定的検収。`total()` を触っていない
   ことが画面で読める）
4. Anthropic のターンで `reasoning=0` が出る — **D4 の観測そのもの**。
   0 であることが「要求していないから返らない」の裏取りになる

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
3. **probe は使い捨てで削除済み**（`crates/agent-core/examples/probe_reasoning.rs`。
   Spec 31 と同じ流儀）。実 API 呼び出し 8 発、ticks 合計約 9.4 億 —
   **tick の単位は応答に書かれていないので金額へ換算しない**（Spec 31 D8 と同じ規律）
4. **コミットは既存の `Spec NN PX — 説明` 形式**（査読 8c は不採用）。
   直近 12 コミットに conventional commits は 0 件で、`data_contract.yaml` は
   機械にパースされていないため P0 は main を壊せない
