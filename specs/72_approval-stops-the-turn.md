# Spec 72: コマンドの承認待ちでターンを止める — 止め方と、個体ごとに 1 回の続行

- 起票: 2026-10-29
- 状態: **Done**（2026-10-30。**起票から Done まで 2 日**。P4 = 実機の検収 1〜8・10 を観測し、9 は結合テストに預けた。
  記録は「P4 実機記録」）← **rev3 承認 → P0 完了 → P1 完了**（2026-10-30。P0 = 契約を `data_contract` の 9 箇所 + Spec 56 / 64 / 39 へ凍結
  （「P0 契約記録」）。P1 = コア 20 ファイル + 結合 14 本 + 単体・ワイヤ凍結。GUI 以外の全テスト緑・clippy 0・
  **ミューテーション 6 回とも予測どおり赤**（1 回は変異を回す側の誤りで最初 0 本に見えた）。記録は「P1 実装記録」。**→ P2 完了**（同日。画面 = 型・受け口・ダイアログの「続けさせる」1 つ・帯の腕・セル色・統計・辞書。vitest 971 全緑・vue-tsc 0・build 緑・**変異 6 回とも予測どおり**。記録は「P2 実装記録」。**→ P3 完了**（同日。DETAIL 3 言語・README 3 言語に 1 句・CLAUDE.md の 3 か所・契約の取り残し 3 か所。記録は「P3 台帳記録」。次は P4 = 実機）← **Draft rev3**（2026-10-30。外部の査読 2 件 = 25 点 → **採用 20 / 前提を訂正して採用 3 / 反証 2**。表は Notes 4。
  rev2 で外していた重いもの 3 つ = 予定の後判定と完了後要約が承認待ちの停止を「終わった」と読んで再依頼・要約へ進む /
  会話の切り替えで印が残る / 印を外す点を `run_turn` の入口に置くと段 1〜2 の早期エラーで `Failed` と同時に立つ）
  ← rev2（2026-10-29。査読 2 系統 35 点 = 論理 19 / 技術 16、重なり 5 → 項目 30。採用 28 / 前提を訂正して採用 2 / 反証 0。
  表は Notes 3）。P0 には着手していない
- 起点: 利用者（2026-10-29）—「コマンド承認を求めたときにエージェントは止めたほうがいい。
  3 つコマンド承認を求められて承認実行したとすると、現行では同一エージェントで 3 つの処理が
  動いてそれぞれ回答を返す。これはおかしい」。同日の裁定 2 点 —
  **「止めたくないときは承認実行（Spec 61 の自動承認 / 自動許可）にしているので、一本で止めてよい」**。
  同じ日に出た要望 3 つのうちの 1 つ目。2 つ目（添付画像を複数枚）と 3 つ目（過去の添付を
  もう一度プロンプトで見せる）は Spec 23 D1 を覆す話なので**別の Spec**。
- 覆す Spec: [Spec 56](56_resume-after-approval.md) D1 / D3 / D6（ボタンは判断待ち 1 件ごと・押した回数 = 配送の通数・
  フロントの 2 段呼び出し）・D8（承認は成功したが配送に失敗したときの 2 段の文 — ボタンを割ると到達しない）・
  P2 実装記録 5（「`resumeAfterApproval` は一覧を引き直さない」）。
  [Spec 64](64_headless-host.md) D5 の `RUN_APPROVAL_REQUIRED` の理由文（「待ちにはならない」が嘘になる）。
  `data_contract` の既存規律「ツールの失敗で会話を止めない」に**ターンループの層で例外を 1 つ**作る（下の「覆す判断」）。
  Spec 15 / Spec 20 の「どちらにも無い = 拒否して `pending` へ積む」は変えず、
  **「積んだ周でターンを閉じる」を足す**（承認モードが `required` のときだけ）。

## rev2 からの変更（外部の査読 2 件の反映。詳細は Notes 4）

- **予定の後判定（Spec 46）と完了後要約（Spec 28）は、根のターンが承認待ちで止まったときは見送る**（D3）。どちらも
  `AgentTyping { active: false }` で発火し、再依頼の門は財布の残額だけなので、承認待ちの停止を「終わった」と読んで
  未承認のまま検収コマンドを走らせ、不一致で再依頼し、その入口で印を消していた。要約は承認を待つ途中の履歴を畳んでいた
- **印を外す点を `handle_message` の先頭へ**（D5）。`run_turn` の入口だと段 1〜2（個体・テンプレート・システムプロンプトの
  組み立て）の `?` で抜けた経路を通らず、`Failed` と印が同時に立つ — rev2 の「同時に立つことは無い」は誤りだった
- **会話の切り替え・新規チャットで全個体の印を外す**（D5 (d)）。切り替え先の無関係な会話に帯とボタンが残り、押すと注記も
  依頼も無い会話へ「続けてください」が届いていた
- **D1 の競合の説明を訂正** — `Unknown` の呼び出しは `Executed` に数えるので `executed_in_round >= 1` が常に成り立ち、
  RepeatGuard の打ち切りとは**構造的に同時成立しない**。同じ周の終わりで競合しうるのは cancel と最終周の上限到達
- **System 行の名指しは `{agent_id}（{display}）`**（既存 2 出口と同じ）、注記の N は `unknown=K` と同じ値（D3）
- **帯の文に「承認画面を開く」を入れない**（`ChatPanel` に独立したボタンが既にある）。判断待ちの帯を優先し、止まった個体が
  複数なら既存の One / Many の形（D6）
- **D9 の「新しい依頼を打てば続く」は `pending` の有無に依らない**と直し、「承認したのに実行されなかった」の読みを退けた —
  承認は `allow` に残り、消えるのはボタンだけ
- P0 / P2 / P3 の対象を名指しで足した — `session_store` の「7 値 / 4 出口」・`turnRecorded` の「4 出口」・
  `plan_verifier_contract` 4 の「閉じた 7 つ」・「引き直しは 3 経路」/ `stats.rs` の `stop_kind`・`types.ts` の `TurnStop`・
  `STATE_LABEL_KEYS`（鍵は `waves.state.*`）・`statsView.test` の `toHaveLength(7)`・ダイアログの空状態の `v-if`・
  `CommandPolicyView` のリテラルを持つテスト 3 本 / Spec 56 の D1・D6・P2 記録 5・DETAIL の句
- **反証 2** — README 3 言語に「承認して続けさせる」の句は無い（`grep` 0 件。足すのであって書き換えではない）/
  DETAIL.md の 382 行目は要約の節で、通知の一覧ではない

## rev1 からの変更（査読 2 系統の反映。詳細は Notes 3）

- **現況 8 と D8 の前提が誤っていた** — ヘッドレスの承認モードは `required` 固定ではなく `--run-approval` で選べ、
  起動前検査の警告 `RUN_APPROVAL_REQUIRED` は既に在る（`headless.rs:87`）。未決 2（警告を足すか）は問いが成立せず消した。
  腐るのは警告の理由文「待ちにはならない（未承認は拒否文が返ってターンは進む）」のほうで、P1 / P3 で書き換える
- **出口は 3 段ではなく 4 段**（D3）— 会話ログへの System 行が抜けていた。抜けたままだと利用者が直接頼んだターンは
  会話ペインに 1 本も出ず個体が黙る（Spec 56 の起点の形が戻る）
- **D4 の検証役の扱いが表の読み違いだった** — `verification_of` が写すのは**検証役自身**の `Reply.kind` で、ワーカーの結末ではない。
  `Err(None)` にすると検証役が止まっただけで束ねが丸ごと捨てられ「ユーザーの指示で打ち切り」と嘘が書かれる。
  `VerifySkip::ApprovalPending` を足す（閉じた 7 → 8）
- **印を消す側にも通知を出す**（D5）— 無いと「続けさせる」を押した直後も押せるまま残り、連打で 2 通目が出る
  （本 Spec が直そうとしている症状そのもの）
- **周の終わりの判定の順を実装の形で書いた**（D1）— 今の周の終わりに cancel の検査は無い。承認待ちの腕の中で
  cancel を先に見る 1 点を足す（RepeatGuard の既存の順は変えない）。最終周の `Unknown` も同じ腕で閉じ、
  まとめ呼び出しへ進まない
- **`author=core` はこの出口に無い**（D3 / D7）— あれは `dispatch_outcome` の `reply:` 行の欄で、早期終了の出口は
  そこを通らない。完遂の軸は `turn:` の `stop=approval_pending` で読む
- **検収の数字をモデルの振る舞いから機構の観測へ書き直した**（検収）— 「3 本」はモデルが 3 本並べて呼ぶ前提で、
  機構が保証するのは `turn cut:` の有無・`turn:` の本数・`turn start: from=system` の本数・`decision=Denied` の行
- **統計の `is_failure` とセルの色は別の分類**（D4 / D7）— 統計は「払ったが答えが無い」で真、セルは「次の手が人」で注意色
- 終了コードは新しい番号へ（未決 1 の推しを反転。Spec 64 D10 が「定型文を解析しないと結末が読めない形」を退けた
  論拠がそのまま当たる）/ 帯は承認画面を開くだけ（未決 3 を閉じた）/ 印は再起動で消える（D9 の条件へ）/
  `Malformed` を範囲外へ / `!broken` を押せる条件へ / 欄名 `pending=K` → `unknown=K` / ワイヤ凍結 3 点を P1 へ

## Goal

1. **承認が要る呼び出しが出た周で、そのターンを閉じる。** 今は `run` が「このターンでは実行できません。
   利用者へ依頼するか、別の手段で進めてください」を返してターンが続き、モデルは別の道具を試すか
   半端な答えを書く。承認後に続行させると**半端な答えと続行の答えが並ぶ**
2. **承認が揃ったら、続行は個体ごとに 1 通。** 承認を何件片付けても配送は 1 回で、再開したターンが 1 本走る
   （承認済みのコマンドをそのターンが全部呼び直すかはモデルの振る舞いで、機構は保証しない）
3. **止まった事実が、会話・承認画面・ログの 3 面に同じ言葉で出る。** 「止まったまま」ではなく
   「承認待ちで止まっている」と名指しする

**やらないこと（範囲外）**: `deny` / `Malformed` に当たった呼び出しで止めること（禁止と形の誤りは承認しても通らないので、
止めても人の手番が生まれない。今までどおり拒否文を返してターンは続く — Spec 61 D1 の「`Denied` と `Malformed` はどのモードでも
拒否」の線のまま）/ ターンを開けたまま承認を待つこと（下の「採らなかった形」）/ 承認の粒度（Spec 20 の 2 択のまま）/
承認を検知した自動の続行（押すのは人 — Spec 56 の凍結のまま）/ 財布の継承（Spec 56 D4 のまま。再開は新しい根・新品の天井）/
ヘッドレスに承認の口を足すこと（D8 は既存の警告の文面の更新まで）/ 添付の 2 件（別の Spec）。

## 現況（実装を数えた。2026-10-29。査読で file:line を突き合わせ済み）

1. **`run` は `Unknown` × `required` で `note_pending` → 拒否文を返し、`Ok(String)` で終わる**
   （`tools/run.rs:345`〜）。`call` の戻りは本文だけで、呼び出し側（`turn.rs` の `CallRunner::on_call`）は
   `CallOutcome::Executed` として数える。**周は続き、ターンも続く。** 拒否文の末尾は
   「**このターンでは実行できません。** 利用者へ依頼するか、別の手段で進めてください」（`unknown_refusal`）
2. **ターンの打ち切りの検査点は 3 つ** — ループの頭で cancel（`turn.rs:1527`）→ 予算の予約（`:1552`）、
   周の終わりで RepeatGuard（`executed_in_round == 0 && blocked_in_round`。`:1849`）。**周の終わりに cancel の検査は無い**
   （cancel は周の頭と LLM の `Err` 腕 `:1643` だけ）。予算の予約は LLM 応答の直後に `commit` 済み（`:1678`〜）なので、
   周の終わりに `ReservationGuard` は握られていない。優先順位は `token_budget.precedence` =
   cancel > budget_exhausted > RepeatGuard > hop / rounds 上限
3. **既存の早期終了の出口は 3 段 + 1** — `finish_interrupted`（`turn.rs:363`〜）と `finish_budget_exhausted`（`:502`〜）は
   (a) 会話ログへ System 行（`shared.record(AgentMessage::new(Endpoint::System, Endpoint::User, …))`）
   (b) 履歴の注記（`push_exchange` = モデルのプロンプト履歴にだけ入る）(c) `settle_turn`（払いの記録・`Record::Turn`）
   (d) `reply_to` への `Reply`。**`settle_turn` は会話ログに触れない**。どちらの出口も `reply:` 行は書かず、
   `author=model|core` は `dispatch_outcome` の `reply:` 行の欄（`turn.rs:3068`〜。印は `TurnProduct.written_by_core`）
4. **`TurnStop` は閉じた 7 値**（`session_store.rs:108`。`#[serde(tag = "kind")]`。`Record::Turn` に保存される）。
   `is_failure`（`:150`）は `!matches!(Completed | Repeat | ToolLimit)` で、**新しい値は黙って「失敗」に数えられる**。
   `statsView.ts:40` の `stopTone` も同じ境界。`own_line`（`turn.rs:261`）は `Interrupted | BudgetExhausted | ReserveShort` だけが
   自前の行で、それ以外は `turn:` 行
5. **`PlanTaskState` は 8 値**（`plan.rs:29`。`Interrupted` / `BudgetExhausted` が加算の前例）。`exit.rs:54`〜の `match` は
   網羅で書かれている（新 variant を足すとコンパイルが指す。`BudgetExhausted` = 9。10 / 11 は `bake` が使う）。
   `PlanWavePane.vue:113`〜の `cellClass` は `handed_off | interrupted` が注意色で **`default` が失敗色**（型は指さない）。
   `verification_of`（`delegation.rs:904`〜）が写すのは**検証役自身**の `Reply.kind`（`:1055` の `state` は検証役への
   `deliver_and_wait` の戻り）。`Err(None)` は `verify_bundle` → `execute_wave` が `None` → `run_plan` の
   「plan はユーザーの指示で打ち切られました」（`:524`）へ合流する
6. **`ToolOutput` は `{ text, images, dropped }`**（`tool.rs:41`）。`run` は `call` だけを実装し、`call_with_media` の
   既定実装が本文を包む。`on_call` の `Ok(output) => (output.text, output.images, output.dropped)`（`turn.rs:2455` 付近）は
   欄アクセスなので**新しい欄を黙って落とす**（`CallOutcome::Executed` の欄追加はコンパイルが指す）。
   `McpTool`（`mcp.rs:669`）は `call` が `call_with_media` に委譲する形
7. **承認画面はボタンが判断待ち 1 件ごとに 3 つ**（`CommandApprovalDialog.vue:208` の `v-for` の中に
   却下 / 承認 / 承認して続けさせる。`:267`）。「承認して続けさせる」は `decide(…, true, true)` で
   `approveCommand` → `resumeAfterApproval` を**押すたびに**呼ぶ。個体の節は `pending.length > 0 || broken` でだけ出る
   （`:172`）— **判断待ちが 0 件になると節ごと消える**。`refreshCommandRequests` の呼び出し点は起動時 /
   承認・却下のあと / `toolInvoked` で **`event.tool === "run"` のとき**（`useOrchestrator.ts:626`）の 3 つで、
   `resumeAfterApproval` の後には無い
8. **`resume_after_approval`（`settings.rs:571`）は呼ばれるたびに 1 通配送する。** 受信箱は直列なので
   3 通は 3 本のターンとして**順に**走る。利用者が見た「並列に 3 つ」は、画面で 3 つの答えが続けて出たことの言い方で、
   機構としては**直列の 3 ターン**。2 本目以降は「承認しました。続けてください」だけが届き、続けるものが無いまま走る
9. **`CommandPolicyView` は `{ agent_id, name, pending, broken }`**（`command.rs:216`）。組むのは
   `Orchestrator::command_policies`（`settings.rs:486`〜。`self.shared` を読める）。IPC 名は `list_command_requests`
10. **`run_turn` の呼び出しは `handle_message` の 1 箇所**（`turn.rs:1118`）で、全配送が `mod.rs:1527` の直列ループを通る。
    `handle_message` の段 1（`:963` 個体とテンプレート）と段 2（`:1043` システムプロンプト）は `?` で **`run_turn` の前に抜けうる** —
    抜けると `agent_loop`（`mod.rs:1557`〜）が `AgentFailed` を出して `Failed` へ遷移し、`stop_agent` は呼ばれない。
    `agent_loop` は cancel 済みの封筒を `handle_message` の前で畳む（`:1476`〜）。`delete_agent`（`lifecycle.rs:219`〜）は先に
    `stop_agent`（`runtime.rs:206`〜）を呼ぶが、`stop_agent` は `tasks` に居ないと `NotRunning` で早期に返る（`:210`）
13. **予定の後判定と完了後要約は `AgentTyping { active: false }` で発火する**（`schedules.rs:44`〜。`acceptance_step` /
    `summarize_causality`）。再依頼の門は `should_redeliver(outcome, attempt, max, has_budget_remaining)`
    （`schedule_probe.rs:265`）で、**財布の残額しか見ない** — 予算切れは残額 0 で止まるが、承認待ちの停止は残額が残る
14. **会話の切り替え（`Shared::open_session` `mod.rs:843`〜）と新規チャットは、稼働を保ったまま全個体の `history` を差し替える**
15. **画面の網羅と鍵** — `PlanWavePane.vue:93` の `STATE_LABEL_KEYS: Record<PlanTaskState, string>` の鍵は **`waves.state.*` の
    snake_case**（`plan.state.*` は無い）。`types.ts:1232`〜の `TurnStop` は union で `statsView.ts:26` の
    `STOP_LABEL_KEYS: Record<TurnStop["kind"], string>` が引く。`statsView.test.ts:295` は `toHaveLength(7)` を直書き。
    `stats.rs:383` の `stop_kind` は網羅 `match`。`CommandApprovalDialog.vue:165` の空状態は
    `every(v => v.pending.length === 0 && !v.broken)`。`ChatPanel.vue:1749` は帯の右隣に「承認画面を開く」のボタンを**常時**描く
    （辞書 `chat.approvalWaitingOne` / `approvalWaitingMany` / `approvalOpen`）。`CommandPolicyView` のリテラルを手書きする
    テストが 3 本（`useOrchestrator.commands.test.ts` / `approvalBanner.test.ts` / `approvalBannerWiring.test.ts`）
11. **ヘッドレス（Spec 64）の承認モードは `--run-approval required|auto-approve|no-approval` で選べ、既定は `required`**
    （`apps/cli/src/args.rs:353`〜、`run.rs:167` で `set_run_approval`）。起動前検査の警告 `RUN_APPROVAL_REQUIRED` は
    **既に在る**（`headless.rs:87`・`:444`〜。識別子の定数は 18 個）。その理由文は「待ちにはならない（未承認は拒否文が
    返ってターンは進む）」「未承認のコマンドは拒否され、誰も承認しません」で、**本 Spec の後は嘘になる**
12. **ワイヤの凍結**（`tests/ipc_contract.rs`）は `toolInvoked`（`:745`）と `PlanTaskState`（`:382`）だけで、
    `CommandPolicyView` と新しいイベントの凍結は無い。`useOrchestrator.ts` の `switch` に `default` も網羅検査も無い

## 覆す判断

- **Spec 56 D3「ボタンは判断待ち 1 件ごと。押した回数 = 配送の通数」を覆す。** あの規則は
  「2 件は『承認』で片付けて最後の 1 件だけ『承認して続けさせる』を押す」という**人の押し方**に寄りかかって
  いた。規則が UI に無く運用に住んでいたので、3 件とも「続けさせる」で片付けると 3 ターン走る。
  続行は**個体ごとに 1 つのボタン**にし、判断待ちが残っている間は押せない形にする（D5）
- **Spec 56 D8「承認は保存しました。続きは配送できていません」の 2 段の文は到達不能になる。** 1 操作で承認と配送の両方が
  起きる形が消えるため。辞書の `resumeFailed` は「続きは配送できていません（稼働中でないか、受信箱がいっぱいです）」の
  1 段へ書き換える（承認の成否を言わない）
- **Spec 64 D5 の `RUN_APPROVAL_REQUIRED` の理由文を書き換える。** 重さは Warn のまま。文は「`required` で `run` を持つ個体は、
  未承認のコマンドで**承認待ちのまま止まる**。`--run-approval` で変えるか、`run.json` の `allow` に書く」へ
- **Spec 56 D1 / D6 / P2 実装記録 5 も覆る** — 「判断待ちの行ごとに『承認して続けさせる』を足す」（D1）/ フロントが
  `approveCommand` → `resumeAfterApproval` の 2 段を 1 操作で呼ぶ（D6）/ 「`resumeAfterApproval` は一覧を引き直さない」（記録 5。
  本 Spec は押下後に引き直す）。取り消し線の対象に含める
- **「ツールの失敗で会話を止めない」規律にターンループの層で例外を 1 つ作る。** あの規律は**ツール層**のもの —
  失敗を `Ok(<エラー文>)` で返してモデルが次の手を選べるようにする（RepeatGuard が本文の一致で数えられるのも帰結）。
  本 Spec でも `run` は `Ok(String)` を返したまま変えない。閉じるのは**ターンループ**で、理由はツールの失敗ではなく
  **人の手番が生まれた**こと（予算切れ・割り込みと同じ列）。`data_contract` の規律の文（`tool_execution` の節）に
  「承認待ち（Spec 72）はツールの失敗ではなく人の手番で、ループの層で閉じる」の 1 文を足す
- **`command_tool_contract` の「どちらにも無い = 拒否して pending へ積む」に 1 文足す** —
  `required` では**積んだ周でターンを閉じる**。積む・拒否する・`deny` は変えない
- **`unknown_refusal` の末尾の 1 文を変える**（`required` のときだけ）。「別の手段で進めてください」は
  ターンが続く前提の案内で、閉じるなら嘘になる。`tool_spec_golden` には掛からない（提示文ではなく結果の本文）が、
  `tools::run` の単体の期待値は動く

## Design

### D1. 止める条件と点 — `required` で `Unknown` が出た周の終わり

止めるのは **承認モードが `required` で、その周の `run` の呼び出しに `Decision::Unknown` が 1 本以上あったとき**。
点は**周の終わり** — その周のツール結果を全部積んだ後、RepeatGuard の判定の前。

- **周の残りの呼び出しは走らせる。** 1 周に `run` を 3 本並べて呼び、1 本が `allow` に当たり 2 本が `Unknown` なら、
  1 本は実行し 2 本は `pending` へ積む（今と同じ）。**止めるのは次の周を回さないこと**で、周の途中で切らない。
  最初の 1 本で切ると、残り 2 本は `pending` に乗らず、承認が分かれる
- **呼び出しと結果の対は揃った状態で閉じる**（#29 の規律。cancel の検査点を周回境界に置いたのと同じ理由）
- **判定の置き場と順**（`turn.rs:1846` の RepeatGuard の判定の直前）:

  ```text
  if approval_pending_in_round > 0 {
      if turn.token.is_cancelled() { return finish_interrupted(…) }   // cancel 最優先
      return finish_approval_pending(…)                               // D3
  }
  // ↓ ここから既存のまま（RepeatGuard の判定 / 上限の到達）
  ```

  **今の周の終わりに cancel の検査は無い**（現況 2）ので、承認待ちの腕の中に 1 点足す。Spec 10 不変条件 1 の
  「周回境界」は「次の LLM 呼び出しを組み立てる前」で、周の終わりはその定義の内側。**RepeatGuard の既存の順は
  変えない**（承認待ちが無い周は今までどおり cancel を見ずに `break` → まとめ呼び出しへ進む）。
  `token_budget.precedence` へは **cancel > budget_exhausted > approval_pending > RepeatGuard > hop / rounds 上限** と足す。
  **同じ周の終わりで承認待ちと競合しうるのは cancel と最終周の上限到達の 2 つだけ** — 予算は周の頭の予約でしか起きず、
  RepeatGuard の打ち切りは `executed_in_round == 0` が条件だが、`Unknown` を返した `run` は `Executed` に数えるので
  承認待ちの周では必ず `executed_in_round >= 1` になり**構造的に同時成立しない**。契約には「周の終わりに予算の検査を
  足さない（頭で判定済み）」と注記する — 書かないと誰かが周の終わりに予算の検査を足す
- **最終周（`iteration + 1 == max_tool_iterations`）で `Unknown` が出ても同じ腕で閉じる。** 上限到達の
  `ToolLimitReached` は出さず、まとめ呼び出しへ進まない（当たったのは上限ではなく承認待ち。理由を 1 つに保つ —
  RepeatGuard が上限の通知を出さないのと同じ判断）
- **`auto_approve` / `no_approval` では何も変わらない。** `Unknown` の腕がそもそも `pending` へ行かない（Spec 61 D1）。
  **`deny` / `Malformed` でも止めない**（範囲外の理由）

### D2. 信号は型で運ぶ — `ToolOutput.approval_pending`

`ToolOutput` に `approval_pending: bool`（既定 `false`）を足す。`run` が `call_with_media` を実装し、
**`Unknown` × `required` の腕でだけ `true`** にする（`McpTool` が `call` を `call_with_media` に委譲している形を写す）。
`CallRunner::on_call` は `Ok(output)` の分解でこの欄も取り（現況 6 の黙って落とす点）、`CallOutcome::Executed` に乗せ、
周の終わりに `approval_pending_in_round` として数える。

- **本文の文字列で判定しない**（Spec 08 の凍結「分類は文言 parse でなく型で運ぶ」）。
  拒否文に「このターンでは実行できません」が含まれるかで止めると、MCP のツールが同じ文を返した日に止まる
- **`images` / `dropped` と同じ運び方**（Spec 70 が `ToolOutput` を足した経路に相乗り）。
  `CallOutcome::Blocked`（RepeatGuard が止めた）には乗せない — 止めた呼び出しは `run` を実行していないので
  `pending` も増えていない。prune の圧縮と RepeatGuard の `observe_result` は `text` しか読まないので影響しない
- **周の数は `pending` の件数で数えない。** `run.json` は人と機械が書くので、周の終わりに読み直すと
  その間の承認で減っていることがある。しかも `note_pending` は同一 `(command, args)` を `count++` で畳むので、
  同じ呼び出しが 3 本あっても判断待ちは 1 件。数えるのは**この周で `true` を返した本数**（計器の `unknown=K`）
- **`tools::run` の単体は `call_with_media` で見る** — `call` を呼ぶ既存の単体は旗を見ない

### D3. 出口 — `TurnStop::ApprovalPending`。4 段。まとめ呼び出しはしない

既存の早期終了の出口（`finish_interrupted` / `finish_budget_exhausted`）と同じ **4 段**で閉じる（`finish_approval_pending`）:

1. **会話ログへ System 行**（`Endpoint::System` → `Endpoint::User`。村の言語。名指しは既存 2 出口と同じ
   **`{agent_id}（{display}）`** — 表示名は一意でない）—
   ja「{agent_id}（{display}）は承認待ちのコマンドが N 件あり、このターンをここで止めました。承認が揃ったら「続けさせる」で再開します」/
   en「{agent_id} ({display}) stopped here: N command(s) await approval. Resume once they are settled.」。
   **N は `unknown=K` と同じ値**（この周で `Unknown` を返した本数。`finish_approval_pending` に渡す。`run.json` の
   `pending.len()` ではない — 畳みと承認で食い違う）。
   **これが無いと利用者が直接頼んだターンは会話ペインに何も出ず個体が黙る**（rev1 の抜け）。
   因果で 1 回の CAS（予算切れの `note_exhausted`）は要らない — 個体ごとの事実で、波の N 体が同じ文を出すのは正しい
2. **履歴の注記**（モデルのプロンプト履歴へ。コアが書く。村の言語）—
   ja「（承認待ちのコマンドが N 件あるため、このターンはここで止めました。利用者が承認すると、次のターンで実行できます）」/
   en「(Stopped here: N command(s) await the user's approval. They can run in the next turn once approved.)」。
   **依頼は未処理と書かない** — 周の途中で走った `allow` のコマンドは実行済みで、「未処理」は嘘になる。
   **画面の操作名を書かない** — この文は `ask_fuseforks` の呼び出し側とヘッドレスにも届き、そこに「続けさせる」のボタンは無い。
   コマンド名も書かない（承認画面に全部出る。履歴に残る文は短く）
3. **`settle_turn`** を `TurnStop::ApprovalPending` で呼ぶ（払いの記録・`Record::Turn`・`TurnRecorded`。
   `turn:` 行の `stop=approval_pending`）。**`rounds_cap` は `Some(max_tool_iterations)`、`spend.repeat_max` は写してから**
   （`own_line` の出口と違い `turn:` 行を書く側なので、`rounds=N/-` や `repeat_max=0` の嘘の観測を出さない。
   この理由を `finish_approval_pending` のコメントに残す）
4. **依頼主への返信**（`reply_to` があるとき）— 本文は「この依頼は承認待ちのコマンドで止まりました。答えはありません。」
   （既存 2 出口の文の形）、`kind` は `PlanTaskState::ApprovalPending`（D4）

**この出口は `reply:` 行を書かない**（既存の早期終了 2 つと同じ。`dispatch_outcome` を通らないので `author=core` も付かない）。
完遂の軸でこの未完遂を数えるのは **`turn:` の `stop=approval_pending`**。

**予定の後判定（Spec 46）と完了後要約（Spec 28）は、根のターンがこの出口で閉じたときは見送る。** どちらも
`AgentTyping { active: false }` で発火し（現況 13）、承認待ちの停止を「終わった」と読む —
(a) 後判定は未承認のまま検収コマンドを走らせ、不一致で `redeliver_for_acceptance` が再依頼を自動配送する（財布の残額は
残っているので門を通る）。再依頼のターンの入口で印が消え、`max_attempts` まで空転する
(b) 要約は承認を待つ途中の履歴（注記を含む）を畳んで `history.clear()` する。
処方は 1 つの述語 — `schedules.rs` の `AgentTyping { active: false }` の受け口で、その根の個体に
**`approval_stopped` の印が立っていれば後判定も要約も走らせない**。見送った事実は計器 1 行
`acceptance: … outcome=skipped reason=approval_pending`（`summarizeAfter` も同じ行の `summarize=skipped`）で残す
（黙って飛ばさない — #44）。~~承認して「続けさせる」で再開した次のターンの完了は普通に後判定・要約の対象になる~~
**→ P1 の実装で訂正（P4 で検収 9 を書く段に気づいた）**: 見送るときは後判定と要約の登録を捨てるので
（P1 実装記録）、続けさせたターン（`from=system` の新しい根で、予定の配送ではない）の完了では後判定も要約も走らない。
後判定はその予定の**次の発火**がやり直す

**稼働は降ろさない** — 閉じるのはターンだけで、個体は `Running` のまま（Spec 10 不変条件 4・`token_budget.exhaustion` と同じ）。
`AgentFailed` は出さず `last_error` にも書かない。D5 の `canResume`（running）はこれが前提。

**まとめ呼び出し（RepeatGuard の出口がする「ここまでで分かったことを書いてください」）はしない。**
理由は 2 つ — (a) 利用者の裁定は「止める」で、半端な答えを出さないことが目的。まとめはその半端な答えそのもの
(b) 払いが 1 呼び出し増える。再開したターンは履歴に注記と「承認しました。続けてください」を見て、
承認済みのコマンドを自分で呼び直す（Spec 56 の運用そのまま）。

**`TurnStop` は 7 → 8 値**（`log_label` = `approval_pending`）。**`is_failure` は真**（統計の `failed` の定義「払ったが答えが
無かった」どおり。`statsView.ts` の `stopTone` も同じ境界で失敗色）。**セルの注意色（D4）とは別の分類** — 統計は払いの結末、
セルは次の手が誰か。`stats.rs` の `is_failure` のテストへ 1 行。
`Record::Turn` に新しい `kind` が入るので、**旧版でこのレコードを含む会話の統計を読むと集計ごと失敗する**（Spec 63 の `assist` と
同じ形。「利用者が負う条件」へ）。

**`unknown_refusal` の末尾**（`required` のとき）: 「**このターンはここで止まります。** 利用者が承認すると、
次のターンで実行できます」。`auto_approve` / `no_approval` はこの関数に到達しない。

**予算の予約・清算は足さない** — 周の終わりに `ReservationGuard` は握られていない（現況 2）。`budget_reserve_wiring.rs` が
数える `try_reserve(` = 2 / `.commit(` = 4 / `budget_stop_reason` = 2 は動かない（P1 の予測）。

### D4. `PlanTaskState::ApprovalPending` — 人の手番の色。検証役の扱いは 2 つに分ける

`PlanTaskState` に `ApprovalPending` を足す（加算。serde は `approval_pending`）。

- **セルの色は注意色**（`Interrupted` / `HandedOff` と同じ腕。`cellClass` の `default` は失敗色なので腕を明示する）。
  刻み手は D3 の出口 1 箇所
- **束ねの本文は `Reply.text` そのもの**（`bundle_answers` は `## {id}（{display}）\n{body}` で状態別の文の表は無い）。
  見出しに表示名が在るので本文に `{name}` は書かない。D3 (4) の文がそのまま束ねに入る
- **ワーカーが `ApprovalPending` で返っても、束ねと検証は普通に走る。** `verification_of` が見るのは**検証役自身**の `Reply.kind`。
  **検証役が止まったとき**だけ `Err(Some(VerifySkip::ApprovalPending))`（閉じた 7 → 8。理由の文は
  「検証役が承認待ちのコマンドで止まった」）。`Err(None)` にしない — 束ねが丸ごと捨てられ「ユーザーの指示で打ち切り」と
  嘘が書かれる（rev1 の読み違い）
- **CLI の終了コードは新しい番号 12**（`exit.rs`）。`NoAnswer` の 6 へ畳まない — Spec 64 D10 が rev1 の「予算切れを 0」を
  退けた論拠（定型文を解析しないと結末が読めない形は Goal 5 と矛盾する）がそのまま当たり、`ApprovalPending` は `NoAnswer` と
  直し方が違う（`--run-approval` か `allow` の追記で確実に通る）。`--events jsonl` は任意の旗で代用にならない。
  `exit.rs:31` の「`bake` だけの番号は 10 番台」は「10 / 11 は `bake`」へ、Spec 64 D10 の表へ 1 行
- **`ask_fuseforks`（Spec 25）の呼び出し側**には D3 (4) の文がそのまま返る。扉の呼び出しは村の中では承認できないが、
  同じ端末の GUI で承認して「続けさせる」を押せば村の中で続く（答えは利用者へ戻る = Spec 56 と同じ）

### D5. 続行は個体ごとに 1 つのボタン。押せるのは判断待ちが 0 件のとき。印は両方向で通知する

- **コアは「承認待ちで止まった個体」を覚える** — `Shared.approval_stopped: Mutex<HashSet<AgentId>>`。
  出し入れは 1 実装 `set_approval_stopped(id, bool)` が担い、**変わったときだけ** `CoreEvent::ApprovalStopped { agentId, stopped }`
  を出す。立てるのは D3 の出口。**外すのは 4 点** — (a) その個体へ封筒が届いて **`handle_message` の先頭**に入った時
  （続行の配送でも人の新しい依頼でも予定の発火でも同じ。**`run_turn` の入口ではない** — 段 1〜2 の `?` で抜けた経路を
  通らず、`Failed` と印が同時に立つ。現況 10）(b) `resume_after_approval` の配送が成功した時（受信箱に入った時点。
  入口まで待つと押した直後の画面に印が残り、連打で 2 通目が出る）(c) `stop_agent` の受信箱を外す所と、`delete_agent`
  （`stop_agent` が `NotRunning` で早期に返る経路があるので両方に置く）(d) **会話の切り替えと新規チャット**
  （`open_session` / `reset_conversation`。全個体の印を外す — 切り替え先の無関係な会話に帯とボタンが残り、押すと注記も
  依頼も無い会話へ「続けてください」が届く。D9 が「どの会話の止まり方か」を決めないのと同じ理由で、印は会話に属する）。
  `agent_loop` が cancel 済みの封筒を畳む経路（Spec 10 2b）は `handle_message` に入らないので印は残る（意図）。
  **`Failed` と同時に立たない**のは (a) を先頭に置いたことの帰結 — 段 1〜2 で落ちても印は先に消えている。
  **ターンより長生きする状態だが、予算・打ち切り・hop の所有者ではない** — `pending` が `run.json` に残るのと
  同じ「人の手番を待つ印」で、Spec 56 D4 が退けた「財布の延命」とは別物。**メモリだけ**（D9）
- **`CommandPolicyView` に `stopped_for_approval: bool`** を足す。`command_policies` が `Shared` から写す
- **承認画面の個体の節は `pending.length > 0 || broken || stoppedForApproval` で出す。** 判断待ちを全部
  片付けても節は残り、「承認待ちで止まっています」の 1 行と **「続けさせる」ボタン 1 つ**が節の末尾に出る
- **押せる条件**は `stoppedForApproval && pending.length === 0 && !broken && canResume(status)`（`lib/resumeGate.ts` の
  純関数 `canResumeNow(view, status)`）。判断待ちが残っていれば灰色 + ツールチップ「判断待ちが N 件残っています。
  先に承認か却下をしてください」。**残っている間に続けさせても、再開したターンが最初の `Unknown` でまた止まるだけ**なので
  押させない。`broken`（`run.json` が壊れている）なら「`run.json` を直してください」 — 照合の読みが既定へ落ちて全部
  `Unknown` になるので、直す前に続けさせても同じ所で止まる。稼働中でなければ Spec 56 D5 の文言のまま
- **押した後は `busy` の間ボタンを殺し、戻りで `refreshCommandRequests`**。コアが (b) で印を外した `ApprovalStopped
  { stopped: false }` も来るので、**どちらか早いほうで節が畳まれる。答えはその後に来る**（配送は受信箱を経て非同期で、
  `turn start: from=system` は数秒後）— 「押したのに答えが来る前に節が消えた」はバグではなく仕様。`resumeGateWiring` の
  テストにこの順を明記する
- **空状態の文（「承認待ちのコマンドはありません」）の `v-if` にも `!stoppedForApproval` を足す** — 節の `filter` だけ広げると、
  判断待ちを全部承認した瞬間に空状態の文と「止まっています + 続けさせる」の節が同じ画面に並ぶ
- **判断待ち 1 件ごとの「承認して続けさせる」は撤去する。** 「承認」「却下」は残る。
  辞書の `commandApproval.approveAndResume` は消し、`resume` / `stoppedNote` / `resumeBlockedPending` / `resumeBlockedBroken` を足す
  （`usedKeys.test.ts` と `resumeGateWiring.test.ts` が追従する）。`resumeFailed` は 1 段の文へ（覆す判断）
- **コア側の門は変えない。** `resume_after_approval` は Spec 56 のまま（受信箱の有無だけ）。判断待ちが
  残っていても呼べる — 画面の `disabled` は導線であって保証ではない（Spec 56 D5 と同じ線）。
  **コアで「判断待ちが 0 件か」を門にしない**のは、承認と配送を別の操作に保つため（Spec 56 D6）。
  **代償**: 画面を迂回して呼び続けると、再開 → 最初の `Unknown` でまた `stop=approval_pending` を繰り返し、
  `is_failure` が真なので統計の `failed` がそのぶん増える。意図した数え方で、`stats.rs` のテストのコメントに残す
- **押さずに人が新しい依頼を打ったら**、その依頼のターンの入口で印が消え `ApprovalStopped { stopped: false }` が出て、
  節は判断待ちが 0 件なら消える。再開を強制しない
- **フロントの `switch` に `approvalStopped` の `case` を足す**（`default` も網羅検査も無いので忘れても黙る —
  走査テストで留める）。`toolInvoked` の経路は `Unknown` の呼び出しでも出ているので判断待ちの件数は今も動くが、
  止まった印は `run.json` に無いので別のイベントが要る

### D6. 承認待ちの帯（2026-10-19）との整合

入力欄の上の帯は `pendingTotal(views)` で数える。判断待ちを全部片付けた後は 0 件になるので帯は消えるが、
**止まった個体が残っている間は「{name} が承認待ちで止まっています」の帯を出す**（`approvalBanner` に
`stoppedForApproval` の腕を 1 つ）。

- **文に「承認画面を開く」を入れない** — `ChatPanel` は帯の右隣に「承認画面を開く」のボタンを常時描いている（現況 15。
  既存の `approvalWaitingOne` も文に入れていない）。入れると二重に出る。開く先は同じ承認画面で、**帯から直接続けさせない** —
  判断待ちが 0 件かの確認を飛ばして押せる形にしない
- **優先は判断待ち > 止まった個体。** 村に判断待ちが 1 件でもあれば既存の帯（`approvalWaitingOne` / `Many`）を出し、
  全件片付いてから止まった個体の帯へ切り替わる（承認が先、続行が後、の順を帯も示す）
- **止まった個体が複数なら既存と同じ One / Many の形**（`approvalStoppedOne` = 「{name} が承認待ちで止まっています」/
  `approvalStoppedMany` = 「{name} ほか {others} 体が承認待ちで止まっています」。`approval_stopped` は集合なので複数が普通に起きる）

帯を出さないと「承認したのに何も起きない」が Spec 56 の起点の形で戻ってくる。

### D7. 計器

- 止めた周に 1 行: `turn cut: agent=… round=N reason=approval_pending unknown=K`
  （`reason=repeat` と同じ接頭辞。`K` はこの周で `Unknown` を返した本数 = D2。**`pending=` と書かない** —
  `run decision:` の `pending=` と `resume after approval:` の `pending_left=` はファイルの件数で、畳みと上限 20 で
  K と食い違う。同じ語で違う量を書くと grep で取り違える）
- `turn:` 行の `stop=approval_pending`（`settle_turn` の 1 実装）。`observability_rule` の使用量の行は**出口 5 つ**
  （完走 / 失敗 / 割り込み / 予算 / **承認待ち**）。接頭辞は `turn:` — 割り込みと予算が自前の行を持つのは Spec 39 より前に
  生まれたからで、新しい出口は `Completed` / `Repeat` / `ToolLimit` と同じく `turn:` の `stop=` で読む（Spec 39 D1 の表に 1 行）
- `resume after approval:` は Spec 56 のまま。`pending_left=0` が常態になる（画面が 0 件まで押させないため）。
  0 でない行は人が `run.json` を手で触ったか、画面の導線を迂回した印
- 統計画面の `stop` の辞書（`statsView.ts` の表）に `approval_pending` → 「承認待ち」。色は失敗側（D3）

### D8. ヘッドレス（Spec 64）— 既存の警告の文面を更新する

ヘッドレスの承認モードは `--run-approval` で選べ、既定は `required`（現況 11）。`required` で `run` を持つ個体が
`Unknown` を踏むと **`ask` は承認待ちで止まり、終了コード 12 で返る**。今は黙って続いていたので、挙動としては**正直になる側**
（走っていないコマンドを走ったように答えない）。直し方は 2 つ — `--run-approval auto-approve|no-approval` で起動し直す /
`run.json` の `allow` を書いて `ask` し直す（Spec 15 の直接編集の経路）。

- **起動前検査の `RUN_APPROVAL_REQUIRED` は既に在る**（Warn）。足すのではなく**理由文を書き換える** —
  `headless.rs:444` のコメント「待ちにはならない（未承認は拒否文が返ってターンは進む）」と `:462`〜の文面
  「未承認のコマンドは拒否され、誰も承認しません」→「未承認のコマンドで承認待ちのまま止まります。`--run-approval` で
  変えるか、`run.json` の `allow` に書いてください」。重さは変えない（拒否にしない — `allow` を書けば通る）
- Spec 64 D5 の表のその行と `headless_host_contract` 6 の理由文も同じ文へ（P0 / P3）

### D9. 旧版との互換と、利用者が負う条件

- **v0.6.4 以前でこの版の会話を開くと、`approval_pending` の `Record::Turn` を含む会話は「全会話」の統計が
  集計ごと失敗する**（レコードは消えない。Spec 63 の `assist` と同じ）
- **`PlanWaveRecord` はメモリのリング**なので保存の互換は無い
- **承認画面の「承認して続けさせる」が無くなる。** 判断待ちを全部片付けてから「続けさせる」を 1 回押す
- **止まった印は再起動と会話の切り替えで消える**（メモリだけ）。再起動後は節に「承認 / 却下」だけが出て、全部片付けると
  節は消え、「続けさせる」は出ない。**失われるのはボタンだけで、承認は失われない** — 承認した行は `run.json` の `allow` に
  残っているので、その個体へ新しい依頼を打てば（`pending` の有無に依らず）そのターンで実行できる。
  「承認したのに実行されなかった」ではなく「承認したが、続行は自分の依頼で起こす」。
  `Record::Turn` から印を組み直す形は採らない — 会話を切り替えた村で「どの会話の止まり方か」を決める規則がもう 1 つ要る
- **ヘッドレスの `ask` が承認待ちで止まるようになる**（D8）。今まで答えが返っていた村で終了コード 12 が出うる
- **委譲（`ask` / `plan`）で止まったワーカーを続けさせると、答えは元の依頼主ではなく利用者へ届く**（P4 検収 5 で観測）。
  止まった時点で依頼主には D3 (4) の文が返って依頼は閉じており、続行は新しい根（Spec 56 D4）で戻り口を持たない。
  依頼主にその答えを使わせたいときは、続けさせた後に依頼主へ頼み直す

## 採らなかった形

- **ターンを開けたまま承認を待つ**（human-in-the-loop の `interrupt()` の形）— 予算・打ち切り・hop の所有者が
  ターン建てで、待つ間の予約や受信箱の扱いを決め直すことになる（対話シェル A 案 / Realtime API / 永続 REPL を
  退けた理由と同じ）。承認までの時間は人の都合で分単位から日単位まで動く
- **最初の `Unknown` で周の途中で切る** — 並列に呼んだ残りが `pending` に乗らず、承認が分かれる（D1）
- **まとめ呼び出しをして「ここまで」を書かせる** — 半端な答えそのもの（D3）
- **コアで `pending` が 0 件になったら自動で続行する** — Spec 56 の凍結「押すのは人」。線は人が引く
- **「全部承認して続けさせる」ボタン** — まとめて承認は粒度の決定を飛ばす（`command_tool_contract` の凍結）
- **`deny` / `Malformed` でも止める** — 承認しても通らないので人の手番が無い
- **`resume_after_approval` に「判断待ちが残っていれば断る」門を足す** — 承認と配送を別の操作に保つ（D5）
- **終了コードを `NoAnswer` の 6 へ畳む** — 直し方が違う結末を同じ番号にすると CI が分けられない（D4）
- **止まった印を `Record::Turn` から再起動後に組み直す** — 会話をまたぐ規則がもう 1 つ要る（D9）
- **帯から直接「続けさせる」** — 判断待ち 0 件の確認を飛ばせる（D6）

## Tasks

### P0 — 契約（**完了 2026-10-30**。下の「P0 契約記録」）

- [x] `data_contract` の `command_tool_contract` 末尾へ本 Spec の節（止める条件・点・信号の運び方・4 段の出口・後判定と要約の見送り・
  個体ごとの続行・押せる条件・印の出し入れ 4 点・稼働は降ろさない）。同じ節の「承認して続けさせる（Spec 56）」の
  冒頭文・3 項目目（1 件ごと = 1 通）・7 項目目（承認と配送は別の操作の 2 段の文）に取り消し線と「→ Spec 72」。
  「**判断待ちの引き直しは 3 経路**」を 5 経路へ（+ `approvalStopped` の受信 / `resumeAfterApproval` の戻り）
- [x] `token_budget.precedence` へ `approval_pending`（cancel > budget_exhausted > **approval_pending** > RepeatGuard > hop / rounds）
  と「周の終わりに予算の検査は足さない（頭で判定済み）。RepeatGuard とは構造的に同時成立しない」の注記。
  `exhaustion` の隣へ「承認待ちも閉じるのはターンだけ」
- [x] `tool_execution` の「ツールの失敗で会話を止めない」規律に例外の 1 文（覆す判断）
- [x] `TurnStop` 7 → 8 値（`is_failure` 真）: `enums.ConfigFileKind.session_store` の「`stop` は閉じた列挙 7 値」と
  「4 出口すべてで 1 件」、`core://event` の `turnRecorded` の「(4 出口すべて)」、`observability_rule` の使用量の行 —
  **「4 出口」は 3 箇所、「7 値」は 1 箇所**（数えてから書く。#62 の形）。Spec 39 D1 の表に 1 行
- [x] `PlanTaskState` に `approval_pending`（色表に注意色・刻み手は周の終わりの出口）。`plan_verifier_contract` 4 の
  「理由は閉じた 7 つ」→ 8 つ（`VerifySkip::ApprovalPending`）。`events` に `approvalStopped { agentId, stopped }`。
  `CommandPolicyView` に `stoppedForApproval`。`turn cut:` に `reason=approval_pending unknown=K`。
  `acceptance:` に `outcome=skipped reason=approval_pending`
- [x] `headless_host_contract` 6 の `RUN_APPROVAL_REQUIRED` の理由文と、Spec 64 D5 の表の行・D10 の表に 12 の行
- [x] Spec 56 の D1 / D3 / D6 / D8 / P2 実装記録 5 に覆しの追記（取り消し線。**凍結を覆すときは凍結の側にも書く**）

### P0 契約記録（2026-10-30）

**書いた場所（`data_contract.yaml` 9 箇所 + Spec 3 本）**:

| 場所 | 変更 |
|---|---|
| `enums.ConfigFileKind.session_store` | 「4 出口」→ 5 出口（2 箇所）/ `stop` 7 値 → 8 値 + `approval_pending` の性質（`is_failure` 真・旧版の互換） |
| `PlanTaskState` | `values` に `approval_pending` / 色表に注意色の行 / 刻み手と `Reply.text` |
| `token_budget.exhaustion` / `precedence` | 稼働を降ろさない / 順序 + 「周の終わりに予算の検査は足さない」「RepeatGuard とは構造的に同時成立しない」 |
| `events` | `approvalStopped { agentId, stopped }`（両方向で出す理由）/ `turnRecorded` の「4 出口」→ 5 |
| `Acceptance.instrumentation` | `outcome=skipped reason=approval_pending` の例外 |
| `plan_verifier_contract` 4 | 「閉じた 7 つ」→ 8 つ（`VerifySkip::ApprovalPending`。`verification_of` は検証役自身の `Reply.kind`） |
| `tool_execution`（RepeatGuard の規律） | 「ツールの失敗で会話を止めない」規律のループ層での例外 1 文 |
| `command_tool_contract` | 「3 経路」→ 5 経路 / Spec 56 の節の冒頭文・1 件ごと = 1 通・2 段の文に取り消し線 / **新節「承認待ちでターンを止める（Spec 72）」**（D1〜D9 の凍結・採らなかった形まで） |
| `headless_host_contract` 6 | `RUN_APPROVAL_REQUIRED` の理由文の書き換え |
| Spec 56 | 冒頭の注記 + D1 / D3 / D6 / D8 / P2 実装記録 5 に取り消し線と「→ Spec 72」 |
| Spec 64 | D5 の表の行（理由文）/ D10 の表に 12 |
| Spec 39 | D1 の表に `approval_pending` の行・見出しの 7 → 8 / D2 の見出しに「→ 5 出口」 |

**数えてから書いた**（#62 の形）— 「4 出口」は `data_contract` に 3 箇所（`session_store` 2 + `turnRecorded` 1）、「7 値」は 1 箇所。
Spec 39 の本文に残る「4 出口」（D2 の本文・P 記録）は当時の記録なので触らず、D1 の表と D2 の見出しだけ現在へ揃えた。

**P1 の入口**: `tool.rs` の `ToolOutput.approval_pending` → `run.rs` の `call_with_media` → `turn.rs` の周の終わり（`:1846` の直前）。
`budget_reserve_wiring.rs` の数は動かない、が最初の予測。

### P1 — コア（**完了 2026-10-30**。下の「P1 実装記録」）

- [x] `tool.rs`: `ToolOutput.approval_pending`。`run.rs`: `call_with_media` を実装し `Unknown` × `required` で `true`。
  `unknown_refusal` の末尾を `required` の文へ。`tools::run` の単体は `call_with_media` で旗を見る
- [x] `turn.rs`: `on_call` の `Ok(output)` の分解で欄を取り `CallOutcome::Executed` に乗せる → 周の終わりで数える →
  承認待ちの腕（cancel → `finish_approval_pending` の 4 段）を RepeatGuard の判定の直前に。`finish_approval_pending` は
  `rounds_cap: Some(max)`・`repeat_max` を写してから `settle_turn`。最終周でも同じ腕
- [x] `Shared.approval_stopped` + `set_approval_stopped`（変化時だけイベント）。立てる: 出口 / 外す: **`handle_message` の先頭**・
  `resume_after_approval` の配送成功・`stop_agent` の受信箱を外す所・`delete_agent`・`open_session` と `reset_conversation`（全個体）
- [x] `schedules.rs`: `AgentTyping { active: false }` の受け口で、根の個体に印が立っていれば `acceptance_step` と
  `summarize_causality` を見送り、計器 1 行
- [x] `session_store.rs`: `TurnStop::ApprovalPending`（`log_label` / `is_failure`）。**`stats.rs:383` の `stop_kind` の腕**（網羅 `match`）。
  `plan.rs`: `PlanTaskState::ApprovalPending` + serde の wire テストの列挙。`delegation.rs`: `VerifySkip::ApprovalPending` と
  `verification_of` の腕・理由の文。`command.rs`: `CommandPolicyView.stopped_for_approval`（`command_policies` が写す）。
  `event.rs`: `ApprovalStopped`。`headless.rs`: `RUN_APPROVAL_REQUIRED` の文面。`apps/cli/src/exit.rs`: `APPROVAL_PENDING = 12` と
  `match` の腕・コメント「10 / 11 は `bake`、12 は `ask` の承認待ち」
- [x] 計器 `turn cut: … reason=approval_pending unknown=K`。`stats.rs` の `is_failure` のテストへ 1 行（迂回で `failed` が増えるのは
  意図、のコメント込み）
- [x] テスト（結合。`FixedBackendFactory` でツール呼び出しを返すスタブ。観測は**機構が保証するもの**だけ）:
  (1) `required` で `Unknown` 1 本 → `turn cut: reason=approval_pending unknown=1` 1 行・次の周の LLM 呼び出しが無い・
  `turn:` が `stop=approval_pending` で `rounds=N/max`・会話ログに System 行 1 本・履歴の末尾がコアの注記
  (2) 同じ周に `allow` 1 本 + 別々の `Unknown` 2 本 → `allow` は実行され `run decision: decision=Unknown` が 2 行・`pending` は 2 件・`unknown=2`
  (3) `auto_approve` / `no_approval` で同じ呼び出し → `turn cut:` 無し
  (4) `deny` / `Malformed` → `turn cut:` 無し
  (5) 承認待ちの周で cancel 済み → `turn interrupted:`（`turn cut:` 無し）
  (6) 最終周で `Unknown` → `stop=approval_pending`・`ToolLimitReached` 無し・まとめ呼び出しの LLM 要求が無い
  (7) `ask` 経由で止まる → 依頼主の `Reply.kind == ApprovalPending`・本文が D3 (4) の文
  (8) `plan` の波でワーカーが止まる → 束ねにその文が入り、検証役（居れば）が走る / 検証役自身が止まる → `VerifySkip::ApprovalPending`
  (9) 止まった後に `resume_after_approval` → `ApprovalStopped { stopped: false }` が出て `command_policies` の欄が偽
  (10) 止まった後に人の依頼が届く → `handle_message` の先頭で印が消える。**段 1 で落ちる依頼**（テンプレートを消してから届ける）でも
  印は消え `Failed` と同時に立たない
  (11) `run` 以外のツールが拒否文と同じ文字列を返しても止まらない（型で運ぶ = D2）
  (12) ワイヤ凍結（`tests/ipc_contract.rs`）: `approvalStopped` / `PlanTaskState` の新値 / `CommandPolicyView.stoppedForApproval`
  (13) 後判定つきの予定（`maxAttempts: 2`）の根のターンが承認待ちで止まる → `acceptance: … outcome=skipped reason=approval_pending` が
  1 行・`redeliver` 無し・印は立ったまま。`summarizeAfter: true` でも `summarize:` が出ない
  (14) 止まった後に `open_session` → 全個体の印が消え `ApprovalStopped { stopped: false }`
  ミューテーションは (1)(2)(3)(5)(6)(8)(9)(10)(11)(13) を予測して回す。`budget_reserve_wiring.rs` の数は動かない（予測）。
  (2) のテストの 2 本は**別々のコマンド**にする（同じなら `note_pending` が畳んで `pending` は 1 件 — コメントに残す）

### P1 実装記録（2026-10-30）

**触ったコア 20 ファイル**（`git diff --stat` で 28 ファイル・+803 / −70。うち台帳 5）: `tool.rs`（欄）/ `tools/run.rs`
（`call_with_media` を実装、`call` は委譲。`unknown_refusal` の末尾）/ `mcp.rs`（リテラルに欄）/ `orchestrator/turn.rs`
（`handle_message` の先頭で印を外す・`CallOutcome::Executed` の欄・`on_call` の分解・周の終わりの腕・`finish_approval_pending`）/
`orchestrator/mod.rs`（`Shared.approval_stopped` と出し入れ 3 本・`open_session` で全個体を外す）/ `bootstrap.rs` /
`settings.rs`（`command_policies` が写す・`resume_after_approval` の配送成功で外す）/ `runtime.rs`（`stop_agent`）/
`lifecycle.rs`（`delete_agent`）/ `sessions.rs`（保存先の無い村の `reset_conversation`）/ `schedules.rs`（見送りと計器）/
`session_store.rs`・`stats.rs`・`plan.rs`・`delegation.rs`・`event.rs`・`command.rs`・`headless.rs`・`apps/cli/src/exit.rs`。

**テスト**: 結合 `approval_stops_turn.rs` 12 本（検収 (1)〜(11) と (14)。(13) は後判定）+ ログ 2 ファイル
（`approval_stops_turn_log.rs` = `turn cut:` と `turn:` の分母 / `approval_skips_acceptance_log.rs` = 後判定と要約の見送り）+
`ipc_contract.rs` にワイヤ凍結 2 本（`approvalStopped` / `CommandPolicyView`）+ 単体（`run` の旗 2 本・`PlanTaskState` の wire・
`VerifySkip` 8 つ・`stats` の `failed` と `by_stop`・`exit.rs` の 12）。**GUI 以外の全テスト緑・clippy 0**（`fuseforks` crate は
開発ビルドの exe を消せないので除外 — README の注記どおり）。

**ミューテーション 6 回 — 6 回とも予測どおり赤**（予測を先に書き、置換で入れて置換で戻し SHA-256 で復元を確認）:

| 変異 | 予測 | 実測 |
|---|---|---|
| M1 `run` が旗を立てない | 11（結合 8 + ログ 2 + 単体 1） | 11 ✓ |
| M2 腕の中の cancel を外す | 1（検収 5） | **最初は 0 → テストを直して 1** ✓（下） |
| M3 `handle_message` の先頭で外さない | 1（検収 10） | 1 ✓ |
| M4 後判定の見送りを外す | 1（後判定のログ） | 1 ✓ |
| M5 分母を `None` | 1（`turn cut:` のログ） | 1 ✓ |
| M6 外す側でイベントを出さない | 3（9・10・14） | 2（9・10）— 14 は `clear_approval_stopped` が別に出すので私の予測が外れ |

**変異を回す側で 2 回躓いた**（どちらも結果の読み違いになる形）: (a) 最初の走行は M2〜M6 が全部 0 本だった —
`cargo test … --test A --test B --lib -- tools::run::` の**名前フィルタが結合テストのバイナリにも掛かり、1 本も走っていなかった**
（#133 の形。走った本数を出力させて気づいた。結合と単体を別の実行に分けた）(b) **検収 5（cancel 優先）は腕の中の cancel を
外しても緑だった** — `AgentTyping` を見て割り込みを立てると、割り込みが**周の頭の検査（`turn.rs:1631`）より先に立ち**、承認待ちの
腕へ届く前に閉じていた。LLM 呼び出しに**入った**合図（`entered`）で待ってから立てる形に直し、`ToolInvoked` が出ていること
（= 周の途中では切らず、分類は周の終わり）を足して判定にした。**一発で緑になったテストは機構を壊して赤を確かめる、が効いた 2 例目**
（Spec 23 の JPEG フォールバック以来）。

**実装で決めた・分かった 4 点**:

- **`ToolOutput` のリテラルは村に 4 箇所**（`run.rs` / `mcp.rs` / テスト 2 本）。欄を足すとコンパイラが全部指す — `on_call` の分解
  （`output.text, output.images, …`）だけは指さないので、D2 のとおり手で足した
- **`Shared.approval_stopped` は `std::sync::Mutex`**（`run_approval` の `AtomicU8` の隣。非同期の待ちを挟まない短い区間だけ）。
  毒されても中身を使う（`tool_calls_lock` と同じ理由 — 表示用の印で、壊れて困る不変条件は無い）
- **`reset_conversation` は保存先の有無で 2 経路**（保存先があれば `open_session` 経由で外れる。無い村の枝にも 1 行）
- **後判定の見送りは登録を捨てる**（`pending_acceptances` / `pending_summaries` から外す）。残すと、承認して続けさせた次のターンの
  完了で**止まる前の試行の検収**が走る。見送った予定は次の発火がやり直す（取りこぼしの規律と同じ）

**この P1 で踏んだ作業の罠**: D: の空きが 622 KB で `link.exe` が 1104 → `target/debug/incremental` が 44.9 GB だった。
消して 35 GB（フルビルドで約 30 GB を使うので、P2 の前にもう一度見る）。

### P2 — 画面（**完了 2026-10-30**。下の「P2 実装記録」）

- [x] `types.ts` の `CommandPolicyView.stoppedForApproval` / `PlanTaskState` / **`TurnStop` の union に `approval_pending`** /
  イベント。`useOrchestrator`: `approvalStopped` の `case` → `refreshCommandRequests`（走査で留める）
- [x] `CommandApprovalDialog.vue`: 節の表示条件（`:172` の `filter` と **`:165` の空状態の `v-if` の両方**）・判断待ち 1 件ごとの
  「承認して続けさせる」を撤去・節末尾の「続けさせる」1 つ・押下後の取り直し（`lib/resumeGate.ts` に `canResumeNow(view, status)` =
  止まっている && 判断待ち 0 && !broken && 稼働）
- [x] `approvalBanner.ts`: 止まった個体の腕（D6。判断待ち優先・One / Many・文に「開く」を入れない）。`statsView.ts`:
  `STOP_LABEL_KEYS` に `approval_pending` と `stopTone`。`PlanWavePane.vue`: **`STATE_LABEL_KEYS` に `approval_pending:
  "waves.state.approval_pending"`**（型が網羅を要求）と `cellClass` の注意色の腕
- [x] 辞書 ja / en（`commandApproval.resume` / `stoppedNote` / `resumeBlockedPending` / `resumeBlockedBroken` /
  `stats.stop.approvalPending` / **`waves.state.approval_pending`** / `chat.approvalStoppedOne` / `approvalStoppedMany`）。
  `approveAndResume` を消す。`resumeFailed` を 1 段の文へ
- [x] 既存テストの追従: `statsView.test.ts` の `toHaveLength(7)` → 8 と `stopTone` の期待値 / `CommandPolicyView` のリテラルを持つ
  `useOrchestrator.commands.test.ts`・`approvalBanner.test.ts`・`approvalBannerWiring.test.ts`
- [x] 走査: `resumeGateWiring` を新しい条件へ（節が畳まれる → 答えが来る、の順も明記）/ `approvalBannerWiring` に腕 / `usedKeys` /
  `useOrchestrator` の `case`

### P2 実装記録（2026-10-30）

- **型と受け口**: `types.ts` に 4 箇所（`PlanTaskState` / `TurnStop` / `CoreEvent.approvalStopped` / `CommandPolicyView.stoppedForApproval`）。
  `useOrchestrator` の `approvalStopped` は立てる・外すの両方で `refreshCommandRequests`。**この `switch` には `default` も網羅検査も無い**ので、
  `case` を忘れても型検査は黙る — 走査 `approvalStoppedWiring.test.ts` で留めた
- **押せるかは純関数**（`lib/resumeGate.ts`）: `resumeBlock(view, status)` が理由を
  `notStopped` → `pending` → `broken` → `notRunning` の順で 1 つ返し、`canResumeNow` はその否定。ツールチップは理由ごとに
  別の文（判断待ち N 件 / `run.json` が読めない / 稼働していない）で、押せるときは `title` を出さない
- **ダイアログ**（`CommandApprovalDialog.vue`）: 判断待ち 1 件ごとの「承認して続けさせる」を撤去し、`decide` から続行の引数を外した。
  節の末尾に止まった個体だけ「止まっています + 続けさせる」の行。押した後は `resumeAfterApproval` → `refreshCommandRequests` の順で、
  **節は押した直後に畳まれ、答えは数秒後に来る**（配送は受信箱を経て非同期）。空状態の `v-if` と節の `filter` を対で広げた
  （片方だけだと、判断待ちを全部承認した瞬間に空状態の文と止まった個体の節が並ぶ）。
  **このファイルは `keyOf` が NUL を区切りに使っており Edit ツールが拒む**ので、バイト単位の置換スクリプトで書き換えた（NUL 3 個は保持）
- **帯**（`approvalBanner.ts`）: 判断待ちがあればそちらを先に、無ければ止まった個体の One / Many（`count` は 0）。文に「承認画面を開く」を
  入れない（右隣のボタンと二重になる）ことを辞書の走査で留めた
- **作業状況と統計**: `PlanWavePane` は `STATE_LABEL_KEYS` に 1 行 + `cellClass` の注意色の腕（**`default` は失敗色**なので明示しないと
  承認待ちが失敗色で出る。型は鍵の網羅しか要求しない）。`statsView` は `STOP_LABEL_KEYS` が 8 値・`stopTone` は `fail`
  （統計では「払ったが答えが無い」側）
- **辞書**: `approveAndResume` を消し、`resume` / `stoppedNote` / `resumeBlockedPending` / `resumeBlockedBroken` / `chat.approvalStoppedOne` /
  `approvalStoppedMany` / `stats.stop.approvalPending` / `waves.state.approval_pending` を足した。`resumeFailed` は 1 段の文
  （続行と承認が別のボタンになったので「承認は保存されました」が到達不能）
- **テスト**: 単体 `resumeGate.test.ts`（6 本）/ `approvalBanner.test.ts` に 2 本 / 走査 `resumeGateWiring.test.ts` を書き直し（7 本）・
  `approvalStoppedWiring.test.ts` を新設（3 本）・`approvalBannerWiring.test.ts` に 1 本。既存の `statsView.test.ts` と
  `CommandPolicyView` のリテラル 3 ファイルを追従。**走査の正規表現を 1 度書き損じた** — 「部品で件数を判定しない」を
  ファイル全体の `pending.length === 0 &&` で見たら空状態の `v-if` に当たった。見る範囲をボタンの `:disabled` の式に絞った
- **変異 6 回とも予測どおり**: 受け口が取り直さない → 1 / 判断待ちの判定を外す → 2 / 帯の優先を逆に → 1 / `:disabled` から述語を落とす → 2 /
  セル色の腕を消す → 1 / en の鍵を欠く → 2（鍵集合の一致と走査）。各回 SHA で復元を確認
- vitest 971 全緑・vue-tsc 0・build 緑。**実機は P4**

### P3 — 台帳（**完了 2026-10-30**。下の「P3 台帳記録」）

- [x] DETAIL 3 言語（承認の節 — 止まる・続けさせるは 1 回・ヘッドレスの終了コード 12。DETAIL.md の
  「『承認して続けさせる』で、その場から続きを走らせられる（…呼ぶと判断待ちが積まれるだけ）」の句を書き換える）/
  README 3 言語は `run` の行に 1 句**足す**（既存に「承認して続けさせる」の句は無い — `grep` 0 件。行は足さない）/
  CLAUDE.md の「承認して続けさせる」の節と「承認待ちの帯」の節・「Spec の状態」の Spec 56 の行に覆しの 1 行 /
  Spec 64 D5・D10（P0 で書いた契約の写し）

### P3 台帳記録（2026-10-30）

- **DETAIL 3 言語**: 日英は `run` の節の「承認して続けさせる」の箇条を、止まる条件（周の終わり・同じ周の他のツールは走る）/
  個体ごとに 1 つの「続けさせる」と押せる条件 / 押すと節が消えて答えは後から来る / 印はメモリだけ / 予定の後判定と要約の見送り、へ
  書き換えた。以前の形（3 回押すと 3 本走る）は 1 段落だけ残し、Spec 56 を名指しした。帯の段落に止まった個体の腕、
  ヘッドレスの終了コードの表に 12、起動前検査の警告の行に「ターンが止まり `ask` は 12」、ログの節の「出口 4 つ」を 5 つへ、
  検証役を飛ばす理由を 7 つから 8 つへ。中国語は抄訳なので `run` の節に 1 箇条・画面の表の帯・終了コードの行の 3 か所だけ
- **README 3 言語**: 同梱ツールの行の括弧の中に 1 句（行は足さない。170 / 170 / 166 行のまま）
- **CLAUDE.md**: 「承認して続けさせる」の節の冒頭・「承認待ちの帯」の節の ③・「Spec の状態」の Spec 56 の行に覆しを 1 行ずつ
- **追従の grep で契約の取り残しを 3 か所回収した**（P0 で数え落としたもの）:
  `observability_rule` の「使用量の行はターンの出口 4 つ」/ 同じ節の「使用量の 4 行は末尾に `model=`」/
  `headless_host_contract` 凍結 10 の終了コードの列に 12 が無かった。**P0 は「4 出口」を 3 か所数えて直したが、
  `session_store` と同じ語を `observability_rule` が別の言い方（「出口 4 つ」「4 行」）で持っていた** —
  数の記述は言い方が 1 つでないと grep の網から外れる（`failures.md` #62 の形）。終了コードは Spec 64 の表と
  ヘッドレスの警告（凍結 6）には書いたが、凍結 10 の列挙は数え落としていた
- **PRIVACY は改訂しない**（外部へ送るものは増えていない。続行の定型文は Spec 56 のときと同じ経路）。
  ランディングページは「承認して続けさせる」に触れていない（`Resume anytime` は会話の再開の話）
- **画面の表示名を台帳と合わせた** — 統計の終わり方は「承認待ちで停止」、作業状況のセルは「承認待ち」（辞書の文言のまま）

### P4 — 実機

### P4 実機記録（2026-10-30。開発ビルド `version: app=0.1.0 profile=debug`・個体は `agent_3` = gemini-3.8-flash）

- **検収 1（観測）** — 04:14:00 の依頼で、1 周目に `run` を 3 本並べて呼び、3 本とも `decision=Unknown`
  （`pending=0 → 1 → 2` と積まれる）。続けて `turn cut: agent=agent_3 round=1 reason=approval_pending unknown=3` が 1 行、
  同じターンの `turn:` が `rounds=1/64 stop=approval_pending`。**`reply:` 行は出ていない**（D3 の設計どおり — 未完遂は
  `stop=approval_pending` で数える）。次の `turn start:` は利用者が「続けさせる」を押した 04:14:39 まで無い
- **検収 7（利用者の確認）** — 判断待ちが残る間は「続けさせる」が灰色で件数が出る。ログに行を残さない操作なので画面だけ
- **検収 2（観測）** — `turn start: agent=agent_3 hop=0 from=system chars=24`（24 = 「【コマンド承認】」8 字 + 改行 + 定型文 15 字。
  元の依頼文は送り直していない）と `resume after approval: pending_left=0` が 1 本だけ。再開したターンは 3 本とも
  `decision=Allowed allow=3`・`turn cut:` 無し・`stop=-` で終わり、`reply:` は 1 本。**再開したターンの `history_msgs=2/16`**
  = 止めたターンの注記が履歴に積まれ、モデルは止まった経緯を読んでから続けている
- **検収 3（観測）** — 04:17:06 の依頼で `cargo` / `rustc` / `bun` の 3 本が `Unknown`、`turn cut: … unknown=3` と
  `stop=approval_pending`。`bun` を却下・残りを承認して「続けさせる」→ `turn start: from=system chars=24` と
  `resume after approval: pending_left=0` が 1 本ずつ、再開したターンは `cargo` / `rustc` が `Allowed`・
  **`bun` が `decision=Denied allow=5 deny=1` でも `turn cut:` は出ず**、`stop=-` で終わって `reply:` が 1 本
- **検収 3 の副産物（Spec 72 の外の不具合）** — `cargo` と `rustc` の `run:` 行が両方 `resolved=…\.cargo\bin\rustup.exe` で、
  出力も 2 本とも `body_chars=271`。この端末の `cargo.exe` / `rustc.exe` は `rustup.exe` へのシンボリックリンクで、
  `process.rs` の `resolve_program` が `canonicalize()` でリンクを辿った先を起動するため、rustup が自分の名前で動いて
  rustup 自身の版を返したと読める（rustup は起動されたときの名前で cargo / rustc / rustup を切り替える）。
  呼び出し名で振る舞いを変える実行ファイルを、シンボリックリンク経由で `allow` に載せると別物が走る形
- **検収 4（観測）** — 05:02:45 に `run approval: mode=auto_approve was=required`、05:03:31 の依頼で `where` が
  `decision=Unknown` の直後に `run bypass: … mode=auto_approve recorded=yes` → `run:` で実行、`turn cut:` 無し・`stop=-`・
  `reply:` 1 本。その前の 04:58 と 05:01 の 2 回は承認が必要なモードのままで、どちらも `unknown=1` で止まり、
  「続けさせる」で 1 本ずつ再開して答えた（検収 1・2 の形の再現）。`allow` が 05:02 の 7 件から 05:03 の 5 件へ減っているのは、
  同じ `where` を `Unknown` へ戻すために `run.json` から 2 行を消したと読んでいる（ログからは区別できない）
- 起動直後の承認モードは端末の記憶から `no_approval` で戻り、利用者が 04:13:07 に `required` へ切り替えてから試した
  （`run approval: mode=required was=no_approval`）。検収の前提は満たしている
- **検収 5 の 1 回目（05:05〜05:06）は数えない** — 承認モードが `auto_approve` のままで（最後の `run approval:` は
  05:02:45）、ワーカーの `where` は `run bypass: … recorded=yes` で走り、止まらなかった
- **検収 5（観測）** — 05:09:56 に `run approval: mode=required was=no_approval`。ルナ（`agent_8`）が `plan` で
  ジェミーへ 1 件撒き（計画の確認を承認して 05:11:03 に `plan dispatch:`）、ジェミーの `where` が `decision=Unknown` →
  `turn cut: agent=agent_3 round=4 reason=approval_pending unknown=1` → `stop=approval_pending`。直後の
  `plan bundle: tasks=1 chars=48` は**見出し「## agent_3（ジェミー）」と改行の 17 字 + D3 (4) の文 31 字**で字数が一致し、
  続けて検証役が走った（`plan verify: verifier=agent_9 outcome=ok`）。セルの注意色は画面だけの観測
- **検収 5 で分かったこと — 続けさせた答えは元の依頼主へ戻らない。** 05:12:31 の続行は `from=system hop=0` の新しい根で
  `reply_to` を持たず、ジェミーの答え（272 字）は `reply: to=user` で利用者へ届いた。ルナの波は 05:11:40 に束ねて閉じ、
  05:12:03 に利用者へ報告済みなので、後から来た答えを取り込む口は無い（Spec 56 D4 の新しい根の帰結。2026-09-02 の
  `failures.md` #114 と同じ形 — 待てる場所が無い）。モデルの取り違えではない。D9 に利用者が負う条件として書いた
- **検収 6（観測）** — 05:16 にルナが `ask` でジェミーへ頼み（`turn start: agent=agent_3 hop=1 from=agent_8`）、
  ジェミーの `where`（許可済みの行とは引数が違う）が `decision=Unknown` → `turn cut: … round=2 reason=approval_pending
  unknown=1` → `stop=approval_pending`、ジェミーの `reply:` 行は無し。同じミリ秒台に
  **`tool: agent=agent_8 round=2 name=ask_agent_3 ok=true … body_chars=31`** = D3 (4) の文の字数と一致（`ask` は見出しを
  付けずに本文を返す）。ルナは 3 周目で利用者へ報告して `stop=-`。続けさせた後のジェミーの答え（348 字）は検収 5 と同じく
  `from=system hop=0` の新しい根から `reply: to=user` で利用者へ届いた（D9）
- **検収 8（ログの側を観測）** — 05:31:21 にジェミーの `hostname` が `decision=Unknown` → `turn cut: … round=1
  reason=approval_pending unknown=1` → `stop=approval_pending`。05:31:59 に利用者の新しい依頼
  （`turn start: agent=agent_3 hop=0 from=user chars=5`）が届き、`stop=-` で答えた。その間に
  `resume after approval:` も `from=system` の `turn start:` も無い = 「続けさせる」を押さずに印が外れる経路を通った。
  **印の出し入れ（`ApprovalStopped`）はログに行を出さない**ので、節が消えたことは画面でしか観測できない。
  **利用者の画面で、新しい依頼を打ったときにジェミーの節が消えたことを確認**（判断待ちは先に片付けてあった）
- **検収 8 の対照（判断待ちを残したまま新しい依頼）** — 05:33:53 に `whoami` で止まり（`unknown=1`）、判断待ちを片付けずに
  05:34:28 に新しい依頼を打った。ターンは `stop=-` で答え、入力欄の上の帯は「ジェミー がコマンドの承認を待っています（1 件）」
  のまま（スクリーンショット）= 判断待ちの腕が優先され、止まった印が外れても判断待ちの 1 件は帯と節に残る（D6）。
  **予定を使っていないので検収 9 の観測ではない**
- **検収 9（テストに預けた・利用者裁定）** — 前半（根のターンが承認待ちで止まると後判定と要約を見送り、再依頼しない）は
  `tests/approval_skips_acceptance_log.rs` が留める（予定の配送は 1 通だけ・`ApprovalStopped { stopped: true }`・
  `acceptance: schedule=… attempt=1/2 outcome=skipped reason=approval_pending summarize=skipped` が 1 行・
  `outcome=no_match` も `summarize:` も無い・直近の結末を書かない。P1 で変異 M4 の赤を確認済み）。
  後半（印の無い発火では後判定が普通に走る）は Spec 46 の `tests/schedule_acceptance.rs` の経路そのもの。
  **「続けさせたターンの完了では後判定が走らない」を直接主張するテストは無い**（D3 の訂正は P1 の実装の読みに依る）
- **検収 10 の前段（観測）** — 06:24:02 にジェミーの `whoami` が `decision=Unknown` → `turn cut: … unknown=1` →
  `stop=approval_pending`。判断待ちを片付けた後のスクリーンショット 2 枚で、帯が「ジェミー が承認待ちで止まっています」
  （止まった個体の腕・「承認画面を開く」はボタン側だけ）、承認画面の節が判断待ち 0 件で「このサーヴァントは承認待ちの
  コマンドで止まっています…」の文と押せる「続けさせる」だけになった（D5 / D6 の止まった印だけが残る形）
- **検収 10（利用者の画面）** — その状態で会話を切り替えると、帯と承認画面の節が消えた（D5 の外す点 (d)）。
  印の出し入れはログに行を出さないので画面だけの観測
- **まとめ** — 検収 1〜8 と 10 を観測（7・8 の節の消失・10 は画面）、9 はテストに預けた。P4 の途中で台帳の誤りを 1 つ回収した
  （D3 と検収 9 の「続けさせたターンの完了で後判定が走る」は P1 の実装と食い違っていた — 契約・DETAIL 日英も同じ文を直した）。
  別件 1 つ（`run` がシンボリックリンクを辿って rustup を起動する — 検収 3 の副産物）は Spec 72 の外に残っている


## 検収（P4）— 観測は機構が保証するものに限る

1. `required` で、別々の 3 つの未許可コマンドを使う依頼を出す → `turn cut: reason=approval_pending unknown=K`（K ≥ 1）が
   そのターンに 1 行だけ、同じターンの `turn:` が `stop=approval_pending`、会話ペインに System 行が 1 本、
   承認画面にその個体の判断待ち（K 件以上。周をまたいだ分は含まない）。**`turn start:` の後に 2 本目の答えが無い**
2. 判断待ちを全部「承認」で片付ける → 節は残り「続けさせる」が押せる → 1 回押す → **節は押した直後に消え**、数秒後に
   `turn start: from=system` が **1 本**、その後の `run decision:` に `decision=Allowed` が 1 行以上、`turn cut:` 無し、答えが 1 つ
   （節が消えるのが答えより先なのは仕様 — D5）
3. 1 件を却下・残りを承認して「続けさせる」→ 再開したターンの `run decision:` に `decision=Denied` が出ても `turn cut:` は出ず、
   ターンは答えで終わる（`stop=-`）
4. 同じ依頼を `auto_approve` で → `run bypass:` が 1 行以上、`turn cut:` 無し
5. 進行役から `plan` で撒かれたワーカーが止まる → 進行役の束ねに「この依頼は承認待ちのコマンドで止まりました」、
   作業状況のセルが注意色（失敗色ではない）、検証役が居れば `plan verify:` が出る
6. 進行役から `ask` で頼まれたワーカーが止まる → 進行役の `tool:` 行（`ask_agent_N`）の `body_chars` が D3 (4) の文の字数と一致
7. 判断待ちが残っている間は「続けさせる」が灰色でツールチップが件数を出す
8. 判断待ちを片付けた後に「続けさせる」を押さず新しい依頼を打つ → `ApprovalStopped { stopped: false }` が出て節が消える
   （承認画面を開いたまま観測する — 依頼が `run` を呼ばなくても消える）
9. 後判定つきの予定（`maxAttempts: 2`・検収コマンドは不一致を返すもの）の根のターンが承認待ちで止まる →
   `acceptance: … outcome=skipped reason=approval_pending` が 1 行、`turn start:` が増えない（再依頼が無い）、
   承認画面に節が残る。~~承認して「続けさせる」の後のターンが終わると後判定が普通に走る~~ **→ 訂正**: 続けさせた後の
   ターンが終わっても `acceptance:` は出ない（登録を捨てている — D3）。承認済みにしてから予定の次の発火を待つと、
   その発火のターンの完了で後判定が走る（`acceptance: … attempt=1/2` で `outcome=skipped` 以外）
10. 止まった個体が居る状態で会話を切り替える → 帯と節が消える（`ApprovalStopped { stopped: false }`）

## 未決

なし（rev2 で 3 つとも閉じた — 終了コードは 12 / 警告は既存の文面の更新 / 帯は開くだけ）。

## Notes

### 1. 起点の症状の読み

利用者の言葉は「3 つの処理が並列に動いて」。機構としては**直列の 3 ターン**（受信箱は 1 本・ターンは直列）で、
画面で 3 つの答えが続けて出たことの言い方。ただし問題の形は同じ — 続行が 3 通になるのは Spec 56 D3 が
人の押し方に寄りかかっていたから。本 Spec は押し方に依らない形へ直す。

### 2. 同日の要望の残り 2 つ（別の Spec）

- 添付画像を複数枚貼る（Spec 23 P4「2 枚目は置き換え」の裁定を覆す）
- 過去の添付をもう一度プロンプトで見せる（Spec 23 D1「渡るのは 1 ターンだけ」を覆す。入口は `@@` の参照と
  同じ形が自然 — 会話の中の添付を選んで渡す）

起票の前に添付の保存（`{workspace}/attachments/{uuid}.{ext}`）・GC（30 日）の実装を数え直す。

### 3. rev1 の査読の反映（2026-10-29。論理 19 点 / 技術 16 点。重なり 5 → 30 項目。採用 28 / 前提を訂正して採用 2 / 反証 0）

査読は方法で分けた — 論理 = 文書どうしの突き合わせ（コードを読まない）/ 技術 = 実装のコードとの突き合わせ（文書を読まない）。
**重なった 5 点は別の方法で同じ穴を指した**（#57 の「独立性は方法で数える」が効いた形）。

| # | 指摘（論理 L / 技術 T） | 判定 | 反映 |
|---|---|---|---|
| L1 / T4 | ヘッドレスは `required` 固定ではなく `--run-approval` で選べる。`RUN_APPROVAL_REQUIRED` は既に在る（18 個の 1 つ）。腐るのは理由文 | **前提を訂正して採用** | 現況 11 / D8 / 未決 2 を消した |
| T1 | 出口の 3 段に会話ログへの System 行が無い。利用者が直接頼んだターンは個体が黙る | **採用（重大）** | D3 を 4 段へ |
| T2 | `verification_of` は検証役自身の `Reply.kind`。`Err(None)` は束ねを丸ごと捨てて嘘を書く | **採用（重大・読み違い）** | D4 を 2 つに分け `VerifySkip::ApprovalPending` |
| L6 / T3 | 印を消す側に通知が無い。押した直後も押せて連打で 2 通目 | **採用（重大）** | D5 に `stopped: bool`・配送成功で外す・押下後の取り直し |
| L2 / T5 | 周の終わりに cancel の検査点は無い。順を実装の形で書く | **採用** | D1 に擬似コード。腕の中で cancel を先に |
| L17 | 最終周の `Unknown` とまとめ呼び出し | 採用 | D1 に 1 行・テスト (6) |
| L3 | `observability_rule` の出口 4 つ・Spec 39 D1 の表 | 採用 | D7 / P0 |
| L4 / T9 | `author=core` はこの出口に無い | 採用 | D3「`reply:` 行を書かない」・テスト (1) の観測を直した |
| L5 | 印は再起動で消え、続行の導線が消える | 採用（条件として） | D9 |
| L7 | 検収の数字がモデルの振る舞いに依る。`note_pending` の畳み | 採用 | 検収を書き直し・D2 に畳みの注記 |
| L8 | `ask` にセルも束ねも無い | 採用 | 検収 5（`plan`）/ 6（`ask`）へ分割・テスト (7)(8) |
| L9 | 終了コードを 6 へ畳む推しへの反論（Spec 64 D10 の論拠） | 採用（推しを反転） | D4 = 12 |
| T6 | 依頼主へ届く文が 2 つ。見出しに表示名が在る | 採用 | D4 束ねの文 = D3 (4) の `Reply.text` |
| T7 | `is_failure` と `stopTone` が黙って真 | 採用 | D3「`is_failure` は真。セルの色と別の分類」・テスト |
| T8 | `cellClass` の `default` は失敗色。`plan.rs` の wire テスト | 採用 | D4 / P1 / P2 |
| T10 | `rounds_cap: None` と `repeat_max=0` の嘘の観測 | 採用 | D3 (3) |
| T11 | `on_call` の `Ok(output)` の分解で欄が落ちる。単体は `call` を呼ぶ | 採用 | D2 / 現況 6 |
| T12 | `stop_agent` の早期 return・`command_policies` が組む | 採用 | D5 外す 3 点・現況 9 / 10 |
| L19 / T13 | ワイヤ凍結 3 点・`switch` の網羅 | 採用 | テスト (12)・P2 走査 |
| T14 | `budget_reserve_wiring` の数は動かない | 採用（予測として） | D3 末尾 |
| L10 | 帯の文と未決 3 の推しの食い違い | 採用 | D6「承認画面を開く」 |
| L11 | 注記に画面の操作名を書くとヘッドレスと扉で嘘 | 採用 | D3 (2) |
| L12 | `PlanTaskState` の色表と刻み手 | 採用 | P0 |
| L13 | Spec 56 D8 が到達不能 | 採用 | 覆す判断・`resumeFailed` を 1 段へ |
| L14 | 印の寿命の非対称・畳みの経路 | 採用 | D5 外す 3 点・`Failed` と同時に立たない理由 |
| L15 | 稼働は降ろさない | 採用 | D3 |
| L16 | `Malformed` が範囲外に無い・`!broken` | 採用 | 範囲外 / D5 |
| L18 | `pending=K` の欄名 | 採用 | D7 `unknown=K` |
| T15 / T16 | 行番号のずれ（`delegation.rs:911` → `:915` 等） | 採用 | 現況 |
| L7 の一部 | Goal 2「3 本をまとめて実行」 | 採用 | Goal 2 を「再開したターンが 1 本」へ縮めた |

**反証は 0。** 論理の査読が指した 19 点のうち 1 点（L1）は私の現況の誤りで、技術の査読（T4）が同じ穴を実装から指した。

### 4. rev2 の査読の反映（2026-10-30。外部の査読 2 件 = A 9 点 / B 16 点。重なり 1 → 25 項目。採用 20 / 前提を訂正して採用 3 / 反証 2）

A は rev2 の設計を読んだ査読（論理矛盾なし、実装で詰まる点 5 + 軽微 4）、B は文書と実装の両面（13 点）。
**B の 1-1 / 1-2 / 1-3 は rev2 の 2 系統が見ていなかった機構間の相互作用**で、どれも実装で確かめた
（`schedules.rs:44` の発火条件と `should_redeliver` の門 / `open_session` の履歴の差し替え / `handle_message` 段 1〜2 の `?`）。

| # | 指摘 | 判定 | 反映 |
|---|---|---|---|
| B 1-1 | 後判定と完了後要約が承認待ちの停止を「終わった」と読む。再依頼の門は財布の残額だけ | **採用（重大）** | D3 に見送りの述語と計器・P1・テスト (13)・検収 9 |
| B 1-2 | 会話の切り替え・新規チャットで印が残る | **採用（重大）** | D5 外す点 (d)・テスト (14)・検収 10 |
| B 1-3 | 外す点を `run_turn` の入口に置くと段 1〜2 の早期エラーで `Failed` と同時に立つ | **採用（重大）** | D5 (a) を `handle_message` の先頭へ・現況 10・テスト (10) |
| B 1-4 | 辞書の鍵は `waves.state.*` の snake_case。`STATE_LABEL_KEYS` は型が網羅を要求 | 採用 | P2・現況 15 |
| B 1-5 | `stats.rs` の `stop_kind`・`types.ts` の `TurnStop`・`statsView.test` の `toHaveLength(7)` | 採用 | P1 / P2・現況 15 |
| B 1-6 | 空状態の `v-if` にも条件が要る | 採用 | D5・P2 |
| B 1-7 / A 5 | 帯の「承認画面を開く」はボタンが別に在る。複数個体の形・判断待ちとの優先 | 採用 | D6 |
| B 2-1 | System 行の名指しは `{agent_id}（{display}）`。N の定義 | 採用 | D3 (1) |
| B 2-2 / A 1 | RepeatGuard とは構造的に同時成立しない。周の終わりに予算の検査を足さない注記 | 採用 | D1・P0 |
| B 3-1 | 契約の対象の位置（`session_store` の 7 値 / 4 出口 ×3・`plan_verifier_contract` 4・3 経路・Spec 56 の節の名前）。「ツールの失敗で会話を止めない」規律との衝突 | **前提を訂正して採用** — 引用の行（5481）は別の文で、規律は `tool_execution` の節（4513 / 4549 / 5011）に在る。衝突そのものは実在するので覆す判断に書いた | 覆す判断・P0 |
| B 3-2 | `stop_agent` は `runtime.rs:206` | 採用 | 現況 10 |
| B 3-3 | `CommandPolicyView` のリテラルを持つテスト 3 本 | 採用 | P2 |
| B 3-4 (1) | Spec 56 の D1 / D6 / P2 記録 5 も覆る | 採用 | 覆す判断・P0・P3 |
| B 3-4 (2) | CLAUDE.md の Spec 表に rev1 の「3 段」が残る。帯の節と Spec 56 の行も対象 | 採用 | CLAUDE.md を同日に直した・P3 |
| B 3-4 (3) | README 3 言語に「承認して続けさせる」の句が在り書き換えになる / DETAIL.md:382 の通知一覧 | **反証** — README 3 言語に当該の句は無い（`grep` 0 件。足すのであって書き換えではない）。DETAIL.md の 382 行目は要約の節。DETAIL.md:1464 の句は実在するので P3 に残す | P3 |
| A 2 | 節が畳まれる → 答えが来る、の順を明記 | 採用 | D5・検収 2・P2 走査 |
| A 3 | 再起動で「承認済みコマンドが失われる」。「新しい依頼を打てば続く」は `pending > 0` のときだけ | **前提を訂正して採用** — 承認は `allow` に残り失われない。新しい依頼は `pending` の有無に依らず通る。失われるのはボタンだけ、と D9 を書き直した | D9 |
| A 4 | 迂回で `failed` が増える。意図をコメントに | 採用 | D5・P1 |
| A 軽 1 | テスト (2) の 2 本は別々のコマンド、のコメント | 採用 | P1 |
| A 軽 2 | 3 面の文面が違う。辞書キーの統一 | **前提を訂正して採用** — System 行は会話、注記はモデルの履歴、`unknown_refusal` はツール結果で読み手が違い、文は 1 つにしない。「承認待ちで止まった」の語だけ 3 面で揃え、辞書に乗るのは画面の 2 面 | D3 |
| A 軽 3 | `exit.rs:31` のコメントの書き換え | 採用 | P1 |
| A 軽 4 | `rounds_cap` の理由をコメントに | 採用 | D3 (3) |
