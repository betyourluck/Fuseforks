# Spec: 改名 — `Concordia` → `Fuseforks`

**ID**: 30
**Date**: 2026-08-09
**Status**: **rev1 承認 → P0 凍結**（2026-08-09。**村の査読は出さない** —
利用者判断「おそらく `Concordia` という語を grep 置換するという以外には技術的な
面になる。それは発生してからでないとわからない」。査読で潰せるのは数える仕事で、
残りは実行時にしか出ない。**4 点とも `git revert` で戻る**ことが起票後に判明し、
前もって潰す価値をさらに下げた）
**Branch**: なし（main へ直接コミット）。**コミット単位は Phase ごと**。特に
P2（副作用が repo の外へ出る 4 点）は**単独コミット**にする — revert の単位を
「既存の村と鍵が見えなくなる変更」に一致させるため（Spec 18 D12 と同じ形）。

## Goal

**製品名を `Concordia` から `Fuseforks` へ改名する。** 正式名は
**`Outcasts Fuseforks`**（`Outcasts` は維持）。

**理由は名前の衝突で、しかも同じ領域での衝突。** 相手は
[google-deepmind/concordia](https://github.com/google-deepmind/concordia)（実測
2026-08-08: 1.6k stars。"a library for constructing and running generative
agent-based models that simulate interactions among entities"）。**マルチ
エージェント LLM シミュレーションという一点でこの村と同じ棚**にあり、検索で
混ざるだけでなく**説明のたびに「あの Concordia とは別」と言い続ける**形になる。

**着手の時期が Goal の一部である。** いま村を持っているのは利用者とテスト環境
だけで、public にした後だと配布した installer を入れた人の村が対象になる。
**MPL の relicense と同じ論理** —「単独の著作権者でいられるうちに済ませる必要が
あった」と同じことが名前にも当たる。**v0.1.0 の直前が最後の安い窓。**

### 名前の選定（2026-08-09 利用者裁定）

候補は `ClawVillage` → `Fuse Folks` → **`Fuseforks`** と動いた。決めた根拠:

- **`fuse` + `fork` は `plan` の機構そのもの**（`plan wave: tasks=6 to=[...]`
  → `plan bundle: tasks=6 chars=3630`）。**名前が検証可能な主張をしている**
- **乗るのは `fork/join` の意味**（POSIX `fork()` / fork-join pool）。git の
  fork（分岐して戻らない）ではない — この村の波は**戻ることが機構の本体**で、
  `fuse` が隣にあるので読む側は join 側へ寄る
- **2 語（`Fuse Folks`）を却下したのは短縮**。2 語は 1 語目に短縮され、`Fuse` は
  開発者領域で最も混んだ語の 1 つ（実測: crates.io `fuser` 4,511,851 DL /
  `fuse-backend-rs` 675,133 / `fuse` 252,224、npm `fuse-native` 138,624 DL/月、
  ほかに Fuse.js・FuseBox）。**閉じた複合語なら短縮先は `Fuse` ではなく `FF`**
- **`Folks` を却下したのは語の温度**。米口語の「みなさん」で、この台帳が一貫して
  禁じてきたもの（比喩を使わない・抽象抒情を避ける・buzz を反指標にする）と
  同じ軸の反対側にある。**殻（村・サーヴァント・絆）は製品名ではなくアプリ内の
  語彙が運んでおり、改名で 1 語も変わらない**ので、名前が世界観を背負う必要はない

**残るコストは口頭の曖昧さ**（日本語話者の耳で `forks` と `folks` は同じ
「フォークス」）。**処方は D7 の防御的登録**で、これが無いと「聞いた人が別綴りを
打って 404 に当たる」形になる。Concordia の失敗が「検索すると**違うものが**出る」
だったのに対し、こちらは「検索しても**何も**出ない」— どちらも発見の失敗。

## 現況（実測 2026-08-09。**起票時の実測で「身元は 1 つ」が覆った**）

`git grep -i concordia` は **84 ファイル・279 行・289 出現**。

**行数と出現数は別に数える。** `git grep -c` が返すのは**一致した行数**であって
出現数ではない（1 行に 2 回書かれている箇所が 10 行ある）。CLAUDE.md の
2026-08-08 の実測「269 箇所」はこの区別を持っておらず、**どちらの数字とも
一致しない**。

| 層 | 実体 | ファイル | 出現 | 難度 |
|---|---|---|---|---|
| 台帳・散文 | `CLAUDE.md` 45 / `specs/` 68 / `data_contract.yaml` 26 / `README.md` 19 / `README_en.md` 19 / `failures.md` 7 | 25 | **184** | **判断が要る**（D6） |
| コードの識別子 | `apps/` 52 / `crates/` 50 / `Cargo.lock` 1 / `.github/` 1 / `.gitignore` 1 | 59 | **105** | 機械的。コンパイラが全部指す |
| **repo の外へ出る** | 下記 4 点（上の 2 層の部分集合） | — | **各 1〜2 行** | **本 Spec の主戦場** |

### 副作用が repo の外へ出る 4 点 — CLAUDE.md は 1 つと数えていた

**CLAUDE.md の「改名」の節は身元を `tauri.conf.json` の `identifier` 1 行だけと
書いている。実測すると 4 つある。**

| | 場所 | 変えると何が起きるか |
|---|---|---|
| **A** | `apps/gui-tauri/src-tauri/tauri.conf.json:5` `"identifier": "jp.outcasts.concordia"` | `{app_data_dir}` が変わる。**workspace は `{app_data_dir}/workspace` 固定**（`state.rs:63`）なので、**既存の村がアプリから見えなくなる** — `world.json` / `sessions.redb` / `Ordinance.md` / `village_id` / `blackboard/` が旧フォルダに取り残される。同じ棚の `mcp_server.json`（`state.rs:130`）と `probe_approvals.json`（`state.rs:138`）も同時に取り残される |
| **B** | `crates/agent-core/src/secret.rs:24` `SERVICE_NAME = "jp.outcasts.concordia"` | **OS の資格情報ストアのサービス名。登録済みの API キーが見えなくなる** — 消えるのではなく旧サービス名の下に残り、画面には「未登録」とだけ出る |
| **C** | `apps/gui-tauri/src-tauri/src/mcp_server.rs:236` `name = "ask_concordia"` | 外部クライアントへ宣言するツール名（Spec 25 P0 凍結） |
| **D** | `crates/agent-core/src/orchestrator.rs:3562` / `:5987` `Endpoint::System => "Concordia"` | 封筒 `【送り手: Concordia】` に入る = **モデルが読むプロンプトの一部**。かつ **System 行は会話ログへ焼き付いて `session_store` に保存される**ので、既存の村のログには旧名が残る |

**A と B は同じ文字列だが別の機構で、独立に決められる。** 一方は
`{app_data_dir}` を決め、他方は OS の資格情報ストアを束ねる。
**`jp.outcasts.concordia` で一括置換すると 2 つの判断を同時に、判断せずに
下すことになる。**

**この 4 点は grep の網に等しく引っかかる。** `concordia` で引くと 289 件が
同じ重みで並び、**289 分の 4 として現れる**。難度が桁で違うことは、
検索結果の形からは読めない。

**4 つとも `git revert` で元へ戻る。** A は旧 `{app_data_dir}` がまた見え、
B は旧サービス名の鍵がまた見え（**見えなくなるだけで、消す処理はどこにも無い**）、
C は次の `tools/list` で戻り、D は以後の発話が旧名に戻る。
**軸は「取り消せるか」ではなく「副作用が repo の外へ出るか」** —
`{app_data_dir}` / OS の資格情報ストア / 外部クライアントの期待 /
保存済みの会話ログは、どれも `git revert` の射程の外にある。

**取り消せる、は「見に行く先が変わるだけ」という意味であって、
安全という意味ではない。** revert しても、その間に新しい村で書いた会話と
旧い村の会話は別々の `sessions.redb` に分かれたままになる。

### 触ってはいけないもの — 過去の観測記録

**`concordia.log` は 70 出現ある**（`CLAUDE.md` 15 / `data_contract.yaml` 10 /
`specs/` 24 / `README` 日英 6 / `failures.md` 3 / コード 12）。**このうち台帳側は
当時のログの実物を指す文字列**で、書き換えると台帳が嘘になる。
**「全部置換」で最初に壊れるのがここ。**

同型のものが 3 種類ある（D6 で境界を定義する）:

- `concordia.log` を指す行（ログファイルの実物）
- `[concordia]` で始まるログ行の引用（`diag.rs:122` が付ける接頭辞）
- `%APPDATA%\jp.outcasts.concordia\...` を含む実測パスの引用

### 既に腐っていたもの（改名とは独立に回収する）

**`apps/gui-tauri/bun.lock:6` の `"name": "concordia"` は
`package.json:2` の `"concordia-gui"` と一致していない。** 名前が過去に一度
ずれており、ロックファイル側だけが古い名前を保持している。P1 で同時に回収する。

## Stories

- **S1**: 利用者が製品を人に説明するとき、**「あの Concordia とは別」と言わずに
  済む**。GitHub / crates.io / npm で名前を引いたときに、この村が出る
- **S2**: **改名の前に作った村を、改名後のアプリで開ける**（A を変える場合は、
  手順どおりにフォルダを 1 つ移せば開ける）
- **S3**: 改名の後に台帳を読んだ人が、**過去の観測記録と現在の名前を取り違えない**。
  `concordia.log` の行を見て「追従漏れ」と読んで直しに来ない
- **S4**: 外部の MCP クライアント（Claude Code）から、**改名後のツール名で
  依頼を投げられる**

## Decisions

### D1. `identifier`（A）を変える。移行コードは書かない

**変える。** ただし**移行処理をコードで書かない** — 旧 `{app_data_dir}` を探して
新しい側へ移す処理は、**public 後に入った人には一度も走らないのに起動経路に
残り続ける**（毎起動で旧フォルダの有無を判定する）。走る回数が有限（村は 2 つ）で
既知なら、**手順を 1 度書くほうが安い**。

**移行手順は `README` 日英と CHANGELOG 相当へ 1 度だけ書く**:
`%APPDATA%\jp.outcasts.concordia\` を `%APPDATA%\jp.outcasts.fuseforks\` へ
リネームする（macOS / Linux の対応パスも併記）。

**「変えない」も成立する選択肢だが、採らない。** 名前空間は内部識別子で検索には
出ないので、衝突の害はここに及ばない。しかし**いま変えないなら永久に
`jp.outcasts.concordia`** であり（public 後は配布済み installer が対象になる）、
利用者は `Ordinance.md` と `blackboard/` を編集するために**このパスを日常的に
目にする**。「Fuseforks を名乗る製品のフォルダが `concordia`」という説明を、
改名で消したはずの場所で続けることになる。

**帰結として `village_id` は作り直される**（新しい workspace に新しい UUID が
生まれる）。手でフォルダを移せば `village_id` も一緒に移るので、**Spec 28 の
probe 承認（`probe_approvals.json`）は移行後もそのまま効く** — 承認鍵の salt は
`village_id` で、ファイル自体も同じ `{app_data_dir}` の中にある。**移し方を
「フォルダごとリネーム」と書くのはこのため**（workspace だけを移すと承認が失効する）。

### D2. `SERVICE_NAME`（B）も変える。旧サービス名は読みに行かない

**変える。** ただし **A とは別の判断として明記する** — 同じ文字列であることは、
同じ機構であることを意味しない。

**旧サービス名を読んで移す処理は書かない。** 書くと「秘密を旧サービス名から
読み出して新サービス名へ書く」コードが起動経路に生えることになり、
**秘密を触る経路を 1 本増やす**。`secret.rs` の doc は「取得系は値を返すが、
それ以外の経路へ値を出さないこと」を凍結している。

**代わりに CHANGELOG へ「API キーの再登録が要る」と書く。** 鍵の原本は利用者が
プロバイダ側に持っているので、失われるものは無い。

**残余リスクを保証と書かない**: 旧サービス名の下に残った鍵は、OS の資格情報
マネージャーに残り続ける（アプリからは見えない）。**消したい人は OS 側で消す**。

### D3. `ask_concordia`（C）→ `ask_fuseforks`。クライアント設定は壊れない

**変える。** Spec 25 P0 の凍結の改訂として記録する。

**壊れない根拠**: MCP のクライアント設定（`.mcp.json`）に書くのは
**サーバーの接続先**（`type` / `url` / ヘッダ）であって、ツール名ではない。
ツール名は `tools/list` で毎回発見されるので、**改名は次の接続で自動的に伝わる**。

**壊れうるのは利用者が書いた散文のほう** — プロンプトやスクリプトに
`ask_concordia` と直接書いている箇所があれば、そこは手で直す。
**この村のリポジトリには 4 箇所**（`mcp_server.rs` 3 / `tests/mcp_server_wire.rs` 1）。

**外へ出る名前は `ask_concordia` だけではなかった**（実装時に数え直して 3 つ）:

| 場所 | 何 |
|---|---|
| `mcp_server.rs:236` | ツール名 `ask_concordia` |
| `mcp_server.rs:273` | **`info.server_info.name = "Concordia"`** — サーバー自身の名乗り |
| `mcp_server.rs:237` / `:276` | ツールの説明文と `instructions`（**モデルが読む**） |

`SettingsDialog.vue` が組み立てるクライアント設定の例（`mcpServers` の鍵
`concordia`）も変える。**既に貼った人の設定は壊れない** — あれは貼る側の
ローカルな鍵で、こちらは次に貼る人への見本。

**型名 `ConcordiaTools` も同じコミットで `FuseforksTools` にする。**

### D4. `Endpoint::System`（D）→ `"Fuseforks"`。既存ログの混在は受け入れる

**変える。** 新しい発話の封筒は `【送り手: Fuseforks】` になる。

**既存の会話ログには `Concordia` が残る**（`session_store` に保存済みで、
1 つの会話の中で送り手名が変わる）。**これは D6 と同じ規律** — 過去に起きたことの
記録であって、いま製品を指す名前ではない。**書き換えない。**

**`stable_len` は動かない。** 封筒は毎ターンの user ロール本文なので、
安定プレフィックスの外にある（Spec 19 で実測済み）— **全員のキャッシュは割れない。**

**表示名も同時に変える。起票時に 2 箇所と書いたが、実測すると 5 箇所あった**:

| 場所 | 何 |
|---|---|
| `apps/gui-tauri/index.html:8` | `<title>` |
| **`tauri.conf.json:15`** | `app.windows[0].title`（**実際の窓の題**。`index.html` は WebView 側で、こちらが窓枠側） |
| **`TitleBar.vue:73`** | `<span>Concordia</span>`（画面に出るワードマーク） |
| **`ChatPanel.vue:89`** | `return "Concordia"` — **`Endpoint::System` の UI 側の対**。`orchestrator.rs:3560` のコメントが「表示は UI と同じ」と書いており、**片方だけ変えるとその根拠が壊れる** |
| 辞書 ja/en `822` | `Concordia を終了しますか？` / `Quit Concordia?` |

CLAUDE.md 冒頭の「アプリ内表示・System の送り手名は『Concordia』のまま」は
**本 Spec で失効する**。

**`Endpoint::System` の側を留めているテストは 1 本も無い**（改名しても 685 本が
全部緑のまま）。**足さない** — これは外へ出るワイヤではなく表示で、しかも
**P5 の検収 5 が封筒を実機で直接見る**。読み口が既にあるものに網を二重に張らない。

### D5. ログは `fuseforks.log` / 接頭辞は `[fuseforks]` へ変える

**両方変える**（`state.rs:70` のファイル名、`diag.rs:122` / `:133` の接頭辞）。

**変えるほうが台帳に優しい。** 過去の記録が指す `concordia.log` は「当時の
ファイル名」として正しいまま残り、**新しいログは別名で共存する**ので、
どの世代の観測かがファイル名から読める。**同名のまま中身だけ世代が変わると、
過去の実測と新しい実測が区別できなくなる。**

**ステータスバーの時計の書式は触らない**（`diag.rs` と同じ形に固定してある規律は
接頭辞ではなく時刻の書式の話）。

### D6. 台帳の置換境界 — 判定は「記録か、名前か」

**置換の可否を「ファイル」や「行の見た目」で決めない。1 行ずつこう問う**:

> **これは当時そうだったという事実の記録か、いま製品を指す名前か。**

| | 例 | 扱い |
|---|---|---|
| **記録 → 触らない** | `concordia.log` を指す行 / `[concordia]` で始まるログ行の引用 / `%APPDATA%\jp.outcasts.concordia\...` を含む実測パス / 過去のコミットメッセージの引用 | **1 文字も変えない** |
| **名前 → 置換** | 「Concordia のマルチエージェント〜」のような製品の説明 / 見出し / `README` の紹介文 | `Fuseforks` へ |
| **境界** | `data_contract.yaml` の凍結文中の `Concordia` | 凍結が**いまの製品**を縛るものなら置換、**当時の観測**を記録するものなら据え置き |

**`failures.md` は原則すべて記録側**（症状 → 真因 → 処方 → 一般化は、いつ何が
起きたかの記録）。**ただし一般化の文は製品名を含まないはず**なので、
含んでいたらそれ自体が書き方の誤り（一般化は他プロジェクトへ転用する文）。

**残った `concordia` を機械で留めるテストは作らない。** 作ると 70 件超の
除外リストを保守することになり、**除外リストは必ずもう一度落ちる**（この台帳の
一般化そのもの）。P5 の検収で 1 度数え、**残った件数と内訳を Spec へ書いて凍結する**
— 次に誰かが `grep` して驚いたとき、その数と突き合わせられる形にする。

### D7. 別綴り `fusefolks` を防御的に押さえる

**押さえる。** 口頭の曖昧さ（フォークス）は綴りを決めても消えず、聞いた人の
半分は `fusefolks` と打つ。**押さえないと 404 に当たる。**

実測 2026-08-09 で**両綴りとも 3 レジストリすべて空き**:

| | `fuseforks` | `fusefolks` |
|---|---|---|
| GitHub | `total_count: 0` | `total_count: 0` |
| crates.io | `total: 0` | `total: 0` |
| npm | 404（未登録） | 404（未登録） |

**これは利用者のアカウント作業で、Phase に含めない**（リポジトリの変更を伴わない）。
Spec には**押さえるべき名前の一覧**として残す。

### D8. 機械的な識別子は一括で変える（コンパイラが全部指す）

| 場所 | 現 | 新 |
|---|---|---|
| `apps/gui-tauri/src-tauri/Cargo.toml:2` | `name = "concordia"` | `fuseforks` |
| 同 `:12` | `name = "concordia_lib"` | `fuseforks_lib` |
| `apps/gui-tauri/package.json:2` | `"concordia-gui"` | `"fuseforks-gui"` |
| `apps/gui-tauri/bun.lock:6` | `"concordia"`（**既にズレている**） | `"fuseforks-gui"` |
| `apps/gui-tauri/src-tauri/tauri.conf.json:3` | `"productName": "concordia"` | `"Fuseforks"` |
| `Cargo.lock:591` | `name = "concordia"` | 再生成 |
| `.github/workflows/build.yml:120` | `"Outcasts Concordia ${{ github.ref_name }}"` | `Outcasts Fuseforks` |

**`productName` は大文字始まりにする**（現在は小文字 `concordia`）。配布物の
名前として画面に出る側で、`identifier` のような機械の鍵ではない。

**一時ファイルの接頭辞（`concordia-test-` / `concordia-attachment-` /
`concordia-session-` / `concordia-edit-` / `concordia-file-` /
`concordia-diag-`）も同時に変える。** テスト専用で害は無いが、**残すと
「変え忘れ」と「意図的な据え置き」が区別できなくなる** — D6 の判定を後から
掛けられる状態を保つ。

### D9. `localStorage` の鍵も変える — **これが 5 つ目の「repo の外」だった**

**起票時に数え落としていた。** 画面設定の保存先は `localStorage` の 4 鍵で、
どれも `concordia.` で始まる:

`concordia.settings.v1`（テーマ・線削除の確認・入退室の表示・閉じる前の確認・
リサイズ後の自動フィット）/ `concordia.layout.v1`（ペイン幅）/
`concordia.chatCleared.v1` / `concordia.workDirHistory.v1`

**`{app_data_dir}` と OS の資格情報ストアと同じ性質**で、`git revert` の射程外に
副作用が出る。**A〜D と同じ扱いにする。**

**変える。移行は書かない。** 失われるのはテーマの選択・ペイン幅・作業フォルダの
履歴・会話の表示クリア状態だけで、**村の内容物は 1 つも含まれない**（あれは
`world.json` 側）。**`localStorage` から読み直して書き移すコードは、
Rust から読めない場所に増える 1 本目の移行機構**になる。

**帰結を検収へ書く**（P5 の 1 に合流）: 改名後の初回起動で**テーマが OS 追従へ
戻り、ペイン幅が既定へ戻る**。これは不具合ではない。

**`.v1` の版は上げない。** 版が意味するのは「保存形が変わったか」で、
名前空間が変わったこととは別。上げると**同じ形のデータに 2 つの版番号が付く**。

## Phases

### P0 — 凍結

- 本 Spec の D1〜D8 を承認して凍結する
- `data_contract.yaml` へ**改名の凍結を書かない**（改名は 1 度きりの作業で、
  契約が縛る継続的な不変条件ではない）。**ただし `data_contract.yaml` の中の
  `Concordia` 26 出現を D6 でどちら側に振ったかは、P4 で本 Spec へ記録する**

### P1 — 機械的な識別子（挙動不変）

- D8 の表を適用し、`cargo build` / `bun install` で `Cargo.lock` / `bun.lock` を
  再生成する
- **`cargo test --workspace` は アプリを終了してから回す**（稼働中は
  実行ファイルを置き換えられず落ちる）
- 検収: workspace 全緑・`bun run build` 緑・clippy 新規警告ゼロ

### P2 — 副作用が repo の外へ出る 4 点（**単独コミット**）

- A `identifier` / B `SERVICE_NAME` / C `ask_concordia` / D `Endpoint::System`
  / **D9 `localStorage` の 4 鍵**
- 表示名 5 箇所（D4 の表）も同じコミットへ
- **検収の書き方を 1 つ誤っていた。** rev1 は「`mcp_server_wire.rs` がツール名の
  改名で赤くなることを先に確かめる」と書いたが、**あのテストはツール名を
  1 つも主張していない**（合鍵が要求の経路に挟まっているかだけを見ており、
  `ask_concordia` は doc コメントに出るだけ）。**改名しても 685 本すべて緑のまま。**
  `#68`（合格条件が存在しない項目を書かない）を本 Spec で引用しておきながら
  同じ形を踏んだ
- **処方は凍結テストの新設**: `the_door_declares_one_tool_under_a_frozen_name`
  （`ToolRouter::list_all()` の名前一覧を `["ask_fuseforks"]` と完全一致で固定）。
  **件数まで固定するのは「扉は 1 枚だけ」（Spec 25 凍結 1）も同時に留めるため。**
  **ミューテーションで赤を確かめる** — 名前を `ask_village` にすると 1 本だけ
  落ち、失敗行が `left: ["ask_village"]` と誤った名前をそのまま出す

### P3 — ログ（`fuseforks.log` / `[fuseforks]`）

- `state.rs:70` / `diag.rs:122` / `:133`
- 検収: `tests/diag.rs` と `tests/grep_include_log.rs` が通ること

### P4 — 台帳（D6 の判定を 1 行ずつ掛ける）

- `CLAUDE.md` / `README.md` / `README_en.md` / `data_contract.yaml` /
  `failures.md` / `specs/`
- **数えるのはファイル単位**（`README` 日英を 1 行の Task に畳まない — #51 (b)）
- **移行手順**（D1 のフォルダのリネーム、D2 の API キー再登録）を README 日英へ
- CLAUDE.md 冒頭の「アプリ内表示・System の送り手名は『Concordia』のまま」を
  失効させ、新しい名前の由来（`fork/join`）を 1 段落で書く
- **本 Spec の「残った `concordia` の件数と内訳」を確定して書く**

### P5 — 実機確認 + タグ

- 下記の検収を観測する
- 観測が済んだら `v0.1.0` を打つ判断を利用者へ返す（**タグは利用者が打つ**）

## P5 検収（観測できる項目だけ — 書く前に読み口を数えた）

| | 何を見るか | 読み口 |
|---|---|---|
| **1** | 改名後のアプリを起動すると **`%APPDATA%\jp.outcasts.fuseforks\workspace\` が生まれ、`fuseforks.log` の行頭が `[fuseforks]`** | エクスプローラ + ログの 1 行目 |
| **1'** | **同じ起動でテーマが OS 追従へ戻り、ペイン幅が既定へ戻る**（D9 の帰結。不具合ではない） | 画面。**戻らなければ D9 が効いていない** |
| **2** | **旧フォルダをリネームした村で、会話・条例・予定・probe 承認が復元する** | 会話一覧に過去のセッションが並ぶ / `schedule probe:` が `outcome=no_match` を出す（`unapproved` ではない = `village_id` が一緒に移った証拠） |
| **3** | **API キーが「未登録」表示になり、貼り直すと依頼が通る** | 設定画面の登録状態 → `turn:` の行が出る |
| **4** | 外部クライアントの `tools/list` に **`ask_fuseforks`** が出て、呼ぶと返る | Claude Code から実行 |
| **5** | **新しい発話の封筒が `【送り手: Fuseforks】`、既存ログの System 行は `Concordia` のまま** | 会話ペインで 1 つの会話に両方が並ぶ |
| **6** | `git grep -i concordia` の残りが**過去の観測記録だけ**である | 件数を数え、内訳を Spec へ書く |

**検収 2 が「`no_match` であること」を見るのは、`unapproved` との対照が要るから。**
`village_id` が変わると承認鍵の salt が変わり `unapproved` に落ちる（Spec 28 P5 の
検収 4' で実測済み）。**フォルダごと移せば `no_match`、workspace だけ移せば
`unapproved`** — 1 行で移行手順の正しさまで読める。

**検収 5 は「両方が並ぶ」ことが合格条件**で、新しい名前が出ることではない。
既存ログを書き換えていないことは、**古い名前が残っていることでしか確かめられない。**

## Notes

1. **`Outcasts` が残るので `jp.outcasts.*` の名前空間は保てる。** 衝突していたのは
   2 語目だけで、改名は 1 語の差し替えとして閉じる
2. **CLAUDE.md の「改名」の節は本 Spec の起票をもって役目を終える。** P4 で
   「次のセッションで Spec として起票」以下を Spec への参照 1 行へ畳む —
   材料の置き場が 2 つあると、次に読む人がどちらを正とするか決められない
3. **身元の数え落としは、grep が「難度」を教えないことの実例。** `concordia` で
   引くと 289 件が同じ重みで並び、A〜D は 289 分の 4 として現れる。
   **一般化: 一括置換の射程を数えるときは、件数ではなく「副作用が repo の外へ
   出るか」で仕分ける。検索結果の並びは、その仕分けを 1 ミリも助けない**。
   **「取り消せるか」は軸にならない** — 4 点とも `git revert` で戻るのに、
   戻る先が repo の中にしかない
4. **Spec 28 で「workspace はパス固定だからパス salt は効かない」と結論した、
   あの固定がここで裏返しに効く。** 同じ 1 行（`state.rs:63`）が、あちらでは
   防御の穴の理由になり、こちらでは村が見えなくなる理由になる
5. **`fusefolks` の防御的登録は「空の欄」ではない。** この台帳は「今その欄を
   作らないのは、空の欄が『何を入れるべきか』を問い続けるため」という判断を
   何度か採っているが、**別綴りの登録はリダイレクトであって欄ではない** —
   問い続けるものが無い
6. **口頭の曖昧さは D7 で消えるわけではない。** 消えるのは「打ち間違えた人が
   404 に当たる」ところまでで、**「綴りを聞き返さないと伝わらない」は残る**。
   これは保証と書かず、受け入れた代償として記録する
