# Spec: コンテキスト使用率の輪 — 選択中の個体の直近の入力が、そのモデルの窓の何 % か

- 起票: 2026-09-04
- 状態: **rev2（査読 2 系統 13 点 → 採用 5 / 訂正して採用 1 / 反証 4 / 確認済み・
  変更なし 3。記録は Notes 4）→ 承認（2026-09-04）→ P0 完了（`data_contract` 凍結 4 箇所 =
  `AgentSnapshot.lastPromptTokens` / `contextLength` の読み手 / `agentStatsUpdated` の欄 /
  `observability` の輪の節）→ P1 完了（コア。記録は「P1 実装記録」）→ P2 完了
  （フロント。記録は「P2 実装記録」）→ P3 完了（台帳）→ P4 完了 = Done**
  （2026-09-04。実機検収 7 件すべて観測。記録は「P4 実機記録」。**起票から Done まで
  同日**）
- 起点: 利用者 —「Claude や Copilot の入力欄の右下にあるコンテキスト使用率の
  プログレスを Fuseforks にも。ザリのコンテキストはどのくらい使っているか、ルナは、と
  **キャラクターを切り替えるごとに**分かるようにしたい。**料金はここでは見せない**。
  現在選んでいるモデルのコンテキストの使用率。**75% 以上は黄、90% 以上は赤、通常は青**」
  （2026-09-04。スクリーンショットは Claude Code の入力欄右下の輪）

## Goal

会話ペインの入力欄の下の行（右側）に、**選択中の個体の直近の LLM 呼び出しの入力トークン数 ÷
その個体のモデルテンプレートの `contextLength`** を輪と % で出す。個体を切り替えると
その個体の値に変わる。色は 3 段（`accent` / `warn` / `fail`）。

**やらないこと**: 料金・実効トークン・累計（統計画面とカードが持つ）/ ステータスバーへの
常駐（あの帯の規律「常に見る必要があるか」を満たさない — `StatusBar.vue` の冒頭の doc が
明記）/ 予算（`tokenBudget`）との比較（それは因果の天井で、窓の大きさとは別の量）/
`contextLength` の自動取得（D2 の Notes）。

## 起票時の実測（2026-09-04。コードを読んだ）

| 事実 | 場所 | 帰結 |
|---|---|---|
| フロントへ届く個体の数字は `uptimeSecs` / `totalTokens` / `promptTokens` / `cachedTokens` の **4 つとも生涯累計** | `event.rs:39-50` `AgentStatsUpdated` / `model.rs:1021-1027` `AgentSnapshot` | **「直近の呼び出し」の欄はどこにも無い** — 足すしかない |
| `TurnSpend.prompt` / `TurnRecord.prompt` は**ターン内の全周の合計** | `turn.rs:109-113` `absorb` | **分子に使うと 6 周のターンで 240K になり 100% を常時超える**。この Spec の最大の罠 |
| 周ごとの `usage.prompt` は `cache:` 行に出ている | `turn.rs:573-600` `note_cache_diag`（呼び出し `:1441` / `:1685`） | 分子はここで取れる値。`last_completion`（宣言 `:1307`・代入 `:1449` / `:1693`）が**同じ場所で「累積ではなく代入」を既にやっている** |
| `settle_turn` が 4 出口の 1 実装で、`world.agent_mut` の書き込みブロックを持つ | `turn.rs:161-231`（`:172-179`） | 個体の記録へ書く場所は既にある |
| `ModelTemplate.context_length` は**定義と既定 128,000 の 2 行以外に読み手が無い** | `model.rs:547` / `:827` | **保存されているだけの死んだ欄**。この Spec で初めて読み手が付く |
| `AgentStatsUpdated` は**稼働中の個体だけ**に毎秒。**稼働中 = `Starting \| Running`（起動している）であって「ターンの最中」ではない** | `bootstrap.rs:314-341`・`model.rs:124` `is_active`・`stats_interval` 1 秒 | ターンが確定しても個体は `Running` のまま（停止は人の操作）なので、確定の次の tick で新しい値が乗る。停止中の個体は更新されない = 直近の値のまま止まる（それで正しい） |
| `AgentRecord` の累計 4 欄は **`PersistedWorld` に無い**（`agents` / `model_templates` / `topology_positions` / 予算だけ） | `world.rs:295-310` | **累計もこの欄も同じく揮発** — 再起動で 0（D6 は既存と整合） |
| `state.templates` から `modelTemplateId` で引く形が既にある | `useOrchestrator.ts:651` | IPC は 1 本も要らない |
| 入力欄の下に `mt-1 flex … text-[10px]` の行があり、右側に `ml-auto` のボタンが 1 つ | `ChatInput.vue:628-655` | **置き場は既にある**。輪はその隣 |
| 輪・進捗の部品は存在しない（SVG は全部その場のアイコン） | `components/` | 最初の 1 つをインライン SVG で書く |
| 色のトークンは `--color-accent` / `--color-warn` / `--color-fail`（`danger` は無い） | `style.css:135-138` | 3 段はそのまま当たる。生の色は書かない |
| 閾値で色を返す computed の前例 | `AgentCard.vue:184-189` `cacheTone` | 同じ形で `contextTone` |
| `AgentSnapshot` の欄集合は凍結テストが握っている | `ipc_contract.rs:149-197` | 欄を足すと literal とリストの両方が落ちる（意図した強制） |

## Design

### D1: 分子は「直近の LLM 呼び出しの入力」。ターンの合計ではない

`AgentRecord.last_prompt_tokens: Option<u64>` を新設し（**`None` = まだ 1 度も呼び出しが
無い。0 を番兵にしない** — rev2、査読 B-4）、**`settle_turn` の既存の書き込みブロック
（`turn.rs:172-179`）で代入** — 隣の 3 行は `+=`（累計）で、この欄だけ性質が違うことを
コメントで明示する。値は `TurnSpend.last_prompt: Option<u64>` から。`TurnSpend` へ 1 欄
足し、`last_completion` と同じ **2 箇所**（`:1449` = ツールループの周・`:1693` = 締めの
呼び出し）で `Some(response.usage.prompt)` を代入する（`:1307` は `let mut` の宣言で
代入箇所ではない — 査読 A-B は反証）。**`settle_turn` の呼び出し 4 箇所は触らない**
（欄が `TurnSpend` に乗るので署名が変わらない）。

**`Usage` が返らなかったターンでは上書きしない**（rev2、査読 B-3）。プロンプト構築の
失敗・DNS・401・タイムアウトなど LLM から `usage` が 1 度も返らずに出口へ来ると
`spend.last_prompt` は `None` のままで、`settle_turn` は **`if let Some` のときだけ
代入**する。前回の正常値が残るので、失敗の直後に輪が消える形にならない。
`OutputTruncated` のように**プロバイダが 200 を返した後にこちらが `Err` にした
variant**は `LlmError::usage()` が `Some` を返すので（#103）、その `prompt` は書く。

**ターン中は更新しない**（12 周のターンで途中の膨らみは見えず、確定後に 1 秒以内に反映）。
周ごとに書く案は世界ロックを LLM 呼び出しごとに取ることになり、得るのは「走っている
最中の値」だけ — Claude Code の輪はそれを出すが、この村では**周の途中でできる操作が無い**
（打ち切りは周回境界）ので、確定値で足りる。頻度を見てから。**「ターンの最中は動かない」は
DETAIL に明記する**（S4 の「1 秒以内」は確定後の話 — rev2、査読 A-D）。

**失敗したターンも書く** — `settle_turn` は 4 出口の 1 実装なので、`stop=failed` でも
直近の呼び出しの入力は記録される（#103 の「払いは 4 出口で精算する」と同じ棚）。

### D2: 分母は `ModelTemplate.contextLength`。フロントで引く

`state.templates.find(t => t.id === agent.modelTemplateId)?.contextLength`。
**テンプレートが引けない、または `contextLength` が 0 なら輪ごと出さない**（0 で割らない。
`model: "<unknown>"` の作法と同じで、無いものを 0% と見せない）。

**この欄は利用者の手入力で、既定は 128,000**。実際の窓（`gemini-3.8-flash` は 1,048,576 —
公式のモデル頁で確認済み。**他社のモデルの窓は本 Spec では確かめていない**。世代で
動く値なので数字を書かない — rev2、査読 A-研究）と違えば % は嘘になる — **輪の正直さは
`contextLength` の正しさに乗る**。自動取得は範囲外（Notes 1。**→ 2026-09-05 に
[Spec 50](50_context-length-fetch.md) で「取得」ボタンが単価と一緒に埋める形で着地**）だが、
`prompt > contextLength` のときは**数字は 100% を超えたまま `fail` で出す**（切り詰めない）。
超えているのに動いている = 設定が実際の窓より小さい、という診断がその数字から読める。
tooltip にはそのとき「設定の上限が実際の窓より小さい可能性」を括弧で添える。

### D3: 分子は入力の全部（キャッシュ込み・ツール取得込み）

`Usage.prompt` = 素の未キャッシュ + `cache_read` + `cache_write`（+ Gemini の
`toolUsePromptTokenCount`、Spec 48 D4 で畳み済み）。**窓を占めるのはキャッシュされていても
同じ**なので、`prompt` をそのまま使う。実効トークン（`budget.rs` の重み）は使わない —
あれは歯止めの単位で、窓の占有率ではない。

**`toolUsePromptTokenCount` は `promptTokenCount` の部分集合ではない**（査読 A の
「含むなら二重加算」は Spec 48 P0 の probe で反証済み）— probe 5（3.8）は
`total 9,107 = prompt 32 + candidates 76 + toolUse 8,999`、probe (e)（2.5-flash-lite）は
`total 6,612 = prompt 25 + candidates 46 + toolUse 6,541`。部分集合なら `total` が
二重に数えることになり成り立たない。畳みは加算で正しい。

### D4: 輪と %・色の 3 段・置き場

- 置き場は `ChatInput.vue:628` の行の右端。**輪と「表示クリア」ボタンを 1 つの
  `ml-auto flex items-center gap-2` で包む**（rev2、査読 B-1 — ボタン自身に `ml-auto` が
  付いたままその左に輪を置くと、輪は左寄せ側の末尾に残りボタンだけが右へ押し出される。
  `ml-auto` はグループへ移す）。**並びは消しゴム（表示クリア）→ 輪で、輪が右端**
  （P4 で利用者裁定 2026-09-04「消しゴムの左より右がいい」。rev2 の「ボタンの左隣」を
  実機で覆した — 右端は「いま話しかけている相手の状態」の席）
- 輪はインライン SVG（`stroke-dasharray`）14px + `n%` の数字。**弧は `min(ratio, 1)` で
  描き、数字は丸めない**（rev2、査読 A-C — 1.0 を超える比をそのまま渡すと弧が 1 周を
  超えて描かれる）。色は `stroke="currentColor"` で親の `text-*` から引く（生の色を
  書かない — `ChatInput.vue:487-490` の規律）
- 色は純関数 `contextTone(ratio)`: `< 0.75` → `text-accent` / `< 0.90` → `text-warn` /
  それ以上 → `text-fail`（利用者の指定「75% 以上は黄、90% 以上は赤、通常は青」を
  境界値込みで固定）。**閾値は定数で設定にしない**
- tooltip（`title`）に `12,157 / 1,048,576 トークン（直近の呼び出し）` と絶対値を出す。
  % だけでは分母が読めない
- **輪を出さない条件は 3 つ**: 個体が選ばれていない / テンプレートが引けないか
  `contextLength` が 0 / **その個体がまだ 1 度も呼び出していない**（`lastPromptTokens`
  が `null`）。起動直後は 3 つ目に当たる（D6）

### D5: 運ぶ経路は既存の 2 本に 1 欄ずつ

`AgentSnapshot.lastPromptTokens: number | null`（`model.rs` + `world.rs:825-827` の投影 +
`types.ts`）と `AgentStatsUpdated.lastPromptTokens`（`event.rs` + `bootstrap.rs:331` +
`types.ts` + `useOrchestrator.ts:504-512` の `patchAgent`）。**IPC・CoreEvent の variant・
画面の新設は無い**。`AgentStatsUpdated` に乗せるのは、乗せないと `refreshAll` まで古いままに
なるから（Spec 39 rev2 で「ターン末の 1 通は毎秒の 1/数十以下」と数えた側）。

**「ターン確定後に個体が停止へ遷移して最終の 1 通が漏れる」は起きない**（査読 A-A /
B-2 は反証）— `is_active()` は `Starting | Running` = **起動している**個体で、ターンの
最中かどうかではない（`model.rs:124`）。個体はターンが確定しても `Running` のままなので、
次の tick（≤ 1 秒）で新しい値が乗る。停止は人の操作で、`stop_agent` は投影の
取り直し（`TopologyChanged` → `refreshAll`）を起こすので、確定の直後に停止された
場合も `AgentSnapshot` 経由で届く。**`settle_turn` の中で emit する案は採らない**
（4 出口の 1 実装に「イベントを撒く」責務が 1 つ増える。毎秒の tick で足りる）。

### D6: 再起動後は最初のターンまで出ない（利用者が負う条件）

`AgentRecord` はメモリの記録で `world.json` に写らない — **累計 4 欄（`uptimeSecs` /
`totalTokens` / `promptTokens` / `cachedTokens`）も同じく揮発**（`PersistedWorld` は
`agents` / `model_templates` / `topology_positions` / 予算しか持たない。査読 A-E の
「累計が永続化されているなら」の前提は成り立たず、揮発で統一されている）。再起動すると
`lastPromptTokens` は `null` に戻り、最初のターンが確定するまで輪が出ない。**`sessions.redb` の `Record::Turn` から
復元しない** — あの `prompt` はターンの合計（D1 の罠そのもの）で、直近の呼び出しの値は
どこにも保存されていない。保存する案（`TurnRecord` に `lastPrompt` を足す）は
「統計の記録に表示用の欄が 1 つ増える」形で、要るときに別 Spec。

### D7: 触る台帳

`data_contract.yaml`（`AgentSnapshot` の欄 + `:5023` の「カード向けに `promptTokens` /
`cachedTokens` を運ぶ」注記に 1 欄）/ `ipc_contract.rs`（literal + リスト）/
`types.ts` ×2 / DETAIL 日英（画面構成の会話ペインの行に 1 文）。README は触らない
（「何ができるか」の表に載せる大きさではない — `StatusBar` の時計と同じ扱い）。
**grep 網の外**（LP / Qiita）は画面要素の追加なので数えるが、嘘にはならない。

## Stories

- S1 個体を選ぶと、その個体の直近の呼び出しが窓の何 % かが入力欄の右下に出る
- S2 別の個体を選ぶと、その個体の値に切り替わる（ザリとルナで違う数字）
- S3 75% / 90% で色が変わり、窓が詰まっていることが数字を読まなくても分かる
- S4 検索や URL 取得で入力が膨らんだターンの後、1 秒以内に輪が追従する

## Phases

- **P0** — `data_contract` の凍結（`AgentSnapshot.lastPromptTokens` = 直近の LLM 呼び出しの
  `Usage.prompt`・累計ではない・失敗ターンでも書く・再起動で 0）
- **P1** — コア: `AgentRecord` / `TurnSpend.last_prompt: Option<u64>` / `settle_turn` の
  `if let Some` 代入 / `AgentSnapshot` の投影 / `AgentStatsUpdated`。結合 3 本（3 周のターンで
  `lastPromptTokens` が**最後の周の値**であって合計ではない、かつ `cached` 込みの値である /
  `OutputTruncated` の失敗ターンでも書かれる / **`usage` が返らない失敗ターンでは前回値が
  残る**）。`ipc_contract` の凍結を更新。**ミューテーション**: 代入を累積に → 1 本目だけ赤 /
  `if let Some` を外して `unwrap_or(0)` に → 3 本目だけ赤
- **P2** — フロント: `lib/contextUsage.ts`（`contextRatio(prompt, contextLength) → number | null` /
  `contextTone(ratio)` / `contextLabel`）+ 単体（境界値 0.75 / 0.90 ちょうど・分母 0・
  分子 0・100% 超）/ `ChatInput.vue` の輪 / `ChatPanel.vue` の配線（`:contextLength` と
  `:lastPromptTokens` を prop で渡す — `workDir` を prop で渡している前例に揃える）/
  `useOrchestrator.ts` の `patchAgent` / 辞書 ja/en（tooltip 1 鍵）
- **P3** — 台帳（D7）
- **P4** — 実機検収

## P1 実装記録（2026-09-04）

全 test binary 緑・workspace clippy 警告ゼロ。**ミューテーション 2 回とも予測どおり
1 本だけ赤** — 代入を累積へ → `last_prompt_is_the_final_round_not_the_turn_sum` だけ /
`if let Some` を外して `None` でも上書き → `a_turn_without_usage_keeps_the_previous_value`
だけ。

- **代入は `TurnSpend::absorb` の中に置いた**（rev2 の「`:1449` / `:1693` で代入」より
  1 段内側）。`absorb` は成功の応答と払ったと分かる失敗（`LlmError::usage()`）の
  **同じ入口**なので、`OutputTruncated` の周の入力が別経路を書かずに乗る（#103 の
  「経路を分けると片方だけが既定値のまま化ける」の逆をやらない）。`last_completion` の
  2 箇所は触っていない
- **`settle_turn` の書き込みは `if let Some(last) = spend.last_prompt`** — `None`
  （`usage` が 1 度も返らなかったターン）では触らない。結合テストで `Config` の失敗の後に
  `Some(250)` が残ることを留めた
- **結合テストで 1 つ踏んだ**: `Config` の失敗は `fatal` で個体が落ちるので、3 通目の
  `send_user_message` が `NotRunning`。`start_agent` で戻してから送る形にし、**その
  再起動で値が消えないこと**（消えるのはアプリの再起動だけ — D6）も同じテストで留めた
- **要約の呼び出し（`summarize_agents`）には載せていない** — ターンループの外の LLM
  呼び出しで、累計へは積むが `settle_turn` を通らない。要約の `prompt` は畳んだ履歴の
  大きさで「会話の窓の占有」と同じ意味ではあるが、頻度が低く手動なので今は書かない
  （要ると分かったら `summarize_agents` の累計加算の隣に 1 行）
- 触った箇所: `world.rs`（欄・既定・投影）/ `model.rs`（`AgentSnapshot`）/ `event.rs` +
  `bootstrap.rs`（`AgentStatsUpdated`）/ `turn.rs`（`TurnSpend` + `absorb` + `settle_turn`）/
  `ipc_contract.rs`（literal + 凍結リスト。**doc どおり P2 の 1 手目は `types.ts`**）/
  新設 `tests/context_usage_ring.rs`（2 本）

## P2 実装記録（2026-09-04）

vitest 458 → 466・`bun run build`（vue-tsc）緑。

- `lib/contextUsage.ts`（`contextRatio` / `contextTone` / `contextArc` / `contextPercent` +
  閾値の定数 2 つ）と単体 8 本（境界値 0.75 / 0.90 ちょうど・分母 0 と null・分子 0 は
  0 であって null ではない・1.0 超は切り詰めない・弧は 1 で止まる）
- `ChatInput.vue`: prop 2 本（`contextLength` / `lastPromptTokens` — `workDir` と同じく
  材料は prop で受けて IPC を呼ばない）+ computed `contextUsage` + 輪（インライン SVG
  14px・`r=5.5`・`stroke-dasharray` を周長で正規化・`rotate(-90)` で 12 時から）。
  **輪と「表示クリア」を `ml-auto flex items-center gap-2` のグループで包み、
  ボタンの `ml-auto` を外した**（rev2 B-1）。`data-context-usage` 属性は検収と
  将来の走査テスト用
- `ChatPanel.vue`: `targetTemplate`（`state.templates` を `modelTemplateId` で引く）を
  足し、2 prop を配線
- `types.ts` ×2 / `useOrchestrator.ts` の `patchAgent` / 辞書 ja/en 2 鍵
  （`chatInput.contextUsage` に分子・分母の絶対値、`contextUsageOver` に 100% 超の注記）
- **vue-tsc が `AgentSnapshot` の literal fixture 2 本を指した**（`agentSpec.test.ts` /
  `batchStart.test.ts`）— vitest は型検査をしないので緑のまま通り、`bun run build` だけが
  落ちる（Spec 45 P2 と同じ形）。`lastPromptTokens: null` を足した
- 数字の整形は `toLocaleString()` の既定ロケール（桁区切りだけが要件で、ja / en で
  同じ形になる）。時計の固定書式（`clock.ts`）とは要件が違う

## P3 台帳記録（2026-09-04）

- DETAIL 日英: 画面構成の表の「右」の行に輪の 1 文（分子・分母・色・**ターンの最中は
  動かない**（rev2 A-D）・100% 超の読み方・再起動後は出ない）+ ディレクトリ木に
  `lib/contextUsage.ts`
- `data_contract`: P0 で凍結済み（`AgentSnapshot.lastPromptTokens` / `contextLength` の
  読み手 / `agentStatsUpdated` / `observability`）。P3 で足すものは無かった
- README 3 言語: 触らない（D7。「何ができるか」の表に載せる大きさではない）
- CLAUDE.md: 「Spec の状態」
- **grep 網の外**: LP / Qiita は画面要素の追加なので嘘にならない

## 検収項目（各項目に到達経路を書く）

| # | 何を見るか | 到達経路 |
|---|---|---|
| 1 | ジェミーを選び 1 本依頼すると、右下に輪と `n%` が出て、`fuseforks.log` の**最後の `cache:` 行の `prompt=`** ÷ テンプレートの `contextLength` と一致する | 会話ペイン + ログ。**`turn:` の `prompt=` ではない**（合計）。`cache:` 行の `prompt=` は decode 後の `Usage.prompt`（`turn.rs:586`）= Spec 48 の畳み後の値で、輪の分子と同じ数（査読 B-5 は確認済み） |
| 2 | ザリを選ぶと別の数字に変わり、ジェミーへ戻すと元の数字に戻る | 一覧のクリック / Alt+↑↓ |
| 3 | 3.8 のテンプレートの `contextLength` を 12,000 にして保存 → 輪が黄（≥75%）または赤（≥90%）になり、`prompt` が超えていれば 100% 超の数字が赤で出る | モデル登録ダイアログ。実際の窓は 1,048,576 なので**設定を嘘にして色を踏む** |
| 4 | 検索が走ったターンの確定から 1 秒以内に輪が更新される（`AgentStatsUpdated` 経由。`refreshAll` を起こさない） | 検索 ON の個体へ依頼。時計と見比べる |
| 5 | アプリを再起動した直後は輪が出ず、最初のターンで出る | 再起動 |
| 6 | 個体を 1 体も選んでいないとき輪が出ない | 選択解除 |
| 7 | API キーを壊した個体へ依頼して失敗させても、直前の輪の数字が残る（消えない） | テンプレートの資格情報を一時的に不正に → `stop=failed:LLM_CONFIG` の後の輪 |

## P4 実機記録（2026-09-04〜。利用者検証）

- **検収 1・2 観測**（2026-09-04。debug ビルド）。ジェミーに依頼すると右下に輪と `n%` が
  出て、個体を切り替えると値が変わり、戻すと戻る（利用者「1, 2 は確認しました」）
- **検収 4・6 観測**（同日）。検索が走ったターンの確定から 1 秒以内に輪が動く
  （`AgentStatsUpdated` 経由。`refreshAll` 無し）/ 個体の選択を外すと輪が消える
  （利用者「6 も 4 も確認しました」）
- **検収 3・5・7 観測**（同日）。3 はスクリーンショット — ジェミーの `contextLength` を
  下げて **`91%` が赤（`text-fail`）**で出た（消しゴムの右。P4 で裁定した並び）。
  利用者「すべて確認はできました。色もね」
- **7 件すべて閉じた。** P4 で入った変更は 1 つ — 輪の位置を消しゴムの**右**へ
  （rev2 の「左隣」を実機で覆した。D4 に記録）
- **利用者の観察（同日）**: Anthropic の個体で使用率が減っていく —「圧縮機構が
  あるのか」。**村の機構**で、ツール結果はターン限り + 滑る窓 8 往復なので、ターンの
  1 周目は頭打ち（ザリ 55K 前後）、最後の周がそのターンの最大。輪は「直近の 1 呼び出し」
  なので実質そのターンの最大を見せる。**100% に近づくのはターンの中**（`file` ×12 周 /
  検索の注入 / 添付）で、超えると 400 で `stop=failed`（切り詰めの機構は無い）。
  効くのは 128K〜200K のテンプレートでツールを回す仕事、と D2 の診断

## Notes

1. **`contextLength` の自動取得は別 Spec の材料。** `prices.json` の出典 LiteLLM は
   `max_input_tokens` を持つので、単価の「取得」ボタン（Spec 41）と同じ経路で
   `contextLength` も埋められる。ただし `prices.json` は今その欄を運んでいない
   （生成側 = 利用者の別プロジェクト）。この Spec では手入力のままとし、輪が
   100% 超を出したら設定を疑う、という読み方を DETAIL に書く。
   **→ [Spec 50](50_context-length-fetch.md) で着地（2026-09-05）** — `prices.json` に
   `max_input_tokens` を加算し（Pages リポジトリに生成スクリプトを置いた）、「取得」が
   `contextLength` も入れる。**起票時の実測: 村の 16 テンプレート中 15 件が既定の 128,000 の
   ままで、実際の窓は 4〜8 倍**（輪はそれだけ大きく出ていた）
2. **ステータスバーに置かない理由は帯の規律。** 選択に依存する値は「常に見る必要」の
   検査を通らない。入力欄の下の行は「いま話しかけている相手」の文脈なので、そこが正
3. **カードにも出さない。** カードは全個体が並ぶ面で、9 体の輪が並ぶと地図と同じ
   「情報が 0 しか増えない」形になる（累計はカードが既に持つ）。要るなら別 Spec
4. **査読の反映（rev2・2026-09-04。2 系統 13 点）**

   | 系統-# | 指摘 | 扱い | 根拠 |
   |---|---|---|---|
   | A-A / B-2 | 確定後に個体が停止して最終の 1 通が漏れる | **反証** | `is_active` = `Starting \| Running`（起動している個体）。ターンの最中かどうかではなく、確定後も tick は続く。停止は人の操作で `refreshAll` が届く |
   | A-B | 代入箇所は 2 ではなく 3 | **反証** | `:1307` は `let mut last_completion = 0u64;` の宣言。代入は `:1449` / `:1693` の 2 箇所（プロバイダ分岐ではない） |
   | A-C | 1.0 超の比で弧が 1 周を超える | 採用 | 弧は `min(ratio, 1)`、数字は丸めない |
   | A-D | ターン中に動かないことを明記 | 採用 | DETAIL へ（P3） |
   | A-E | 累計が永続化されているなら不整合 | **反証** | `PersistedWorld` に累計 4 欄は無い。全部揮発で整合 |
   | A-F1 | 分子は `input + cacheRead + cacheWrite` | 確認済み | `Usage.prompt` が既にその合計（Spec 40 D1） |
   | A-F2 / 閾値 | 色の慣習・75/90 | 変更なし | 利用者指定のまま |
   | A-研究 1 | `toolUsePromptTokenCount` が `promptTokenCount` の内数なら二重加算 | **反証** | Spec 48 P0 probe 5 / (e) の恒等式（加算で `total` に一致。内数なら二重） |
   | A-研究 2 | `claude-sonnet-5` の窓は 200K ではない | **訂正して採用** | 確かめていない数字を書いていた。他社の窓は書かない（3.8 の 1,048,576 だけ文書で確認済み） |
   | B-1 | `ml-auto` の配置で輪が左に残る | 採用 | 輪とボタンをグループで包み `ml-auto` を移す |
   | B-3 | `usage` 無しの失敗ターンで 0 に上書きされ輪が消える | 採用 | `Option` にして `Some` のときだけ代入。検収 7 を新設 |
   | B-4 | `u64` の 0 を番兵にせず `Option` | 採用 | `Option<u64>` / `number \| null` |
   | B-5 | `cache:` 行の `prompt=` が畳み後の値か | 確認済み | `turn.rs:586` は `usage.prompt`（Spec 48 rev2 で確認したのと同じ行） |
