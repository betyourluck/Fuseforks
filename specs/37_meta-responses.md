# Spec: Meta の Responses ワイヤ — web 検索と多モーダル

**ID**: 37
**Date**: 2026-08-13
**Status**: Draft（rev2・**probe 計 24 発**。査読 8 点を反映 = 採用 6 / 訂正して採用 2 / 反証 0）
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
| 受理集合の列挙 | **層で違う** — 型は列挙しない / スカラ enum は全列挙（下記） |

**citations は xAI より強い** — `start_index` / `end_index` が **0 ではなく実区間**
（実測 106〜253）。**主張単位の範囲を持っている**。Spec 31 の xAI は
**77/77 がすべて 0**（メッセージ単位）だったので、**出典の選別がこちら側の仕事**
だった度合いが下がる。

**受理集合の列挙は第 3 の様式だった。** 誤ったツール型を 1 つ送っても
`tools[0] did not match any supported type` だけで、**候補を列挙しない**
（OpenAI / Anthropic は全列挙、xAI は untagged enum の 422）。
~~**Spec 34 の「400 が受理集合を教えてくれる」手筋はこのワイヤでは使えない。**~~

**訂正（2026-08-13。P5 の実機で反証）— 手筋が効かないのは「型」の層だけ。**
`reasoning.effort` に `max` を送ったら

```text
`reasoning.effort`: unknown variant `max`,
 expected one of `none`, `minimal`, `low`, `medium`, `high`, `xhigh`
```

が返り、**Spec 34 の手筋がそのまま効いた**。つまり様式は「Meta は列挙しない」ではなく
**「型は列挙しない / スカラの enum 欄は全列挙する」**。

**誤りの機序は射程の取り違え。** 観測したのは `tools[0]` と content part、つまり
**どちらも「型」の層**で、そこから**欄全般**へ広げた。**部分集合で観測した性質を
集合全体の性質として書いた形**（#91 と同族 — あちらは述語、こちらは散文）。
**残る正しい射程は「測っていない *型* は送らない」。** 欄の値は 1 つ撃てば教わる。

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

### P0b 実測（2026-08-13。査読が求めた 4 点 + 出力上限の下限。probe 12 発）

**査読 1 — `function_call` の往復は OpenAI / xAI と同一だった**（懸念は実測で解消）:

```json
{"type":"function_call","id":"fc_…","call_id":"call_…","name":"get_weather",
 "arguments":"{\"city\":\"東京\"}","status":"completed"}
```

`call_id` の命名も `arguments` が JSON **文字列**であることも同じ。返す側の
`function_call_output.output` は **string のみ**（object を渡すと
`input[2].output did not match any supported type`）。**共有 encoder の
関数往復部分はそのまま使える。**

**新しい制約を 1 つ見つけた（rev1 に無い）— `tool_choice` は `"auto"` のみ。**
`none` / `required` / 名指しは 400 で
`only "auto" is supported for tool_choice` と断られる。**Kataribe の
wire 分割で観測済みの制約がここでも出た**（CLAUDE.md の Kataribe の節、
送り分け 3 軸の ③）。**この村は `ToolChoice::None` を使う**（ツール上限後の
まとめ呼び出し）ので、**Responses の既存の作法（`None` なら tools ごと送らない）で
回避できる** — `tool_choice` 欄そのものを送らない。

**査読 4 — `usage` は分解できる**（懸念は解消）:

```json
{"input_tokens":545,"input_tokens_details":{"cached_tokens":113},
 "output_tokens":212,"output_tokens_details":{"reasoning_tokens":142},
 "total_tokens":757}
```

**`cached_tokens` と `reasoning_tokens` の両方がある** ので、既存の計上
（Spec 32 の思考トークン / キャッシュ率）がそのまま効く。

**査読 6 — web_search に options は実在した。**
`search_context_size: "high"` は **200 で受理**。`filters.allowed_domains` /
`max_results` / 未知欄は **400 で名指し**（`tools[].X is not supported for
Responses web_search`）。**D3 の「トグル 1 つ」は据え置くが、軸が在ることは記録する**
（将来 `search_context_size` を出すなら固有スキルのもう 1 つの器になる）。

**査読 3 — 出力上限の下限を測った**（PDF 添付つき）:

| `max_output_tokens` | `output_tokens` | 本文 |
|---|---|---|
| 512 | 512（使い切り） | **空** |
| 1,024 | 1,024（使い切り） | **空** |
| **2,048** | 1,636 | **あり** |
| 4,096 | 1,146 | あり |

**下限は 1,024 と 2,048 の間。** 既定の `reasoning.effort` が **`high`**
（応答に出る）なのが原因で、思考で使い切っている。

**査読 2 — video の実運用経路**:

- **`https` URL は通る**（server-side fetch。MDN の花の動画を正しく説明した）。
  ただし**公開到達が要る**ので、この村のローカルファイルには使えない
- **`file_id` は数値 ID**（`input_file file_id must be a numeric ID`）で、
  **`/v1/files` が実在する**（`GET` が 200 でファイル一覧を返す。`/v1/uploads` は 404）
- **data URL で送ったものは Meta 側にファイルとして保存される**（下記）

### **`/v1/files` に恒久保存される（rev2 の最重要発見）**

`GET /v1/files` が **10 件**を返し、**すべて `expires_at: null`（期限なし）/
`purpose: "user_data"`**。**バイト数がローカルの probe 素材と 1 バイト単位で一致**する
（PDF 671 ×5 / mp4 4,533 / webp 406）ので、**data URL で送った添付が
サーバ側にファイルとして残っている**と読める。

**利用者の実データが 1 件含まれていた** — 204,074 B・02:28:01 は、
`{workspace}/attachments/` の webp と**同じバイト数・同じ時刻**。
**これは `/v1/chat/completions`（互換の口）で送ったもの**なので、
**Responses ワイヤに限らず、この接続先へ送った添付は残る。**

**`https` URL で渡した動画も取得されて保存された**（1,128,375 B）。
**音声（`input_audio.data`）だけは一覧に出ていない** — 経路によって扱いが違う。

**これは「学習に使われる」より一段具体的な事実**（一覧でき、期限が無い）なので、
**D7 の判断材料が変わる**（下記）。

**削除口は在り、実際に消える**（2026-08-13 に実測。**利用者の裁定を得てから撃った**）:

```
DELETE /v1/files/{id}   → 200 {"deleted": true}
GET    /v1/files/{id}   → 404 file_not_found   ← **確認はこちら**
GET    /v1/files        → 一覧。**最大 20 秒ほど遅れる**
```

**一覧は結果整合で、真実は個別 GET 側。** 削除直後の一覧は件数が変わらず、
20 秒後に減った。**1 回目の観測だけで「削除は効かない」と結論しかけた** —
**遅延を疑って取り直したから分かった**（#90 の「計器の検定を先に取る」の逆側で、
**遅れて正しくなる計器を、壊れていると読まない**）。

**この Spec の probe で作ったファイルと、利用者の実データ 1 件は削除済み**
（9 件すべて `DELETE` 200 → 個別 GET 404 → 一覧 0 件）。

**音声だけは一覧に現れなかった** — `input_audio.data` で送った 401,020 B の wav は
最後まで `/v1/files` に出ていない。**経路によって扱いが違う**（画像・PDF・動画の
data URL と、Meta が `https` URL から取得したものは残る）。**機序は未確定。**

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
- **D2（最も重い）— 折衷で確定**（査読 1 を採用。**実測が前提を埋めた**）。
  Spec 34 D2 は「**共有するのは要素の型が同じだと実測しているから**」と書いた。
  **rev1 はその前提が崩れると書いたが、崩れたのは添付だけだった** — P0b で
  `function_call` / `function_call_output` の形が 3 社で**同一**と確認できた
  （`call_id` の命名も `arguments` が JSON 文字列であることも一致）。

  **確定形**: 共有の `responses_input.rs` は
  **テキストと関数往復（`function_call` / `function_call_output` の文字列化規則）
  だけ**を持ち、**添付の part は各ワイヤの adapter が足す**。
  **`encode(messages, provider)` は作らない** — 査読の指摘どおり、encoder が
  `carries` を見ると `adapters_match_the_carries_table` が**同語反復**になり、
  網が 1 枚死ぬ（adapter が述語から導出されるので食い違いようがない）。

  分ける線は Spec 34 D2 の規則をそのまま content 層へ当てたもの —
  **「相手が決める側」（関数の往復形）は共有、「自分が決める側」（どの添付を
  載せるか）は分ける。**

  **`output` が string 固定であることは型で既に成立している**（査読の宿題を
  確かめた結果 — `wire::ResponsesInputItem::FunctionCallOutput.output: String`）。
  **object を作りようがない**ので、Meta の 400 を踏む経路は構造的に無い。
  **新しく強制する必要は無く、型を緩めない限り守られる**（緩めたら 3 社で割れる、
  と doc へ書く）。
- **D8（新設・rev2）`tool_choice` は `"auto"` 以外を送らない。**
  Meta は `none` / `required` / 名指しを 400 で断る（P0b 実測）。
  **既存の Responses の作法で回避できる** — `use_tools` が偽か
  `ToolChoice::None` のときは**関数ツールも検索ツールも送らない**規律が既にあり、
  そのとき `tool_choice` 欄そのものを出さなければよい。
  **`meta_responses.rs` でも同じ分岐を踏む**（査読 2。xAI / OpenAI の adapter と
  同じ形で書き、**ここだけ独自の判定を持たない**）。
  **`Required` / `Specific` はこのワイヤでは表現できない**ので、
  **契約に「このワイヤは強制ツール呼び出しを持たない」と書く**
  （`NoStructuredOutput` の経路がこのワイヤで使えないことの明示）。
- **D9（新設・rev2）動画は carries ✓ だが、実用経路は data URL に限られる。**
  `file_id` は**数値 ID** で `/v1/files` へのアップロードが要り、
  `https` URL は**公開到達が要る**（この村のローカルファイルには使えない）。
  **rev2 では data URL のみを実装する** — アップロード API を足すと
  「送る前に別の往復が要る」形になり、`AttachmentStore` の外に**第 2 の実体**が
  生まれる（Files API 不採用の D10 = Spec 36 と同じ判断がそのまま当たる）。
  **代償**: Spec 36 D4 の動画上限 12MB は base64 で 16MB になり、
  **JSON の要求サイズとしては大きい**。**P1 の実測で通る上限を測り直す**
  （通らなければ Meta の動画だけ carries を ✗ へ倒す。**測ってから決める**）。
- **D10（新設・rev2）出力上限の下限を画面で警告する**（査読 3 への**訂正した回答**）。
  査読は「`max_output_tokens` を最低 4000 へ引き上げるか `reasoning.effort: low` を
  強制するか」を求めたが、**どちらも利用者が決めた値を実装が黙って書き換える**形で、
  この村が繰り返し退けてきた「親切心」に当たる（Spec 20 の
  「親切心で 2 段目を戻さない」/ Spec 36 D12 の「警告はするが送信は止めない」）。

  **代わりに Spec 36 D12 と同じ形を採る** — **`max_tokens` が下限を割っている
  個体へ添付や検索を送るとき、画面で警告する（送信は止めない）**。
  **下限は実測できた**（1,024 では空・2,048 で本文あり）ので、
  **閾値を推測ではなく観測で書ける**。既定の `reasoning.effort` が `high` なのが
  原因なので、警告文には「思考で使い切ります」と機序を書く。
  **計器は既にある**（`dropped content blocks`）。

  **警告文は観測値であることを明記する**（査読 4）— **下限は `effort` で動く**ので、
  「2,048 未満なら必ず空」ではない。書くのは
  「**観測では `effort=high` のとき 1,024 で空・2,048 で本文が返った**」。
  **閾値を法則として書くと、`effort` を下げた個体で誤った警告になる。**
- **D3 固有スキルは「web 検索」のトグル 1 つ**（Spec 31 の器）。
  `ModelTemplate.meta_web_search` + **AND 述語 `meta_web_search_active()`**
  （`provider == MetaResponses` と併せて判定 — フラグ単独を判定に使わない規律の 4 例目）。
- **D4 `Grounding.engine` に Meta を足す**（Spec 31 の閉じた列挙）。
  **`start_index` / `end_index` を捨てるか使うかは別の判断** — 器（`GroundingNote.vue`）は
  出典の一覧しか持たない。**実区間を持つのはこのワイヤだけ**なので、
  使うなら他社が持たない欄が 1 つ増える。**rev1 では捨てる**（使う先が無い）。
- **D5 受理集合の列挙が層で違うことを契約へ書く**（2026-08-13 に改題）。
  ~~このワイヤでは「誤った値を 1 つ送ると 400 が全部教えてくれる」（Spec 34）が
  **効かない**。~~ **効かないのは型の層だけ** — 未知の**型**は 1 つずつ名指しさせるか
  素直に試すしかないが、**スカラの enum 欄は 1 つ撃てば全列挙が返る**。
  **`reasoning.effort` の `max` がその実例**（P5 で実機が返した）。
- **D6 出力上限の罠を画面へ出すか。** 思考で使い切って本文ゼロは #72 の再演で、
  **計器（`dropped content blocks`）は既にある**。`max_tokens` が小さい個体で
  添付を送ると起きるが、**頻度が未知**なので機構は足さない（#47 の規律）。
  **契約と台帳に書いて観測する。**
- **D7（未決・利用者判断。rev2 で判断材料が変わった）学習利用と
  「添付が恒久保存される」ことを画面に出すか。**

  **rev1 の推奨（固有スキルには載せず README に 1 行）は、学習利用だけを
  見ていた。** P0b で**より具体的な事実**が出た — **送った添付は
  `/v1/files` にファイルとして残り、`expires_at` は `null`**（期限なし）。
  **利用者の実データが実際に 1 件保存されていた**（`/v1/chat/completions` 経由の
  画像なので、**このワイヤに限らずこの接続先すべて**）。

  **性質が違う**: 学習利用は*規約*、恒久保存は**観測できる事実**。
  Spec 31 の基準（このアプリがその機構を実際に使っているか）は前者を弾くが、
  **後者はこのアプリが送ったものの行き先**なので、同じ理由で弾けない。

  **選択肢**:

  | 案 | 置き場 | 性質 |
  |---|---|---|
  | a | README の接続先の表に 1 行 | 読み物。**貼る瞬間には見えない** |
  | b | **添付チップの警告**（Spec 36 D12 の器） | **貼った時点で見える**。運べない警告と同じ場所 |
  | c | パッシブスキルのバッジ | 分類。**Spec 31 の基準に反する**（働きではない） |

  **決着（2026-08-13 利用者裁定）= a + b。c は不採用。**
  b を採るのは、利用者の起点が「**秘匿情報を渡さないように注意すれば**」であり、
  **注意が要るのはまさに貼る瞬間**だから。c を採らないのは Spec 31 の基準
  （このアプリがその機構を実際に使っているか）が 2 つに割れるため。

  **文面（利用者提示。観測した事実だけを書く）**:

  > `api.meta.ai` は送った添付を `/v1/files`（`purpose=user_data` /
  > `expires_at=null`）に**期限なしで保存**します。一覧で取得でき、
  > `DELETE /v1/files/{id}` で消せます。

  **「学習に使われます」とは書かない** — あれは規約で、こちらは観測できる事実。
  **性質の違う 2 つを 1 文に混ぜると、どちらも確かめられなくなる**
  （Spec 05 が「検索した事実」と「出典が返らない事実」を分けたのと同じ形）。
- **D11（新設・rev2）`/v1/files` の削除経路を実装するか。**
  **決着（2026-08-13 利用者裁定）= 実装しない。**
  **この村の機構ではない** — 送ったものの
  後始末は接続先の管理画面か API の仕事で、ここに載せると
  「Fuseforks が外部の状態を管理する」新しい責務が生まれる
  （`mcp_server_contract` が「村の状態は外からブラックボックス」と決めたのと
  逆向きの越境）。**ただし存在は台帳へ書く** — 消したい人が
  `/v1/files` を知らないまま残り続けるのは、#44 の「歯止めの先に道を書く」の
  逆側（**行き先を書かないと、消す道があることに気づけない**）。

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
- [x] **P1 ワイヤ — 完了（2026-08-13）**。コア lib 549・`carries_table` 4 本
      （20 → **24 マス**）・workspace 全緑・clippy 警告ゼロ。

  **P1 実装記録（実装で決めた 6 点）**:
  1. **D2 の分割線は「関数ポインタを adapter が渡す」形で引いた** —
     `responses_input::encode(messages, part_for)` の `part_for` は
     `fn(&PromptAttachment) -> Option<ResponsesInputPart>`。
     **`carries` を読まない**ので `adapters_match_the_carries_table` は
     同語反復にならず、各 adapter は「自分が何を組み立てられるか」を独立に主張する。
     共有側に残ったのは**テキストと関数往復の骨格**だけ（3 社で同形と実測済み）
  2. **`wire::MetaResponsesRequest` は `tool_choice` の欄を持たない。**
     `"auto"` 以外が 400 なので、**送れる形が無いほうが強い**
     （#77 で `temperature` を欄ごと持たなかったのと同じ判断）。
     `ToolChoice::None` は**ツールごと出さない**ことで表し、
     `Required` / `Specific` は表現できない（**このワイヤは強制ツール呼び出しを
     持てない** — 契約に明記した）
  3. **`store` / `include` / `summary` / `context` を送らない** — このワイヤで
     測っていない。未測定の欄を既定で送ると 400 が出たとき原因がこちらか相手か
     分からなくなる。**当時の理由は「受理集合が列挙されないワイヤだから」と
     書いていたが、P5 でその前提は型の層に限ると分かった**（スカラの enum 欄は
     全列挙する）。**判断は変えない** — 欄の存否そのものは相変わらず
     名指しでしか教わらないので、送らない側が正しいままだった
  4. **`phase: "commentary"` は読まない**（`intentionally_unread`）。
     本文として繋ぐ — ここだけ独自の選別を持ち込むと「なぜこの社だけ発言が
     消えるのか」が画面から読めない。雑音だと分かったら**そのとき観測を根拠に**落とす
  5. **検索の計器に `actions=` / `sources=` の内訳を書かない** — Meta の
     `web_search_call` に `action.sources` が在るかを測っていない。
     **無い欄を `-` で埋めると「測ったが無かった」と「見ていない」を畳む**
     （Spec 34 の `ticks` と同じ判断）
  6. **`meta_web_search_active()` は AND 述語**（フラグ単独を判定に使わない規律の
     5 例目）。互換のまま真になっている設定は `world.json` の直接編集で作れる

  **`carries` の修正が実地で効いた** — 6 値目を足した瞬間、コンパイラが
  `client.rs:125`（`carries`）を **non-exhaustive で指した**。
  **Spec 36 のワイルドカードのままなら、Meta が audio=false / video=false を
  黙って受け取っていた**（実測は 4 種別すべて ✓ なので 2 マスが静かに誤る）。
  **doc に「コンパイラが指す」と書いた主張が、今回はじめて本当になった。**

  **ミューテーション 2 回で、網 2 本が別々の仕事をすることを実証した**
  （**予測を 1 つ外して、より良いことが分かった**）:

  | 変異 | `adapters_match` | 逐語凍結 |
  |---|---|---|
  | **adapter だけ**（動画を落とす） | **赤**（`MetaResponses × Video`） | 緑 |
  | **adapter と表を同時に** | **緑**（一致してしまう） | **赤**（`MetaResponses の動画`） |

  予測は「(a) で 1 本 / (b) で 2 本」だったが、**同時に変えると一致するので
  `adapters_match` は緑になる**。**逐語凍結を別に置いた理由がそのまま出た** —
  「両方を同時に変えると通ってしまう」は、あの網が塞ぐために書かれている。
- [ ] **P2 固有スキル**: `ModelTemplate.meta_web_search` + `meta_web_search_active()` +
      `LlmConfig` への配線 + `Grounding.engine` の Meta
- [x] **P3 フロント — 完了（2026-08-13）**。vitest 359 全緑・vue-tsc + vite build 緑。

  **union と `Record` の非対称が実地で出た**（契約に書いたとおりの形）。
  `types.ts` の `Provider` へ 6 値目を足した瞬間に落ちたのは
  **`carries.ts` の `Record<Provider,…>` だけ**で、`providerSkills.ts` の
  `DEFAULT_BASE_URL` / `ALSO_SERVES_COMPAT` / `providerSkills` は**黙って通った**。
  **同じ TS でも、union か Record かで網羅性が変わる**（`variant_addition_sites`）。

  **手で数えた 5 箇所**（コンパイラが 1 つも指さない）:
  `DEFAULT_BASE_URL` / `ALSO_SERVES_COMPAT` / `providerSkills` の判定と戻り値 /
  `anyOffered` の OR / `PROVIDERS` の選択肢。
  **`anyOffered` が最も静かな穴** — 足さなくても型は通り、
  **見出しだけが出なくなる**（トグルは描かれるのに区切りが消える）。

  **引数の型に欄を足した瞬間、呼び出し元は全部指された** —
  `providerSkills(draft)` の `Pick<…>` に `metaWebSearch` を入れたので、
  テストの `Draft` 型とダイアログの初期値 2 箇所が TS2345 / TS2741 で落ちた。
  **union で黙って吸われる部分と、構造体で必ず指される部分が同じファイルに同居している。**

  **同期テストが 3 本とも捕まえた** — Rust の凍結表が 6 行になったのに TS 側の
  期待値が 5 のままだったので、`expected 6 to be 5` で赤。**Rust を直して TS を
  忘れる**という、この網が塞ぐために書かれた形がそのまま出た。

  **Spec 36 の追従漏れを 2 件回収した**（#51 (b) — 腐るのは Spec 37 の節ではない）:
  `openaiResponsesCaveats` と `responsesHint` が
  **「切り替えると画像は相手に届きません」**と言い続けていた。
  **Spec 36 D9 で画像は全ワイヤが運ぶようになっている**ので、これは嘘。
  日英とも直した。**機能を回収したとき、その制約を説明していた文言が腐る。**
- [x] **P4 台帳 — 完了（2026-08-13）**。**ファイル単位で数えた**（#51 (b)）—
      README 日英 / DETAIL 日英 / CLAUDE.md / `data_contract`（`meta_responses` の
      ブロックは P0b で書いたので、ここは carries 表と数の追従）。
      **README は 161 行のまま**（入口の規律。表の 1 行を書き換えただけ）。

      **回収した追従漏れ 6 件。どれも Spec 37 の節ではない**（起票時に Notes 2 で
      「腐るのは Spec 36 の節」と予告したとおりになった）:

      | 場所 | 何が嘘になっていたか |
      |---|---|
      | DETAIL 日英 | carries 表が 5 行 / **「音声と動画を運ぶのは Gemini だけ」** |
      | DETAIL 日英 | 「**5 つの**ワイヤすべてでテストが固定」→ 6 つ |
      | `data_contract` | 「全 **20 マス**」×2（表は 24 マスになった） |
      | README 日英 | グラウンディングの列挙に **Meta が無い** |
      | README 日英 | 接続先の列挙に **Meta が無い** |

      **「動画は Gemini だけ」は 2 日で 2 回書き換わった** — Spec 36 で
      「Gemini だけ」と凍結し、Spec 37 で「ネイティブの 2 本」になった。
      **排他を主張する文は、集合に 1 つ足すたびに腐る**（#91 と同族 —
      あちらは述語、こちらは散文）。
- [x] **P5 実機確認**（2026-08-13。5 件とも観測。ログは `fuseforks.log`）:
  1. [x] **出典つきで答えた** — `05:15:59 meta search: calls=4 sources=2 queries=2` /
     `prompt=65014 backend=meta-responses`。対照は同じ個体の直前 7 ターンで
     `prompt` 13,379〜14,989 = **検索した回だけが 4.4 倍**
  2. [x] **同一ターンでの併用は機構として観測**（`04:54` — `round=1` で
     `MCP_DOCKER__fetch` → `meta search: calls=1` → `round=2` で再び fetch →
     `rounds=3/16 stop=-`）。**文言どおりの「同梱ツール」では未踏**で、
     利用者裁定で閉じた。**根拠は `encode` の `tools` が平坦な 1 配列**であること —
     server 側の検索は `ResponsesTool::Server`、同梱も MCP も等しく `Function` で、
     **区別は encode の手前で消えている**。なお同梱ツール自体は同じ日に
     `file` / `run` ×2 / `grep` ×2 が全部 `ok=true`（理由欄つき）で回っている
  3. [x] **4 種別すべて通った** — `attachment kind:` が `image` / `video` / `audio` /
     `pdf` の 4 行、すべて `provider=meta-responses outcome=ok`、
     各直後のターンが `stop=-` で完走。**carries の 6 行目が実機で全 ✓**
  4. [x] **跳ねない** — OFF の個体で `prompt=12233` / `12228`（`meta search:` 行なし）。
     検索した回の 65,014 に対し **5.3 分の 1**。
     **なお「`meta search:` 行が無い」は OFF の証拠にならない** — `tool_choice` は
     `auto` のみなので **ON でもモデルが選ばなければ行は出ない**。判定には
     OFF のテンプレートを明示的に作る必要がある（#68 の形）
  5. [x] **踏んだ。ただし症状は本項の記述と違った** — `maxOutputTokens=512` で
     `turn failed: code=LLM_OUTPUT_TRUNCATED fatal=false: 出力上限に達し、応答が
     途中で切れました（512 トークン）…`。**本文が空になる経路はこのワイヤに無い。**
     応答が `incomplete_details.reason = "max_output_tokens"` を持つ = **理由が判る**ので、
     `Finish::Length` → `LlmError::OutputTruncated` へ名前が付く。**#72 の空本文は
     理由が判らないときの受け皿**で、判る経路では到達しない。
     `error.rs` の `stops_the_agent()` が `OutputTruncated` **だけ** false なので
     **個体は生き残る**（`failures.md` #40 / 2026-07-31 の分離が効いた実例）

  **検収の書き方の誤りが 4 例目**（#68 / #80 / #85 に続く）。今回の型は
  **probe の観測をアプリの観測として書いた**こと — 生の HTTP probe には
  切り詰めを名前へ変換する層が無いので空本文が見えたが、**アプリにはその層がある**。
  **一般化: probe で見た症状を検収項目に書くときは、その症状がアプリの層を
  通った後でも同じ形で出るかを数える。**

  **未確認で残す観察が 1 件** — 失敗したターンは `turn:` 行を出さないので、
  **払った出力 512 と入力 3 万近くがログのどこにも出ない**。予算に計上されて
  いるかは未確認（`reject_empty_reasoning` は `usage` を持つ `ChatResponse` を
  受けて `Err` へ落とす）。**#72 と同型の疑い**で、今日の走行だけで数回起きている。

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

5. **査読の指摘 5（Rust 7 箇所 vs P1 の 5 箇所）を数え直したら、
   自分が Spec 36 で作った罠が出た。**

   `Provider::carries` を `match (self, kind)` に
   `(_, K::Image) => true` の形で書いていた。**種別側がワイルドカードなので、
   `Provider` に 6 値目を足してもコンパイラが指さず、Meta が
   `[image=true, audio=false, video=false, pdf=true]` を黙って受け取る** —
   実測は**4 種別すべて ✓** なので、**2 マスが静かに誤る**。

   **doc には「`match` を網羅で書くのは 6 値目を足す人へ問いを出すため」と
   書いてあった。その主張自体が嘘だった。** `variant_addition_sites` が
   「ワイルドカード match は新 variant を黙って吸う」と警告している当の形を、
   その警告を引用した Spec で作っていた（#62 と同族）。

   **rev2 で直した** — provider ごとに 1 腕の配列リテラル
   （`Self::Gemini => [true, true, true, true]`）にして、**種別側の
   ワイルドカードを消した**。並びは `tests/carries_table.rs` の凍結表と同じ
   `[image, audio, video, pdf]`。**これで 6 値目はコンパイラが指す。**

   **一般化: 「コンパイラが指す」と doc に書いたら、variant を 1 つ足して
   実際に落ちるか確かめる。** 網羅性の主張は、網羅していない書き方でも
   同じ文章で書けてしまう。

## 改訂履歴

- **rev2**（2026-08-13）: 査読 8 点を反映（**採用 6 / 訂正して採用 2 / 反証 0**）。
  **P0b で probe を 12 発追加**（計 24 発）。
  - **D2 を確定**（査読 1）— `function_call` の往復が 3 社で同一と実測できたので、
    共有は「相手が決める側」だけ・添付は各 adapter。`encode(…, provider)` は作らない
  - **D8 / D9 / D10 / D11 を新設** — `tool_choice` は `auto` のみ（新発見）/
    動画の実用経路は data URL のみ / 出力上限の警告 / Files の削除は実装しない
  - **訂正して採用 2 点**: (a) 査読 3 の「上限を引き上げるか effort を強制」は
    **利用者が決めた値を黙って書き換える**形なので、**警告へ倒した**
    （Spec 36 D12 と同じ器。**下限は実測したので閾値が書ける**）
    (b) 査読 5 の数え直しで、**`carries` のワイルドカードという自分の罠**が出た
    （Notes 5。**その場で直した**）
  - **D7 の判断材料が変わった** — `/v1/files` に**添付が恒久保存**されており、
    **利用者の実データが 1 件含まれていた**。学習利用（規約）より具体的な
    **観測できる事実**なので、置き場の選択肢に「添付チップの警告」を足した
- **rev1**（2026-08-13）: 起票。**P0 の probe 12 発を先に撃ってから書いた** —
  この村では文書が 4 回続けて実装と食い違っており、設計を文書から起こすと
  最初の作業がその作り直しになる。
