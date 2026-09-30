# Spec 63: AI による下書き補助（SKILL.md / Construct.md / judge.toml）

- 状態: **rev3 → P0〜P1 完了**（P1 = コア。記録は末尾の「P1 実装記録」）。以下は P0 時点の記述 —
  **rev3 → P0 完了**（2026-09-30 起票 → 同日、査読 2 系統 15 点（重複を畳んで 13 項目）を反映して rev2 →
  P0 の測定を反映して rev3 → `data_contract.yaml` の `assist_contract` と `entities` を凍結。rev3 の差分は D2 / D5 / D10 と「P0 実測」）
- 起点: 利用者（2026-09-30）—「SKILL.md や Construct.md、条例、判断特化の設定を AI 自動生成で書き込む
  仕組みをつけたい。世間一般の skill-creator などを参考に、AI に生成補助させたい。『〜をするスキルを
  作りたい』と書くだけで、要件のヒアリングから SKILL.md のドラフト生成までを自動で行ってくれるイメージ」
- 利用者裁定（同日）:
  1. **生成に使うモデルはダイアログで毎回テンプレートを選ぶ**（前回の選択は端末に覚える）
  2. **範囲は SKILL.md・Construct.md・判断役の `judge.toml` の 3 つ** —「たぶん判断特化が一番書くのが
     難易度が高いから」。条例は範囲外
  3. **下書きまで。** 生成物を試しに走らせて比べる評価の輪は入れない

## rev1 からの変更（査読の反映。詳細は Notes 2）

- **会話の履歴はコアが作ったメッセージを逐語で往復する**（D3・D4）。rev1 の `AssistTurn = { role, content }` では
  `submit_draft` の呼び出しが履歴から消え、2 回目以降の「もっと短く」で生成役が自分の下書きを見られず、
  しかも「前回は平文で出した」と読んで次も平文で出す（= 質問として扱われる）形になっていた
- **`AssistReply` を組み直した** — `notes` の欄 / 検査の結果を判別共用体 1 つに / `chars` を `draftChars` に揃える
- **「下書きを出して」はボタンの型で運ぶ**（`forceDraft`。D5）。文言の一致に頼らない
- **使用量の置き場を 1 つ決めた**（D10）— `Record::Assist` を開いている会話へ書き、統計画面の行と合計に入れる。
  個体の累計と村の予算には入れない。rev1 の未決 1 は閉じた
- **ログの `outcome` を分けた**（D10）— 上限に達して返した不正な下書きは `draft_invalid`
- **`current` はフロント由来の唯一の文字列として区別して渡す**（D7）
- **MCP はツール名まで渡す**（接続している個体のとき。D7）/ **言語は利用者の明示の指定を優先**（D12）/
  **検証の輪の異常系を定義**（D8）/ **テンプレートの選択は「選べない」に統一**（D2）/ `AssistTarget` の欄名を
  `id` に統一（D4）/ 反映の前に字数と行数の差を出す（D9）

## P0 実測（2026-09-30。**こちらが正**）

村の `world.json` のテンプレートと OS の資格情報ストアの鍵で、**実物のアダプタ**（`HttpBackendFactory::strict`）から
撃った。プローブは `crates/fuseforks-core/examples/` の使い捨て（コミットしない。数字はここが正）。
依頼は「天気予報を調べて 3 行で要約するスキルを作りたい。質問はせず下書きを出して」、2 周目は応答を
**`serde_json` で文字列にして戻した** `assistant` + `tool` の 2 通 + 「もっと短く、2 行に」。
**予測を先に書いた**（Anthropic は 400 / Meta は強制したつもりで auto / 他の 5 ワイヤは通る / 往復は全ワイヤ通る）
— **4 つとも当たった**。

| ワイヤ（テンプレート） | (A) `Specific("submit_draft")` | (B) 逐語往復の 2 周目 |
|---|---|---|
| OpenAI 互換（Gemini 3.8 Flash を `/v1beta/openai` で） | ✓ 下書き 1,090 字 | ✓ `json_equal=true`・`extra` あり（思考署名）・1,090 → 516 字 |
| **Anthropic**（claude-opus-5-5） | **✗ 400** `tool_choice: type "tool" and "any" are not supported for this model.` | ✓（auto で下書き → 2 周目 1,081 → 807 字） |
| Gemini ネイティブ（3.8 Flash） | ✓ 808 字・`extra` あり | ✓ 808 → 572 字 |
| xAI Responses（grok-4.7） | ✓ 804 字 | ✓ 804 → 117 字 |
| OpenAI Responses（gpt-6-sol） | ✓ 508 字 | ✓ 508 → 70 字 |
| **Meta Responses**（muse-spark-1.3） | **✓ に見えるが強制ではない** — アダプタは `tool_choice` を送らない（Spec 37:`auto` のみ受理）。指針の文だけで呼んだ | ✓ 516 → 326 字 |
| Perplexity Responses（`perplexity/sonar` / `openai/gpt-5-mini`） | ✓ 681 字 / 2,677 字 | ✓ 681 → 291 / 2,677 → 2,185 字 |

- **2 周目は全ワイヤで通った。** JSON を往復した `ChatMessage` は元と `==`（`extra` = 思考署名も含めて）で、
  2 周目も 7 ワイヤすべてが `submit_draft` を呼んで下書きを縮めた（平文に落ちた例は 0）
- **強制を受け付けないのは Anthropic（5 世代）と Meta の 2 つ。** 本番のコードで `Specific` / `Required` を送る
  経路は今まで 1 本も無い（`orchestrator` の grep は 0 件。`emit_plan` はテストの fixture だけ）ので、
  **`forceDraft` が本番で初めて強制を送る経路になる**。D5 のフォールバックの条件はワイヤで決める
- **固有スキルがテンプレートから付いてくる。** 1 回目の gpt-6-sol は `prompt=11854`（`openaiWebSearch` と
  `openaiReasoningPro` が ON）で、**外すと 170**。grok-4.7 は 3,008 → 1,452（`xaiWebSearch` / `xaiXSearch`）。
  **生成役からは固有スキルを外す**（D2）
- 条件側で撃てなかったもの（測定の外）: `claude_sonnet` / `claude_fable5` / `cloud_opus_5` の鍵は無効（401）/
  `perplexity` テンプレートのモデル `perplexity/deepseek-v4-flash-0731` は Perplexity 側が受け付けない
  （`model ... is not supported`）/ gpt-6-sol を OpenAI 互換で撃つと `max_tokens` が拒否される —
  `uses_max_completion_tokens` が `gpt-5` と o 系の名前しか見ていない（**別件**。村の実運用は Responses ワイヤ）

## Goal

1. 編集画面から「AI で作成」を開き、**作りたいものを 1 行書くだけで**、ヒアリング → 下書きまで進む
2. **下書きは編集中の本文へ流し込むだけで、保存は人が既存の保存ボタンで押す**（書き込みの経路は増えない）
3. 判断役は、**コアのパーサーを通った下書きだけを「通った」として見せる**。落ちたら生成役へ理由を返して
   直させる
4. 払ったトークンは**統計画面の合計に入り、`assist:` 行にも出る**（`failures.md` #50 / #103 を新しい経路で
   開け直さない。置き場は D10 の 1 つ）

## 前提の実測（2026-09-30）

- **Fuseforks の SKILL.md は「呼び出されるスキル」ではない。** `compose_system_prompt`
  （`config_store.rs`）が 条例 → あなたについて → … → Construct → Skill → 顔ぶれ → Memory の順で
  **毎ターン全文を連結する**。発火条件も前付け（frontmatter）も読まない（`frontmatter` の grep は 0 件）。
  skill-creator の指南のうち `description` を強めに書く / 500 行まで / `references/` へ逃がす、は
  「必要なときだけ読み込まれる」前提から来ているので**この村には当たらない**。書いた分だけ全ターンの
  固定費になる（ただし安定プレフィックスなのでキャッシュは効く）
- **SKILL / Construct は `agents/<id>/`、判断役は `judges/<id>/` にあり、どちらも work_dir の外。**
  `file` ツールの囲い（`resolve_in_work_dir`）の外なので、サーヴァントは今これらを書けない
- **判断役の保存時検査は `save_judge_file`（`orchestrator/judging.rs`）の 2 段** —
  `JudgeFile::parse`（文法・型・定義域。失敗は `location` + `message`）→ `missing_targets`
  （サーヴァントでない ID）。検証の輪はこの 2 段をそのまま使える
- **ターンループの外で LLM を呼ぶ前例は `summarize_agents`**（`orchestrator/mod.rs`）—
  `backend_for(&template)` で組み、`backend.chat` を 1 回呼び、使用量を個体の累計へ積み、`summarize:` 行を出す
- **`ChatMessage` と `ToolCall` は `Serialize` / `Deserialize` を持つ**（`llm/canonical.rs`）。`ToolCall::extra` は
  プロバイダ固有の不透明な随伴データ（Gemini の思考署名）で、**欠くと会話の 2 周目が 400 で落ちる**。
  ターンループはこの `ChatMessage` を周回の間で逐語に積み直して動いている
- **`McpServerStatus` は接続中なら `tools`（修飾後のツール名）を持つ**（`mcp.rs`）。未接続なら名前だけ
- `ModelTemplate.use_tools` が偽のテンプレートはツールを送らない。`ToolChoice::Specific` は「構造化出力の主経路」
  として既にあり、ターンループのまとめ呼び出しが使っている
- 編集部品: SKILL / Construct は `MarkdownEditor.vue`（タブで種別を切り替え、`readConfig` / `writeConfig`）、
  判断役は `JudgeDialog.vue`（`readJudgeFile` / `saveJudgeFile` / 「試す」= `tryJudge`）。
  どちらも `CodeEditor` を使う
- `sessions.redb` の `Record` に variant を足すと、**旧い版は新しい村の `records()` で落ちる**（Spec 39 Notes 6 の
  互換の向き (b)。`summary` / `turn` を足したときと同じ位置づけ）

## Design

### D1. 入口は 2 部品・3 箇所。村の会話からは頼まない

- `MarkdownEditor.vue` の **SKILL と Construct のタブ**（`editable` のときだけ）と、`JudgeDialog.vue` に
  「AI で作成」ボタン。**Memory / mcp.json / run.json には出さない**（Memory は本人が `remember` で書く
  もの、あとの 2 つは許可の設定で、生成させる種類の文ではない）
- **村の会話（サーヴァントへの依頼）から作らせる形は採らない。** 理由は 3 つ:
  - 書き込み先が `file` の囲いの外にある。サーヴァントに書かせるには囲いを開ける必要がある
  - 開けると、ある個体が別の個体の Construct や判断役を書き換えられる経路になる（外部研究 2607.29167 の
    「永続記憶の汚染」と同じ形。`api_key_env` と同じく注意書きは制御ではない）
  - 頼んだ相手の人格・口調・Memory が下書きに写る

### D2. 生成役はダイアログで選ぶテンプレート（利用者裁定 1）。サーヴァントではない

- ダイアログ上部でモデルテンプレートを選ぶ。**選べるのは `use_tools` が真のものだけ。** 偽のものは一覧に
  並べるが `disabled` にし、理由（ツールを使わない設定なので下書きを受け取れない — D5）を添える
- 前回の選択は `localStorage` の `fuseforks.assist.v1`（`{ templateId }`）に覚える。一覧に無い id・`use_tools` が
  偽になった id は捨てる
- 生成役は**人格・履歴・Memory・村のシステムプロンプトを持たない**。渡すのは D6 の指針と D7 の文脈だけ
- **テンプレートの固有スキル（検索・pro モード・URL 取得など）はすべて外して呼ぶ**（P0 実測 — gpt-6-sol は
  外すだけで入力が 11,854 → 170 トークン）。生成役は調べものをしない（文脈は D7 でコアが渡す）。
  思考段階（`effort`）はテンプレートのまま使う — 強いモデルを選ぶ理由の側
- **村の予算（`tokenBudget`）と個体の累計には入れない。** 村の仕事の因果ではなく人が押した操作で、判断役には
  そもそも個体の累計が無い。使用量の置き場は D10 の 1 つ

### D3. 会話は使い捨て。履歴はコアが作ったメッセージを逐語で往復する

- ヒアリングの会話はダイアログの `ref` に持つ。**`sessions.redb` の会話にも広場ログにも残さない**
  （使用量の数字だけは D10 で残す）。閉じると消える（会話があれば閉じる前に確認を出す）
- **生成役の発話（`submit_draft` の呼び出しを含む）と、それに対するツール結果はコアが作る。** コアは作った
  メッセージを `appended` として返し、フロントは**中身を解釈せず逐語で保持して次の呼び出しで送り返す**。
  フロントが作るのは利用者の発話だけ
  - 理由: 下書きを出した後の「もっと短く」で、生成役が**自分が `submit_draft` で出した下書き**を見られる
    必要がある。平文に直して渡すと「前回は平文で下書きを書いた」と読んで次も平文で出し、D5 で質問として
    扱われる。`ToolCall::extra`（Gemini の思考署名）も落ちると 2 周目が 400 で落ちる
  - ターンループが周回の間でやっていること（`ChatMessage` を逐語で積み直す）を、IPC をまたいでやるだけ
- **下書きの呼び出しには、コアがツール結果を必ず対で付ける**（検査に通ったら「下書きを利用者に見せました。
  以後の依頼に従って直してください」、落ちたら検査の理由。村の言語で書く）。対が無いとプロバイダが拒否する
- コアは受け取った履歴の**形だけ**を検査する（役割が `user` / `assistant` / `tool` のどれか / 最後が `user` /
  ツール結果は直前の呼び出しと対になっている）。崩れていれば送らずに `CoreError` を返す
- 指針 + 文脈は呼び出しの間で変わらないので `cacheable_prefix_len` に載せる（2 回目以降はキャッシュが効く）

### D4. IPC `assist_draft`

```ts
type AssistTarget = { kind: "skill" | "construct" | "judge"; id: AgentId };

/** コアが作ったメッセージ。フロントは解釈しない（逐語で送り返すだけ）。 */
type AssistMessage = unknown;

assistDraft(req: {
  target: AssistTarget;
  templateId: ModelTemplateId;
  history: AssistMessage[];   // これまでの利用者の発話 + appended を順に
  input: string | null;       // 今回の利用者の発話。forceDraft のときは null でよい
  forceDraft: boolean;        // 「下書きを出して」ボタン（D5）
  current: string;            // 編集中の本文（未保存の変更を含む。D7）
}): Promise<AssistReply>;

type AssistReply = {
  appended: AssistMessage[];  // 今回の利用者の発話 + コアが作ったメッセージ。history の末尾へ足す
} & (
  | { type: "question"; text: string }
  | { type: "draft";
      text: string | null;          // 生成役が下書きに添えた本文
      content: string;              // 下書き
      notes: string | null;         // submit_draft の notes（仮定・未確認点）
      draftChars: number;           // content のコードポイント数
      validation: null              // skill / construct（検査しない）
        | { valid: true }
        | { valid: false; location: string; message: string };
      attempts: number }            // 検証の輪を回した回数（skill / construct は 1）
);
```

- `appended` に利用者の発話まで入れるのは、フロントが「自分で足した発話」と「コアの応答」を別々に積む
  必要を無くすため（積み順の取り違えを作らない）
- 文脈（D7）は**コアが対象の id から集める**。例外は `current` と利用者の発話だけ（D7）
- 失敗は `CoreError`（テンプレートが無い / `use_tools` が偽 / 対象が無い / 履歴の形が崩れている /
  LLM の失敗で、まだ下書きが 1 つも無い）

### D5. 質問と下書きは型で分ける

- 生成役に**ツール `submit_draft { content: string, notes?: string }` を 1 本だけ**提示する
  - ツール呼び出しがあれば**下書き**。`content` が本文、`notes` は仮定・未確認点（画面に出す）
  - 呼び出しが無く本文があれば**質問**
  - 両方あれば下書き（本文は `text` として画面に添える）
  - どちらも無ければ `empty`（D8 と同じ扱い）
- 本文の中の目印（```` ```draft ```` など）で分けない。Spec 08 の凍結「分類は文言 parse でなく型で運ぶ」と同じ理由
- **`forceDraft: false` のとき `tool_choice` は auto。`true` のときは `Specific("submit_draft")` で必ず下書きにする。**
  あわせてコアが**定型の利用者発話**（「ここまでの情報で下書きを出してください。分からない点は仮定として
  notes に書いてください」。村の言語）を足す。ボタンの意味を型で運ぶので、文言の一致に頼らず言語にも依らない
  - **強制を送らないワイヤは Anthropic と Meta の 2 つ**（P0 実測 — Anthropic は 400、Meta はアダプタが
    `tool_choice` を送らない）。この 2 つでは auto + 定型発話で呼び、`assist:` 行に `forced=fallback` と出す。
    判定は `Provider` の述語 1 本（ワイヤで決める。モデル名では決めない）
  - フォールバックでも下書きが返らなかった（生成役が質問を返した）ときは `type: "question"` で返す — 強制できない
    ワイヤで「必ず下書き」を約束しない。P0 では 2 つとも定型の依頼で下書きを返した
- 下書きの後も会話は続けられる（「もっと短く」→ 新しい下書き）

### D6. 指針（生成役のシステムプロンプト）— 種類ごとに Fuseforks 用に書く

skill-creator の**手順**（意図の確認 → 質問 → 下書き → 直し）は借りるが、**本文は写さず自前で書く**
（書き方の前提が違う — 前提の実測の 1 点目）。コアの定数で、ja / en の 2 本ずつ（Spec 35 の規律 —
英語は翻訳ではなく英語で書く）。

**共通（ヒアリングの規律）**:
- 最初の発話から分かることは訊かない。**1 回に訊くのは 3 つまで**
- 足りたと判断したら `submit_draft` を呼ぶ。**下書きは必ず `submit_draft` で出し、本文に書かない**
- 既に本文がある（`current` が空でない）なら、**書き直しではなく改稿として扱う**（残す部分を残す）
- 下書きの言語は村の言語。**利用者が会話の中で別の言語を指定したらそれに従う**（D12）

**SKILL.md**:
- **毎ターン全文が読まれる。発火条件・`description`・前付けは書かない**。長さはそのまま毎ターンの費用
- 手順を命令形で。**名前が渡されたツールだけを名指しし、持っていないツールを前提にしない。**
  MCP のサーバー名だけが渡されている（未接続の）ときは、**関数名を推測せず**サーバーの用途として書く
- 個体の人格や口調は書かない（Construct の役割）。対になる Construct と重複させない

**Construct.md**:
- 人格・口調・役割の範囲・しないこと。**役職のラベル（「調査役です」）に寄せず、振る舞いで書く**
  （Spec 14 D6 — 役職名は本人に見せない。ラベルの含意に人格が引きずられる）
- 「他のエージェントの発言を代筆しない」はコアが既に書いているので繰り返さない
- 手順は書かない（SKILL の役割）。対になる SKILL と重複させない

**judge.toml**:
- 文法の正は**コアのパーサー**。指針には Spec 62 D3（ファイルの形）・D4（条件式の文法と型の表）の要約と、
  `starter_template(language)` を例として載せる
- **1 問 1 論点**（Kataribe Spec 32 / Fuseforks Spec 60 の実測 — 論点を 2 つ持つ問いは 2 つ目が沈む）
- Choice に「その他」を置き、`otherwise` より前に `kind == other` の規則を置く（Spec 62 D3 の勧め）
- `to` には渡した一覧の ID だけを書く
- 文法を説明しても外すことは前提にする。**通るまで直すのは D8 の輪の仕事**で、指針を長くして防ぐ形にしない

### D7. 生成役に渡す文脈（コアが集める・種類ごとに閉じた範囲）

| 種類 | 渡すもの | 渡さないもの |
|---|---|---|
| skill / construct | 個体の表示名 / 有効な同梱ツールの名前 / MCP — **接続中のサーバーはツール名まで、未接続は名前だけ** / 接続先の表示名 / 村の言語 / **対になるファイルの保存済み本文**（SKILL なら Construct、逆も） | Memory.md / 条例 / 会話ログ / 役職名 |
| judge | 判断役の名前 / **接続先に選べるサーヴァントの ID・表示名・役職名** / 村の言語 | 各サーヴァントの Construct・SKILL / 条例 / 会話ログ |

- **プロンプトの中で 2 つの区画に分ける** — 「村の事実（コアが集めたもの）」と「利用者が書いたもの」。
  `current`（編集中の本文。未保存の変更を含む）は**フロントから受け取る唯一の本文**で、後者の区画に
  「利用者が編集中の本文（未保存）」と見出しを付けて入れる。利用者の発話も同じ区画の側
  - `current` をコアの保存済み本文で代えない理由: 利用者が直しかけの本文を元に頼むことがある
  - 利用者の手元の文字列なので、区画を分けるのは信頼のためではなく**生成役が「村の事実」と取り違えない**ため
- 対になるファイルを渡すのは重複を避けるため（重複は毎ターンの固定費になる）
- 役職名を skill / construct に渡さないのは D6 と同じ理由。judge に渡すのは、ラベルを読むのが人と生成役で、
  その個体自身ではないから

### D8. 判断役の検証の輪と、異常系

- `submit_draft` の `content` を、**`save_judge_file` と同じ検査**（`JudgeFile::parse` → `missing_targets`）に通す。
  **検査は関数 1 本に切り出して保存と下書きで共有する**（2 箇所に書くと、下書きでは通るのに保存で落ちる形が生まれる）
- 落ちたら、同じ IPC 呼び出しの中で `tool_result` に `location` と `message` を返して作り直させる。
  **上限は 3 回**（コードの定数。`attempts` に回数を返す）
- 通れば `validation: { valid: true }`。3 回目も落ちたら、最後の下書きを `{ valid: false, … }` で返し、
  画面にそのまま出す（反映はできるが、保存は既存の検査が拒否する）
- **輪の途中の異常系**:
  - **生成役が下書きではなく質問を返した** → `type: "question"` で返して会話へ戻す。それまでの不正な下書きと
    検査の理由は `appended` に残る（次の下書きで生成役が見られる）
  - **LLM の失敗 / 空の応答** → それまでに下書きがあれば、**最後の下書きを `{ valid: false, … }` で返す**
    （利用者が見た下書きを失わせない）。1 つも無ければ `CoreError`
  - どちらも `assist:` 行に 1 行出る（D10）
- skill / construct は検査しない（Markdown に文法は無い）。`validation: null`・`attempts: 1`

### D9. 反映 — 下書きを編集中の本文へ。保存はしない

- 右側に下書きのプレビュー（原文のまま。`MarkdownEditor` と同じく描画しない）と `notes`、判断役なら検査の結果
- **反映の前に、編集中の本文との差を出す** — 字数（`current` → `draftChars`）と、増えた行・消えた行の数。
  SKILL / Construct は字数がそのまま毎ターンの費用なので、ここで見せる（全文の差分表示は未決 1）
- 「エディタへ反映」で編集中の本文を置き換える。**編集中の本文が空でなければ確認を出す**
- 反映の直前の本文を 1 つ持ち、「反映を取り消す」で戻せる
- **保存は既存の保存ボタン / `Ctrl+S`**。書き込みの経路は 1 本も増えない。判断役は反映の後に既存の「試す」で確かめられる

### D10. 計器と使用量の置き場（Goal 4）

**使用量の置き場は `Record::Assist` の 1 つ。** 統計画面はそこから読み、ログ行は同じ値から書く。

- **`Record::Assist`**（`sessions.redb` の `Record` に variant を 1 つ足す）— LLM 呼び出し 1 回ごとに 1 件、
  **開いている会話**へ書く。欄は `ts_ms` / `draftKind`（`kind` は `Record` の種別タグと衝突する — P1 実装記録）/ `model` / `templateId` / `prompt` / `cacheRead` / `cacheWrite` /
  `cacheWrite1h` / `completion` / `reasoning` / `outcome`。**本文・会話・下書きは入れない**
  - 統計画面では個体の行とは別に「AI 作成補助（モデル）」の行として出し、合計と `≈ $` に入れる
    （単価の当て方は `Record::Turn` と同じ `pricing.rs`）
  - **`StatsReport` の既存の欄の意味は変えない**（`totals` はターンだけ・`turns` は数え方が違う）。
    足すのは `assist: { rows: [{ model, templateId, ...Slice }], total: Slice }` の 1 欄（加算。`#[serde(default)]`。
    **`Slice.turns` は LLM 呼び出しの回数**）。スコープと期間の規則は `turn` と同じ
    （`session` はその会話、`all` は `tsMs` の半開区間）。**画面の総計と `≈ $` は `totals + assist.total`** で組む
  - 履歴の入力にはならない（`restore_histories` / `tail_messages` / `fork_points` は読まない — `Turn` と同じ）
  - 会話を開いていない（保存先が開けない）ときは書けない。そのときはログ行だけになり、WARN を 1 行出す
  - **互換**: 旧い版は `kind: "assist"` で `records()` が落ちる（前提の実測の最後の点。`turn` と同じ位置づけ）
- **`assist:` 行**（LLM 呼び出し 1 回ごと）:
  `assist: kind=judge model=… attempt=2 forced=no outcome=invalid prompt=… cached=… total=… reasoning=… current_chars=… draft_chars=…`
  - `outcome` は `question` / `draft`（検査なし、または通った）/ `invalid`（検査に落ちて作り直す）/
    `draft_invalid`（上限に達した、または輪の途中で失敗して、不正な下書きを返した）/ `empty` / `failed`
  - `forced` は `no` / `yes` / `fallback`（D5）
  - **本文・会話・下書きはログに書かない**（字数だけ。`failures.md` #71 の線）
- 失敗でも使用量が分かるもの（`LlmError::usage()` が `Some`）は数字を出し、`Record::Assist` にも書く（#103 の処方と同じ）
- 個体の累計・村の予算には入れない（D2）

### D11. 外へ送るもの（PRIVACY）

- 送り先は**利用者が選んだテンプレートの接続先だけ**。新しい送り先は増えない
- ただし送る中身は増える: ヒアリングの会話 / **編集中の本文（`current`。未保存の変更を含む）** / D7 の文脈
  （対になるファイルの本文、サーヴァントの名前、MCP のツール名など）。**押したときだけ送る**
  （ダイアログを開いただけでは送らない）
- `PRIVACY.md` 日英の 4-1 へ追記する

### D12. 言語

指針の言語と、コアが足す定型文（D3 のツール結果・D5 の定型発話）は村の `language`。**下書きの言語も既定は
村の言語だが、利用者が会話の中で別の言語を指定したらそれに従う**（D6 の共通の規律）。英語で運用したい個体の
SKILL を日本語の村で書く、は正当な使い方（Spec 35 P5 — 偏りの操縦は村の中身の言語に依る）。

## 採らなかった形

- **村の会話から頼んでサーヴァントに書かせる** — D1
- **生成物を自動で保存する** — 人が見ずに毎ターンのプロンプトが変わる。書き込み経路が増える
- **対象のサーヴァント本人のモデルで書く**（手動要約の「本人が書く」）— 生成には強いモデル、運用は安いモデル、
  という分け方ができなくなる（利用者裁定 1）
- **評価の輪**（テスト用の依頼を実際に走らせて比べる）— 利用者裁定 3。この村には評価の基盤が無い。
  判断役は既存の「試す」で代用できる
- **skill-creator の書き方の指南をそのまま渡す** — 前提の実測の 1 点目
- **本文の目印で質問と下書きを分ける** — D5
- **会話の履歴をフロントが平文で組み立てる**（rev1）— D3
- **「下書きを出して」を文言の一致で判定する** — D5
- **使用量を対象の個体の累計へ積む**（手動要約の前例）— 判断役には個体の累計が無く、SKILL の作成に使った
  強いモデルの払いがその個体の運用の数字に混ざる
- **条例・役職の Construct（`RoleDialog`）** — 利用者裁定 2 の範囲外。同じ部品で後から足せる形にしておく

## Tasks

### P0 — 測ってから凍結する（**2026-09-30 完了。上の「P0 実測」が正**）
- **`Specific("submit_draft")` を 7 ワイヤ（OpenAI 互換 / Anthropic / Gemini / xAI / OpenAI / Meta / Perplexity の
  Responses）に撃つ** — 受け付けるか、思考との併用で 400 になるか。受け付けない組み合わせを D5 の
  フォールバックの条件として凍結する
- **`appended` の逐語往復で 2 周目が通るか** — Gemini（思考署名）と Responses 系 1 本で、下書き → 利用者の
  発話 → 再下書き を撃つ
- `data_contract.yaml` へ `assist_contract`（D1〜D12 の凍結: 書き込み経路を増やさない / 型で分ける /
  逐語で往復 / 検査の共有 / 上限 3 / 文脈の閉じた範囲と 2 区画 / ログとレコードに本文を書かない /
  予算と個体の累計に入れない / 置き場は `Record::Assist` の 1 つ）と `entities` の `AssistTarget` / `AssistReply` /
  `Record` の `assist`

### P1 — コア
- `crates/fuseforks-core/src/assist.rs`（純機構）: 指針 6 本（3 種 × ja/en）/ 文脈の組み立て（2 区画）/
  `submit_draft` の ToolSpec / 応答の振り分け / 履歴の形の検査
- `orchestrator/assist.rs`: `assist_draft` — 文脈の収集・呼び出し・判断役の検証の輪と異常系・`Record::Assist`・`assist:` 行
- 判断役の検査を `save_judge_file` から関数へ切り出して共有
- `session_store.rs`: `Record::Assist`（履歴の読み手 3 つは読まない）/ 統計の集計へ行を足す
- テスト: 振り分け（質問 / 下書き / 両方 / 空）・逐語往復（`extra` を持つ呼び出しが 2 周目にそのまま送られる）・
  履歴の形の検査・検証の輪（スタブが 1 回目に不正・2 回目に正 → `attempts=2 valid=true` / 3 回とも不正 →
  `valid=false` / 2 回目に質問 → `question` で `appended` に不正な下書きが残る / 2 回目に失敗 → 1 回目の下書きを
  `valid=false` で返す）・`forceDraft` で `tool_choice` が変わり定型発話が足される・`use_tools` 偽の拒否・
  文脈に Memory と条例が入らないこと・ログとレコードに本文が出ないこと

### P2 — 画面
- IPC `assist_draft` と型 / `AssistPanel.vue`（左に会話、右に下書き。`MarkdownEditor` と `JudgeDialog` から開く）/
  テンプレートの選択と `fuseforks.assist.v1` / 「下書きを出して」ボタン / 反映前の差（字数と行数）/ 反映と取り消し /
  閉じるときの確認 / 統計画面の「AI 作成補助」の行 / 辞書 ja/en（`errors.INVALID_ASSIST_REQUEST` を含む — P1 で足したコード。
  コードと辞書を突き合わせるテストは無く、欠けると原文のまま出る）
- 走査テスト: 入口が SKILL / Construct / 判断役の 3 箇所だけにあること / フロントが `appended` の中身を読まないこと

### P3 — 台帳
- DETAIL 3 言語 / README 3 言語に 1 行（既定で動かず、使うと外部へ送る機能は入口で読めるべき、の判断）/ PRIVACY 日英

### P4 — 実機
1. SKILL: 「〜をするスキルを作りたい」の 1 行 → 質問が返る（`assist: outcome=question`）→ 答える →
   下書き（`outcome=draft`）→ 反映 → 保存 → 次のターンでその個体の `system_digest` が変わる
2. 「下書きを出して」を最初の質問の直後に押す → `forced=yes`（または `fallback`）で、`notes` に仮定が書かれた下書き
3. 下書きの後に「もっと短く」→ 前の下書きを元に短くなった下書き（逐語往復が効いている。`draft_chars` が減る）
4. 判断役: 「調査と実装で振り分けたい」→ `valid: true` の下書き → 反映 → 「試す」で規則に当たる → 保存できる
5. 統計画面に「AI 作成補助」の行が出て、`assist:` 行の合計と一致する
6. `use_tools` が偽のテンプレートは選べない / 会話を残して閉じると確認が出て、閉じた後は何も保存されていない

検証の輪（D8）が 2 回目で通る形と異常系は実機で狙って出せないので、P1 の結合テストへ預ける。

## 未決

1. 反映のときに全文の差分を見せるか（rev2 は字数と行数の差 + 置き換え + 取り消し 1 段）
2. 条例と役職の Construct への拡張の時期

## Notes

### 1. skill-creator との対照

| skill-creator | この Spec |
|---|---|
| Capture Intent（何を・いつ・出力形式・テストするか） | D6 共通の規律。「いつ」は訊かない（常に読まれる） |
| Interview and Research（MCP で調べる） | 調べない。文脈は D7 でコアが渡す |
| Write the SKILL.md（name / description / 本文） | 本文だけ。`description` と前付けは書かない |
| Progressive Disclosure / references/ | 無い（常に全文が読まれる） |
| Test Cases → 評価 → 反復 → description 最適化 | 採らない（利用者裁定 3）。判断役だけ機械の検査の輪（D8）と既存の「試す」 |

### 2. rev1 の査読の反映（2026-09-30。2 系統 15 点 → 重複を畳んで 13 項目 → 採用 9 / 採用して形を変えた 4）

| # | 指摘 | 系統 | 判定 | 反映 |
|---|---|---|---|---|
| 1 | `AssistTurn` に `submit_draft` の呼び出しが入らない | 1・2 | **採用して形を変えた** — 査読案はフロントの型を広げてツール呼び出しを表現する形だったが、それでは `ToolCall::extra`（Gemini の思考署名）と Responses 系の随伴データをフロントが組み立てることになる。**コアが作ったメッセージを不透明なまま逐語で往復する**形にした | D3 / D4 |
| 2 | `notes` の置き場が無い / `chars` と `draft_chars` の不揃い | 1・2 | 採用 | D4 |
| 3 | 「だけを並べる」と「灰色で出す」が排他 | 1 | 採用 | D2 |
| 4 | Goal 4 に対して置き場がログ 1 行だけ / 判断役に個体の累計が無い | 1・2 | **採用して形を変えた** — 査読 1 の「個体の別カウンタ」は判断役に当てはまらない。`Record::Assist` の 1 つに置き、統計画面の行と合計に入れる。rev1 の未決 1 を閉じた | D10 / Goal 4 |
| 5 | 3 回失敗して返したときログが `invalid` で区別できない | 1 | 採用 | D10 |
| 6 | `current` がフロント由来で D4 の原則と矛盾 | 1 | 採用 — 2 区画に分けて名指しする。ただし区画を分ける理由は信頼ではなく取り違えの防止（利用者の手元の文字列） | D7 / D11 |
| 7 | 村の言語に固定と、利用者の指定が衝突 | 1 | 採用 — 利用者の明示の指定を優先 | D12 |
| 8 | 改稿を頼んでも反映は全置換で、消えても気づかない | 1 | **採用して形を変えた** — 全文の差分は未決に残し、反映前の字数と行数の差 + `assist:` 行の `current_chars` / `draft_chars` | D9 / D10 |
| 9 | 輪の途中で質問が返った / 3 回目が失敗したときが未定義 | 2 | 採用 | D8 |
| 10 | `valid` と `error` が独立で不正な状態を表せる | 2 | 採用 — 判別共用体 1 つ | D4 |
| 11 | MCP のサーバー名だけでは関数名を捏造する | 2 | 採用 — 接続中はツール名まで渡し、未接続は推測しないと指針に書く（`McpServerStatus.tools` の実在を確認） | D6 / D7 |
| 12 | `judgeId` だけ欄名が違う | 2 | 採用 — `id` に統一 | D4 |
| 13 | 「下書きを出して」の契約が無い / 文言一致だと多言語で壊れる | 2 | **採用して形を変えた** — `forceDraft` で型として運び、`tool_choice` を強制する。強制を受け付けないワイヤは P0 で測る | D5 / P0 |

## P1 実装記録（2026-09-30）

コアだけ。IPC と画面は P2。

### 置いたもの

- `crates/fuseforks-core/src/assist.rs`（純機構）: 型（`AssistKind` / `AssistTarget` / `AssistRequest` / `AssistReply` /
  `AssistValidation` / `AssistOutcome` / `AssistContext`）/ 指針 6 本（3 種 × ja/en）/ `compose_system`（安定部）と
  `current_block`（利用者の区画）/ `submit_draft_spec` / `classify` / `check_history` / `force_supported` / 定型文 4 本
- `orchestrator/assist.rs`: `Orchestrator::assist_draft` — 形の検査 → テンプレート → 文脈 → 呼び出し（判断役は輪）→
  `assist:` 行と `Record::Assist`
- `ModelTemplate::without_provider_skills` / `judging::check_judge_text`（`save_judge_file` と共有）/
  `CoreError::InvalidAssistRequest`（`INVALID_ASSIST_REQUEST`）/ `Record::Assist` + `AssistRecord` /
  `stats::aggregate_assist` + `StatsReport.assist` / `pricing::summarize` を「(モデル, 切片) の列」へ

### 実装で決まったこと

- **生成役のバックエンドはキャッシュへ入れない。** 固有スキルを外した複製は `id` が元と同じなので、`backend_for` の
  キャッシュへ入れると**村の個体がスキル無しのバックエンドを掴む**。`factory.create` を毎回呼ぶ（安い）。退避
  （`degraded_reason`）は生成役では拒否にする — エコー応答を「質問」として画面へ出さないため
- **`classify` は最初の `submit_draft` だけを見る。** ツール結果を対で付ける側（`pair_calls`）と規則を揃えないと、
  空の 1 本目の後ろの 2 本目を拾ったとき、どの呼び出しに `draft_shown` を返したかがずれる
- **質問・空の応答にも呼び出しの対を付ける**（`extra_call_ignored`）— 下書きにならなかった呼び出しでも、答えずに
  履歴へ残すと次の呼び出しでプロバイダが拒否する
- **`Record::Assist` の欄名は `draftKind`**（`kind` は種別タグと衝突する。`failures.md` #140）
- **`StatsReport.assist` の行は `StatsSlice` を展開する形**（`turns` = 呼び出しの回数、`avgElapsedMs` は 0）。
  金額は `by_agent` と `assist.rows` を同じ列で `summarize` へ渡す
- 使用量が分からない失敗（HTTP の失敗など）は `assist:` 行だけで、`Record::Assist` は書かない（数字を捏造しない）

### テストと変異

単体 12 本（`assist::tests` 11 + `model::spec63_provider_skills` 1）+ 結合 12 本（`tests/assist_draft.rs` 11 +
`tests/assist_log.rs` 1）。**変異は予測を先に書いてから回した**:

| 変異 | 予測 | 実際 |
|---|---|---|
| M1 `force_supported` を常に真 | 1 本 | **2 本** — 関数自身の単体テストを数え忘れた |
| M2 判断役の検査を素通し（常に通る） | 判断役 4 + ログ 1 | 5 本（一致） |
| M3 生成役に固有スキルを付けたまま / M3b 1 欄だけ外し忘れ | 各 1 本 | 各 1 本（結合 / 単体の直列化検査） |
| M4 `Record::Assist` を書かない | 1 本 | 1 本（統計） |
| M5 ツール結果を対で付けない | 3 本 | 3 本（往復・輪の再試行・輪の途中の質問） |

**変異スクリプトが 2 回、コンパイルエラーを「赤 0 本」と読んだ**（M2 の最初の 2 回。存在しない関数 / import していない型を
変異に書いた）。スクリプトにコンパイルエラーの検出を足してから数え直した — **赤が 0 のとき、まずビルドが通ったかを見る**。
workspace 全体のテスト・clippy は緑（clippy が出したのは `summarize` が列を受けるようになってテストの `vec!` が
配列で足りるようになった 3 件だけで、直した）。

