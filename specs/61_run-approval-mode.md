# Spec 61: コマンドの承認モード（ステータスバーの 3 択）

- 状態: **P0〜P3 完了 → rev2（帯の表記とクリックで巡る形。2026-09-27 同日）。残るのは P4 の実機**
- 起点: 利用者（2026-09-27）—「確認なしのような、`run` のコマンドも承認無しに実行できるようにする
  スイッチが欲しい。3 つの切り替え — 承認が必要 | 自動承認して許可 | 承認せずに許可」
- 査読: **窓を開けていない**。決めどころ 2 点を利用者がその場で裁定したため（下の D1 / D3）。
  実機で違和感が出たら rev を切る

## Goal

`run` の許可リスト（`run.json` の `allow`）に無い呼び出しを、人がステータスバーで一時的に
通せるようにする。無人で回したい村で、承認待ちのたびに止まらないため。

**Spec 15 の「閉じた許容」を、人の操作で緩める経路を 1 つ開ける。** 覆したことをここと
`data_contract` の `command_tool_contract` に書く（`failures.md` #56）。

## Design

### D1. 3 つのモード（利用者裁定 —「記録が残るかどうか」）

| モード | `allow` にも `deny` にも無い呼び出し |
|---|---|
| **承認が必要**（`required`・既定） | 実行せず `pending` へ積む（Spec 15 以来のまま） |
| **自動承認して許可**（`auto_approve`） | その呼び出しの**完全一致**を `allow` へ書き足してから実行。記録が残るので、スイッチを戻しても同じ呼び出しは通り続け、承認画面や `run.json` から後で見直せる |
| **承認せずに許可**（`no_approval`） | その場で実行するだけ。`allow` にも `pending` にも何も書かない。戻せば元どおり拒否 |

- **変わるのは `Decision::Unknown` の扱いだけ。** `Denied`（利用者が一度した判断）と
  `Malformed`（照合できない形）はどのモードでも拒否する
- 型は `command::RunApproval`（serde は snake_case。ログの `mode=`・IPC・保存値が同じ綴り）

### D2. 自動承認が書くパターン

`exact_allow_pattern(command, args)`。パターンは空白で割って 1 語ずつ照合し、末尾の `*` は
「以降の引数は自由」なので、**次の形は完全一致として書けない**:

- 空白を含む引数（`-Command "lake build"` — #137）→ 書くと永久に一致しない行
- 空の引数 → 割ると消える
- **最後の引数が `*`** → 完全一致のつもりが「以降は自由」へ広がる

これらは**書かずに実行する**（`recorded=unrepresentable`）。途中の `*` は文字どおりの一致なので書く。
書き足しは `update_command_policy`（読み直し → 差分適用 → 3 回 retry）で、足した行が覆う
判断待ちは `prune_settled` が落とす（承認画面と同じ後始末）。**書き足しの失敗でも実行の答えは
変えない**（`note_pending` と同じ規律 — モードが「許可」なので、記録の失敗で拒否すると
利用者の選んだモードと食い違う）。

### D3. 置き場と寿命（利用者裁定 —「ステータスバー・保存しない」→ 同日「端末には記憶」）

- **コアは `Shared` のメモリだけ**（`AtomicU8`）。`world.json` にも `run.json` にも書かない。
  起動時は必ず `required`
- **画面が端末の `localStorage` に覚え、起動時に設定し直す**（`"fuseforks.switches.v1"`・
  `lib/switchMemory.ts`）。利用者 —「保存しないと言ってもブラウザのストレージには記憶される
  ようにしておいてほしい」。**同じ裁定で Spec 53 の計画の確認のスイッチも同じ扱いにした**
  （凍結 11 (b) を一部覆した。`data_contract` に記録）
- **書くのはコアが受け入れた値だけ**（設定の成功とイベント）。起動時にコアから読んだ既定を
  書くと、覚えた値を読む前に消す
- **村には持たせない** — 村を配った先で、受け取った人の承認が黙って省かれないように
- 1 プロセス 1 村なので、スコープはプロセス全体 = その村（全サーヴァント）

### D4. 提示と実行が同じ値を読む

`ToolContext.run_approval`。`turn.rs` の提示（`present_tools`）と実行（`execute_tool`）の 2 箇所で
`shared.run_approval()` から解く。

- `required` 以外では `run` の説明文に 1 文足す（一覧に無い呼び出しも実行される / 禁止は
  実行されない）。**言わないと、モデルは使える道具を「許可されていない」と読んで諦める**
- **`required` の説明文は 1 バイトも変えない**（`tool_spec_golden` が留める）
- 切り替えた周から tools の前方一致は切れる（入力キャッシュの書き直し 1 回）。切り替えは
  稀なので受ける

### D5. 計器

- 承認を経ずに走った呼び出しごとに 1 行:
  `run bypass: agent=… command=… args=N mode=auto_approve|no_approval recorded=yes|already|unrepresentable|save_failed|-`
  （引数は数だけ — `run decision:` と同じ。秘密を運びうるので中身は書かない）
- 切り替え: `run approval: mode=… was=…`
- イベント `CoreEvent::RunApprovalChanged { mode }`（`ipc_contract` がワイヤ形を留める）

### D6. 画面

ステータスバーの統計の左に、端末アイコン + いまのモードの字のボタン。**押すたびに次のモードへ
巡る**（承認あり → 自動承認 → 自動許可 → 承認あり。順番は `lib/runApproval.ts`）。
既定以外の間は発光する — 自動承認は注意色、自動許可は失敗色（記録も残らないので
強く知らせる）。色だけに頼らない（いまのモードが字で出ている）。

**帯の表記**（rev2・2026-09-27 利用者裁定。初版はアイコンと select で「分かりにくい」）:

| 状態 | 帯の字 |
|---|---|
| 計画の確認 オフ / オン | 計画：確認する / 計画：確認なし |
| `required` / `auto_approve` / `no_approval` | コマンド：承認あり / コマンド：自動承認 / コマンド：自動許可 |
| 統計の入口 | アイコン + 統計 |

**計画のスイッチの字は常に出す**（初版はオンの間だけ「確認なし」を出していた — オフの間は
アイコンだけで何のスイッチか読めなかった）。本文の呼び名（承認が必要 / 自動承認して許可 /
承認せずに許可）は仕組みの説明の語で、帯の字は短い表示の語として分けている。

## 採らなかった形

- **サーヴァントごとの設定**（`run.json` に保存）— 利用者裁定でステータスバーの 1 つへ
- **`world.json` への保存** — 村を配ると設定も配られる
- ~~**3 つをボタンの押し回しで巡る形**~~ → **rev2 で採った**（利用者裁定）。初版は「押し間違いの
  代償が大きい」として select にしたが、利用者はクリックで巡る形を選んだ。押し間違えても
  2 回押せば戻り、いまのモードは帯の字で読める

## Tasks

- [x] P0: `data_contract` — `command_tool_contract` の末尾へ本 Spec / `settings_contract` へ
  `"fuseforks.switches.v1"` / `plan_edit_window` 凍結 11 (b) の一部撤回
- [x] P1: コア — `RunApproval` / `exact_allow_pattern` / `CommandPolicy::auto_approve` /
  `ToolContext.run_approval` / `Shared.run_approval` / `Orchestrator::{run_approval, set_run_approval}` /
  `CoreEvent::RunApprovalChanged` / `run` の提示文と分岐 / 計器。
  単体 8 本 + ワイヤ凍結 1 本。**変異 2 回とも狙った 1 本だけ赤**（自動承認で書き足さない /
  提示文をモードで変えない）
- [x] P2: IPC 2 本（`get_run_approval` / `set_run_approval`）+ `types.ts` + `useOrchestrator` の
  投影と記憶の復元 + `StatusBar.vue` + 辞書 ja/en。**変異 1 回で赤**（記憶を復元しない）
- [x] P3: DETAIL 日英 / README 3 言語 / CLAUDE.md
- [ ] P4: 実機 — (1) 3 つのモードでそれぞれ許可リストに無いコマンドを呼ばせ、`run bypass:` の
  `mode=` と `recorded=` が表どおりに出る (2) 自動承認のあと承認が必要へ戻しても同じ呼び出しが
  通り、別の引数は拒否される (3) `deny` に書いたコマンドはどのモードでも走らない
  (4) モードとスイッチを入れたまま再起動して、同じ状態で始まる（帯が発光している）
  (5) 空白入りの引数を自動承認で走らせて `recorded=unrepresentable` になり、`allow` が増えない

## P4 実機記録（2026-09-27）

**(1) と (5) を観測**（ミュゼに `lake build` を頼んだ 1 ターン）:

```text
02:48:37 run bypass: agent=agent_9 command=lake args=1 mode=auto_approve recorded=yes            (allow 5 → 6)
02:48:49 run bypass: agent=agent_9 command=lake args=1 mode=auto_approve recorded=yes            (allow 6 → 7)
02:49:00〜02:49:52 run bypass: agent=agent_9 command=bash args=2 mode=auto_approve recorded=unrepresentable ×7 (allow は 7 のまま)
```

`bash -c "…lake.exe build"` は引数に空白を含むので書き足さずに実行し、`Build completed successfully
(141 jobs)` まで走った。**残りは (2) (3) (4)。**

**前段で 1 つ躓いた** — 最初の依頼ではミュゼに `run` のチェックが無く（有効なのはザリとルナだけ）、
モードに関係なく道具が提示されなかった。ミュゼは道具が無いので `MCP_DOCKER__mcp-add` /
`mcp-config-set` でシェル実行の MCP サーバーを自分で足そうとした。**利用者裁定: `run` を渡して
いない個体は使えないほうがよい**（モードは許可リストの段にだけ効き、道具を持つかどうかの段は
個体ごとのチェックのまま）。MCP の動的追加は今回は触らない（別件）。

コストの観察: このターンは 22 周・`prompt=572786 cached=484022`。`lake` を直接呼ぶ形が通らず
（WSL 側に lake が無い）、`bash -c` で実体のパスを探しながら試行した分。
