# Spec 63: AI による下書き補助（SKILL.md / Construct.md / judge.toml）

- 状態: **Draft rev1**（2026-09-30 起票）
- 起点: 利用者（2026-09-30）—「SKILL.md や Construct.md、条例、判断特化の設定を AI 自動生成で書き込む
  仕組みをつけたい。世間一般の skill-creator などを参考に、AI に生成補助させたい。『〜をするスキルを
  作りたい』と書くだけで、要件のヒアリングから SKILL.md のドラフト生成までを自動で行ってくれるイメージ」
- 利用者裁定（同日）:
  1. **生成に使うモデルはダイアログで毎回テンプレートを選ぶ**（前回の選択は端末に覚える）
  2. **範囲は SKILL.md・Construct.md・判断役の `judge.toml` の 3 つ** —「たぶん判断特化が一番書くのが
     難易度が高いから」。条例は範囲外
  3. **下書きまで。** 生成物を試しに走らせて比べる評価の輪は入れない

## Goal

1. 編集画面から「AI で作成」を開き、**作りたいものを 1 行書くだけで**、ヒアリング → 下書きまで進む
2. **下書きは編集中の本文へ流し込むだけで、保存は人が既存の保存ボタンで押す**（書き込みの経路は増えない）
3. 判断役は、**コアのパーサーを通った下書きだけを「通った」として見せる**。落ちたら生成役へ理由を返して
   直させる
4. 払ったトークンは必ずどこかに出る（`failures.md` #50 / #103 を新しい経路で開け直さない）

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
- `ModelTemplate.use_tools` が偽のテンプレートはツールを送らない
- 編集部品: SKILL / Construct は `MarkdownEditor.vue`（タブで種別を切り替え、`readConfig` / `writeConfig`）、
  判断役は `JudgeDialog.vue`（`readJudgeFile` / `saveJudgeFile` / 「試す」= `tryJudge`）。
  どちらも `CodeEditor` を使う

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

- ダイアログ上部でモデルテンプレートを選ぶ。**`use_tools` が真のテンプレートだけを並べる**（下書きを
  ツール呼び出しの型で受け取るため — D5）。偽のものは理由を添えて灰色で出す
- 前回の選択は `localStorage` の `fuseforks.assist.v1`（`{ templateId }`）に覚える。一覧に無い id は捨てる
- 生成役は**人格・履歴・Memory・村のシステムプロンプトを持たない**。渡すのは D6 の指針と D7 の文脈だけ
- **村の予算（`tokenBudget`）には入れない** — 村の仕事の因果ではなく、人が押した操作。手動要約と同じ扱い

### D3. 会話は使い捨て。状態はフロントが持ち、コアは 1 往復だけを受ける

- ヒアリングの会話はダイアログの `ref` に持つ。**`sessions.redb` にも広場ログにも残さない。**
  閉じると消える（会話があれば閉じる前に確認を出す）
- コアの IPC は状態を持たない 1 本（D4）。毎回、会話の全体を受け取り、指針と文脈を組み直して呼ぶ。
  指針 + 文脈は呼び出しの間で変わらないので `cacheable_prefix_len` に載せる（2 回目以降はキャッシュが効く）

### D4. IPC `assist_draft`

```ts
type AssistTarget =
  | { kind: "skill"; agentId: AgentId }
  | { kind: "construct"; agentId: AgentId }
  | { kind: "judge"; judgeId: AgentId };

type AssistTurn = { role: "user" | "assistant"; content: string };

assistDraft(target: AssistTarget, templateId: ModelTemplateId,
            turns: AssistTurn[], current: string): Promise<AssistReply>;

type AssistReply =
  | { type: "question"; text: string }
  | { type: "draft"; text: string | null; content: string; chars: number;
      valid: boolean | null;          // judge のみ true/false。skill / construct は null
      error: { location: string; message: string } | null;
      attempts: number };
```

- `current` は**編集中の本文**（未保存の変更を含む）。保存済みの本文とは違いうるので、フロントから渡す
- 文脈（D7）は**コアが対象の id から集める**。フロントから受けると、コアが検証できない文字列が「村の事実」
  として生成役へ入る
- 失敗は `CoreError`（テンプレートが無い / `use_tools` が偽 / 対象が無い / LLM の失敗）。空の応答は
  `LLM` の失敗として返す（黙って空の下書きを出さない）

### D5. 質問と下書きは型で分ける

- 生成役に**ツール `submit_draft { content: string, notes?: string }` を 1 本だけ**提示する（`tool_choice` は auto）
  - ツール呼び出しがあれば**下書き**。`content` が本文、`notes` は仮定・未確認点（画面に出す）
  - 呼び出しが無く本文があれば**質問**
  - 両方あれば下書き（本文は `text` に入れて画面に添える）
- 本文の中の目印（```` ```draft ```` など）で分けない。Spec 08 の凍結「分類は文言 parse でなく型で運ぶ」と同じ理由
- 下書きの後も会話は続けられる（「もっと短く」→ 新しい下書き）

### D6. 指針（生成役のシステムプロンプト）— 種類ごとに Fuseforks 用に書く

skill-creator の**手順**（意図の確認 → 質問 → 下書き → 直し）は借りるが、**本文は写さず自前で書く**
（書き方の前提が違う — 前提の実測の 1 点目）。コアの定数で、ja / en の 2 本ずつ（Spec 35 の規律 —
英語は翻訳ではなく英語で書く）。

**共通（ヒアリングの規律）**:
- 最初の発話から分かることは訊かない。**1 回に訊くのは 3 つまで**
- 足りたと判断したら `submit_draft` を呼ぶ。利用者が「下書きを出して」と言ったら、その時点の情報で出し、
  分からない点は `notes` に仮定として書く
- 既に本文がある（`current` が空でない）なら、**書き直しではなく改稿として扱う**（残す部分を残す）

**SKILL.md**:
- **毎ターン全文が読まれる。発火条件・`description`・前付けは書かない**。長さはそのまま毎ターンの費用
- 手順を命令形で。**使えるツールの名前で書き、持っていないツールを前提にしない**（D7 で一覧を渡す）
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
| skill / construct | 個体の表示名 / 有効な同梱ツールの名前 / 接続している MCP サーバーの名前 / 接続先の表示名 / 村の言語 / `current` / **対になるファイルの保存済み本文**（SKILL なら Construct、逆も） | Memory.md / 条例 / 会話ログ / 役職名 |
| judge | 判断役の名前 / **接続先に選べるサーヴァントの ID・表示名・役職名** / 村の言語 / `current` | 各サーヴァントの Construct・SKILL / 条例 / 会話ログ |

- 対になるファイルを渡すのは重複を避けるため（重複は毎ターンの固定費になる）
- 役職名を skill / construct に渡さないのは D6 と同じ理由。judge に渡すのは、ラベルを読むのが人と生成役で、
  その個体自身ではないから

### D8. 判断役の検証の輪

- `submit_draft` の `content` を、**`save_judge_file` と同じ検査**（`JudgeFile::parse` → `missing_targets`）に通す。
  **検査は関数 1 本に切り出して保存と下書きで共有する**（2 箇所に書くと、下書きでは通るのに保存で落ちる形が生まれる）
- 落ちたら、同じ IPC 呼び出しの中で `tool_result` に `location` と `message` を返して作り直させる。
  **上限は 3 回**（コードの定数。`attempts` に回数を返す）
- 通れば `valid: true`。上限に達したら最後の下書きを `valid: false` + `error` で返し、画面にそのまま出す
  （反映はできるが、保存は既存の検査が拒否する）
- skill / construct は検査しない（Markdown に文法は無い）。**字数だけ返して画面に出す**（固定費の目安）

### D9. 反映 — 下書きを編集中の本文へ。保存はしない

- 右側に下書きのプレビュー（原文のまま。`MarkdownEditor` と同じく描画しない）と `notes`、判断役なら検査の結果
- 「エディタへ反映」で編集中の本文を置き換える。**編集中の本文が空でなければ確認を出す**
- 反映の直前の本文を 1 つ持ち、「反映を取り消す」で戻せる
- **保存は既存の保存ボタン / `Ctrl+S`**。書き込みの経路は 1 本も増えない。判断役は反映の後に既存の「試す」で確かめられる

### D10. 計器と費用

- LLM 呼び出し 1 回ごとに 1 行:
  `assist: kind=judge model=… attempt=2 outcome=invalid prompt=… cached=… total=… reasoning=… draft_chars=…`
  - `outcome` は `question` / `draft` / `invalid`（判断役の検査に落ちた）/ `empty` / `failed`
  - **本文・会話・下書きはログに書かない**（字数だけ。`failures.md` #71 の線）
- 失敗でも使用量が分かるもの（`LlmError::usage()` が `Some`）は数字を出す（#103 の処方と同じ）
- 統計画面へ入れるかは未決（Spec 62 の Jev のトークンと一緒に決める）

### D11. 外へ送るもの（PRIVACY）

- 送り先は**利用者が選んだテンプレートの接続先だけ**。新しい送り先は増えない
- ただし送る中身は増える: ヒアリングの会話 / 編集中の本文 / D7 の文脈（対になるファイルの本文、サーヴァントの名前など）。
  **押したときだけ送る**（ダイアログを開いただけでは送らない）
- `PRIVACY.md` 日英の 4-1 へ追記する

### D12. 言語

村の `language` で指針と下書きの言語を決める。会話の途中で切り替えない。

## 採らなかった形

- **村の会話から頼んでサーヴァントに書かせる** — D1
- **生成物を自動で保存する** — 人が見ずに毎ターンのプロンプトが変わる。書き込み経路が増える
- **対象のサーヴァント本人のモデルで書く**（手動要約の「本人が書く」）— 生成には強いモデル、運用は安いモデル、
  という分け方ができなくなる（利用者裁定 1）
- **評価の輪**（テスト用の依頼を実際に走らせて比べる）— 利用者裁定 3。この村には評価の基盤が無い。
  判断役は既存の「試す」で代用できる
- **skill-creator の書き方の指南をそのまま渡す** — 前提の実測の 1 点目
- **本文の目印で質問と下書きを分ける** — D5
- **条例・役職の Construct（`RoleDialog`）** — 利用者裁定 2 の範囲外。同じ部品で後から足せる形にしておく

## Tasks

### P0 — 契約
- `data_contract.yaml` へ `assist_contract`（D1〜D12 の凍結: 書き込み経路を増やさない / 型で分ける /
  検査の共有 / 上限 3 / 文脈の閉じた範囲 / ログに本文を書かない / 予算に入れない）と `entities` の
  `AssistTarget` / `AssistReply`

### P1 — コア
- `crates/fuseforks-core/src/assist.rs`（純機構）: 指針 6 本（3 種 × ja/en）/ 文脈の組み立て / `submit_draft` の ToolSpec /
  応答の振り分け
- `orchestrator/assist.rs`: `assist_draft` — 文脈の収集・呼び出し・判断役の検証の輪・`assist:` 行
- 判断役の検査を `save_judge_file` から関数へ切り出して共有
- テスト: 振り分け（質問 / 下書き / 両方 / 空）・検証の輪（スタブが 1 回目に不正・2 回目に正を返す → `attempts=2 valid=true` /
  3 回とも不正 → `valid=false`）・`use_tools` 偽の拒否・文脈に Memory と条例が入らないこと・ログに本文が出ないこと

### P2 — 画面
- IPC `assist_draft` と型 / `AssistPanel.vue`（左に会話、右に下書き。`MarkdownEditor` と `JudgeDialog` から開く）/
  テンプレートの選択と `fuseforks.assist.v1` / 「下書きを出して」ボタン / 反映と取り消し / 閉じるときの確認 / 辞書 ja/en
- 走査テスト: 入口が SKILL / Construct / 判断役の 3 箇所だけにあること

### P3 — 台帳
- DETAIL 3 言語 / README 3 言語に 1 行（既定で動かず、使うと外部へ送る機能は入口で読めるべき、の判断）/ PRIVACY 日英

### P4 — 実機
1. SKILL: 「〜をするスキルを作りたい」の 1 行 → 質問が返る（`assist: outcome=question`）→ 答える →
   下書き（`outcome=draft`）→ 反映 → 保存 → 次のターンでその個体の `system_digest` が変わる
2. 「下書きを出して」を最初の質問の直後に押す → `notes` に仮定が書かれた下書きが返る
3. Construct: 既に本文がある個体で「もっと簡潔に」→ 残す部分が残った改稿が返る
4. 判断役: 「調査と実装で振り分けたい」→ `valid: true` の下書き → 反映 → 「試す」で規則に当たる → 保存できる
5. `use_tools` が偽のテンプレートは選べない
6. 会話を残して閉じると確認が出て、閉じた後は何も保存されていない

検証の輪（D8）が 2 回目で通る形は実機で狙って出せないので、P1 の結合テストへ預ける。

## 未決

1. `assist:` の使用量を統計画面へ入れるか（Spec 62 の Jev のトークンと一緒に決める）
2. 反映のときに差分を見せるか（rev1 は置き換え + 取り消し 1 段）
3. 条例と役職の Construct への拡張の時期

## Notes

### 1. skill-creator との対照

| skill-creator | この Spec |
|---|---|
| Capture Intent（何を・いつ・出力形式・テストするか） | D6 共通の規律。「いつ」は訊かない（常に読まれる） |
| Interview and Research（MCP で調べる） | 調べない。文脈は D7 でコアが渡す |
| Write the SKILL.md（name / description / 本文） | 本文だけ。`description` と前付けは書かない |
| Progressive Disclosure / references/ | 無い（常に全文が読まれる） |
| Test Cases → 評価 → 反復 → description 最適化 | 採らない（利用者裁定 3）。判断役だけ機械の検査の輪（D8）と既存の「試す」 |
