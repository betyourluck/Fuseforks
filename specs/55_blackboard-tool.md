# Spec: 黒板の書き込みをツールにする — `blackboard` ツールと `blackboard/` の囲い

- 起票: 2026-09-17
- 状態: **rev1（査読待ち）**
- 起点: 利用者 —「黒板は条例にユーザーが任意で書く使い方と、プログラムで書かれた固定の機構に
  依存している。これは必ず一致するとは限らないため、私以外の環境ではまともに動くのかが不安」→
  「付箋を書くのをツール化することはできませんか？ファイルの命名規則も厳格化するとか」（2026-09-17）。
  同日の裁定 2 点（提示は `enabledTools` の外 + オプトアウト 1 欄 / `file`・`sd` から
  `blackboard/` への書き込みを塞ぐ）と、利用者査読 4 点（主キーを `agent_id` へ / `write` は
  create-only / 状態遷移のガード / `move`・`remove` の CAS）を受けて起票。

## Goal

1. **条例が空の村でも黒板が動く。** 規約（置き場・綴り・状態）を運ぶのは条例ではなく
   ツールの説明文と schema。新しい村の `Ordinance.md` は空で、コアのプロンプトは黒板に
   1 字も触れていない（起票時の実測 1）
2. **ファイル名と状態をモデルに書かせない。** モデルが渡すのは仕事名・本文・行き先の状態だけ。
   `blackboard/<状態>/<agent_id> - <仕事名>.md` はコードが組む
3. **「書けるのは自分の付箋だけ」を構造にする。** 判定は `ToolContext.agent_id`。条例の文言ではない
4. **`blackboard/` へ書く経路をこのツール 1 本にする。** `file` / `sd` / `yq` の書き込み系は
   `blackboard/` の下で拒否し、拒否の文面がツールを指す。読み取りは塞がない

**やらないこと（範囲外）**: GUI から内容を書く経路（Spec 54 の凍結のまま）/ GUI のドラッグで
状態を動かす / 指示書 `briefs/`（`file` のまま。囲いの外）/ `run` が起動したプロセスの書き込みを
塞ぐこと（`run` は境界ではない — Spec 15 の凍結）/ `agent_id` の発行規則の変更 /
旧形式 `<表示名> - <仕事名>.md` の互換読み込み。

## 起票時の実測（2026-09-17。コードを読んだ）

1. **新しい村に黒板の規約を伝える経路が無い。** `config_store.rs` の条例は
   `if !ordinance.is_empty()` のときだけプロンプトへ入り、初期文面は同梱していない。コアで
   `blackboard` / `黒板` に当たるのは `blackboard.rs` / `error.rs` / `lib.rs` /
   `orchestrator/settings.rs` / `refusal.rs` で、プロンプトを組む側には 0 件
2. **`ToolContext` は `agent_id` / `work_dir` / `cancel` / `rag_roots` / `language` を持ち、
   表示名も他個体の名前も持たない**（`tool.rs:34`）。組むのは `turn.rs` の 2 箇所（提示時 1126 行付近 /
   実行時 2881 行付近）
3. **`BUNDLED_TOOL_NAMES` の外のツールは `is_bundled_tool_presented` が無条件で通す**
   （`turn.rs:2775`）。`rag` はこの形で、提示の門は `spec_for(ctx)` が `ctx.rag_roots` から決める。
   **`rag` にオプトアウトの欄は無い**（宣言そのものがオプトイン — Spec 18 D13）。個体の真偽値の
   前例は `hearsRoomLog` / `allowHandoff`（既定 true）と `planReview`（既定 false）
4. **`agent_id` は削除後に再利用される。** 発行はフロントの `deriveId`（`AgentList.vue:154`）で、
   名前から `[a-z0-9_-]` へ落とし、日本語名は `agent` / `agent_2` / … の**空いている最小の番号**を
   取る。`agent_3` を消して新しく 1 体作ると `agent_3` が再び出る。文字集合に空白は無いので、
   ファイル名の最初の ` - ` で id と仕事名は一意に割れる
5. **同一個体のターンは直列で、周の中のツールも直列。** 受信箱は個体に 1 本、`run_turn` は
   `for call in &calls { runner.on_call(call).await }`（`turn.rs:1607`）。自分の付箋を書けるのが
   自分だけなら、**サーヴァントどうしで同じ付箋を同時に書く経路は構造上存在しない**。残る同時の
   書き手は人（黒板タブのごみ箱）と `run` が起こしたプロセスだけ

## Design

### D1. ツールは `blackboard` 1 本。`enabledTools` の外（裁定 1）

- `AgentTool` として登録し、**`BUNDLED_TOOL_NAMES` にも `DEFAULT_ENABLED_TOOLS` にも
  `WORK_DIR_TOOL_NAMES` にも入れない**（`rag` と同じ棚）。既存の村は全個体の `enabledTools` が
  明示配列なので、表へ入れると誰にも生えない（Spec 18 D13 の穴）
- 提示の門は `spec_for(ctx)` の 2 条件: `ctx.work_dir` がある / `ctx.uses_blackboard` が真。
  提示集合が実行フィルタを兼ねるので、実行側に 2 つ目の門は置かない
- **オプトアウトは `AgentSpec.usesBlackboard: bool`（既定 true・`#[serde(default = "default_true")]`）。**
  裁定は「`rag` のオプトアウトと命名を揃える」だったが、`rag` にその欄は無い（実測 3）。揃える先は
  `hearsRoomLog` / `allowHandoff` = **肯定形・既定 true**。否定形（`blackboardDisabled`）は
  二重否定を読ませるので採らない
- 欄を足す先は `AgentSpec` / `AgentSnapshot` / `snapshotToSpec` / `AgentSettingsDialog`
  （投影に無い欄は保存のたびに既定へ戻る — Spec 14 P1）。**役職の雛形には入れない** —
  `role_contract` 凍結 2 の分類は `hears_room_log` と同じ「入れない」側で、表の総数が 1 増える
- 理由欄（Spec 27）は既定どおり生やす（引数が他の形で画面に出ない）。ツール名の動作名は
  `toolLabel.ts` へ 1 鍵（表との突き合わせは `BUNDLED_TOOL_NAMES` を読むので、`rag` と同じ扱いを P4 で確かめる）

### D2. 主キーは `agent_id + 仕事名`。置き場は `blackboard/<状態>/<agent_id> - <仕事名>.md`（利用者査読 1 を採用）

Spec 54 D1 の `<表示名> - <仕事名>.md` を覆す。表示名は可変で、改名のたびに孤児が出る設計だった。

- ファイル名の前半は `ctx.agent_id` をそのまま書く。**自分の付箋か = 前半が自分の id か**
- 表示名は保存しない。`list` / `read` の出力と黒板タブが、その時点の名前を id から引いて付ける
- **裁定済みの「`ToolContext` へ `displayName`」は形が変わる。** 主キーが id になると自分の
  表示名は要らず、要るのは `list` が他人の付箋へ名前を付けるための **id → 表示名の表**
  （`agent_names`。村の全個体）。顔ぶれは接続先しか載せないので、表が無いと非接続の持ち主が
  id だけで出る。`rag_roots` と同じく `turn.rs` の 2 箇所で world から解いて渡す
- **仕事名の正規化は純関数 1 本**（全 op が同じ関数を通す）: 前後の空白と末尾の `.` を落とす /
  `\ / : * ? " < > |` と制御文字を `_` へ / 空は拒否 / 上限 80 字（code point）。正規化後の名前を
  結果に必ず書く。別の原文が同じ名前へ落ちたら D3 の create-only が名指しで止める
- **旧形式は読み替えない。** `<表示名> - …` の付箋は id に当たらないので黒板タブで孤児
  （`orphanUnknown`）になる。付箋は作業の寿命で、消し口は既にある（互換読み込みは本来の形が
  来た時点で害へ反転する — `failures.md` #48）

**id の再利用（実測 4）が、この決定の代償。** 改名の孤児は消えるが、個体を消して同じ id で
作り直すと、**残っていた付箋が新しい個体のものとして読まれ、書ける**（孤児のバッジも付かない）。
→ D7 で受ける。

### D3. op は閉じた 6 つ

| op | 引数 | 規則 |
|---|---|---|
| `list` | なし | 盤面。1 行 = 状態・持ち主 `id（表示名）`・仕事名・字数・更新時刻。**本文は返さない**。直下と「その他」のフォルダも場所つきで出す |
| `read` | `name`, `owner?` | `owner` 省略 = 自分。**誰の付箋でも読める**。上限超えは `file read` と同じ作法で次の手を書く |
| `write` | `name`, `body` | **create-only・置き場は `doing` 固定**（利用者査読 2 を採用）。同じ `id + 名前` が**どの状態・直下にあっても**拒否し、今の場所と次の手（`append` / `move` / 別の名前）を書く |
| `append` | `name`, `body` | 自分の付箋だけ。`doing` / `on-hold` にあるもの。**`done` は追記も拒否**（閉じた仕事）。追記後の大きさに `file` と同じ上限 |
| `move` | `name`, `to` | 自分の付箋だけ。下の遷移表 |
| `remove` | `name` | 自分の付箋だけ。状態を問わない。**OS のごみ箱**（`file remove` と同じ `trash`） |

**遷移（利用者査読 3 を採用）**: `doing ⇄ on-hold`、`doing → done`、`on-hold → done` は可。
**`done → *` は拒否。** 同じ状態への `move` は何もせず、そうだと書く。

- 代償を 1 つ数える: create-only と組むと、**終わった仕事のやり直しは「`remove` してから `write`」か
  「別の仕事名で `write`」の 2 手**になる（検証役に差し戻された仕事がこれに当たる）。拒否の文面は
  この 2 つを名指しする（歯止めの先に道を書く — `failures.md` #44）
- 他人の付箋への `append` / `move` / `remove` は「持ち主は `agent_x（名前）` です。書けるのは
  自分の付箋だけです」で拒否

**`move` / `remove` に「期待する現状態」の引数は取らない（利用者査読 4 — 前提を訂正して、形を変えて採用）。**
指摘の前提「`list` から `move` の間に別ターンが割り込む」は実測 5 で成立しない — 書けるのは
持ち主だけで、持ち主のターンもツールも直列。残る同時の書き手は人のごみ箱で、その場合は
「その付箋はありません」で止まる。**現状態を必須にすると、滑る窓で状態を忘れたモデルが推測で
埋めて 1 周を失う**側の失敗が増える。指摘の趣旨（モデルの前提と実際のずれを黙って通さない）は
別の形で受ける — D2 と create-only により `id + 名前` は盤面に 1 枚しか無いので、ツールは名前だけで
引き、**結果に実際の遷移（`on-hold → done`）を必ず書く**。ずれていれば結果の 1 行で分かる。

### D4. `blackboard/` の囲い（裁定 2）

- 対象: `file` の `write` / `append` / `mkdir` / `move` / `copy` / `remove`（**元か宛先のどちらかが
  `blackboard/` の下**）、`sd` の `apply`、`yq` の書き込み op。**`blackboard` フォルダ自体**への
  `remove` / `move` も同じ
- 対象外（塞がない）: `file read` / `fd` / `grep` / `diff` / `sd` の preview / `rag`
- 判定は**解決後のパス**で行う 1 実装（`resolve_in_work_dir` / `resolve_creatable` が返した
  パスの、work_dir からの最初の要素が `blackboard` か。Windows は大文字小文字を無視）。
  文字列の前置検査にしない（`./blackboard/..` や junction を取りこぼす）
- 文面（ja。en も持つ）:「`blackboard/` への直接の書き込みはできません。`blackboard` ツールの
  write / append / move / remove を使ってください。書けるのは自分の付箋だけです。」
  ツールを持たない個体（オプトアウト）には後半を「この個体は黒板を使わない設定です」へ差し替える
- 移行期間も警告も置かない（裁定）。**`run` は塞がない** — 契約に「囲いはツール層のもので、
  `run` を許した村では保証にならない」と書く

### D5. `まとめ.md` は廃止を推す（利用者「Spec 55 内で決める」）

囲いを入れると `まとめ.md` を書ける経路が無くなる。ツールに 7 つ目の op を足す案もあるが、
**束ねは返信で返る**（条例「仕事の答えは黒板でなく返信で」）ので、黒板に置く理由が弱い。
推奨: ツールは書かない / 黒板タブの最上部固定を P4 で外す / 既存のファイルは直下の「状態なし」に
孤児として出る（消すのは人）。Spec 54 の `まとめ.md` の例外を覆す。

### D6. 説明文が規約を運ぶ（Goal 1）

ツールの説明文に入れるのは 4 点だけ — 1 仕事 1 付箋 / 着手時に `list` で盤面を見る /
状態の意味（`doing` 進行中・`on-hold` 管理人に止められた・`done` 終わった）/ 仕事の答えは
黒板ではなく返信で返す。ja / en の 2 面（Spec 35）。**全員の毎ターンに乗る固定費**なので、
字数を P1 で測って Spec へ書く。

### D7. 個体の削除で、その個体の付箋をごみ箱へ（D2 の代償を受ける）

`delete_agent` が、消す個体の**現在の `work_dir`** の `blackboard/` から `<その id> - *.md` を
OS のごみ箱へ送る。失敗は WARN 1 行で削除は通す。

- GUI から**内容を書く**経路ではない（削除は `delete_blackboard_note` で既に在る）
- **残余**: 以前の work_dir に残した付箋は届かない（Spec 29 の副作用と同じ）。同じ id の個体が
  後でそのフォルダを向くと引き継ぐ。契約に残余として書き、保証とは書かない
- 採らない案: id を再利用しない発行（`world.json` に連番が要る。会話ログ・`sessions.redb`・
  `run.json` の置き場まで id が鍵なので、発行規則は本 Spec で動かさない）

### D8. 黒板タブ（GUI）

- 持ち主の解決を `ownerNameOf`（表示名の完全一致）から **id の一致**へ。カードの見出しは
  「表示名 + 仕事名」、`title` に id（識別子は `title` に残す規律）
- 孤児の意味が変わる: `orphanUnknown` = 前半がどの個体の id にも当たらない（旧形式の付箋 /
  消えた個体）。`orphanMoved`（個体は居るがこの work_dir を向いていない）は据え置き
- 重複のバッジは残す（`run` や手作業で 2 枚できる形は残るため）。畳みの鍵・削除の IPC は不変
- D5 を採るなら最上部固定の撤去

### D9. 計器

`blackboard op: agent=… op=… state=… outcome=ok|exists|not_owner|frozen|not_found|…` を 1 行
（`tool:` 行は `args_chars` しか持たず、どの op が何で断られたかが読めない — Spec 16 の
`grep include:` と同じ理由）。囲いの拒否は `blackboard fence: agent=… tool=… op=…`。
本文と仕事名は出さない（#71）。

### D10. 条例は利用者の資産 — 改訂案を渡す

黒板の節から**置き場・綴り・`file move` の手順が消え**、残るのは運用（いつ読むか・何を書くか・
指示書 `briefs/` の使い分け）。案は P0 で Notes へ書き、貼るのは利用者。

## 覆す凍結

| 元 | 内容 | 本 Spec |
|---|---|---|
| Spec 54 D1 / `blackboard_contract` | ファイル名の前半は表示名 | `agent_id`（D2） |
| Spec 54 D2 | 動かすのはサーヴァントの `file move` だけ | `blackboard` ツールの `move` だけ（D3・D4） |
| Spec 54 `まとめ.md` の例外 | 列の外・最上部固定 | 廃止（D5。査読で裁定） |
| `file_tool_contract` / `write_tools_contract` | work_dir の中ならどこでも書ける | `blackboard/` の下は拒否（D4） |
| `role_contract` 凍結 2 の分類表 | 欄の総数 | `uses_blackboard` を「入れない」へ 1 行 |

**維持**: GUI から内容を書く経路は作らない / 状態は `doing | on-hold | done` の 3 値 /
コアは読み手として状態名を検査しない（書き手のツールだけが 3 値を知る）/ push 注入はしない。

## Tasks

- **P0 契約** — `data_contract.yaml`: `blackboard_contract` の書き換え（鍵・op・遷移・囲い・残余）/
  `file_tool_contract` と `write_tools_contract` へ囲い / `role_contract` の表 / `tool_extension_point` の
  列挙 / `AgentSpec`・`AgentSnapshot` の欄。条例の改訂案を Notes へ
- **P1 コア: `ToolContext`** — `agent_names` / `uses_blackboard` を足し、`turn.rs` の 2 箇所で解く。
  `AgentSpec.uses_blackboard` + 投影 + 役職の分類テスト
- **P2 コア: ツール本体** — `tools/blackboard.rs`（正規化の純関数 / 6 op / 遷移表 / ja・en の説明文）。
  読みは `blackboard.rs` の `read_blackboard_dir` を共有。単体 + ミューテーション
  （create-only を外す / 持ち主の検査を外す / `done` の凍結を外す — それぞれ狙った本だけ赤）
- **P3 コア: 囲い** — 判定 1 実装を `tools/fs.rs` へ。`file` 6 op・`sd` apply・`yq` 書き込みへ配線。
  読み取りが通ることを対で留める。`delete_agent` の掃除（D7）
- **P4 GUI** — 持ち主の解決を id へ / 設定ダイアログのチェック / `snapshotToSpec` / `toolLabel` /
  辞書 ja・en / D5 の撤去 / 走査テスト
- **P5 台帳** — DETAIL 日英（黒板タブ・同梱ツール表）/ README 3 言語の該当行 / 初回案内の黒板の歩 /
  CLAUDE.md / Spec 54 へ覆しを取り消し線で
- **P6 実機**

## 検収（P6）

1. **条例が空の新しい村**で、依頼を受けた個体が `blackboard` の `write` を呼び、黒板タブの
   `doing` 列に付箋が出る（`blackboard op: … op=write outcome=ok`）
2. 同じ仕事名で 2 回目の `write` → `outcome=exists`。モデルが `append` へ切り替える
3. `done` の付箋へ `move` → `outcome=frozen`。盤面は変わらない
4. `file write` で `blackboard/doing/x.md` を書かせる → `blackboard fence:` が出て、次の周で
   ツールへ切り替える。**同じ個体の `file read` は通る**（対で読む）
5. 個体を改名 → 付箋の持ち主の表示が新しい名前へ変わり、孤児のバッジが付かない
6. `usesBlackboard` を外した個体の提示集合に `blackboard` が無い（`fuseforks.log` の提示の行で読む）
7. 旧形式の付箋が孤児として出て、列の一括とごみ箱で消せる
8. 個体を削除 → その個体の付箋が OS のごみ箱に入る

## Notes

### 1. 利用者査読 4 点の扱い（2026-09-17）

| # | 指摘 | 扱い |
|---|---|---|
| 1 | 主キーを `agent_id` へ | **採用**（D2）。ただし id は再利用される（実測 4）ので、代償を D7 で受ける。`ToolContext` へ足すのは `displayName` ではなく id → 表示名の表 |
| 2 | `write` は create-only | **採用**（D3）。検査の範囲は全状態 + 直下 |
| 3 | `done → *` の禁止 | **採用**（D3）。やり直しが 2 手になる代償を拒否の文面で受ける |
| 4 | `move` / `remove` を CAS に | **前提を訂正して、形を変えて採用**（D3）。同一個体のターンとツールは直列で、書き手は持ち主だけ（実測 5）。現状態の必須引数は取らず、結果に実際の遷移を書く |

### 2. 開いたままの点

- 説明文の字数と、`list` の出力の上限（付箋が 100 枚を超えた盤面）。P2 で測る
- `agent_names` が村の全個体の名前をツール層へ渡すこと。黒板は今もファイル名で全員の名前を
  露出しているので新しい露出ではないが、契約に 1 行書く
