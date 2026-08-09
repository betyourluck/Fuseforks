# Spec: Grok の Live Search — `/v1/responses` ワイヤと「固有スキル」トグル

**ID**: 31
**Date**: 2026-08-10
**Status**: rev2 承認 → **P0〜P3 完了**（2026-08-10。P0 = 契約凍結 `5c3c5e1` /
P1 = `llm/xai_responses.rs` + wire 型 + `Provider::XaiResponses` +
`GroundingEngine` + AND 述語（`4bf7435`）/ P2 = モデル登録の「固有スキル」（`47fd023` / `af1ee54`）/
P3 = 来歴の表示（エンジン名・X の投稿者・横流し）。
査読 7 点 → 採用 4 / 訂正して採用 2 / 実測で機構を訂正 1 —
指摘 1・3・4 は対の再 probe で決着）
**Branch**: なし（main へ直接コミット。Phase ごと）

## Goal

**Grok の Live Search（現行名 Agent Tools の `web_search` / `x_search`）を村へ入れる。**
入口は**モデル登録ダイアログ**で、**Grok のワイヤを選んだときだけ「固有スキル」の
カテゴリが出る**（利用者裁定 2026-08-09 —「Gemini の Google Search と同じように
モデル登録のときに Grok の固有スキルのカテゴリに Grok の場合に表示」）。

動機は技術ではなく流通（CLAUDE.md「最優先は xAI の Live Search」の節が正）—
ファイナンス系の需要。**「いま何に注目が集まっているか」は市場についての事実**で、
X の投稿 + 反応の実数がそのデータになる。

## 接地（2026-08-09 の probe 4 発。CLAUDE.md の実測ブロックが正）

- **legacy の `search_parameters` は HTTP 410 Gone**。サーバー自身が
  Agent Tools API への移行を名指しして断る
- **現行は `/v1/responses` のサーバー側ツール** `{"type":"web_search"}` /
  `{"type":"x_search"}`。**採用の前提 = 4 本目のワイヤ**
- **citations は返る（判定 A）** — `output_text` への `annotations`
  （`type: "url_citation"`）。TechCrunch の日付つき記事 URL を実物で確認。
  `x_search` は **X の実 status URL + いいね・リポスト・ブックマークの実数**を返す
- **`start_index` / `end_index` は全部 0** = 出典はメッセージ単位。
  rapidtables / YouTube も混ざる — 選別はこちら側の仕事
- decode の罠: `x_search` の output 種別は **`custom_tool_call`**
  （`web_search` は `web_search_call`）/ `reasoning` が interleave し
  **thinking トークンがこの経路では流れる**（実測 1,075 tokens）
- コスト: 検索結果がプロンプトへ注入され input が膨らむ（実測 98,213 うち
  cached 62,720）。`usage` に `cost_in_usd_ticks` /
  `server_side_tool_usage_details` が生える。重い問いは分単位
  （1 回目の x_search は 180 秒内に ConnectionReset、問いを小さくして完走）

**査読（2026-08-10）を受けた対の再 probe** — 既定と
`include: ["no_inline_citations"]` を同じ問いで 1 回ずつ:

- **既定**: 本文に `[[1]](url)` の markdown 印が混入し、annotations は
  **本文で実際に参照した分だけ**（2 件）。`start_index` / `end_index` は
  **印そのものの文字位置**（実測 `text[49:73] = '[[1]](https://x.ai/news)'` —
  公式の定義「first `[` の位置 / closing `)` の直後」と一致）
- **`no_inline_citations`**: 本文は無汚染。annotations は**調べて触れた全 URL**
  （18 件）で、位置は全部 `(0, 0)`
- **トップレベル `citations` 鍵は REST の生応答に存在しない**（2 回測って
  2 回なし。公式の「`response.citations`」は SDK の集約属性で、生ワイヤの欄ではない）
- rev1 の「`start_index` が全部 0」は**既定でも観測された**（初回 probe）が、
  再現の対では既定側に非零が出た — 位置の有無は**保証ではなく揺らぐ**。
  設計はどちらに転んでも壊れない側（D5）へ倒す

## 決定（D）

### D1: 4 本目のワイヤ `Provider::XaiResponses`（名前は査読で確定）

`POST {base_url}/responses`。encode は `input` + `tools` +
**`include: ["no_inline_citations"]` を常に送る**（D5 の帰結。省くと本文へ
`[[N]](url)` の markdown が混入し、annotations も参照分に痩せる）。
decode は `output` 配列 — `message` / `reasoning` / 検索呼び出しの 3 系で、
**検索呼び出しは `web_search_call` / `x_search_call` / `custom_tool_call` の
3 名を同じ腕で受ける**（公式文書は `x_search_call`、実ワイヤの観測は
`custom_tool_call` — 移行途中の未文書挙動と読む。どの名で来たかは計器に残す）。
未知種別は #72 の処方で**数えてから捨てる**。既定 base URL は
`https://api.x.ai/v1`（プロトコル切り替えで追随する既存挙動に乗る）。

**範囲は xAI のみ。OpenAI の Responses 移行は対象外** — 同じエンドポイント名でも
検証できるのは繋いだ側だけで、OpenAI 側の thinking 復活・web search は
別 Spec（回収先が同じ 1 本であることは CLAUDE.md に記録済み。**型を 2 社で
使い回せる形にする努力はするが、使う前から一般化はしない** — Spec 24 D2 で
`kind` 欄を先に作らなかったのと同じ判断）。

### D2: 「Grok である」の判定は provider であってモデル名ではない

村には「Grok である」という述語がまだ無い（Grok は `OpenAiCompat` に相乗り中）。
判定は **`effective_provider() == XaiResponses`** に一本化し、モデル名
`grok-` 前置での自動検出はしない — `reasoning_effort` のモデル名送り分けは
**同じワイヤ内の方言**の吸収であって、ワイヤそのものの選択に名前を使うと
`Provider::detect` と判定が 2 系統になる。

### D3: `web_search` と `x_search` は別トグル

測定で**別ツール・別課金カウンタ・別 output 種別**と確認済み。1 つの
「Live Search」トグルに畳むと、web だけ欲しい村が X の攻撃面（後述 D6）まで
一緒に開けることになる。カテゴリ見出しは「固有スキル」（表示は provider 名を
冠して「Grok の固有スキル」相当。辞書鍵は P2 で確定）。

**Gemini の `googleSearch` は動かさない。** 既存の欄・表示・契約はそのまま
（カテゴリ表示への再編は見た目だけの変更として別途判断）。

### D4: 効きの判定は AND 述語 1 実装（`grounding_active` と同型）

`live_search_active(tool) = トグル AND effective_provider() == XaiResponses`。
フラグ単独を見ない — システムプロンプトの告知・encode・UI の 3 箇所が
別々にフラグを読むと、互換経路のまま真にした `world.json` 手編集で
「検索できないモデルに検索できると教える」が再演する（`model.rs` の
`grounding_active` doc の規律をそのまま写す）。stranded 警告
（トグル真のまま provider を互換へ戻した状態）も Gemini と同型で出す。

### D5: citations は canonical の `Grounding.sources` へ decode する

器は既にある — `Grounding.sources: Vec<GroundingSource>`（URL で重複を潰す・
`AgentMessage.grounding` 経由で `GroundingNote.vue` まで配線済み）。
**`no_inline_citations` で受けた annotations 全件**をここへ入れる — 対 probe で
これが「調べて触れた全 URL」（18 件）であり、査読の指摘 3 が求めた包括リストは
**トップレベル欄ではなく、この形で届く**と確認した。表示側は自動で生きる。
**Spec 05 で「空の器」だった `sources` が初めて埋まる側で使われる。**

- 表示の食い違いは 1 点 — `grounding.engine` の辞書値が「Google 検索」固定。
  `Grounding` に engine 種別の欄を足す（値は closed enum。自由文字列にしない）
- **出典はメッセージ単位であり、表示で偽らない** — 主張単位に見える UI
  （文中アンカー等）は作らない。根拠は「観測で 0 だったから」ではなく
  **意図して位置を持たない形（`no_inline_citations`）を選んだから**。
  位置付きが要る日が来たら、それは inline 印を本文へ受け入れる判断とセット
  （位置は印の座標なので、印を消せば位置も嘘になる — 半分だけは選べない）

### D6: X 本文の防御は「出典の分離」と「Verify の分離」で持つ

X には誰でも投稿でき、検索結果本文は xAI 側でプロンプトへ注入される —
**村のツールループは介在せず、`defuse` の射程外**（封筒の形しか潰さない。
Spec 26 一般化 3「自由入力欄は 1 つとは限らない」の実例で、今度の自由入力欄は
世界中）。村側で持てる防御は 2 つで、どちらも既存の規律の適用:

1. **「そう投稿された」と「本当である」を分けて見せる**（Spec 05 が「検索した
   事実」と「出典が返らない事実」を分けたのと同じ形。X の出典は実在検証が
   できる分だけ強いが、内容の真偽は別）
2. **取ってきた個体に真偽を判定させない**（条例の Verify 段の既存規律が
   ベンダーの層でそのまま効く — 検証は接地能力の別な個体へ。機構は足さない）

### D7: `reasoning` は本 Spec では受け取らず、数える

`dropped content blocks:` と同型の計器で種別と tokens を 1 行出す（#72 の処方）。
thinking の受け取りは 3 社まとめて別 Spec（v0.2.0 裁定の「受け取りが最初」の項。
この村の decode の捨てている行は 1 箇所なので、そちらで一括して拾う）。

### D8: コストの計器

検索を使った呼び出しのときだけ専用行を 1 行出す（`turn:` 行に混ぜない。
`grep include:` と同じ形）。**正の計器は呼び出し回数**（観測欄名
`server_side_tool_usage_details`。公式文書の名は `server_side_tool_usage` —
decode は未知欄を無視する寛容な形にし、**どちらの名で来ても回数が読める**ように
する）。`cost_in_usd_ticks` は**補助として生値のまま**書く — tick の単位は
未検証なので、換算した金額は出さない（確かめていない換算を計器に入れると、
計器が嘘の桁を運ぶ）。トークン天井（Spec 11）には注入された input が
そのまま実効トークンで乗る — 追加の機構は不要だが、**検索 1 回で input が
10 万に膨らむ実測**を README の説明に書く（天井の小さい村では 1 回で尽きうる）。

## Phases

- **P0**: 契約凍結 — `data_contract.yaml` へ `xai_responses_wire` 節
  （Provider 4 値目 / トグル 2 欄 / `live_search_active` / engine enum /
  reasoning は数えて捨てる / 計器の行の形式）。`ModelTemplate` の分類表更新
- **P1**: ワイヤ — `llm/xai_responses.rs` encode/decode + `wire.rs` の型 +
  `client.rs` の分岐 + `Provider::detect`（`api.x.ai` → XaiResponses は**しない**。
  既存の互換運用の村を黙って新ワイヤへ動かさない — detect は現状維持で、
  新ワイヤは provider 明示のみ。ここは査読で確認したい点）。単体は probe の
  実応答を golden に
- **P2**: モデル登録 UI — 「固有スキル」カテゴリ + トグル 2 つ + stranded 警告 +
  辞書 ja/en + `types.ts`
- **P3**: 表示 — `Grounding.engine` の追加と `GroundingNote` の表示切り替え
- **P4**: README / DETAIL 日英 + `data_contract` の回収（**数えるのはファイル単位** —
  #51 (b)。台帳は日英 4 ファイル）
- **P5**: 実機

## P1 実装記録（2026-08-10）

**着手前に probe 3 発で関数ツールの可否を測った** — P0 の decode 契約は
message / reasoning / 検索 3 名しか凍結しておらず、関数が通らないなら
「Grok ワイヤの個体は検索専任」という別の獣になるため（新規個体の既定は
同梱ツール 8 本 — 通らないのに送ると全ターンが壊れる）。結果は全部通った:

- flat 形 `{"type":"function", name, ...}` で提示 → `function_call`
  （`call_id` / `arguments` = JSON 文字列）が返る
- `function_call_output` を input へ返す roundtrip も完走（返した値で答えた）
- **検索ツールと関数ツールは同居できる**（gpt-5 系の chat/completions が
  400 で拒む組を、この口は受ける）
- 副産物: **`web_search_call.action.query` が検索語を運ぶ** —
  `Grounding.queries` の原料。`action.sources` は読まない（出典の正は
  annotations の 1 系統 — 契約へ追補）

実装で決めた 4 点:

- **`ToolChoice::None` では検索ツールも送らない**（契約へ追補）。
  「ツールを使わせない」は server-side tool にも及ぶ — summarize の呼び出しで
  検索が走ると、押した人が検索の注入 input（実測 10 万規模）まで払う
- **接地の告知（`compose_system_prompt` の `grounded`）は Gemini 専用のまま。**
  あの文面は「参照 URL は手元に渡らない」と教える処方で、**xAI では URL が
  渡るので流用すると嘘になる**。`grounding_active()` が Gemini 限定なので
  XaiResponses では自然に告知なし = 正しい。xAI 用の告知を出すか
  （出すなら「出典は画面に自動表示される」の向き）は **P3 の決めどころ**
- **添付画像はこのワイヤでは送らない**（Spec 23 D8 — 画像は互換経路のみ。
  gemini ネイティブと同じ棚）
- **`XaiAnnotation.title` が URL の複製で返る**（実測）ので decode で空へ落とす
  （表示側が URL を二重に出さない）

**`ipc_contract` の凍結網が 2 段で TS 追従を強制した** — `Grounding.engine` で
1 回、`ModelTemplate` の 2 欄で 1 回。どちらも「期待値を直すのではなく、
TS 側へ欄を足してから期待値を直す」の順で消化（`useUiSettings.test.ts` の
前例と同じ扱い。網は緩めない）。

単体 11 本（encode golden / ToolChoice::None / トグル独立 / roundtrip 再送形 /
decode golden / 検索 3 名 / function_call / 壊れた arguments / 未知種別 /
Length / status 不明は Other）+ model の AND 述語 1 本 + detect の現状維持 1 本。

## P2 実装記録（2026-08-10）

**着手前に既定 URL 表の影響を数えて、実装を止めて先に直した。**
`api.x.ai/v1` を `DEFAULT_BASE_URL` へ足すと、その URL が「既知の既定値」に
入るため、**いま互換経路で動いている Grok 個体（イクス）の設定に
「base URL が食い違う」という嘘の警告が出る**。しかも同じ形の誤検知が
**Gemini 互換運用では既に発生していた** — `generativelanguage.googleapis.com`
は OpenAI 互換の口も持つのに、`provider: open_ai_compat` のままだと
「他社の既定値」として弾かれる。

| ケース | 現行 | x.ai を足した後（修正前） |
|---|---|---|
| Grok を互換で運用（イクスの実物） | 無警告 | **警告** ← 新規の退行 |
| Gemini を互換で運用 | **警告** | **警告** ← 既存の誤検知 |
| 本当に食い違い（anthropic + openai の URL） | 警告 | 警告 |

**一般化: 「既知の値の集合」に要素を足す変更は、その集合を*否定*に使っている
述語を先に数える。** 追加は集合の意味を広げるが、`!includes(x)` 側の判定は
黙って厳しくなる。ここでは「既定値の一覧」が「切り替え時の追随」（肯定）と
「他社の設定が残っている」（否定）の 2 つに使い回されており、**足したい理由は
前者にしか無かった**。

処方は `ALSO_SERVES_COMPAT`（互換の口も同時に持つホストの既定 URL）を持ち、
`provider: open_ai_compat` のときだけ免除すること。**免除は互換側だけ** —
ネイティブを選んだのに別ホストなら従来どおり指摘する。

**判定を `lib/providerSkills.ts` の純関数へ切り出した**（SFC の computed の
ままでは赤で示せない）。`baseUrlMismatch` / `presetBaseUrlFor` /
`providerSkills` の 3 本 + 単体 14 本。**ミューテーションで実測** — 免除を
外すと**予測どおり 1 本だけ**が赤（`1 failed | 13 passed`）。

そのほかの判断:

- **表示条件と stranded は 1 本の純関数から対で引く**（`SkillVisibility`）。
  2 つの computed に分けると、スキルが増えるたびに対を書き足す形になり、
  片方だけ足した状態が「押しても効かないチェック」または「黙って消える設定」
  として現れる
- **stranded の行はスキルごとに 1 本**。まとめて 1 つの警告にすると
  どれを直せばよいかが読めない
- **検索のコスト注記を画面へ入れた**（実測値つき）。天井（Spec 11）の小さい村は
  検索 1 回で尽きうるので、押す前に言う。README ではなく画面なのは、
  押す判断をする場所がここだから
- **TS の `Provider` union に `xai_responses` を足した**。doc も「gemini と
  xai_responses は自動判定されない」へ揃えた（Rust 側の doc と同じ文）

**Notes 1 を実装で 1 つ広げ、利用者が裁定して確定した**（2026-08-10）—
「固有スキル」の見出しは **Gemini でも出す**。起票時は「Gemini を寄せるかは
Spec の外」と書いていたが、見出しを Grok 限定にすると**同じダイアログの同じ
位置で、Gemini だけ見出しの無い裸の行**になる。利用者の裁定は
「**基本的に今後はそれぞれのモデルの固有スキルを書くようにしたい**」で、
この見出しは Spec 31 の産物ではなく**各社機能を並べていく恒久の器**になった。

### パッシブなスキル（2026-08-10 利用者要望）

利用者 —「**パッシブのスキルもある。Anthropic のキャッシュとかのようにね**」。
**チェックボックスは置かず、ラベルとバッジ「パッシブ」で出す**（同日追加裁定）。

- **操作できないものを操作の形で出さない。**「押しても効かないチェックを
  見せない」（Spec 05 由来）の裏返しで、**無効化したチェックも同じ嘘**になる
  — 何かすれば有効にできる、と読める。ラベル + バッジ + 説明の 3 点で、
  「在るが触れない」を形で示す
- **載せるのは「このアプリがそのプロバイダ固有の機構を実際に使っている」ものだけ。**
  実装を数えた結果、**該当は Anthropic のプロンプトキャッシュ 1 つ**
  （`place_message_breakpoint` / `build_system_blocks` が `cache_control` を
  組み立てる。TTL 1 時間・2,048 トークン超で発火）。**互換 / Gemini / xAI は
  `cached_tokens` を読んでいるだけ**で、こちらは何も送っていない —
  サーバー側が勝手にやる最適化を並べると、**アプリの働きではないものを
  働きとして見せる**ことになる。テストで負の対照つきで留めた
- 説明文には**発火条件（2,048 トークン超）を書く**。「常に効く」と書くと
  小さい依頼で嘘になる — バッジの「パッシブ」は*操作できない*の意であって
  *無条件*の意ではない
- バッジの見た目は会話ペインの「外部」と同じものを再利用（同じ規律を
  2 箇所で持たない）
- **見出しはパッシブだけの provider でも描く**（`anyOffered` にパッシブを含む）
  — 見出しの意味は「ここから先はこのプロバイダにしか無い能力」で、
  操作できるかどうかとは別

## P3 実装記録（2026-08-10。**実機の画面から起票した**）

利用者が実機の画面を出した。イクス（grok-4.5）の x_search が**実 status URL を
45 件**持ち帰っており、そこで問題が 3 つ同時に見えた:

1. **エンジン名が「Google 検索」** — 辞書値が固定文だった。P3 の本体
2. **45 件すべてのラベルが `x.com`** — `sourceLabel` は表題が空なら
   ホスト名で代用する。**同じホストの投稿が数十件返る経路では、全部が
   同じ文字列になってリンクの区別が付かない**
3. **縦に 45 行** — 来歴だけで会話ペインが埋まる

**2 が一番深い。** 3 だけを直して横へ流しても、`x.com, x.com, x.com…` が
並ぶだけで読めない。**縦長は症状で、ラベルが区別を運んでいないのが病**。

- **エンジン名は記録から引く**（`grounding.engine.{google|xai}`）。
  `GroundingView.engine` を足し、**欄を持たない古い記録は `google` として
  読む**（コア側の serde 既定と同じ向き。redb に保存済みの発話は欄を持たない）
- **X の投稿は `@handle` で出す。** status URL は投稿者を経路に持っている
  （`/{handle}/status/{id}`）。**プロフィールや検索結果の URL は投稿として
  扱わない** — 押した先が想像と違うものになる。
  **投稿者名は URL から読んだ値であって本人性の検証ではない**が、URL の一部
  として確実に正しく、45 個の同じ文字列よりは判断材料になる
- **横へカンマ区切りで流し、件数を先に出す**（`参照元 45 件`）。読む前に規模が
  分かる。**打ち切りはしない** — 黙って減らすと「これで全部」と読まれる
  （no silent caps）
- ミューテーション 2 回で赤を確認（engine を固定文へ戻す → 1 本 /
  P2 の免除を外す → 1 本。どちらも予測どおりの本数）

**X のマークを SVG で足した**（2026-08-10 利用者提供）。

- **`fill` を `currentColor` へ直した。** 原本は `fill` 指定を持たず、SVG の
  既定である**黒**になる — ダーク背景では見えず、テーマにも追従しない。
  **Auto Fit のアイコンで踏んだのと同じ形の 2 例目**（あちらは `#212121` 固定）。
  **もらった図形もそのままでは置かない。** ビルド出力で確認済み
  （静的 props へ巻き上げられ `fill: currentColor` のまま。黒の直値ゼロ）
- **付けるのは X の投稿だけ**（`sourceIcon`）。プロフィールや検索結果の
  x.com には付けない — それらのラベルはホスト名 `x.com` のままなので、
  アイコンを添えると **`[X] x.com` になって同じことを 2 回言う**。
  アイコンが意味を足すのは、ラベルが `@handle` で**ホストがどこか読めない**とき
- **表題を持つ参照元にも付けない**（表題が何のページかを既に語っている）

## 検収（書く前に読み口の実在を数えた — #68 / #90 の規律）

| | 何を見るか | 読み口 |
|---|---|---|
| 1 | x_search ON の Grok 個体が X の話題を**実 status URL つき**で答え、GroundingNote に出典が並ぶ | 画面 + `fuseforks.log` の計器行 |
| 2 | トグル OFF の同じ個体は `server_side_tool_usage` が全 0 | 計器行（**ON の対照を先に取る** — #85） |
| 3 | provider を互換へ戻すと stranded 警告が出て、送信は互換のまま通る | 画面 + ログに検索計器行が**出ない**こと |
| 4 | `reasoning` の dropped 計器が種別と tokens を出す | `fuseforks.log` |
| 5 | 検索なしの依頼では計器行が増えない | 計器行（負の対照） |

## Notes

1. **「固有スキル」カテゴリは構想（CLAUDE.md「各社固有機能」の節）の最初の実装**。
   ~~Gemini の Google 検索を後からこのカテゴリへ寄せるかは、この Spec の外~~ →
   **P2 で見出しの射程を広げた**（Gemini でも見出しが出る。理由と戻し方は
   P2 実装記録の末尾）
2. probe の使い捨て script は scratchpad にあり、リポジトリへは入れない。
   golden にするのは応答 JSON の形だけ
3. **既存の互換運用の村は自動では新ワイヤへ乗らない**（P1 — `Provider::detect` は
   現状維持）。README / DETAIL に「Grok の Live Search は provider を
   xAI（Responses）へ明示的に切り替える」を明記する。移行を促すバナー
   （互換 + `grok-` モデル名の組を検出して知らせる）は**頻度を見てから別途** —
   一度も鳴らない機構は空の欄になる（`run.json` の起動 WARN を置かなかった判断と同じ）
4. **ツールのフィルタ引数は今は作らないが、契約に予約として書き残す** —
   `web_search.allowed_domains` / `x_search.allowed_x_handles`（上限 20）/
   `from_date` / `to_date` が公式に存在する。欄は作らない（空の欄は
   「何を入れるべきか」を問い続ける — Spec 15 D7 と同じ判断）。契約の
   コメントに名前を残すのは、**将来足すときに破壊的変更ではなく加算で
   済む形を今の設計が塞がないため**の覚え書き
