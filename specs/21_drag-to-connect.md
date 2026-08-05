# Spec: カードから絆を引く — サーヴァントリストから地図のノードへのドラッグで接続

**ID**: 21
**Date**: 2026-08-06
**Status**: **rev2 承認 → P0〜P1 + P3 完了（2026-08-06）。
残は P2（D2 の再計測が通ったときだけ）と P4 実機確認**
**Branch**: なし（main へ Phase 単位で直接コミット。**ただし Phase 0 の PoC は
main コミット対象外** — 捨てブランチで行い、結論だけを本 Spec の実測記録へ書く）

## Goal

サーヴァントリストのカードを「サーヴァントの絆」のノードへドラッグして落とすと、
カードのサーヴァントからそのノードのサーヴァントへの絆が張られる。

起点は利用者の言葉（2026-08-06）—「サーヴァントの編集ダイアログを開いて
絆をつなげるのは面倒」。

**「線は人が引く」は動かない** — 引く操作の入口が 1 つ増えるだけで、
機械が線を引く経路は 1 本も増えない。

## 現況（実測 2026-08-06）

絆の追加経路は既に 2 つある:

| 経路 | 実装 | 反映 |
|---|---|---|
| 編集ダイアログのチェックボックス | `AgentSettingsDialog.vue` の `toggleConnection` | 保存ボタンでまとめて |
| 地図のハンドル同士のドラッグ | `TopologyMap.vue` の `onConnect`（Vue Flow の `@connect`） | 即時・一方向・重複は `includes` で弾く |

- 保存はどちらも source の `connectedAgents` へ target を足す**一方向**
  （`orchestrator.setConnections`）
- **方向は見た目に出ている** — 辺は `markerEnd: "arrowclosed"` で終端に矢を持ち、
  双方向（逆向きがもう 1 本ある）のときだけ `markerStart` も付けて**両端矢の
  1 本**にまとめる（`TopologyMap.vue:105-107`）。削除はその 1 本で両方向とも切れる
- **`onConnect` に自己接続のチェックは無い**（`includes` の重複弾きだけ）。
  自己 no-op は本 Spec の**新規規則**である（Notes 4）
- **地図のノードは `state.agents` からしか生えない** — サーヴァント以外の
  ノードは存在しない（`nodes` computed の実測）
- リストの並び替えは `AgentList.vue` の `VueDraggable`（`:force-fallback="true"`）。
  **controlled** — `:model-value="agents"` + `@update:model-value="reorder"` →
  `orchestrator.reorder(ids)`。**`@end` の時点で SortableJS は DOM の並び替えを
  完了している**（Notes 5 の巻き戻しが要る理由）

**仮説（Phase 0 で実測して確定させる）**: ネイティブ HTML5 DnD はこのアプリでは
使えない。根拠 — `tauri.conf.json` は `dragDropEnabled` を書いていない =
Tauri v2 の既定 `true` で、Windows WebView2 では Tauri がドロップイベントを
横取りするため、ページ内の `dragstart` / `dragover` / `drop` が発火しない
（`force-fallback: true` が付いている理由もこれ、と読んでいる）。
**確定するまで本 Spec はネイティブ DnD を前提にしない**（反証されても機構は
変えない — fallback 前提の機構はネイティブが使える環境でも成立する）。

## 設計の核

**核の機構は「終端 1 回のヒットテスト」**。VueDraggable（SortableJS）の
fallback はドラッグ中の分身をリストの外まで追従させ、`@end` の
`originalEvent` に終端座標が残る。終端で:

```ts
ghost.style.display = "none";            // 分身が elementFromPoint を塞ぐ（定石で外す）
const el = document.elementFromPoint(x, y);
const node = el?.closest(".vue-flow__node"); // ハンドル・ラベル等の子要素から辿る
ghost.style.display = "";
```

- **座標の取り出し**: `originalEvent` が `TouchEvent` のときは
  `changedTouches[0]` から取る。座標が取れなければ**絆は張らず、通常の
  並び替えとして扱う**（安全側 = 既存挙動）
- **`closest` で判定する** — `elementFromPoint` はハンドル
  （`.vue-flow__handle`）やラベルを返す。`closest` が `.vue-flow__node` を
  返さなければ no-op（辺・ミニマップ・コントロール・地図の余白はここで落ちる）
- **方向はカード = source、ノード = target の一方向**（D1）
- **追加の規則は 1 実装へ寄せる** — `onConnect` から共有関数へ切り出し、
  drop 経路から同じものを呼ぶ（同じ規則を 2 箇所に書かない）
- **地図ヒット時は並び替えを確定しない**（機構は Notes 5）
- **D2（ドラッグ中のハイライト）は核と別機構**（連続ヒットテスト）。
  PoC の合否基準を通ったときだけ約束する
- 削除は射程外（既存の `@edge-click` + 確認のまま）

## 決めること

- **D1 方向 = 一方向。接続済みの判定は方向付き**。
  「接続済み」とは **`source → target` が既に存在する場合だけ**を指す。
  `A→B` がある状態で **B のカードを A のノードへ**落とすのは接続済みでは
  なく、`B→A` が追加されて双方向になる — これは既存の「逆も引く」と同じ規則で、
  **見た目も変わる**（1 本のまま始端に矢が生えて両端矢になる）。
  ドラッグには始点と終点がある以上、双方向で張ると既存のハンドルドラッグと
  規則が割れる
- **D2 ドラッグ中の視覚フィードバック — PoC の合否基準 3 点を通ったときだけ
  S4 を有効化**:
  1. 分身を `display: none` にした瞬間の `elementFromPoint` が
     `.vue-flow__node`（の子孫）を返せること
  2. ドラッグ中のイベントから座標が安定して取れること（TouchEvent 込み）
  3. `requestAnimationFrame` で回して 60fps を割らないこと

  1 つでも落ちたら **S4 ごと削除**し、代わりに**最低限の発見性**として
  「地図ペインの boundingRect に入ったらカーソルを `copy` に変える」だけを
  入れる（矩形判定なので `elementFromPoint` と分身の問題に依存しない）
- **D3 成功の告知はトースト無し。ただし no-op を無音にしない**:
  - 成功: 線（または新しい矢）が現れることが応答そのもの
  - 接続済み（`source → target` あり）・自分自身への drop: **対象ノードを
    短くパルスさせる**（0.3 秒のクラス付与。終端 1 回の機構だけで実装できるので
    D2 の成否に依存しない）。無音だと「認識されなかったのか、繋がっていたのか」を
    利用者が判別できない
  - 地図の外への drop: 完全に no-op（並び替えの意図と区別できないため）

## Stories

- S1 カードを地図のノードへ落とすと絆が張られ、線（または新しい矢）が即時現れる
- S2 `source → target` が既にある相手・自分自身へ落とすと絆は増えず、
  対象ノードがパルスして「対象には届いたが張られなかった」ことが分かる
- S3 リスト内の並び替えは今までどおり動く（本 Spec で挙動を変えない）
- S4 落とせる場所がドラッグ中に分かる（**D2 の PoC 合否基準を通ったときだけ有効**）

## Tasks

### Phase 0 — PoC（main コミット対象外。捨てブランチで行い、結論だけ実測記録へ）

- **核の成立**: `@end` の `originalEvent` から座標が取れること /
  分身 `display: none` → `elementFromPoint` → `closest` でノードが引けること
- **巻き戻しの成立**（Notes 5）: `@end` と `@update:model-value` の発火順を実測し、
  「地図ヒット時に `orchestrator.reorder` を呼ばせない」フラグの置き場所と、
  DOM の差し戻し（`evt.from.insertBefore(evt.item, evt.from.children[oldIndex])`
  が第一候補。駄目なら key 変更で再描画を強制）を確定する
- **D2 の合否基準 3 点**（上記）を判定する
- ネイティブ DnD の仮説を 1 回の実測で確定させ、現況の記述を書き換える
- Vue Flow のハンドル操作で**自己接続が実際に作れるか**を実測する（Notes 4 の
  「既存挙動の変更」の実害範囲が決まる）
- **PoC の結論しだいで機構を差し替える判断はここで下す**（pointer イベントの
  自前実装が代替）。結論を本 Spec の「P0 実測記録」に書いてから Phase 1 へ

### Phase 1 — 実装（ここから main へ）

- 追加規則の共有関数（`onConnect` から切り出し。方向付き重複 no-op /
  自己 no-op / 追加）
- `AgentList.vue` の `@end` にヒットテスト + 共有関数 + no-op パルス
- 地図ヒット時の並び替え巻き戻し（P0 で確定した形）
- **地図ペインの boundingRect による `copy` カーソル**（D2 失敗時の
  フォールバック。`elementFromPoint` に依存しないので、D2 の成否と独立に
  ここで入れる — Phase 2 に置くと D2 が落ちたとき Phase ごと消えて代替も消える）
- `:force-fallback="true"` の**真上**に理由のコメントを置く
  （「Tauri Windows WebView2 ではページ内のネイティブ DnD が発火しない。
  外すと並び替えごと動かなくなる」）
- vitest: 共有関数の規則 3 点（方向付き重複 / 自己 / 追加）+
  巻き戻しフラグの経路

### Phase 2 — 視覚フィードバック（D2 の合否基準を通ったときだけ。落ちたら
Phase ごと削除 — 代替の `copy` カーソルは Phase 1 で入っている）

- ドラッグ中、ノード上で対象をハイライト
- 落とせない対象（自分自身・`source → target` 済み）は区別して見せる

### Phase 3 — 台帳

- README 日英（絆の張り方は 3 つ、の形へ。「絆は有向で、双方向は両端矢の
  1 本」も画面の説明に載せる）
- CLAUDE.md
- `data_contract.yaml` の要否は P0 の後に確定する（現時点の見込みはゼロ —
  新しい型・ワイヤ・IPC が生まれない）

### Phase 4 — 実機確認

1. 未接続の相手へ drop → 線が現れ、終端に矢
2. `A→B` がある状態で **B のカード → A のノード** → 両端矢の 1 本に変わる
3. `A→B` がある状態で **A のカード → B のノード** → 絆は増えず、B のノードがパルス
4. 自分自身のノードへ drop → 絆は増えず、パルス
5. ハンドル・辺・ミニマップ・地図の余白へ drop → 何も起きず、**並び替えも
   確定しない**
6. リスト内の並び替えが今までどおり動く
7. チャットペイン等、地図でもリストでもない場所へ drop → 何も起きない

## Notes

1. **契約面はゼロの見込み。** `setConnections` / `TopologyEdge` /
   `topologyPositions` / `list_topology` は据え置きで、変わるのはフロントの
   操作経路だけ。ライトテーマ・StatusBar と同じ「フロントで閉じる」形だが、
   決めること（D1〜D3）と機構の選択（Phase 0）があるので Spec を切った
2. **3 つ目の入口であって、新しい規則ではない** — ただし自己 no-op だけは
   新規（Notes 4）。追加の規則が 1 実装に寄っていることを Phase 1 のテストで留める
3. **`force-fallback: true` の理由を初めて台帳に書いた。** 今までコメントが
   無く、次に誰かが「ネイティブに戻せば軽くなる」と親切心で外すと、
   Windows WebView2 では並び替えごと動かなくなる。コードコメントの置き場所は
   `AgentList.vue` の `:force-fallback` の真上（次に外すのはそこを触る人）
4. **自己 no-op は共有関数化に伴う既存挙動の変更**（安全側）。現行の
   `onConnect` は自己接続を検査しておらず、Vue Flow のハンドル操作で自己接続が
   作れるなら、共有化した瞬間にハンドル経路でも塞がる。これは「新しい規則では
   ない」の例外なので Spec に明記して入れる（黙って変えない）
5. **「地図ヒット時は並び替えを確定しない」は無為では成立しない。**
   SortableJS は `@end` の時点で DOM の並び替えを完了しており、state を
   更新しないだけでは DOM と描画がずれる。機構は 2 段 — (a) フラグで
   `orchestrator.reorder` を呼ばせない (b) DOM を `oldIndex` へ差し戻す。
   `@end` と `@update:model-value` の発火順に依存するので P0 で実測してから
   フラグの形を確定する
6. **rev1 からの主な変更**（査読 2026-08-06）: ネイティブ DnD の断定を仮説へ
   格下げ / 接続済みの定義を方向付きへ / 分身は `display: none` の定石へ /
   並び替え巻き戻しを機構として明記 / P0 を main コミット対象外へ /
   `closest` によるハンドル等の中間地帯の処理 / TouchEvent の座標 /
   no-op のパルス / D2 に合否基準と代替。**査読の前提 2 点は実測で訂正して
   採用** — (a) 矢印は既にあり方向は見た目に出ている（`markerEnd` /
   `markerStart`） (b) 逆向きの追加は「見た目変化なし」ではなく両端矢へ変わる。
   どちらも指摘の本体（定義の明記が要る）は正しいまま
7. **rev2 承認時の条件 1 点**（2026-08-06）: `copy` カーソルの矩形判定を
   Phase 2 から Phase 1 へ移動（Phase 2 は D2 が落ちたら削除されるので、
   そこに置くと代替も一緒に消える）

## P0 実測記録（2026-08-06。playground = `poc21.html` + `Poc21.vue`、
vite dev 1420。合成 Pointer/Mouse イベント + 実マウスの併用。破棄済み）

1. **発火順は `update:model-value` → `end` で確定**（同梱 Sortable の
   `_onDrop` L1015→1025 + 実走ログの両方）。査読の懸念どおり `@end` の
   フラグでは間に合わない。**機構は「保留コミット」で確定** —
   `@update:model-value` は配列を保留するだけ、`@end` のヒットテストで
   確定（`orchestrator.reorder`）or 破棄
2. **終端ヒットテストは素の `elementFromPoint` で成立**。Sortable は分身を
   イベント配送**より前**に DOM から除去する（`_onDrop` 冒頭 + 実走で
   `ghost-in-dom(at update/at end)=false`）。実走で `nodeId=node_x` /
   `node_y` に命中。**終端 1 回に `display: none` の定石は不要**。
   `closest(".vue-flow__node")` は必要（ハンドル・pane 対策）
3. **破棄時の DOM 差し戻しは必要**（rev2 のとおり）。ソース読みでは
   vue-draggable-plus が emit 前に差し戻して見えたが、**実走で反証** —
   破棄後の DOM は移動後の並びのまま残る（`dom-order=[a,b,d,c]` vs
   `state=[a,b,c,d]` を観測）。Phase 1 は
   `evt.from.insertBefore(evt.item, evt.from.children[oldIndex])` を実装する。
   確定（コミット）側は Vue の flush 後に DOM と state の一致を観測済み
4. **リストを縦断せず地図で落とすと `update` 自体が発火しない**
   （`old=new` の実走）— 保留が無いので破棄も要らず、自然に no-op
5. **D2 は条件付き** — 機構は成立（ドラッグ中の連続ヒットテストで
   `hoverHits` 増加を観測）だが **fps 47.5〜52.9 で基準 60 に未達**。
   ただし毎フレーム分身を `display` トグルする素朴実装での数字。
   処方候補: 分身へ `pointer-events: none` を**ドラッグ開始時に 1 回**打てば
   `elementFromPoint` が分身をスキップし、トグルが消える。
   **Phase 2 冒頭でこの最適化後に再計測して合否を出す。S4 はそれまで保留**
6. **自己接続はハンドル経路に届かない** — この版の Vue Flow は
   self-connect で `@connect` を発火しない（接続ラインは出るが drop で
   発火せず。対照実験の X→Y は発火）。共有関数の自己 no-op は drop 経路の
   防御であって、**ハンドル経路の観測可能な挙動は変わらない**（Notes 4 の
   「既存挙動の変更」は実質ゼロに縮んだ。ただしライブラリ更新で変わりうる
   ので no-op 自体は入れる）
7. **ネイティブ DnD の仮説は未実測のまま**（ブラウザでは検証不能、実アプリの
   WebView2 が要る）。機構はどちらでも成立するため Phase 1 を阻まない。
   `force-fallback` コメントの文言は **P4 の実機で 1 回裏取りしてから断定形に
   する**（それまでは「Tauri の既定 dragDropEnabled=true と衝突する報告がある」
   の形で書く）
8. **罠を 1 つ記録**: 合成 (composited) されていないページでは Vue Flow の
   ノード初期化が rAF 待ちで止まり、全ノードが `visibility: hidden` のまま
   `elementFromPoint` に当たらない。ヘッドレス検証や自動テストでこの経路を
   確かめるときは、ページが可視であることを先に確認する
