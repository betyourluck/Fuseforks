# Spec: Gemini ネイティブ経路と Google 検索による接地

**ID**: 05
**Date**: 2026-07-29
**Status**: **Done**（rev2 査読承認 → Phase 0〜4 + 実機確認 完了、2026-07-29）
**Branch**: なし（main へ Phase 単位で直接コミット — Spec 01〜04 と同じプロセス）

> **本 Spec は例外的に実装先行である。** Spec 01〜04 は「起票 → 査読 → Phase 0
> 契約凍結 → 実装」の順だが、本件は利用者の明示指示で Phase 1・2 を先に着地させた。
> 動機は、Gemini の実挙動が**推測できず実測でしか決まらなかった**こと — 実際に
> 4 つの 400（`Invalid tool type` / `additionalProperties` / `$schema` /
> `include_server_side_tool_invocations`）を順に踏んで初めて契約が確定した。
> 机上で凍結していたら、4 回とも間違った契約を凍結していた。
> **Phase 0 はこの実測結果を後追いで固定する作業**である。

---

## Goal

Gemini のモデルテンプレートで **Google 検索による接地**を選べるようにする。

そのために `Provider::Gemini`（ネイティブ `generateContent`）を新設する。
既存の OpenAI 互換経路では**構造的に不可能**だからで、代替手段が無い。

```
POST /v1beta/chat/completions  {"tools":[{"type":"google_search"}]}
→ 400 Invalid tool type: google_search        （実測 2026-07-29）
```

接地は関数呼び出しと**併用できる**（検索 → `transfer_to_*` が 1 応答の中で
連鎖する）ため、委譲や同梱ツールを犠牲にしない。

---

## 現状の構造（接地）

- `Provider` は `OpenAiCompat` / `Anthropic` の 2 値。`detect()` は
  `api.anthropic.com` 以外を互換へ落とす
- **Gemini は既に動いている** — 互換経路で。ジェミー（`gemini-3.6-flash`）と
  ロボットくん 2・3 号（`gemini-3.1-flash-lite`）が
  `https://generativelanguage.googleapis.com/v1beta` を base URL に稼働中
- 思考署名の往復は `ToolCall.extra`（Spec 04 時点で既存）で解決済み。
  Gemini はこの枠の**最初の利用者**として最初から想定されていた（failures.md #18）
- `Provider::path()` は `&'static str` を返す。モデル名を含むパスを組めない

---

## Stories

### P1: Gemini ネイティブ経路

> 利用者として、モデルテンプレートのプロトコルに Gemini を選べる。
> なぜなら Google 検索による接地はネイティブ経路にしか存在せず、
> 互換経路のままでは 400 で拒否されるから。

**仕様**

- `Provider::Gemini` を追加。`POST {base_url}/models/{model}:generateContent`
  + `x-goog-api-key`（Bearer でも `x-api-key` でもない第三の形）
- `Provider::path()` は `&'static str` → `String`（モデル名を埋めるため
  シグネチャごと変わる）。既存 2 プロバイダの呼び出し側も巻き込む
- **`detect()` は変えない。** `generativelanguage.googleapis.com` は今も
  互換へ落ちる。自動判定を変えると、設定を触っていない既存エージェントが
  黙って別のワイヤへ移る。ネイティブは**明示選択でのみ**有効になる
- 互換層との差は adapter (`llm/gemini.rs`) に閉じる:
  system は `contents` に混ぜず `systemInstruction` へ／role は
  `user`・`model` の 2 値でツール結果も `user`／`args` は最初から
  オブジェクト／part は `type` タグを持たない判別共用体

**Acceptance**

- Given `provider: gemini` のテンプレート、When 発話、Then
  `/models/{model}:generateContent` へ `x-goog-api-key` 付きで送られる
- Given `provider: null` の既存 Gemini テンプレート、Then 互換経路のまま
  （現状互換。世界を書き換えない）
- Given `finishReason: "STOP"` かつ `functionCall` あり、Then
  `Finish::ToolUse` になる（**STOP を終了と読まない**）
- Given `toolCall` / `toolResponse` パート、Then `tool_calls` に**入らない**
  が、**`grounding.queries` は抽出される**（実行はしない／記録は残す）。
  この 2 つは両立する — 検索は応答が返る前に Google 側で**完了している**ので、
  こちらが実行しないことは連鎖の妨げにならない
- Given 思考ブロック（`thought: true`）、Then 本文に混ざらない
- Given 思考署名、Then `ToolCall.extra` を経由して履歴再送で同じ位置に戻る

### P2: Google 検索による接地のオプトイン

> 利用者として、Gemini のテンプレートに「Google グラウンディング」のチェックを入れる。（起票時のラベルは「Google 接地」。2026-08-01 に外向け文言をグラウンディングへ統一）
> なぜなら学習時点より後の事実を答えさせたいから。

**仕様**

- `ModelTemplate.googleSearch: bool`（serde default = **false**、現状互換）
- UI: `provider === "gemini"` のときだけ表示する。互換経路のまま真にしても
  接地は起きない（400 で拒否される）ので、**押しても効かないチェックを見せない**
- `tools` には組み込みと関数宣言を**別要素**として並べる
- **`toolConfig.includeServerSideToolInvocations: true`**。欠くと
  `Please enable tool_config.include_server_side_tool_invocations to use
  Built-in tools with Function calling.` で 400（実測）。
  **`googleSearch` が真のときだけ送る**（偽なら送らない）。
  実測エラーは「関数併用時」に出たが、関数の有無で送り分けはしない —
  必須条件が併用時であることは、単独時に**送ってはいけない**ことを意味しない。
  かつ、このフラグは応答に `toolCall` パートを乗せる指示でもあり、
  関数を持たないエージェントでも検索語の取得経路として働く

**Acceptance**

- Given `googleSearch: true` + 関数宣言あり、Then `tools` が 2 要素になり
  `includeServerSideToolInvocations` が真で送られる
- Given `googleSearch: false`、Then `includeServerSideToolInvocations` を送らない
- Given `provider !== "gemini"`、Then UI にチェックが出ない
- Given 旧 world.json（フィールド不在）、Then `false` として読める

### P3: スキーマの削り落とし

> 開発者として、同梱ツールと MCP ツールの JSON Schema をそのまま提示できる。
> なぜならツールの定義側に Gemini の都合を持ち込みたくないから。

**仕様**

- Gemini の `parameters` は JSON Schema ではなく **OpenAPI 3.0 の部分集合**。
  未知キーを黙って無視せず `Unknown name "..." Cannot find field` で弾く
- adapter が**許可リスト**で削る。除外リストにしない —
  **MCP ツールのスキーマは接続先のサーバーが書くもので、こちらから中身を
  制限できない**（同梱ツール 7 本の `additionalProperties` だけなら列挙で
  足りたが、MCP ツールが `$schema` を持ち込んで同じエラーで再発した）
- 意味を保てるものは写す: `const: X` → `enum: [X]`／
  `type: ["string","null"]` → `type: "string"` + `nullable: true`
- 削った結果 `properties` が空になったら `parameters` ごと省く
- **`$ref` / `$defs` は解決しない。** 参照先を失った空の項目は Gemini に
  拒否されるが、型を推測して埋めるほうが害が大きい（嘘のスキーマで
  モデルを動かすことになる）。踏んだら 400 として表に出す

**Acceptance**

- Given `additionalProperties` を持つ同梱ツール、Then そのキーが送られず
  `properties` / `required` は保たれる
- Given `$schema` 付きで入れ子にも未対応キーを持つ MCP スキーマ、Then
  入れ子まで再帰的に削られる
- Given `const`、Then `enum` へ写る（制約ごと消えない）
- Given `properties` が空、Then `parameters` キーごと省かれる

### P4: 接地の来歴を捨てない

> 利用者として、エージェントが何を根拠に答えたかを追える。
> なぜならモデルの言う「出典」は、根拠が手元に無いとき捏造されるから。

**仕様**

- `ChatResponse.grounding: Grounding { queries, sources }` を新設。
  **`ChatMessage` ではなく `ChatResponse`** — 後者は履歴に積まれないので、
  ラウンドトリップの契約に影響せず観測の席を増やせる
- `queries` は `groundingMetadata.webSearchQueries` と `toolCall.args.queries`
  の両方から集めて重複を潰す（**必ず取れる**）
- `sources` は `groundingMetadata.groundingChunks[].web` から。
  **空であることが「出典は存在しない」の判定になる**
- **接地が実際に起きるエージェント**には、システムプロンプトで
  **「参照した URL は手元に渡ってこない／代わりに検索語と発表元は言える」**
  を伝える。作業フォルダの実パス開示（README）と同じ処方で、
  **人格ではなくワイヤ経路の性質なので SKILL.md ではなく実装側から入れる**
- 判定は `googleSearch` フラグではなく **`ModelTemplate::grounding_active()`**
  （= フラグ AND 実効プロバイダが Gemini）。互換経路のまま真になっている設定は
  `world.json` の直接編集で作れてしまい、フラグだけを見ると**検索できない
  モデルに「検索できます」と教える**。告知は「持っていない情報を埋める」ための
  節なので、そこで嘘をつくと処方そのものが毒になる
- UI: その状態（真だがプロトコルが Gemini でない）では、チェックを隠す代わりに
  **理由と直し方を出す**。隠すだけだと真のまま見えなくなる

**この節はプロンプト側の告知に閉じる。接地の実 URL をプロンプトへ戻す経路は
作らない**（Notes 9）。

**Acceptance**

- Given `groundingMetadata` あり、Then `sources` に URL と表題が入り、
  URL の無い chunk と web 以外の接地は出典に数えない
- Given `groundingMetadata` 無し + `toolCall` あり、Then `queries` だけ埋まり
  `sources` は空
- Given 接地が有効（`grounding_active()`）、Then システムプロンプトに接地の節が
  入り、**そのエージェントの**安定部分（`stable_len` の内側）に収まる。
  `stable_len` は `compose_system_prompt` がエージェントごとに算出する値で、
  ワールド共通の不変プレフィックスではない（作業フォルダの有無・Construct /
  Skill の内容で既にエージェントごとに違う）。テンプレート由来の節が増えても
  他エージェントのキャッシュには影響しない
- Given `googleSearch: true` だが `provider` が Gemini でない、Then
  **接地の節は入らない**（できないことをできると教えない）
- Given `googleSearch: false`、Then その節は入らない

---

## Tasks

- [x] Phase 0 — 契約凍結: `data_contract.yaml` の
      `Provider.values` / `ModelTemplate.googleSearch` / `llm_wire` へ
      Gemini の不変条件を追記
- [x] Phase 1 — コア実装: `llm/gemini.rs` 新規 +
      `Provider::Gemini` + `path()` シグネチャ変更 + `x-goog-api-key` +
      `ModelTemplate.googleSearch` + `types.ts` / `ModelTemplateDialog.vue`。
      テスト 26 本（うち Red→Green 実証: スキーマ削り 5 本）
- [x] Phase 2 — 接地の告知: `compose_system_prompt` に
      `grounded` 引数と接地の節。テスト 1 本
- [x] Phase 2.5 — 接地判定の是正（rev2 査読の指摘 C）: `grounding_active()` 新設。
      フラグ単独で告知していたのを実効プロバイダとの AND に。UI に不整合表示。
      テスト 1 本
- [x] Phase 3 — 台帳整合: README（未実装表の Gemini 行 / 接地の節）/
      failures.md #30・#31
- [x] Phase 4 — 来歴の配線: `AgentMessage.grounding` を新設し、`MessageSent` が
      運ぶ（**専用イベントを立てない** — Notes 10）。オーケストレーターが
      周をまたいで `Grounding::absorb` で畳み、fan-out では先頭の 1 通にだけ
      載せる（トークンと同じ規則）。ChatPanel が吹き出しの外へ検索語と参照元を
      出し、**参照元 0 件のときも欄を消さない**。テスト 8 本
      （Rust 6 / TS 2 ファイル。うち「プロンプトへ戻らない」を 2 ターン走らせて固定）
- [x] 実機確認（2026-07-29、ジェミー / `gemini-3.6-flash`）: 放送中のテレビ番組を
      調べさせた（学習時点より後の事実）。結果は Notes 5 の**「来なかった」側**。
      - `queries` は 2 件届いた（`ファーストクライ 番組` / `ファーストクライ`）
      - **`sources` は空**。参照元 URL は取れない
      - モデルは URL を作らず、**発表元を名前で挙げた**（日本テレビ公式サイト /
        Wikipedia / TVer）。#31 と同じ「出典を出せ」の圧力がかかる場面で、
        Phase 2 の告知が意図どおりに働いた
      - **`groundingMetadata` が来て `groundingChunks` が空だったのか、
        `groundingMetadata` 自体が来ず `toolCall.args.queries` だけから
        検索語が取れたのかは、表示からは判別できない。** 運用上の結論は同じ
        （参照元 URL は出せない）ので追わない。判別が要る場面が来たら、
        decode の直前で生 JSON を一度吐く

---

## Notes（査読論点）

1. **`detect()` を変えない判断**: 技術的には
   `generativelanguage.googleapis.com` → Gemini が自然だが、**動いている
   設定を黙って移す**副作用のほうが重い。明示選択に限る。既存 world.json が
   そのまま読めることをテストで固定した
2. **許可リスト vs 除外リスト**: 除外リストは必ずもう一度落ちる。同じ 400 の
   メッセージの中に既に 2 種類（`additionalProperties` と `$schema`）が
   並んでいたことが、その証拠として残っている
3. **`ChatResponse` に席を置いた理由**: 当初「canonical に不透明な席が無い」と
   判断したが誤りで、`ToolCall.extra` が既にあった。ただし接地の来歴は
   **呼び出し単位ではなくターン単位**なので `extra` には乗らない。
   履歴に積まれない `ChatResponse` を選べば契約を触らずに済む
4. **`toolCall` / `toolResponse` を履歴へ返していない**（未解決）: 現状は
   decode で落としている。「検索 → こちらの関数呼び出し →
   `functionResponse` を返して次の周」の経路が**未通過**で、そこで 400 が
   出るなら `ChatMessage` 側にもターン単位の席が要る。出ないなら現状で足りる。
   **確かめる前に席を作らない**
5. **`groundingMetadata` が実際に来るかは未確認 → 実測で決着（2026-07-29）**:
   型は書いたが実データを見ていなかった。当初「原理的に取れない」と断定したのは
   検証していない推測で、それを一度取り下げて未確認へ戻したのは正しかった。
   実測の結果は**「参照元は返ってこない」側**で、`sources` は空だった。
   よって **Phase 2 の告知だけが処方として残る**（「URL は手元に来ない／
   代わりに検索語と発表元は言える」）。表示層は 0 件を 0 件として書く。
   なお「取れなかった」ことと「原理的に取れない」ことは別で、
   実測は 1 件（日本語の番組名クエリ）。取れる条件が存在する可能性は
   否定していない — 断定へ戻さない
6. **`use_tools: false` のフォールバックを持たない**: ネイティブ経路は
   関数呼び出しを常に解釈できる。Anthropic adapter と同じ扱い
7. **思考トークンを出力に数える**: `completion = candidatesTokenCount +
   thoughtsTokenCount`。実測で `totalTokenCount` と一致する数え方であり、
   課金の実感と揃う
8. **接地の告知を Ordinance に置かない**: 条例は全村人に等しく効くが、
   この制約は接地が有効な者だけのもの。条例に書くと接地していない
   エージェントにまで無関係な制約を負わせる
9. **`sources` をプロンプトへ戻さない（rev2 で決定）**: 当初 Phase 4 に
   「本物の URL があればプロンプトへ戻す」と書いたが、**時系列が成立しない**。
   接地はそのターンの中で起き、`sources` は**答えと同時に**返る。次ターンの
   プロンプトへ入れても、それは前の話題の出典であり、モデルが今まさに
   引用したい相手ではない。前ターンの URL を現ターンの根拠として見せるのは
   新種の誤帰属で、**捏造を別の形に置き換えるだけ**。
   ゆえに `sources` の行き先は**表示層**（発話に添えて利用者へ見せる）とし、
   モデルへは返さない。これで Phase 2 の告知（URL は手元に来ない）と
   矛盾しなくなり、`ChatMessage` 側の席も不要なままで済む
10. **来歴に専用イベントを立てない（Phase 4 で決定）**: `CoreEvent::Grounded`
    のような変種を足さず、`AgentMessage.grounding` に載せて `MessageSent` に
    相乗りさせた。別便にすると (a) フロントが発話 ID との対応を自前で持つ必要が
    あり、(b) 起動時の `list_messages` による再投影で**来歴だけが消える**
    （イベントは再生されない）。発話に載せればどちらも起きない。
    `AgentMessage` は履歴・広場ログの原料でもあるが、プロンプトを組む経路は
    `content` しか読まないので、フィールドを増やしても発話の中身は変わらない。
    この不変条件は結合テスト `grounding_never_returns_to_the_prompt` で固定した
    （2 ターン走らせ、全リクエストの本文に URL と検索語が現れないことを確認する）

---

### rev1 → rev2 の査読反映記録（2026-07-29 査読）

**採用（4 件）**

- **指摘 C（最重要・実バグ）**: `world.json` 直編集の
  `provider: null + googleSearch: true` に対するガードが無かった。UI はチェックを
  隠すのに**システムプロンプトには接地の節が入り**、検索できないモデルに
  「検索できます」と教えていた。`grounding_active()` を新設して是正（Phase 2.5）。
  告知は「持っていない情報を埋める」処方なので、そこで嘘をつくと処方が毒になる
- **指摘 2（前半）**: `ChatResponse` に置く設計と「プロンプトへ戻す」が衝突。
  Phase 4 の当該項を削除し、根拠を Notes 9 に明記
- **指摘 4（前半）**: 実装先行がプロセス宣言と矛盾。Status 冒頭に例外である旨と
  その理由（実挙動が実測でしか決まらず、机上凍結なら 4 回とも誤った契約を
  凍結していた）を明記
- **指摘 3**: `includeServerSideToolInvocations` の条件記述が曖昧。送出条件を
  「`googleSearch` が真のときだけ」と明示し、関数の有無で分けない理由も併記

**不採用（3 件・前提が実装または機序と異なる）**

- **指摘 1**: 「`toolCall` を一律で落とすと連鎖が不可能／`queries` が取れない」。
  どちらも成立しない。(a) 検索は**応答が返る前に Google 側で完了している** —
  実測の生 JSON で `toolCall` → `toolResponse` → `functionCall` が 1 応答に
  並んでおり、こちらが実行しないことは連鎖の妨げにならない。(b) `queries` は
  `toolCall.args.queries` から実際に抽出しており、テスト
  `server_side_search_queries_survive_without_grounding_metadata` で固定済み。
  ただし両立することが読み取りにくかったので Acceptance の文面を補強した
- **指摘 A**: 「`stable_len` にテンプレート由来の節を入れるとキャッシュキーが割れる」。
  `stable_len` は `compose_system_prompt` が**エージェントごとに**算出する値で、
  ワールド共通の不変プレフィックスではない。作業フォルダの有無や Construct /
  Skill の内容で既にエージェントごとに違う。定義の明記は妥当なので Acceptance に追記
- **指摘 B**: 「接地単独の `STOP` + `groundingMetadata` を継続扱いにしないと
  検索結果が捨てられる」。捨てられない。接地単独のとき、モデルは検索結果を
  **織り込んだ最終本文**を返しており、`Finish::Stop` が正しい。継続扱いにすると
  無意味な往復が 1 周増える。`grounding` は `ChatResponse` に保持済み。
  実機（ジェミー）でも `STOP` + 本文 + 接地で完結した回答が得られている
- **指摘 4（後半）**: 「`path()` のシグネチャ変更が『既存テンプレートは互換のまま』
  という Acceptance と矛盾する」。矛盾しない。`&'static str` → `String` は
  Rust の内部 API の変更で、既存 2 プロバイダが返す**文字列は同一**。
  Acceptance が縛るのは実行時の振る舞いで、そちらは保たれている
  （テスト `model_template_saved_before_google_search_still_loads` で固定）
