# Spec: 波ペイン — plan 実行の可視化（Airflow Grid 相当）

**ID**: 08
**Date**: 2026-07-30
**Status**: rev2 査読承認 → Phase 0〜4 完了（2026-07-30）。残は実機確認 2 本
**Branch**: なし（main へ Phase 単位で直接コミット。契約凍結 Phase は本 Spec の
査読承認を前提条件とする — Spec 01〜07 と同じプロセス）

---

## Goal

中央ペインを上下に割り、下部に**波ペイン**を置く。進行役が `plan` を撒くたびに
波が 1 列として現れ、各タスクのセルが配送 → 解決の状態で色を変える。
いま stderr の `[concordia] plan wave:` / `plan bundle:` でしか見えない
実行の形を、GUI で追えるようにする。

起点は利用者要望（2026-07-30、README 未実装表「波の実行ビュー」）。
Spec 04 Notes 12 が置いた `plan_id` の発火条件 (b)
「波の実行ビューを作るとき」が、これで成立した。

### 立場の宣言

- **Airflow「風」であって Airflow ではない。** 人間は DAG を書かない
  （Spec 04 の立場は不変）。描くのは**モデルが作った計画の実行痕**であり、
  編集する場所ではない。波ペインは読み取り専用
- **Grid を採り、Graph を捨てる。** ノードと辺の図は上段の `TopologyMap` が
  既に持っている。波の関心は「誰に・いつ・どうなったか」の時系列であり、
  列 = 波・行 = エージェント・セル = 状態の格子が正しい形。
  同じ情報を 2 つの図で持つと、片方だけ直したときに嘘が生まれる

---

## Stories

### P1: plan_id と型付きの解決（コア）

> 利用者として、波とタスクの同一性を機械が追える形で持ちたい。
> なぜなら描画には「どの波のどのタスクが、どう終わったか」の対応が要り、
> 現状はターン内連番の `wave` と結果文字列の文言しか無いから。

- **`plan_id`**: プロセス内で単調増加する `u64`（`AtomicU64`）。
  **1 始まり・0 は予約**（未採番と空状態の区別に使える値を残す — rev2 B4）。
  跨プロセスの一意性は持たせない — 記録がプロセス寿命だから（P2）。
  **モデルには見せない**: 束ねの文言・plan のツール説明は 1 字も変えない。
  変えるとプロンプトが変わり、Spec 04 Notes 7 の実測（束ねサイズ）と
  既存の実機確認の前提が揺れる
- **タスクの同一性は `(plan_id, to)` で取る**（rev2 指摘 1）。これが成立する
  根拠は Spec 04 の静的な不正「同一宛先の重複」— 同じ波に同じ宛先が 2 回
  あると**何も配送せず差し戻す**（仕様: Spec 04 失敗の 3 分類表 /
  実装: `run_plan` の重複検査）。つまり配送された波の中で `to` は必ず一意。
  `task_index` を持たないのは**この禁止に依存した設計**である —
  将来もし同一宛先を許すなら、その Spec は `task_index` の導入とセットになる
- **タスク解決の分類 `PlanTaskState`**（閉じた列挙）:

  | 値 | 意味 | 今の文言（deliver_and_wait / handle_message） |
  |---|---|---|
  | `running` | 配送済み・解決待ち | —（状態遷移の始点） |
  | `answered` | 答えが返った | 本文そのもの |
  | `handed_off` | 転送で応じた | 「〇〇へ会話を渡したため…」（Spec 04 rev3 指摘 4） |
  | `undeliverable` | 停止中・受信箱飽和 | 「相手に尋ねられませんでした: …」 |
  | `no_answer` | 答えず終了した | 「相手から答えが返りませんでした。」 |
  | `timed_out` | 時間内に返らなかった | 「相手からの答えが時間内に返りませんでした。」 |

- **分類を取るのに必要な構造変更は 2 つ**（文言からの逆引きはしない —
  文言 parse は文言を直した瞬間に黙って壊れる）:
  1. `deliver_and_wait` の返り値を `String` → `(String, PlanTaskState)` へ。
     `ask` 側は分類を捨てるだけ（配送・失敗の文言を `ask` と共有して境界の
     ずれを構造で防ぐ、という Spec 04 Phase 1 の判断をそのまま延長する)
  2. 転送は現状 `reply_to` 経由で**普通の文字列**として返るため、oneshot
     チャネルの積み荷を `String` → `Reply { text, kind }` へ。`kind` は
     `handle_message` の `Finish` / `Handoff` 分岐が刻む
- **所要の計測点は波の実行器（`run_plan`）**（rev2 指摘 2）。JoinSet の
  各タスク内で `deliver_and_wait` の前後を `Instant` で挟み、
  `(index, answer, state, elapsed)` を返す。**`deliver_and_wait` の
  シグネチャに計時は入れない** — 計時は plan の観測の関心であって配送の
  関心ではなく、`ask` に不要な荷物を背負わせない

**Acceptance**

- Given 2 体へ plan、When 片方が答え片方が転送、Then 記録上の分類が
  `answered` / `handed_off` に分かれる（文言 parse ではなく型で）
- Given 停止中 1 体を含む plan、Then そのタスクは `undeliverable`
- Given `ask_agent`、Then 従来どおり文字列だけが返り、挙動・文言とも不変

### P2: 波の記録と再投影（コア + IPC）

> 利用者として、GUI を開き直しても直近の波を見たい。なぜなら描画層の
> 再マウント（エラー境界の差し替え・再読み込み）のたびに白紙へ戻るなら、
> 「たまたま画面を見ていた人」しか実行の形を知れないから。

- コアが `PlanWaveRecord` の**リングバッファ（上限 50 波）**を持つ。
  超過は**状態を問わず**古い方から捨てる（会話ログの窓と同じ流儀。
  50 は実測前の仮値 — 概算は Notes 5）。押し出された波への後続更新
  （`Resolved` / `Finished` 相当の書き込み）は記録側では**窓の外として
  無視する** — event は普通に飛ぶので、投影側の欠けであって配送の
  欠けではない
- **所有者は in-memory、ファイルへは書かない**（Spec 07 Phase 3 の契約追記と
  同じ立場。会話ログ・統計と同じくプロセス寿命 — 再起動生存は README
  未実装表の「セッションの再起動生存」Spec の管轄で、ここで先取りしない。
  その Spec が波の記録を生かす側に置くなら、`plan_id` の採番位置の持ち越しも
  セットで要る — rev2 B4）
- `PlanWaveRecord` の形（wire は **camelCase** — `CoreEvent` の既存 variant と
  同じく `serde(rename_all = "camelCase")` を明示する。rev2 B5）:
  ```jsonc
  {
    "planId": 7,               // プロセス内で単調増加（1 始まり・0 は予約）
    "agentId": "agent_1",      // 進行役
    "wave": 2,                 // ターン内連番（stderr の wave= と同じ値）
    "startedAtMs": 1785398400000,  // epoch ms（chrono 採用済み — Spec 07）
    "tasks": [
      // elapsedMs は「配送からそのタスクの解決までの個別所要」。
      // 相手のキュー待ちを含む（並列なのは配送 — Spec 04 Notes 8）。
      { "to": "agent_2", "state": "answered", "elapsedMs": 5210, "msgChars": 120 },
      { "to": "agent_3", "state": "running",  "elapsedMs": null, "msgChars": 88 }
    ],
    "bundleChars": null,       // 波の完了時に埋まる
    // 波全体の所要（= キュー待ち込みの最遅 1 体分）。タスク個別の
    // elapsedMs とは別の値。波の完了時に埋まる。
    "elapsedMs": null
  }
  ```
- `CoreEvent` へ 3 種追加（**更新は event、再投影は list** — `MessageSent` と
  `list_messages` の既存規律の踏襲）:
  - `PlanWaveStarted { plan_id, agent_id, wave, tasks: [{to, msg_chars}], started_at_ms }`
  - `PlanTaskResolved { plan_id, to, state, elapsed_ms }`
  - `PlanWaveFinished { plan_id, bundle_chars, elapsed_ms }`
- IPC `list_plan_waves` を追加（引数なし、保持中の全記録を古い順で返す）。
  **実行中の波（`running` のタスクを含む波）も返す**（rev2 B1）。完了だけを
  返すと「再読み込みした瞬間に走っていた波」が event でしか届かず、
  再投影の穴になる
- stderr の `plan wave:` / `plan bundle:` は**残す**。grep 可能な観測線は
  event の有無と独立に立っている（Spec 04 Notes 12 の観測ログの役割は不変）

**Acceptance**

- Given plan 実行中、Then **同一 `plan_id` について** `PlanWaveStarted` →
  タスクごとの `PlanTaskResolved` → `PlanWaveFinished` の順で届く
  （`Resolved` の相互順序は保証しない。**波を跨いだ全体順序も保証しない** —
  並行する波の `Started` は交互に来うる。rev2 B3）
- Given 51 波、Then `list_plan_waves` は 50 波を返し、最古の 1 波が
  **その状態を問わず**消えている
- Given 実行中の波、Then `list_plan_waves` の返答に `running` のタスクを
  含んだままの波が入る
- Given 発火順、Then 記録の `tasks` は**入力順**（束ねと同じ。解決順ではない）

### P3: 波ペイン UI（中央ペイン分割）

> 利用者として、接続マップの下で波の進行を眺めたい。なぜなら plan の実行は
> 「いま誰が働いていて、誰が終わったか」が本体で、それは会話ペインの
> 束ね結果（終わった後の全文）では追えないから。

- `App.vue` の中央 `main` を上下 2 段に分割。行テンプレートは `columns` と
  同じく **computed + `:style` バインディング**で組む（rev2 指摘 6 —
  リテラルには書けない）:
  ```ts
  const centerRows = computed(
    () => `minmax(0, 1fr) 2px ${layout.bottomHeight}px`,
  );
  ```
  上段 `TopologyMap` / 下段 `PlanWavePane.vue`（新規）。
  それぞれを既存どおり `ErrorBoundary` で包む（片方の描画失敗で道連れにしない）
- 仕切りは既存 `PaneSplitter` の `direction="row"`（実装済み・未使用の向き）
- `usePaneLayout` へ `bottomHeight` を追加（既定 160、min 80 / max 480）。
  **前提の確認済み**（rev2 指摘 7）: `load()` は全置換マージではなく
  **キーごとの `?? DEFAULTS` + `clamp`** で復元している。`bottomHeight` も
  同じ形で 1 行足す — 保存済みの旧 JSON からは既定値へ落ち、範囲外の
  保存値は読み込み時に丸まる。localStorage の鍵 `concordia.layout.v1` は
  上げない
- **再投影と購読の順序規律**（rev2 指摘 3）: (1) event リスナー登録 →
  (2) `list_plan_waves` 取得 → (3) `planId` で upsert。この順でないと
  list と購読の隙間に飛んだ `PlanTaskResolved` が欠落する。既存の
  `initialize` が同じ順序を既に踏んでいる（「購読を先に張る。読み込み中に
  発生したイベントを取りこぼさない」）— 波ペインはその規律を継承する。
  upsert の鍵は `planId`、タスクの鍵は `(planId, to)`（P1 の一意性）。
  フロントの保持もコアと同じ**上限 50・古い方から捨てる**
- 描画（Airflow Grid 風）:
  - **列 = 波**（古い→新しいを左→右、横スクロール）。**右端への自動追従は
    「既に右端に居るときだけ」**（rev2 B6 — 過去を遡って読んでいる最中に
    視点を奪わない。Airflow も追従しない）
  - **行 = エージェント**（左ペインと同じ `order` 順で全登録エージェント。
    波に登場しない行のセルは空白 — 行集合を波の内容から導出すると、
    波が流れるたびに行が増減して目が追えない）
  - **セル = タスク状態**: `running` はアニメーション、`answered` は正常色、
    `handed_off` は注意色、`undeliverable` / `no_answer` / `timed_out` は失敗色。
    ツールチップに `elapsedMs` / `msgChars`
  - 列見出しは**進行役の表示名 + wave 番号**、ツールチップに `planId`。
    表示名は一意でない（Spec 04 Notes 6）が、見出しは識別子ではなく
    ラベルであり、同定は `planId` が持つ
- 空状態: 「plan の実行はまだありません。」— **句点あり**で既存の空状態文言
  （「予定はまだありません。」「エージェントがまだありません。」）と揃える
  （rev2 C3）。折りたたみ機構は入れない — 仕切りのドラッグで min 80px まで
  縮められることを避難路とする（Notes 6）

**Acceptance**

- Given ザリが 2 体へ plan、Then 波が 1 列現れ、セル 2 つが `running` の
  アニメーションから解決色へ**個別に**変わる（全滅まで灰色、ではない）
- Given 波の完了、Then 列のどこかに所要（`elapsedMs`）が読める
- Given GUI の再読み込み、Then 保持中の波（実行中を含む）が再投影で戻る
- Given plan を一度も使っていない、Then 空状態の文言が出る
- Given 仕切りのドラッグ、Then 高さが 80〜480px で動き、再起動後も残る
- Given 過去の波へ横スクロールした状態で新しい波が届く、Then 視点は動かない。
  右端に居るときだけ新しい波へ追従する

---

## Tasks

- [x] Phase 0 — 契約凍結（**査読承認後**）: `data_contract.yaml` へ
      `PlanTaskState` / `PlanWaveRecord` / **`Reply { text, kind }`**（oneshot
      チャネルの積み荷 — rev2 指摘 5）/ `CoreEvent` 3 種 / `list_plan_waves` /
      リング上限 50（押し出しは状態を問わず・窓の外の更新は無視）/
      `plan_id` の採番規則（プロセス内単調増加・1 始まり 0 予約・モデル非公開）/
      wire の camelCase 明記 / **セル色マッピング表をコメントで併記**
      （rev2 C2 — 文言が変わっても色の対応は契約側に残る）
- [x] Phase 1 — コア: oneshot チャネルの積み荷を `Reply { text, kind }` へ、
      `deliver_and_wait` が `(String, PlanTaskState)` を返す形へ。
      **計時は `run_plan` の JoinSet タスク内**（deliver_and_wait には入れない）。
      `plan_id` 採番、リングバッファ、event 3 種の発火。分類ごとのテスト
      （`plan.rs` 単体 7 本 + event 直列化 1 本 + 結合 3 本 + 既存 2 本へ
      配送ゼロ非記録のアサート追加。実装で 1 点確定 — 完了した波に
      `running` を残さない: JoinSet パニック経路のみ `finish_wave` が
      `no_answer` に倒す。契約の invariants へ追記済み）
- [x] Phase 2 — IPC: `list_plan_waves`（Tauri command + `ipc.ts` + 型）。
      types.ts ミラー（`PlanTaskState` / `PlanTaskRecord` / `PlanWaveRecord` /
      `PlanTaskAnnounced` + `CoreEvent` 3 種）と、`ipc_contract.rs` の
      ワイヤ凍結テスト 1 本を含む
- [x] Phase 3 — UI: `usePaneLayout` 拡張 / `App.vue` 中央分割 /
      `PlanWavePane.vue` / `applyEvent` 配線（リスナー登録 → list → upsert の順）。
      upsert の合流は「進んでいる方を採る」（記録の遷移は片方向 —
      running → 解決、null → 値 — なのでこれで正しく合流できる）。
      投影規律の vitest 1 本（巻き戻し禁止・完了時の running 倒し・上限 50）
- [x] Phase 4 — 台帳整合: README（未実装表から「波の実行ビュー」を消し、
      並列委譲の節へ波ペインの小節・画面の構成表・ディレクトリツリーを追従）/
      CLAUDE.md の Spec 状態 / Spec 04 Notes 12 へ「(b) が発火し Spec 08 で
      実装。ただしモデル非公開のままなので (a) は未発火で残る」を追記 /
      failures.md は追記なし（実装で罠は出なかった）
- [ ] 実機確認: ザリに plan を撒かせ、(1) 波が列として現れセルが個別に
      解決色へ変わる (2) GUI 再読み込みで波（実行中を含む）が残る

---

## Notes（査読論点）

1. **命名「波ペイン」**: Spec 04 が確立した「波」の語彙をそのまま使う。
   「タイムライン」（時間軸が主役ではない）「ガントチャート」（バーの長さで
   所要を描く形式は採らない — Notes 8）との比較で選んだ
2. **Grid を採り Graph を捨てた**: Goal の立場の宣言のとおり。Airflow の
   Graph ビュー相当は `TopologyMap` と役割が重なる
3. **`plan_id` はモデル非公開**: 束ねの文言に載せない。載せる案は
   「波の因果を追えなくて困った実例」（Spec 04 Notes 12 の発火条件 (a)）が
   出たときに別途。UI の観測とモデルへの提示は別の判断
4. **静的差し戻し・hop 上限の波は描かない**: 配送ゼロの plan は波として
   存在しない（波 = 配送が起きた単位）。差し戻しの事実は進行役の会話に
   ツール結果として既に見える。**stderr との数え方は既に一致している**
   （rev2 指摘 4 の検証結果）: `run_plan` の静的検証と hop 検査は
   `plan wave:` の `eprintln!` より**前**で早期 return しており、
   配送ゼロの plan は stderr にも波として出ない。揃えるための変更は不要。
   **反論の余地**: Airflow は失敗した run も描く。「配送前に死んだ計画」の
   観測が要るなら、ここが最初の再訪点
5. **リング上限 50 は実測前の仮値**: 記録は文字列の本文を持たない
   （`msgChars` / `bundleChars` と分類だけ）。概算（rev2 C1）:
   1 タスク ≈ 60 byte（`to` 文字列 + 分類 + 数値 3 つ）、1 波 5 タスクで
   ≈ 400 byte、50 波で **≈ 20 KB**。メモリではなく「見て意味のある遡り幅」が
   律速で、そこは実測が出たら動かす
6. **折りたたみを入れない**: `bottomHeight` の min 80px で「ほぼ消す」は
   できる。トグルを足すと `usePaneLayout` に表示状態という別種の値が入り、
   寸法だけを持つ現在の形が崩れる。要望が出たら別途
7. **`ask` は描かない**: 1 件の直列委譲は「波」ではない。分類（P1）は
   `deliver_and_wait` 共有の副産物として `ask` にも通るが、描画対象は
   plan のみ。ask も可視化したくなったら、それは委譲全般の観測 Spec
8. **セルの中に所要バーを描かない**: 波内の並列タスクは「キュー待ちを含む
   最遅 1 体分」（Spec 04 Notes 8）が壁時計であり、バーの長さ比較は
   「実行が並列」という誤読を誘う。所要は数字（ツールチップ）で出す
9. **`wave` 番号はターン内連番のまま**: ターンを跨いで重複するが、同定は
   `plan_id` の仕事。`wave` を通し番号へ変えると stderr の観測線
   （`wave=` の意味）が黙って変わる
10. **タイムスタンプの規律**: `startedAtMs` は epoch ms（壁時計）、
    `elapsedMs` は `Instant` 由来（単調時計）。テストは壁時計の固定値に
    依存しない（同時 in-flight・イベント順序で測る — Spec 04 rev2 指摘 2 と
    同じ規律）
11. **`list_plan_waves` と event の突き合わせ**: 順序規律は
    リスナー登録 → list → `planId` upsert（P3 本文）。upsert だけでは
    欠落を防げない — 順序が本体で、upsert は重複側の対策（rev2 指摘 3）

---

### rev1 → rev2 の査読反映記録（2026-07-30・利用者査読）

**A 重大 7 件: 採用 6 / 検証により変更不要 1**

| # | 指摘 | 処方 |
|---|---|---|
| 1 | `to` だけではタスクを一意にできないのでは | **禁止を明記して採用**: Spec 04 静的な不正「同一宛先の重複」により配送された波内の `to` は必ず一意。`(plan_id, to)` が鍵。`task_index` 不在はこの禁止に依存する設計と明記し、同一宛先を許す将来 Spec は `task_index` とセットと縛った |
| 2 | `elapsedMs` の計測点が未定義 | **採用**: `run_plan` の JoinSet タスク内で計測。`deliver_and_wait` のシグネチャに計時は入れない（`ask` に不要な荷物を背負わせない） |
| 3 | list → 購読の順だと隙間の event が欠落 | **採用**: リスナー登録 → list → upsert の 3 段を規律として明記。既存 `initialize` が同じ順序を既に踏んでいる（useOrchestrator.ts「購読を先に張る」）ことを確認し、参照で接地 |
| 4 | stderr `plan wave:` はゼロ配送でも出るのでは | **検証により変更不要**: 静的検証・hop 検査の早期 return は `eprintln!` より前にあり、配送ゼロの plan は stderr にも出ない。数え方は既に一致。検証結果を Notes 4 へ明記 |
| 5 | `Reply { text, kind }` が Phase 0 の凍結対象から漏れ | **採用**: Phase 0 へ追加 |
| 6 | `grid-template-rows` のリテラル記述は動かない | **採用**: `columns` と同型の computed + `:style` バインディングへ書き換え |
| 7 | `load()` の補完方式が実装依存 | **検証して前提を明記**: `load()` はキーごとの `?? DEFAULTS` + `clamp` 方式（全置換マージではない）。`bottomHeight` は同型で追加し、読み込み時クランプも既存の形で掛かる |

**B 明確化 6 件: 全件採用** — (1) `list_plan_waves` は実行中の波も返す・
押し出しは状態を問わず (2) タスク個別と波全体の `elapsedMs` を jsonc コメントで
分離 (3) event 順序の保証を per `plan_id` に限定 (4) `plan_id` は 1 始まり
0 予約 + 再起動生存 Spec への持ち越し課題を明記 (5) wire は camelCase と
Phase 0 で明記 (6) 右端追従は「右端に居るときだけ」

**C 改善 3 件: 全件採用** — (1) リング 50 の概算 ≈ 20 KB を Notes 5 へ
(2) セル色マッピング表を data_contract のコメントに残す（Phase 0）
(3) 空状態文言は句点ありで既存と統一
