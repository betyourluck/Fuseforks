# Spec: Meta の Responses ワイヤ — web 検索と多モーダル

**ID**: 37
**Date**: 2026-08-13
**Status**: Draft（rev1・**P0 の probe 12 発は実施済み**。査読待ち）
**Branch**: なし（main へ Phase 単位で直接コミット）

## Goal

`api.meta.ai` のモデル（`muse-spark-1.2-contributor`）に **web 検索の固有スキル**を
足す。そのために **6 本目のワイヤ `Provider::MetaResponses`**（`/v1/responses`）を通す。

起点は利用者（2026-08-13）—「**データを学習に使われるという条件で破格の安さ**。
ここ数日使って数十円にしかなっていない。もともと OpenSource なので外部の学習に
使われることは構わないし、**秘匿情報を渡さないように注意すればかなり使える**。
さらに Gemini のように web_search が API では使える。**これは Meta の Facebook や
Instagram などのお宝の宝庫だ**」。

**Goal は「安い接続先に検索を足す」ではなく、「この村が持つ接地の選択肢を
1 つ増やす」。** Spec 31（Grok の Live Search）が「取ってきた個体に真偽を
判定させない」という条例の規律をベンダー層へ持ち込んだのと同じ棚に乗る —
**取ってくる社が増えるほど、条例の Verify 段が効く。**

## 現況（実測 2026-08-13）

**今日 `muse-spark-1.2` は `provider` 未設定で、`detect("api.meta.ai")` により
OpenAI 互換（`/v1/chat/completions`）へ倒れている。** 互換の口に web 検索は無い。

| | 状態 |
|---|---|
| ストリーミング | **この村には 1 行も無い**（`llm` モジュール全体で `stream` の語がゼロ） |
| Responses の入力組み立て | `responses_input.rs` を **xAI / OpenAI の 2 本が共有** |
| 固有スキルの器 | Spec 31 で恒久化済み（`providerSkills.ts` のアクティブ／パッシブ 2 形） |
| `Provider` 追加の射程 | Rust **7 箇所** + TS **23 箇所**（`carries.ts` の `Record<Provider,…>` は網羅を要求するのでコンパイラが指すが、`providerSkills.ts` の string union は**黙って吸う**） |

### P0 実測結果（2026-08-13。probe 12 発。**判定は内容照合**）

**最重要の 1 発目で分岐が決着した — `stream` は不要。**
提示された curl には `"stream": true` と `Accept: text/event-stream` が入っていたが、
**どちらも省いて 200 が返る**。**SSE をクライアント核へ入れる話にはならず、
6 本目のワイヤの規模で収まる**（ここが真なら、再試行・タイムアウト・打ち切り・
トークン計上のすべてがその上に乗っているので桁が変わっていた）。

| 問い | 観測 |
|---|---|
| 非ストリーミングで通るか | **通る**。`kinds=['reasoning','message']` = **xAI / OpenAI Responses と同じ形** |
| doc の欄（`temperature` / `top_p` / `reasoning.effort`） | 通る |
| `tools: [{"type":"web_search"}]` | **通る**（`web_search_call` が 11 件） |
| **citations** | **返る**（`annotations` の `url_citation`） |
| 関数ツールとの併用 | **通る**（`web_search` と関数を同時に送って `function_call` が返った） |
| 受理集合の列挙 | **しない**（下記） |

**citations は xAI より強い** — `start_index` / `end_index` が **0 ではなく実区間**
（実測 106〜253）。**主張単位の範囲を持っている**。Spec 31 の xAI は
**77/77 がすべて 0**（メッセージ単位）だったので、**出典の選別がこちら側の仕事**
だった度合いが下がる。

**受理集合の列挙は第 3 の様式だった。** 誤ったツール型を 1 つ送っても
`tools[0] did not match any supported type` だけで、**候補を列挙しない**
（OpenAI / Anthropic は全列挙、xAI は untagged enum の 422）。
**Spec 34 の「400 が受理集合を教えてくれる」手筋はこのワイヤでは使えない。**

**ただし content part は 1 つずつ名指しで教える** — `{"type":"input_video"}` を
payload 無しで送ると `input_video content part requires video_url or file_id`、
`input_audio` なら `requires input_audio, audio_url, or file_id`。
**この「名指しの 400」が受け口の実在を確かめる唯一の手**になった。

### carries の 6 行目（**全 4 種別が内容照合まで合格**）

| 種別 | 送り方 | 観測 |
|---|---|---|
| 画像 | `input_image` の data URL | **◎**「左が赤、右が青」 |
| 音声 | `input_audio` の `{data, format}` | **◎**「ガーネット」 |
| **動画** | **`input_video` の `video_url` に data URL** | **◎**「赤、青」 |
| PDF | `input_file` の `{filename, file_data}` | **◎**「GARNET-77」 |

**予測を覆した** — OpenAI Responses からの類推で動画は ✗ と書くところだった。
`enum-part` の名指し 400 が `input_video` の実在を教えたので撃てた。
**Meta は Gemini に次ぐ 2 本目の「4 種別すべてを運ぶワイヤ」。**

### 実測で出たコストと罠

- **検索は入力を桁で膨らませる** — web probe の `input_tokens` は **66,350**
  （検索結果がプロンプトへ注入される）。検索なしの同型は 12〜141。
  Spec 31 の xAI（98,213 / うち cached 62,720）と同じ形
- **出力上限が低いと本文がゼロになる**（#72 と同型）。`max_output_tokens: 512` で
  PDF と音声が **`kinds=[]` / 本文空 / `output_tokens` がちょうど 512** になり、
  6,000 へ上げたら本文が返った。**思考で使い切っている。**
  この村の `max_tokens` の既定が低い個体では、**添付を送ると本文が返らない**

## 決めること

- **D1 `Provider::MetaResponses` を足す**（6 値目）。`detect` は変えない —
  既存のテンプレートは互換として動いており、**自動判定を変えると設定を触っていない
  利用者の個体が黙って別のワイヤへ移る**（Spec 34 D7 と同じ判断）。**明示選択のみ。**
- **D2（最も重い）`responses_input.rs` を共有し続けるか。**
  Spec 34 D2 は「**共有するのは要素の型が同じだと実測しているから**」と書いた。
  **その前提が今回崩れる** — Meta は `input_audio` / `input_video` を持ち、
  xAI / OpenAI は持たない。選択肢は 2 つ:

  | 案 | 中身 | 代償 |
  |---|---|---|
  | **A: 共有のまま、encoder が provider を受ける** | `encode(messages, provider)` が `carries` を見て組み立てる | **`adapters_match_the_carries_table` が同語反復になる**（adapter が述語から導出されるので、食い違いようがない）= 網が 1 枚死ぬ |
  | **B: Meta を別の encoder へ** | `meta_responses.rs` が自分の match を持つ | **`function_call` / `function_call_output` の組み立てが 2 箇所**（#88 / #96 の「片方だけ直す」形） |

  **推奨は B に近い折衷** — 共有の encoder は**テキストと関数往復だけ**を持ち、
  **添付の part は各ワイヤの adapter が足す**。分ける線を「相手が決める側（関数の
  往復形）」と「自分が決める側（どの添付を載せるか）」に引き直す
  （Spec 34 D2 の分割規則をそのまま content 層へ適用する）。**要査読。**
- **D3 固有スキルは「web 検索」のトグル 1 つ**（Spec 31 の器）。
  `ModelTemplate.meta_web_search` + **AND 述語 `meta_web_search_active()`**
  （`provider == MetaResponses` と併せて判定 — フラグ単独を判定に使わない規律の 4 例目）。
- **D4 `Grounding.engine` に Meta を足す**（Spec 31 の閉じた列挙）。
  **`start_index` / `end_index` を捨てるか使うかは別の判断** — 器（`GroundingNote.vue`）は
  出典の一覧しか持たない。**実区間を持つのはこのワイヤだけ**なので、
  使うなら他社が持たない欄が 1 つ増える。**rev1 では捨てる**（使う先が無い）。
- **D5 受理集合が列挙されないことを契約へ書く。** このワイヤでは
  「誤った値を 1 つ送ると 400 が全部教えてくれる」（Spec 34）が**効かない**。
  未知の欄は **1 つずつ名指しさせる**か、素直に試すしかない。
- **D6 出力上限の罠を画面へ出すか。** 思考で使い切って本文ゼロは #72 の再演で、
  **計器（`dropped content blocks`）は既にある**。`max_tokens` が小さい個体で
  添付を送ると起きるが、**頻度が未知**なので機構は足さない（#47 の規律）。
  **契約と台帳に書いて観測する。**
- **D7（未決・利用者判断）学習利用を画面に出すか。**
  Spec 31 の固有スキルの基準は「**このアプリがその機構を実際に使っているか**」で、
  学習利用は**接続先の規約**であってアプリの働きではない。基準に忠実なら**載せない**。
  一方、利用者の起点は「秘匿情報を渡さないように注意すれば」— **注意の助けになる**。
  **私の推奨は「固有スキルには載せず、README の接続先の表に 1 行」**（分類ではなく
  説明の側。Spec 13 の用語境界と同じ線引き）。**裁定を仰ぐ。**

## Stories

- S1 利用者が Meta のテンプレートで **ワイヤを `MetaResponses` に選び、web 検索を
  ON** にすると、そのサーヴァントが最新の情報を出典つきで答える
- S2 出典は**検索した事実**と**出典**と**見つからなかった事実**を分けて出る
  （Spec 05 の縮小を食わない — citations が実際に返ることは P0 で確認済み）
- S3 web 検索と**同梱ツールが併用できる**（`run` や `file` を使いながら検索できる）
- S4 Meta のワイヤでも**添付が 4 種別すべて通る**（carries 表の 6 行目）
- S5 検索を使わない村・使わないターンでは、送るワイヤも払うトークンも今日と同じ

## Tasks

- [x] **P0 実測 — 完了（2026-08-13。probe 12 発）**。上記「P0 実測結果」が正。
      **残: `data_contract` の凍結**（carries 表の 6 行目 / 受理集合の非列挙 /
      出力上限の罠）は rev 承認後に P0b として書く
- [ ] **P1 ワイヤ**: `Provider::MetaResponses` + `meta_responses.rs`（encode / decode）+
      `carries` の 6 行目 + `as_str` / `path` ほかコンパイラが指す 5 箇所。
      **D2 の結論に従って input 組み立ての分割線を引く**
- [ ] **P2 固有スキル**: `ModelTemplate.meta_web_search` + `meta_web_search_active()` +
      `LlmConfig` への配線 + `Grounding.engine` の Meta
- [ ] **P3 フロント**: `providerSkills.ts`（**string union は網羅検査を持たないので
      手で数える**）+ `carries.ts` の 6 行目（**こちらは `Record` が指す**）+
      モデル登録画面のトグル + 辞書 ja / en
- [ ] **P4 台帳**: README 日英 / DETAIL 日英 / CLAUDE.md / `data_contract`。
      **ファイル単位で数える**（#51 (b)）
- [ ] **P5 実機確認**:
  1. Meta の個体が web 検索して**出典つき**で答える（`GroundingNote` に URL が出る）
  2. 検索と同梱ツールを**同じターンで**使う
  3. 4 種別の添付が通る（carries の 6 行目の実機側）
  4. 検索 OFF の個体で `input_tokens` が跳ねない（S5 の裏）
  5. **`max_tokens` が小さい個体で本文が空になる**のを 1 度踏む（D6 の観測）

## Notes

1. **文書と実装は 4 回続けて食い違った** — Spec 23 の WebP（文書は jpg/png のみ）/
   Spec 31 の legacy 410 / Spec 36 の xAI PDF（記述なしで通る）/ **今回の
   `stream` 必須（curl にあるが不要）と `input_video`（記述を見ていないのに在る）**。
   **文書は「撃つ場所」を教えるが「何が通るか」は教えない。**

2. **この村で「4 種別すべてを運ぶワイヤ」が 2 本になった**（Gemini / Meta）。
   Spec 36 の carries 表は「動画は Gemini だけ」を前提に書いた節が**台帳の側に
   ある**ので、P4 で数え直す（DETAIL 日英の「**Only the Gemini native path carries
   audio and video**」など）。**#51 (b) — 腐るのは Spec 37 の節ではなく Spec 36 の節。**

3. **安さは Spec の判断材料に入れない。** 利用者の起点は価格だが、
   **価格は接続先の都合でいつでも変わる**。この Spec が残すべきものは
   「6 本目のワイヤ」と「carries の 6 行目」と「citations の実区間」で、
   どれも価格が変わっても価値が残る。

4. **`start_index` / `end_index` が実区間なのは、将来の 2 段検証で効きうる。**
   条例の Verify 段（束ねに参加していない個体へ検証を頼む）へ渡すとき、
   **「どの文がどの出典に基づくか」が渡せる**のは今のところこのワイヤだけ。
   rev1 では捨てるが、**捨てたことを書いておく**（次に読む人が「対応漏れ」と
   読んで足しに来ないように — Spec 34 の `intentionally_unread` と同じ形）。

## 改訂履歴

- **rev1**（2026-08-13）: 起票。**P0 の probe 12 発を先に撃ってから書いた** —
  この村では文書が 4 回続けて実装と食い違っており、設計を文書から起こすと
  最初の作業がその作り直しになる。
