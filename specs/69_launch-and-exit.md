# Spec 69: 起動と終了 — トレイ常駐・起動時の一括起動・サインイン時の自動起動

- 状態: **rev3・未決 0 → P0〜P1 完了**（2026-10-20 起票 → 同日、査読 2 系統 19 点を実コードと依存のソースに当てて rev2。
  判定は採用 12 / 前提を訂正して採用 5 / 反証 2。対応表は Notes 4。→ 同日、利用者が未決 7 つを推奨どおりに裁定し、
  条件 2 つ（未決 4・5）を付けた。裁定を反映して rev3。裁定の表は Notes 6。同日 P0 = 窓の作り方 3 通り・レジストリ・
  依存の増分・Linux の実行時依存を測り、`data_contract` の `launch_exit_contract` を凍結。D5 は案 A に決まった。
  記録は「P0 実測記録」。同日 P1 = Rust 側（記録は「P1 実装記録」）。P2 には着手していない）
- 利用者裁定（2026-10-20）: 7 つとも推奨どおり。理由はいずれも Goal 4（OFF なら今と何も変わらない）。
  条件つきが 2 つ — **未決 4**: Windows の登録は `windows-registry` で自前で書く / 外すときは HKCU と HKLM の両方を消す /
  登録名は実機の値で大小まで固定する。**未決 5**: 組み立てに失敗したら必ず窓を出す（rev2 の D0 の 5 と同じ）
- 起点: 利用者（2026-10-20）—「システム設定ダイアログの全般の『起動と終了』に追加したい」
  - 閉じるでトレイに最小化するかどうかのチェック
  - アプリケーション起動時に一括起動
  - macOS または Windows にサインインしたときに Fuseforks を自動起動
- 同日に調査だけのターンを 1 回取った（三点測量: Memoria の裁定 / 台帳の grep / コードと
  `tauri-plugin-autostart` 2.7.0・`auto-launch` 0.6.0・Tauri 2.11.5・tao 0.35.3・tauri-runtime-wry 2.11.4 のソース）。
  下の「現況」はその実測。**macOS は実機が無いので、macOS の行はソースの読みであって実測ではない**
- **覆す凍結が 2 つある**（下の「覆す凍結」）。どちらも製品の行き先（トレイ常駐）へ戻る向きで、
  既定値はどちらも現在の挙動のまま（OFF）にする

## rev2 からの変更（裁定の反映。詳細は Notes 6）

- **Windows の自動起動の登録は `windows-registry` で自前で書く**（D4。未決 4 の条件）。`auto-launch` は macOS と
  Linux だけで使い、Windows の依存から外す。裁定の前提「`auto-launch` には引用符つきのパスを渡す口が無い」は
  rev2 の実測と食い違う（`build` はパスを検査しないので、包んだ文字列はそのまま値に入る）が、自前で書くことには
  別の理由が立つ — 値の形が `auto-launch` の内部の `format!` に寄りかからない / 読む側（`registeredCommand`・`scope`）は
  どのみち `windows-registry` で書くので、書く側と読む側が同じ 1 ファイルに揃う。前提の食い違いは Notes 6 に残した
- **外すときは HKCU と HKLM の両方を消しにいく**（D4。未決 4 の条件）。rev2 は「HKLM は消そうとしない」だったが、
  裁定に合わせて消しにいく。管理者でない起動では HKLM の削除は拒否されるので、そのときは**拒否されたことを
  握らずに** `scope: machine` で返し、画面が直し方を出す
- **登録名は `fuseforks`（小文字）で固定**（未決 4 の条件）。`tauri.conf.json` の `productName` を再確認した
  （`"productName": "fuseforks"`）。レジストリの値の名前は大文字小文字を区別しないので、NSIS の掃除との一致は
  大小に依らない。実機の値名は P4 で読む
- **辞書の `closeWindow.intro` の「常駐しません」を廃止し、トレイの ON / OFF で出し分ける**（D6。未決 2 の付帯）
- **アンインストール前に OFF にする旨を 3 箇所に書く**（D11。未決 6 の付帯。DETAIL / PRIVACY / リリースノート）

## rev1 からの変更（査読 19 点の反映。詳細は Notes 4）

- **起動の順序を 1 本に固定した**（D0 を新設。査読 1-1・1-2・2-1・2-2b）— rev1 は「`setup` で隠すか決める」（D5）と
  「トレイは組み立ての後に作る」（D2）を別々に書いており、隠して起動したまま組み立てに失敗すると
  **窓もトレイも無いプロセスが多重起動の枠だけを握って居座る**形があった。組み立てに失敗したら必ず窓を出す
- **窓を隠すのは `CloseRequested` の 1 箇所だけ**（D2。査読 1-3）— macOS の Cmd+Q は `CloseRequested` を
  通らないことをソースで確かめた（現況）。あわせて、**Cmd+Q が今も閉じる確認を素通りしている**ことが分かった
  （Notes 5。本 Spec の範囲外）
- **一括起動は `manage` の後に別のタスクで直列に回す**（D3。査読 2-2b）— `manage` の前に待つと、全個体の MCP の
  接続が終わるまで覆いが外れない
- **画面が `minimizeToTray` を知る経路と、`hide` の権限を足した**（D2・D6。査読 2-3）— `initialize()` で読んで
  `state` に持つ。capability に `core:window:allow-hide` が無い
- **終了の確認を重ねない**（D2。査読 2-4）— 終了の確認を出している間は 2 回目を積まない
- **自動起動の登録を「どこにあるか」で返す**（D4。査読 1-4 を前提を訂正して採用）— Fuseforks は自動起動を
  1 度も配っていないので「旧プラグインが書いた HKLM」は無い。ただし `auto-launch` の `is_enabled` は
  HKLM と HKCU の両方を読み、`CurrentUser` の `disable` は HKCU しか消さない。HKLM に値があると
  チェックを外しても ON のまま残るので、その場合は残っていることと直し方を出す
- **引用符はパスを渡す側で包む**（D4。査読 1-6 を前提を訂正して採用）— `auto-launch` 0.6.0 の `build` は
  パスを検査しないので、包んだ文字列を渡せばそのまま値に入る。フォークは要らない。macOS は包まない
  （plist の配列の要素なので、包むと引用符までパスになる）
- **macOS は `MacOSLaunchMode::LaunchAgent` を明示する**（D4。査読 2-7）— 既定も LaunchAgent だが、
  D9・D11 の plist のパスと対で固定する
- **言語の切り替えでトレイの文言を差し替える配線**（D2・P1。査読 2-6）— 今の `commands::set_language` は
  `AppHandle` を受けていない
- **設定ページはページを開くたびに読み直す**（D7。査読 2-5 の後半）— `pricing` / `jev` と同じ形。タスクマネージャーで
  切った状態が開き直しで出る
- **失敗したときのチェックの戻し方を決めた**（D7。査読 1 の軽微 5）
- 反証した 2 点（Notes 4）: 「Rust で起動するとフロントがイベントを取りこぼす」（`initialize()` は購読してから
  全件を取り直す）/「登録名は `Fuseforks` かもしれない」（`productName` は小文字の `fuseforks`）

## 覆す凍結

1. **「この村はタスクトレイに常駐しない」**（2026-08-07〜08 の利用者判断。マイルストーン 3 の
   「閉じる前の確認（トレイ常駐は見送り）」）。この前提に立っている記述は 4 箇所:
   - `apps/gui-tauri/src/lib/closeConfirm.ts` の doc（「常駐は『開いただけで課金が始まる作りにしない』を裏返す」）
   - `apps/gui-tauri/src/composables/useUiSettings.ts` の `confirmClose` の doc
   - `data_contract.yaml` の `settings_contract`（`confirmClose` の「この村はタスクトレイに常駐しない」）
   - 辞書 `settings.messages.closeWindow.intro`（ja「このアプリはタスクトレイに常駐しません。…」/ en 同義）

   ただし製品の行き先はもともとトレイ常駐だった（2026-07-31 の宣言。Spec 07「最終的には
   タスクトレイ常駐でサーバー的な動きをするようになる」）。Spec 07 は常駐したときに重くなる論点を
   11〜13 として記録済みで、本 Spec はそのうち 13（✕ が終了でなくなる）を引き受ける。
   11（無人の出力チャネル）と 12（時間軸の総量の上限）は D10 で扱いを決める
2. **`batch_start_invariant` の「アプリを開いた時点では誰も走らない」**（`data_contract.yaml`。
   Spec 64 D6 で「GUI では不変」と再確認した）。本 Spec は **GUI に端末側の明示のオプトインを 1 つ足す**。
   CLAUDE.md の「システム設定への追加候補」には、既に「既定 OFF の明示的なオプトインなら不変条件の理由
   （開いただけで課金しない）は保てる」と書いてある。**既定 OFF の間、不変条件は 1 文字も変わらない**

## Goal

1. **閉じる（✕ / OS の閉じる操作）でウィンドウを隠し、プロセスは動き続けられる。** サーヴァント・予定・
   MCP の扉は止まらない。トレイ（Windows の通知領域 / macOS のメニューバー / Linux の AppIndicator）から
   窓を戻せ、そこから終了できる
2. **アプリを起動したとき、一括起動の対象（全体 ▶ と同じ集合）を 1 回だけ起こせる。** 画面の再読み込み
   （F5）では起こさない
3. **OS へのサインインで Fuseforks を起動できる。** チェックの状態は OS の登録そのものを読む
   （タスクマネージャーで無効にされたら OFF と出る）
4. **3 つとも既定 OFF。** OFF のままなら、閉じる・起動の挙動は今と 1 つも変わらない
5. **起動に失敗したことは、隠して起動していても必ず見える。** 窓もトレイも無いプロセスを残さない
6. 入口はシステム設定 ＞ 全般 ＞ **起動と終了**（新しいページ）

**やらないこと（範囲外）**: 時間あたりの総量の上限（D10 で別 Spec を推奨）/ 無人運転の通知
（Spec 07 の P4・論点 11）/ macOS のメニューバー専用モード（Dock から消す `ActivationPolicy::Accessory`）/
macOS の Cmd+Q を閉じる確認へ通すこと（Notes 5）/ Microsoft Store（MSIX）配布での自動起動（StartupTask が要る。
今は Store に出していない）/ `fuseforks-cli` への影響（CLI はトレイも自動起動も持たない。`--start` の規則は
Spec 64 のまま）。

## 現況（実測 2026-10-20）

### 閉じる経路

- **閉じる経路は 1 本。** タイトルバーの ✕（`TitleBar.vue` の `win('close')`）も OS の閉じる操作も、
  `App.vue:178` の `registerCloseGuard` が張る `onCloseRequested` を通る。`settings.confirmClose` が
  真なら `preventDefault()` → `askConfirm` → 承認で `win.destroy()`（`close()` だと同じイベントが
  再発火して無限ループになるので `destroy`。capability に `core:window:allow-destroy` がある）
- 確認の本文は `lib/closeConfirm.ts` の `closeConfirmLines` が組む（前置き / 稼働中の数 / 扉が開いていれば
  扉 / 予定は常に）。**本文は「閉じると止まる」ことを言う文**で、トレイに隠す挙動では嘘になる
- `confirmClose` は localStorage（`fuseforks.settings.v1`）にあり、システム設定 ＞ ユーザーインターフェース ＞
  メッセージ表示/非表示（`SettingsDialog.vue` の `page === 'messages'`）で線削除の確認と並んでいる
- capability（`src-tauri/capabilities/default.json`）の窓の権限は `minimize` / `close` / `destroy` /
  `start-dragging` / `toggle-maximize` の 5 つ。**`hide` と `show` は無い**
- `askConfirm`（`composables/useConfirm.ts`）は待ち行列で、**画面に出るのは先頭の 1 件だけ**。後から来た確認は
  前の確認に答えるまで出ない
- `lib.rs:240` は `.run(tauri::generate_context!())` で、**`RunEvent` を受けていない**。macOS の Dock の
  アイコンを押したとき（`RunEvent::Reopen`。Tauri 2.11.5 の `app.rs:279` に在る）にアプリは何もしない
- **macOS の Cmd+Q は `CloseRequested` を通らない**（ソースの読み。実測ではない）— Tauri は既定で macOS の
  アプリメニューを作り（`enable_macos_default_menu: true`）、その「終了」は muda 0.19.3 で `terminate:` の
  セレクタ（`PredefinedMenuItemType::Quit`）。NSApplication の終了は tao 0.35.3 の
  `applicationWillTerminate` → `AppState::exit()` へ進み、`applicationShouldTerminate` の委譲は無い。
  tauri-runtime-wry 2.11.4 が `CloseRequested` を出すのは tao の `WindowEvent::CloseRequested` のときだけ
- 最後の窓が `Destroyed` になると、tauri-runtime-wry は `RunEvent::ExitRequested { code: None }` を出す
  （止められる形）。止めなければアプリは終了する
- `tauri.conf.json` のウィンドウに `visible` も `create` も指定は無い（既定で見える状態で、起動時に作られる）。
  Tauri 2.11.5 の `WindowConfig` は `create: bool`（既定 true）を持ち、偽にすると `setup` で
  `WebviewWindowBuilder::from_config` から作れる

### 起動の組み立て

- `lib.rs` の `setup` は窓を出したまま `state::build_state` を別のタスクへ逃がす（MCP の接続で 10 秒を超えることが
  ある）。成功で `spawn_event_bridge` → `handle.manage(app_state)`、失敗で `BootError` に理由を入れて終わる。
  **失敗時は「窓が既に出ている」ことを前提にしている**（コメント「ウィンドウは既に出ている」）
- 画面の `initialize()`（`useOrchestrator.ts`）は `boot_status` を 250 ms ごとに訊き、`ready` になったら
  **イベントの購読を張ってから** `refreshAll()` で全件を取り直す（「購読を先に張る。読み込み中に発生したイベントを
  取りこぼさない」）。`list_agents` は各個体の状態を返す
- `initialize()` は再読み込み（F5）のたびに走る（コアは生きている）。2026-09-27 からは、端末に覚えた 2 つの
  スイッチ（計画の確認を飛ばす / コマンドの承認モード）をここでコアへ戻している（`useOrchestrator.ts:885`）。
  **コアは起動のたびに既定（確認あり / 承認が必要）で立ち上がり、画面が戻すまでは既定のまま**。扉
  （`build_host` の中で開く）と予定のティッカーは画面より先に動くので、この間の依頼は既定で処理される
  （厳しい側。今も同じ）
- 言語は `Orchestrator::bootstrap` が未確定なら OS のロケールで確定して保存するので、組み立ての後は必ず決まっている。
  `commands::set_language` は `state` だけを受け、`AppHandle` を受けていない

### 多重起動

- `tauri-plugin-single-instance` 2.4.3 のコールバック（`lib.rs`）が、生き残っている側で
  `unminimize()` → `show()` → `set_focus()` を呼ぶ。**隠した窓も、もう一度起動すれば必ず戻る**
- 村の排他（Spec 64 D3）は `build_host` の OS ロックが持つ。起動に失敗した場合（`VILLAGE_LOCKED` ほか）、
  覆いに失敗の理由を出す

### 一括起動

- 対象の規則は 2 実装が同じ: コア `World::batch_start_ids`（`world.rs:1273`。Spec 64 の `--start batch` が
  使う）と画面の `batchEligible`（`lib/agentGroups.ts`。全体 ▶）。どちらも
  `agent.batch_start && (無所属 || group.batch_start)`（Spec 51 凍結 7）
- 一括の配送は**直列**（`batch_start_invariant`）。CLI の `start_agents`（`apps/cli/src/run.rs:199`）も直列
- `start_agent` は個体ごとの MCP の接続（タイムアウト最大 30 秒）を待つ
- **個体の起動は MCP の接続に失敗しても止まらない**（`McpManager::connect_all` は「1 台の失敗で全体を
  止めない」。失敗は接続先ごとの `McpServerStatus.error`）。サインイン直後でネットワークが整う前に
  起動すると、その接続先の道具を持たないまま稼働する。自動で繋ぎ直す機構は無い
- 待機中の個体は LLM を呼ばない。払いが始まるのは依頼が届いたとき（人・予定・外部の扉）

### 端末の棚

- アプリの外（ワークスペースの外）の設定は `{app_data_dir}` に 1 ファイルずつ置く:
  `mcp_server.json` / `probe_approvals.json` / `pricing.json` / `jev.json`（PRIVACY「端末ごとの設定」の表）。
  どれも「村を配っても付いて回らない」ことが置き場の理由
- 画面だけの設定は localStorage `fuseforks.settings.v1`（`useUiSettings.ts`）。**Rust は起動時に読めない**
- 設定ダイアログで `pricing` / `jev` のページは、ページを選ぶたびに設定を読み直す（`selectPage`）

### トレイの機構（Tauri）

- tauri の機能 `tray-icon` で作る。`tray-icon` 0.24.2 は **`Cargo.lock` に既にある**（ロックファイルは
  機能に依らず解決するので、機能を有効にしても crate は増えない。コンパイルされる量は増える）
- Linux の CI（`.github/workflows/build.yml`）は `libayatana-appindicator3-dev` を既に入れている
- **実行時の依存は未確認**: deb / rpm の依存に AppIndicator の実行時ライブラリが自動で入るか（P0）。
  GNOME は AppIndicator 拡張が無いとトレイが出ない

### サインイン時の自動起動（`tauri-plugin-autostart` 2.7.0 / `auto-launch` 0.6.0 のソースを読んだ）

| OS | 書く先 | 備考 |
|---|---|---|
| Windows | `Run` キーの値（名前 = アプリ名、値 = `{パス} {引数}`） | `WindowsEnableMode` の既定 `Dynamic` は **まず HKLM（全ユーザー）へ書き、権限が無いときだけ HKCU**。プラグインはこの指定を変える口を持たない |
| macOS | `~/Library/LaunchAgents/{アプリ名}.plist`（既定の `LaunchAgent`）/ AppleScript のログイン項目 / SMAppService（macOS 13+） | プラグイン 2.7.0 は LaunchAgent と AppleScript だけを選ばせる。plist の `ProgramArguments` はパスと引数を 1 要素ずつ `<string>` に入れる |
| Linux | `~/.config/autostart/{アプリ名}.desktop` | `Exec={パス} {引数}`。AppImage ならその AppImage のパス |

- **値はパスを引用符で包まない**（Windows `format!("{} {}", app_path, args.join(" "))` / Linux の `Exec` も同じ形）。
  MSI の入る先 `C:\Program Files\fuseforks\` は空白を含む。**ただし `build` はパスを検査しない**（名前とパスが
  指定されているかだけ）ので、包んだ文字列を渡せばそのまま値に入る
- **`is_enabled`（Windows）は `WindowsEnableMode` に関わらず HKLM と HKCU の両方を読み**、どちらかに値があり、
  かつタスクマネージャーの「スタートアップ」の無効化（`Explorer\StartupApproved\Run`）が掛かっていなければ真。
  **`disable` は指定した側だけを消す**（`CurrentUser` なら HKCU だけ）
- **プラグイン 2.6.0 以降は tauri `^2.12` を要求する**（ロックは 2.11.5）。2.5.1 は tauri `^2.8.2`・
  `auto-launch` `^0.5`
- **アンインストールでの掃除**（Tauri 2.11.5 の雛形）: NSIS のアンインストーラは更新時を除き
  `HKCU\…\CurrentVersion\Run` の値 `${PRODUCTNAME}` を消す。**MSI の雛形（`main.wxs`）には消す処理が無い**。
  winget で配っているのは MSI。HKLM の値はどちらも消さない
- `productName` は `fuseforks`（小文字。`tauri.conf.json`）。NSIS の `${PRODUCTNAME}` はこの値
- 登録の名前は 1 つを共有する — MSI（Program Files）と NSIS（`%LOCALAPPDATA%`）が並んで入った端末
  （`failures.md` #112 の形）や開発ビルド（`target\debug`）で ON にすると、最後に ON にした exe が登録される

## Stories

- S1: 予定を毎朝回している。✕ で閉じても予定が止まらないようにしたい
- S2: 起動するたびに ▶ を押している。起動したら一括起動の対象が起きていてほしい
- S3: PC を立ち上げたら Fuseforks も立ち上がっていてほしい（S1・S2 と組めば、サインインだけで村が回り出す）
- S4: トレイに隠したまま忘れても、もう一度起動すれば窓が戻る
- S5: 3 つとも使わない人には、今と何も変わらない
- S6: 自動起動で隠れて立ち上がったときに村を開けなかったら、それが分かる

## Design

### D0. 起動の順序（rev2 で新設。査読 1-1・1-2・2-1・2-2b）

1. `setup`: `startup.json` を読む（同期・小さい）。argv に `--autostart` があるかを見る。
   **隠して起動する** = `--autostart` かつ `minimizeToTray`（裁定 5）
2. `setup`: 窓を作る。**隠して起動するときだけ見えない状態で作る**（D5）。それ以外は今と同じく見える状態で作る
3. `setup`: `build_state` を別のタスクへ逃がす（今と同じ）
4. **成功**: `spawn_event_bridge` → `manage(app_state)`（今と同じ）→ `minimizeToTray` が真ならトレイを作る
   （言語はここで決まっている）→ `batchStartOnLaunch` が真なら一括起動を**別のタスクで**直列に回す（D3）。
   隠して起動していれば窓は見えないまま
5. **失敗**: `BootError` に理由を入れる（今と同じ）。**隠して起動していたら `show()` + `set_focus()`** して
   覆いの失敗の理由を見せる。**トレイは作らない**
- 隠して起動してから 4 か 5 に至るまでの間（MCP の接続で最大数十秒）は、窓もトレイも無い。この間にもう一度
  起動すると、多重起動のコールバックが `show()` する（現況）
- **計器**: 起動行の隣に `launch: source=autostart|manual hidden=true|false` を 1 行（D10 で無人の起動を数える）

### D1. 置き場 — 3 つとも端末側。村（`world.json`）には入れない

| 設定 | 置き場 | 理由 |
|---|---|---|
| 閉じるでトレイへ | `{app_data_dir}/startup.json` の `minimizeToTray` | Rust がトレイを作るか・隠して起動するかを起動時に決めるので、localStorage では読めない |
| 起動時に一括起動 | 同 `batchStartOnLaunch` | **村に入れると、配った先で開いただけで全員が起動する** — `batch_start_invariant` が防いでいるものそのもの。端末の棚なら配っても付いて回らない（`mcp_server.json` と同じ判断） |
| サインイン時の自動起動 | **OS の登録そのもの**（アプリは保存しない） | タスクマネージャーやログイン項目で OS 側から切れる。アプリに写しを持つと真実が 2 つになる。チェックは毎回読む |

- `startup.json` は GUI だけの棚なので **GUI のクレート（`src-tauri`）に置く**。`fuseforks-host` は CLI と
  共有しており、CLI はトレイも一括起動の設定も持たない（`--start` が明示する）
- 読めない・壊れた `startup.json` は既定（両方 false）として読み、**書き込みは拒否しない**
  （`mcp_server.json` が「読めないなら書かない」なのは合鍵を消すからで、こちらは消えて困る値を持たない）。
  WARN を 1 行出す
- 画面は `initialize()` で `get_startup_settings` を引いて `state.startup` に持つ（`state.mcpHost` の隣）。
  `set_startup_settings` が成功したら `state.startup` を差し替える。**閉じる経路（D2）はこの `state` を読む**

### D2. 閉じるでトレイへ

- **窓を隠すのは `onCloseRequested` の 1 箇所だけ。** 条件は `state.startup.minimizeToTray` が真、かつ
  組み立て済み（`state.ready` が真で `state.initError` が無い）。満たせば `preventDefault()` → `win.hide()`。
  **確認は出さない**（何も止まらない）。満たさなければ今のまま（確認 → `destroy()`）
- **組み立てていない間は隠さない。** 村を開けていないプロセスがトレイに残ると、多重起動の枠だけを握り、
  次の起動が「前面化して閉じる」に吸われて村を開けなくなる
- capability に **`core:window:allow-hide`** を足す（窓を戻す `show()` は Rust 側で呼ぶので `allow-show` は要らない）
- `RunEvent::ExitRequested` と `RunEvent::Exit` では隠さない（隠す分岐をそこへ置かない）。macOS の Cmd+Q は
  `CloseRequested` を通らずに終了へ進むので（現況）、トレイ ON でも Cmd+Q で終了できる
- トレイのメニューは 2 項目: **「Fuseforks を表示」/「終了」**。アイコンのクリックは OS で分ける:
  - Windows / Linux: 左クリックで「表示」と同じ。右クリックでメニュー
  - macOS: クリックでメニューを開く（メニューバーの作法）
- **「終了」は今の閉じる確認を通る。** Rust はメニューのイベントで窓を `show()` + `set_focus()` し、フロントへ
  イベント `tray://quit-requested` を送る（P1 で名前を決めた）。フロントは:
  - 終了の確認を出している最中（`exitConfirmInFlight`）なら何もしない（窓は前面に出ている）。**2 回目を積まない**
  - `confirmClose` が真なら `askConfirm` → 承認で `destroy()`。他の確認が待ち行列の先にあれば、それに答えた後に
    出る（他の確認を取り消さない — 別の操作の結果を黙って変えない）
  - `confirmClose` が偽ならすぐ `destroy()`
  - 確認の本文は今の `closeConfirmLines` のまま（「終了すると止まる」で正しくなる）
- **トレイのアイコンは `minimizeToTray` が真の間だけ出す**（裁定 1）。設定を切り替えた瞬間に出し入れする
  （`set_startup_settings` の中で Rust が作る / 消す）
- メニューの文言は村の言語で出す。組み立ての後は言語が必ず決まっているので（現況）、フォールバックは書かない。
  文言は GUI クレートに ja / en の 2 組を持つ（2 語ずつ。フロントの辞書は Rust から読めない）。
  **`commands::set_language` に `AppHandle` を足し、保存が成功したらトレイの文言を差し替える**
- macOS: 隠した窓は Dock のアイコンを押しても戻らない（`Reopen` を受けていない）。**`lib.rs` を
  `.build(…)?.run(|app, event| …)` へ変え、`Reopen` で `show()` する**

### D3. 起動時に一括起動

- **プロセスに 1 回だけ、`manage` の後に別のタスクで回す**（D0 の 4）。`batchStartOnLaunch` が真なら
  `World::batch_start_ids` の順に `start_agent` を直列に呼ぶ。画面の `initialize()` には置かない（F5 で再発火する）
- `manage` の前に待たない — `start_agent` は MCP の接続を待つので、全個体ぶん覆いが外れなくなる
- **画面は取りこぼさない**: `initialize()` は購読を張ってから `refreshAll()` で全件を取り直す（現況）。
  購読より前に起きた個体は取り直しで、後に起きた個体はイベントで映る。**新しい IPC は要らない**
- 1 体の失敗で残りを止めない（CLI の `start_agents` は失敗で全部止めて終わるが、あちらは「指定どおり
  起動できなければ実行しない」コマンド。GUI は人が後から ▶ を押せる）
- 計器を 1 行: `launch batch: targets=N started=M failed=K`（失敗した個体の id を `failed=` の後ろに列挙。
  理由の文は載せない — 理由は既存の個体の失敗の行が持つ）
- **起動しても LLM は呼ばれない**（待機中は払わない）。払いが始まるのは依頼が届いたとき。ただし D4 と組むと
  「サインイン → 一括起動 → 予定の発火」が人の居ない間に成立する（D10）

### D4. サインイン時の自動起動（2026-10-20 裁定。rev3 で Windows を自前の書き込みへ）

- **プラグインは使わない。** OS ごとに次の形で書く:

  | OS | 書き方 | 指定 |
  |---|---|---|
  | Windows | **`windows-registry` で自前で書く**（裁定の条件） | `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` の値 `fuseforks` = `"{current_exe}" --autostart`（パスを引用符で包む） |
  | macOS | `auto-launch` 0.6 | `MacOSLaunchMode::LaunchAgent` を明示（既定も同じだが、D9・D11 の plist のパスと対で固定する）。パスは包まない（plist の `<string>` 1 要素がパスそのもの） |
  | Linux | `auto-launch` 0.6 | `Exec=` に渡すパスを引用符で包む（`auto-launch` の `build` はパスを検査しないので、包んだ文字列がそのまま入る）。AppImage ならその AppImage のパス |

  `auto-launch` は macOS と Linux の依存にだけ置く（`[target.'cfg(not(windows))'.dependencies]`）。Windows は
  `windows-registry` を直接の依存にする（`auto-launch` 0.6 が連れてくるのと同じ crate で、版は P0 で揃える）
- 名前は **`fuseforks`**（`productName`。小文字で固定）。NSIS のアンインストーラが消す値の名前 `${PRODUCTNAME}` と一致する。
  レジストリの値の名前は大文字小文字を区別しないので、仮に大小が違っても NSIS の掃除は効く
- 引数は **`--autostart`**。起動時に argv で見分け、D0 の 1 と計器に使う
- **Windows の登録（`enable`）**: HKCU の `Run` へ値を書く。`Explorer\StartupApproved\Run` に同名の値があり無効
  （タスクマネージャーで切られた状態）なら、有効の値で上書きする（`auto-launch` 0.6 の `enable_with_root_key` と同じ
  振る舞い — チェックを入れたのに OS 側の無効が残って起動しない形を作らない）。**HKLM には書かない**
- **Windows の読み（`is_enabled` 相当）**: HKLM と HKCU の両方の `Run` を読み、どちらかに値があり、かつ両方の
  `StartupApproved\Run` で無効になっていなければ真（`auto-launch` 0.6 の `is_enabled` と同じ判定。ここは写す）
- **Windows の外し（`disable`）**: HKCU の値を消し、**HKLM に値があるときだけ HKLM の値も消しにいく**（裁定の条件。
  P0 で、管理者でない起動では HKLM の `Run` を書き込みで開く段で値の有無に依らず拒否されると分かった — 常に開きにいくと
  値が無くても `scope: machine` を誤って出す）。値がある HKLM の削除が拒否されたら（`E_ACCESSDENIED`）握らず、
  返り値の `scope` を `machine` にし、画面が
  「全ユーザー向けの登録が残っています。管理者として `Run` キーから `fuseforks` を削除してください」と出す。
  値が無いことによる失敗（`E_FILENOTFOUND`）は成功として扱う
- **HKLM に値が入る経路は Fuseforks には無い**（自動起動を配ったことが無く、本 Spec も HKCU にしか書かない）。
  残っているとすれば、利用者や管理者が手で入れたか、別の道具が入れたもの。両方を読み・消しにいくのは、その形でも
  画面と OS が食い違わないため
- IPC は 2 本:
  - `get_autostart` → `{ enabled, scope, registeredCommand, currentExe }`
    - `scope` は登録がどこにあるか（Windows: `user` / `machine` / `both` / `none`。macOS・Linux: `user` / `none`）
    - `registeredCommand` は登録されているコマンドの文字列。Windows は `Run` の値（HKCU を優先し、無ければ HKLM）。
      macOS は plist の `ProgramArguments`、Linux は `.desktop` の `Exec` — どちらも `auto-launch` が書く固定の形なので、
      その形だけを読む小さな関数を書く（汎用の plist / desktop パーサは足さない）
  - `set_autostart(enabled)` → 書いた後に `get_autostart` と同じ形を返す（HKLM を消せなかったときも、エラーではなく
    この形で返す — 外す操作のうち HKCU の分は済んでいるので、全体を失敗にすると済んだ分が画面から読めなくなる）
- 画面は登録のコマンドが今動いている exe と違えば「別の場所の Fuseforks が登録されています」と出す
  （ON にし直すと今の exe へ書き換わる）。開発ビルドで ON にしたときの取り違えをここで見せる

### D5. 隠して起動するときの窓（2026-10-20 裁定）

- `--autostart` かつ `minimizeToTray` が真のときだけ、窓を見えない状態で作る（D0）。それ以外は今までどおり見える
- **トレイが無いのに窓を隠さない** — 戻す手段がもう一度起動することしか残らない
- **組み立てに失敗したら必ず窓を出す**（裁定の条件。D0 の 5）。裁定に添えられた形
  「`initError || !ready` なら `show()` を強制 / `--autostart && minimizeToTray` なら隠したまま / それ以外は `show()`」は、
  **判定を 2 つの時点に割って実装する** — `setup` の時点では組み立てが終わっていない（`ready` は必ず偽）ので、
  1 つの時点で評価すると常に `show()` になり隠せない。`setup` で「隠して作るか」を決め（D0 の 1・2）、
  組み立ての結果が出た時点で「失敗なら出す」を決める（D0 の 4・5）。2 つ合わせて裁定の 3 行と同じ結果になる
- 作り方は **案 A**（P0 で決定。P0 実測記録）: `tauri.conf.json` の窓に `"create": false` を足し、`setup` で
  `WebviewWindowBuilder::from_config(…).visible(!隠す).build()`。見える側は作った瞬間から見え、隠す側は 1 度も見えない
  - OFF の経路の時間は今と同じ範囲（ページの読み込み完了 958 対 921〜939 ms、初期化の最後の呼び出し 1,070 対 1,030〜1,066 ms）
  - 退けた案 B（窓は今のまま作り、隠して起動するときだけ `setup` で `hide()`）は、窓が `setup` の前に見える状態で
    作られ、隠すまで見えている
- **隠した窓でも webview は読み込まれ、`initialize()` は最後まで走る**（P0 で確かめた）。2 つのスイッチは隠したままでも
  戻り、トレイの「終了」のイベントも画面が受けられる。rev3 まで書いていた「走らなければ『終了』を Rust だけで処理する
  形へ変える」の分岐は要らなかった

### D6. 閉じる確認の置き場（2026-10-20 裁定）

- **`confirmClose` のチェックを「起動と終了」へ移す。** トレイが ON のとき、確認が掛かるのは ✕ ではなく
  トレイの「終了」になり、意味がトレイの設定に依る。依る 2 つを別のページに置くと、片方だけ読んで誤解する
- 移した後の文言: 「終了する前に確認する」。トレイが ON の間は「（トレイの『終了』で確認します。✕ は窓を隠すだけです）」を添える
- 保存先は localStorage のまま（`fuseforks.settings.v1` の `confirmClose`。鍵は変えない）
- **辞書の `settings.messages.closeWindow.intro`（「このアプリはタスクトレイに常駐しません。…」）は廃止する**（裁定の付帯）。
  説明はトレイの設定で出し分ける — OFF:「閉じるとサーヴァントも予定も MCP の扉も止まります」/
  ON:「✕ は窓を隠すだけで、止まりません。終了はトレイのメニューから」。鍵は移した先の `settings.startup.*` に置き、
  `settings.messages.closeWindow.*` の鍵は ja / en から消す（鍵集合一致テストが両言語の消し忘れを捕まえる）

### D7. 画面 — システム設定 ＞ 全般 ＞ 起動と終了

- `SettingsDialog.vue` の `Page` に `"startup"` を足し、全般の 3 つ目（ユーザー / 言語 / 起動と終了）に置く
- 村のページ（`VILLAGE_PAGES`）には入れない。**ページを選ぶたびに `get_startup_settings` と `get_autostart` を
  読み直す**（`pricing` / `jev` と同じ `selectPage` の形）。タスクマネージャーで切った状態が開き直しで出る。
  注記は「この端末に保存されます。村を配っても付いて回りません」
- **押した時点で反映する**（保存ボタンを持たない）。MCP の扉のページと同じ — 自動起動は OS へ書けたかどうかが
  結果で、押すまで分からない
- **失敗したときのチェック**:
  - `startup.json` の書き込みに失敗 → チェックを押す前の値へ戻し、理由を出す（ファイルは書けていない）
  - 自動起動の登録に失敗 → `get_autostart` を読み直した値で表示し、理由を出す（OS 側が途中まで書いた可能性があるので、
    押す前の値ではなく読み直した値を出す）
- 3 つのチェックと 1 つの確認:

  | 項目 | 説明文に書くこと |
  |---|---|
  | 閉じるとトレイに隠れる | 閉じてもサーヴァント・予定・MCP の扉は止まりません。終了はトレイのメニューから |
  | 起動したら一括起動の対象を起こす | 起こすのは一覧の ▶ と同じ対象。起こすだけでは課金されませんが、予定や外部からの依頼が届けば動きます |
  | サインインしたら Fuseforks を起動する | OS のスタートアップ / ログイン項目に登録します。今の登録先のコマンドと、全ユーザー向けの登録が残っていればその直し方（D4） |
  | 終了する前に確認する（D6 で移す） | 今の文言 + トレイ ON の注記 |

- 3 つ全部が ON になったとき、ページの下に 1 文を出す: 「サインインするだけで、誰も見ていない間に予定が
  発火します。トークン制限（コスト管理）を確かめてください」（D10）

### D8. 一括起動の不変条件の書き換え

`data_contract.yaml` の `batch_start_invariant` を次の形へ（覆したことを残す）:

> **自動起動ではない。** アプリを開いた時点では誰も走らない — 開いただけで課金が始まる作りにしない。
> **例外は GUI の端末側の明示のオプトイン 1 つだけ**（Spec 69 `batchStartOnLaunch`。既定 false・
> `{app_data_dir}/startup.json`・村には入らない）。**ヘッドレス（`fuseforks-cli`）はこの例外を持たず、
> `--start` を必須にする**（Spec 64 D6。変更なし）

Spec 64 D6 の「GUI では不変」の行へ、本 Spec で例外が 1 つ入ったことを追記する。

### D9. 外へ書くもの（PRIVACY）

- **外部への送信は 1 つも増えない。**
- **アプリの外（OS の登録）へ書く設定が初めて入る**。PRIVACY 日英の「2. 端末に保存される情報」へ節を足す:
  Windows のレジストリ `HKCU\Software\Microsoft\Windows\CurrentVersion\Run` の値 `fuseforks` /
  macOS の `~/Library/LaunchAgents/fuseforks.plist` / Linux の `~/.config/autostart/fuseforks.desktop`。
  中身は実行ファイルのパスと `--autostart` だけ
- 「端末ごとの設定」の表に `startup.json` を足す
- 「8. 利用者による管理」に「自動起動はアプリの設定か OS の設定から切れる。アンインストールの前に切る
  （MSI では登録が残る — D11）」を足す

### D10. 無人運転の歯止め — 本 Spec には入れない（2026-10-20 裁定）

- 3 つを組むと、Spec 07 の論点 12（「無人 + 定期 + LLM = 見張りのないコスト」）が初めて現実になる。
  トークン制限（Spec 11）は**依頼 1 本の因果ごと**で、時間あたりの総量は縛らない
- 端末に覚えた 2 つのスイッチ（計画の確認を飛ばす / コマンドの承認モード）は**画面の `initialize()` が戻す**
  （2026-09-27）。戻るまでの間、コアは既定（確認あり / 承認が必要）で動く — 予定と扉は画面より先に動く（現況）。
  戻った後は、「承認せずに許可」のまま閉じた端末は無人でもその状態で動く。隠して起動したときに `initialize()` が
  走らなければ（D5・P0）、スイッチは既定のまま
- **本 Spec では上限を作らない。** Spec 07 が決めた順序（まず実測 → 必要なら日次 / 月次の上限）に従う。
  実測の器は統計画面の月の集計（Spec 42）が既にある。本 Spec が足すのは D7 の 1 文（3 つ全部 ON のとき）と、
  `launch: source=` の行（無人で起きた起動を数えられる）だけ

### D11. アンインストール後に残る登録（2026-10-20 裁定）

- **文書で受ける。WiX の断片は書かない。**
  - NSIS は雛形が HKCU の値を消す（D4 で登録名を `fuseforks` に揃えれば効く）
  - MSI は全ユーザーへ入れる形（`ALLUSERS=1`）で、アンインストールを走らせた利用者の HKCU しか触れない。
    WiX の断片で `RemoveRegistryValue` を足しても、他の利用者の登録は残る
  - 残った値は存在しない exe を指すだけで、Windows はサインイン時に黙って失敗する（実害は「スタートアップに
    消えた項目が残る」こと）
- 書く先: DETAIL 3 言語の「起動と終了」の節 / PRIVACY の「8. 利用者による管理」/ 次のタグのリリースノートの
  「利用者が負う条件」
- Homebrew の tap（`betyourluck/homebrew-tap` の `Casks/fuseforks.rb`）は `zap` に
  `~/Library/LaunchAgents/fuseforks.plist` を足す（別リポジトリ。P3）

## 採らなかった形

- **`tauri-plugin-autostart` を使う** — 書き込み先が HKLM に倒れる・tauri 2.12 を要求する（D4）。2.5.1 を使えば
  版の問題は消えるが、HKLM は残る（引用符は直接使う場合と同じく包んで渡せる）
- **`auto-launch` をフォークする** — Windows は自前で書き（裁定 4）、macOS と Linux は `build` がパスを検査しないので
  そのまま使える
- **一括起動を画面の `initialize()` で走らせる** — F5 で再発火する（D3）
- **一括起動を `manage` の前に待つ** — 全個体の MCP の接続が終わるまで覆いが外れない（D3）
- **`batchStartOnLaunch` を `world.json` に置く** — 配った村が開いただけで起動する（D1）
- **自動起動の状態をアプリに保存する** — OS 側で切られたときに真実が 2 つになる（D1）
- **チェックを外すとき、HKLM の削除の拒否を握って「外した」と見せる** — 管理者でない起動では HKLM の削除は拒否される。
  rev2 は「HKLM は消そうとしない」で避けたが、裁定 4 で「消しにいく」へ変えたので、避け方を「拒否を `scope: machine` で返す」へ移した（D4）
- **トレイの「終了」で他の確認を取り消してから出す** — 別の操作の答えを黙って「取り消し」に変える（D2）
- **トレイのアイコンを常に出す** — OFF の人の画面が今と変わる（Goal 4）。裁定 1 で退けた
- **✕ を「最小化」（タスクバーに残す）にする** — 要望は「トレイに最小化」で、タスクバーに残すなら
  今の最小化ボタンと同じになる
- **閉じたときに「隠しました」の通知を出す** — 通知の機構が無い（Spec 07 P4）。初回だけトレイの吹き出しを
  出す案は、トレイの機構が 3 OS で揃って吹き出しを持たないので採らない

## Tasks

### P0 — 測ってから凍結する（**完了 2026-10-20**。下の「P0 実測記録」）

- [x] Windows: 隠して作った窓でも webview が読み込まれ `initialize()` が最後まで走ること / 隠した窓へ二度目の起動が
  `show()` すること — 案 A・案 B とも成立
- [x] Windows: D5 の案 A と案 B を比べる — **案 A を採る**（OFF の経路の時間は今と同じ範囲。案 B は作られた窓が
  `setup` で隠されるまで見えている）
- [x] Windows: レジストリの書き方 — 引用符つきのパスがそのまま読み戻せる / 値の名前は大文字小文字を区別しない /
  `StartupApproved\Run` の無効・有効の書き換え / 無い値を消すと `0x80070002` / **管理者でない起動では HKLM の `Run` を
  書き込みで開くこと自体が `0x80070005`**（値の有無に依らない）
- [x] 依存の増分を数える
- [x] Linux: deb / rpm / AppImage に AppIndicator の実行時ライブラリが入るか — Tauri CLI のソースで確かめた（下）
- [x] macOS: `Reopen` と Cmd+Q の経路はソースの読み（現況）を根拠とし、実測ではないと記録した
- [x] `data_contract.yaml` へ `launch_exit_contract` を新設して凍結 / `batch_start_invariant` の書き換え（D8）/
  `settings_contract` の `confirmClose` と置き場の列挙へ `startup.json` を足す / `boot_contract` に隠して起動する
  ときの例外を足す
- **P0 で測らなかったもの（P4 へ送る）**: トレイのアイコンのクリックで窓が戻ること（トレイのコードが P1 で入る）/
  タスクマネージャーで実際に無効にしたときの読み（P0 は `StartupApproved\Run` の値を直接書いて確かめた）/
  HKLM に値を手で置いたときの外し（管理者が要る）/ macOS の CI ビルド（コードが入る P1 の後）/ 案 B の点滅を目で見ること

### P0 実測記録（2026-10-20）

**測り方**: 計器入りの開発ビルドを識別子 `jp.outcasts.fuseforks.p0` で作った（`tauri build --debug --no-bundle --config`。
データの棚・多重起動の枠・村のロックが本物と分かれる）。計器は `p0probe.rs`（`run()` に入ってからの経過ミリ秒を
`FF_P0_LOG` へ書く）と 7 行の差し込みで、測った後に外した（コミットしていない）。窓が見えているかは
`IsWindowVisible` でタイトル `Fuseforks` の窓を見た — **.NET の `MainWindowHandle` は隠した窓でも 0 にならないので
可視の判定に使えない**（最初の 2 本はそれで読み違えかけた）。

**窓の作り方**（暖まった状態の 2〜3 本。ミリ秒は `run()` から）

| 作り方 | `setup` に入る | 窓 | ページの読み込み完了 | 初期化の最後の呼び出し | 隠した窓 |
|---|---|---|---|---|---|
| 今（設定ファイルの窓） | 582〜584 | `setup` の前に作られる | 921〜939 | 1,030〜1,066 | — |
| 案 A（`create: false` + `from_config`）見える | 18〜42 | 607〜890 で作る | 958（1 本目は 1,227） | 1,070（1 本目は 1,338） | — |
| 案 A 隠す | 42 | 647 で見えない状態で作る | 999（`window_visible=false`） | 1,114 | 11,947 ms の 2 回目の起動で `visible=True` |
| 案 B（作ってから `setup` で `hide()`） | 528〜627 | `setup` の前に見える状態で作られ、543〜642 で隠す | 866〜1,028（`window_visible=false`） | 974〜1,138 | 12,009 ms の 2 回目の起動で `visible=True` |

- **隠した窓でも webview は読み込まれ、画面の `initialize()` は最後の呼び出し（`get_external_icon`）まで走る**（案 A・B とも）。
  D5 の「走らなければ設計を変える」の分岐は要らなかった。裁定 5 の推奨（隠して起こす）を倒す理由は出なかった
- **案 A の OFF の経路は今と同じ範囲**（ページの読み込み完了 958 対 921〜939 ms、初期化 1,070 対 1,030〜1,066 ms）。
  1 本目だけ遅いのは今の側の最初の 1 本（1,505 / 1,663 ms・村を作った回）と同じ傾向
- **案 B は、窓が `setup` の前に見える状態で作られ、`setup` で隠すまで見えている**（ページの読み込みが始まるのは
  隠した後なので、出るとすれば中身の無い枠）。点滅を目では見ていないが、構造として案 A には無い窓なので案 A を採る
- 多重起動の受け口（`get_webview_window("main")` → `show()`）は、`from_config` で作った窓にも効いた
  （ラベルは既定の `main`）

**レジストリ**（使い捨ての probe。試験用の値 `fuseforks-p0probe` を HKCU に書いて読み、最後に消した。HKLM は読むだけ）

| 確かめたこと | 結果 |
|---|---|
| 測る前の登録 | HKCU・HKLM とも `fuseforks` の値は無い |
| `"C:\Program Files\fuseforks\fuseforks.exe" --autostart` を書いて読む | 1 字も変わらず読み戻せる |
| 大文字の名前（`FUSEFORKS-P0PROBE`）で読む | 読める（値の名前は大文字小文字を区別しない） |
| `HKCU\…\Explorer\StartupApproved\Run` | キーは在る。先頭のバイト `0x03`（無効）/ `0x02`（有効）の 12 バイトを書いて読める |
| 消した値をもう一度消す | `0x80070002`（`E_FILENOTFOUND`） |
| 管理者でない起動で HKLM の `Run` を書き込みで開く | **`0x80070005`（`E_ACCESSDENIED`）。値の有無に依らず開く段で拒否される** |

- 帰結（D4 へ反映）: 外すときに HKLM へ常に書き込みで開きにいくと、HKLM に値が無くても毎回拒否が返り、
  `scope: machine` を誤って出す。**HKLM は読みで値の有無を見てから、ある場合だけ消しにいく**

**依存の増分**（名前で数えた。本体のロックに同じ名前がある crate は既存の版に揃うので数えない）

| | Windows | Linux | macOS |
|---|---|---|---|
| `tray-icon` 機能 | +1（`tray-icon`） | +4（`tray-icon`・`libappindicator`・`libappindicator-sys`・`libloading`） | +2（`tray-icon`・`objc2-core-graphics`） |
| 自動起動 | +1（`windows-registry`） | +2（`auto-launch`・`os_info`） | +5（`auto-launch`・`os_info`・`smappservice-rs`・`objc2-security`・`objc2-service-management`） |

- `tray-icon` 0.24.2 は機能を有効にしてもロックの行は増えない（ロックは機能に依らず解決済み）。上の数は `cargo tree` の
  一意 crate の前後の差
- 測った後に `Cargo.toml` とロックファイルを退避から戻し、`git status` で差分が無いことを確かめた

**Linux の実行時依存**（ソースの読み。Tauri CLI 2.11.4 のタグ `tauri-cli-v2.11.4` の `crates/tauri-cli/src/interface/rust.rs:1398〜1454`）

- 有効な機能に `tray-icon` があると、CLI は pkg-config で AppIndicator の種類を決め、Ayatana なら
  **deb の `Depends` に `libayatana-appindicator3-1`**、**rpm の `Requires` に `libayatana-appindicator3.so.1()(64bit)`** を足し、
  **AppImage には `libayatana-appindicator3.so.1` を `/usr/lib/` へ同梱する**
- CI は `libayatana-appindicator3-dev` を入れているので Ayatana 側に決まる。成果物の control はまだ読んでいない
  （トレイが入ったタグのビルドで読む）
- GNOME は AppIndicator 拡張が無いとトレイが出ない（依存とは別の問題）。その端末で窓を戻す道は、もう一度起動すること

### P1 — Rust（GUI クレート）（**完了 2026-10-20**。下の「P1 実装記録」）

- [x] `src-tauri/src/startup.rs`: `startup.json` の読み書きと argv の `--autostart` の判別（純関数）
- [x] `tauri.conf.json` の窓に `"create": false` / `lib.rs`: D0 の順序 / 窓の作り方（D5 の案 A — `from_config` の `visible`）/
  `RunEvent` を受ける形（macOS の `Reopen`）/ 失敗時に隠していれば `show()` + `set_focus()`
- [x] トレイ（`tray.rs`）: 組み立て成功後に作る / `set_startup_settings` で出し入れ / メニュー 2 項目と OS ごとのクリック /
  「終了」で `show()` → イベント `tray://quit-requested`
- [x] `commands::set_language` に `AppHandle` を足し、保存が成功したらトレイの文言を差し替える
- [x] 一括起動（`launch.rs`）と計器 `launch batch:` / `launch: source=… hidden=…`
- [x] 自動起動（`autostart.rs`）: Windows は `windows-registry` で書く・読む・消す / macOS と Linux は `auto-launch` /
  IPC 2 本 / 登録のコマンドを読む小さな関数
- [x] IPC `get_startup_settings` / `set_startup_settings`
- [x] テスト（下の記録）。**レジストリへ書くテストは CI に置いていない**（端末の登録を汚す）

### P1 実装記録（2026-10-20）

**足したもの**

| ファイル | 中身 |
|---|---|
| `src-tauri/src/startup.rs` | `StartupSettings`（camelCase の 2 欄・既定 false）/ `StartupStore`（読めなければ既定 + WARN・一時ファイルから置き換えて書く）/ `launched_by_autostart`（先頭の実行ファイルは見ない）/ `starts_hidden` / `StartupState`（棚だけ） |
| `src-tauri/src/autostart.rs` | `AutostartView { enabled, scope, registeredCommand, currentExe }` / `AutostartScope`（user / machine / both / none）/ 純関数 `windows_command`・`approved_disabled`・`windows_view`・`parse_plist_program_arguments`・`parse_desktop_exec` / `platform` を Windows（`windows-registry`）と非 Windows（`auto-launch`）に割った |
| `src-tauri/src/tray.rs` | `install`（既にあれば何もしない）/ `remove` / `relabel` / `show_main`（多重起動の受け口・トレイ・Dock が共有）/ 文言 `labels`（ja / en） |
| `src-tauri/src/launch.rs` | `start_in_order`（直列・1 体の失敗で止まらない。起動の口を引数で受けるので純粋に試せる）/ `LaunchBatchReport::line` / `run` |
| `lib.rs` | D0 の順序。窓は `setup` で `from_config(…).visible(!hidden)`。`build(…)` → `run(|app, event|)` で macOS の `Reopen` を受ける。`after_boot` = トレイ → **隠しているのにトレイが無ければ窓を出す**（トレイを作れなかったときと、組み立ての間に設定が切られたときの両方）→ 一括起動を別のタスク |
| `commands.rs` | IPC 4 本（`get_startup_settings` / `set_startup_settings` / `get_autostart` / `set_autostart`）。`set_language` は保存の成功後にトレイの文言を差し替える |
| コア | `Orchestrator::batch_start_ids`（`World::batch_start_ids` を読むだけ）/ `CoreError::Autostart { reason }`（コード `AUTOSTART`） |
| 依存 | tauri の機能 `tray-icon` / Windows: `windows-registry` 0.6・`windows-result` 0.4 / 非 Windows: `auto-launch` 0.6。ロックに増えた名前は P0 で数えた 6 つと一致 |

**Spec から動いた点**

- **画面へのイベント名は `tray://quit-requested`**（rev3 は `trayQuitRequested` と書いていた）。コアのイベント `core://event` と
  同じ書き方に揃えた。`data_contract` の `launch_exit_contract` 4 も同じ名前へ直した
- **`StartupState` は棚だけを持つ。** rev3 は自動起動か・隠して作ったかも置くと書いたが、どちらも `setup` の中で決まり、
  組み立ての結果を待つタスクへそのまま渡せば足りた（置くと読まれない欄になり警告が出た）
- **`CoreError::Autostart` を新設した。** `ConfigIo` へ寄せると「設定ファイルの入出力に失敗しました」になり、書く先が
  レジストリ / LaunchAgent / autostart のときに嘘の 1 文になる（`PricingFetch` / `JevProbe` を新設したのと同じ理由）。
  画面の辞書 `errors.AUTOSTART` は P2 で足す
- **`StartupApproved\Run` の「無効」は先頭バイトの最下位ビットで見る。** `auto-launch` 0.6 は「末尾 8 バイトが全部 0 なら有効」
  （無効にした時刻の有無）で見ている。P0 で書いた 12 バイトの `0x03 00…` は `auto-launch` の見方では有効と読まれる形で、
  **2 つの見方が実際のタスクマネージャーの値で食い違うかは P4 で確かめる**（P4 の 7）
- `set_autostart` は書いた後に `autostart: enabled=… scope=…` を 1 行出す（Spec に無かった計器。登録・解除の事実をログで読むため）
- `windows-registry` は `Error` 型を公開していないので、HRESULT を読むために `windows-result` を直接の依存に足した

**テスト**（GUI のクレートの単体 16 本 + 走査 4 本 + コアの結合 1 本。すべて緑）

- 単体: 棚（無い / 壊れた / 欄が欠けた / camelCase で往復して一時ファイルを残さない）/ `--autostart` の判別 / 隠す条件 /
  Windows の値の形 / 無効の判定 / 両方の hive とタスクマネージャーの組み合わせ / JSON の形 / plist（`ProgramArguments` より前に
  別の配列があっても取り違えない）/ `.desktop` / 一括起動（直列・失敗で止まらない・対象 0）/ 計器の 1 行 / トレイの文言
- 走査（`tests/launch_exit_wiring.rs`）: 窓は `create: false` から `.visible(!hidden)` で作る / 成功は中継 → manage → `after_boot`、
  失敗は理由を残してから隠していれば窓を出しトレイは作らない / `after_boot` はトレイ → 隠しているのにトレイが無ければ窓を出す → 一括起動を別のタスクで /
  **Rust 側（`src/*.rs`）に `.hide(` が無い** / `set_language` は保存の成功後にトレイの文言を差し替える
- コアの結合（`agent_groups.rs`）: `Orchestrator::batch_start_ids` が 2 段の門を通り一覧の順で並ぶ
- **変異 9 回とも予測どおり 1 本だけ赤**（窓を常に見える / 失敗で窓を出さない / 一括起動をトレイより先に / 隠しているのにトレイが無いとき窓を出さない /
  一括起動を最初の失敗で止める / 無効の判定を `0x03` だけ / HKLM 側の無効化を見ない / `--autostart` で先頭を飛ばさない /
  コアの対象を逆順）。変異は元のバイトを退避して戻し、SHA-256 の一致を確かめた
- **macOS / Linux の分岐は手元でコンパイルされない。** 非 Windows の `platform` モジュールを使い捨ての crate へ写し、`cfg` を外して
  Windows の上で `auto-launch` 0.6 に対して `cargo check` を通した（API と型の確認まで）。macOS 固有の 2 行（`set_macos_launch_mode`・
  plist の読み）と Linux 固有の 1 行（`APPIMAGE` と引用符）は、トレイが入ったタグの CI で初めてコンパイルされる

**P2 までの中間の状態**: 画面にまだ入口が無いので、3 つの設定は `startup.json` を手で書かない限り OFF のまま（既定の挙動は今と同じ）。
`minimizeToTray` を手で真にするとトレイは出るが、✕ で隠す分岐と「終了」を受ける側は P2 で入る（それまでトレイの「終了」は窓を出すだけ）

### P2 — 画面

- [ ] `initialize()` で `get_startup_settings` を `state.startup` へ（`state.mcpHost` の隣）
- [ ] `App.vue` の `registerCloseGuard` を分岐（D2）/ `tray://quit-requested` を受ける（`exitConfirmInFlight`）/
  辞書 `errors.AUTOSTART`（P1 で新設した `CoreError::Autostart`）
- [ ] capability に `core:window:allow-hide`
- [ ] `SettingsDialog.vue` に「起動と終了」のページ（D7）。`confirmClose` を移す（D6）
- [ ] 辞書 ja / en（`settings.startup.*`、`settings.messages.closeWindow.*` の移動と intro の書き換え）
- [ ] 走査テスト: 閉じる分岐が組み立て前に隠さないこと / ページの 3 つのチェックが IPC と localStorage の
  正しい側へ繋がっていること / `selectPage('startup')` が 2 本の読み直しを呼ぶこと

### P3 — 台帳

- [ ] DETAIL 3 言語（「起動と終了」の節・画面構成・ディレクトリ木・「▶ は自動起動ではない」の書き換え）
- [ ] README 3 言語に 1 行（サインインで起動し、閉じても動き続けられる — 入口で読めるべき挙動の変化）
- [ ] PRIVACY 日英（D9）
- [ ] `closeConfirm.ts` / `useUiSettings.ts` の doc の「常駐しない」を書き換え
- [ ] Spec 07 の論点 13 と Spec 64 D6 へ、本 Spec への行き先を追記
- [ ] Homebrew tap の `zap`（別リポジトリ）
- [ ] CLAUDE.md の現在地と「システム設定への追加候補」の「起動時に一括起動」の行

### P4 — 実機（Windows）

検収は次の順で、**どの行がログに出るか / 何が画面に出るかを先に書いてから**踏む（#68 / #85 の規律）。
自動起動の経路はサインアウトしなくても `fuseforks.exe --autostart` で踏める（6 と 9 以外）。

1. トレイ OFF・自動起動 OFF・一括起動 OFF のまま: ✕ で今と同じ確認が出て、承認で終了する（`Get-Process fuseforks` が消える）。
   `launch: source=manual hidden=false` が出る
2. トレイ ON: ✕ で窓が消え、確認は出ず、`Get-Process` は残る。トレイの左クリックで窓が戻る
3. トレイ ON: トレイの「終了」で確認が出て、承認でプロセスが消える。確認が出ている間にもう一度「終了」を押しても確認は 1 つ
4. トレイ ON で隠したまま、スタートメニューからもう一度起動すると窓が戻る（プロセスは 1 つのまま）
5. 一括起動 ON: 起動すると対象だけが直列に起き、`launch batch:` が 1 行出る。1 体を止めて F5 しても、その個体は起きない
6. 自動起動 ON: `HKCU\…\Run` に `fuseforks` が `"{パス}" --autostart` で入る。サインアウト → サインインで起動し、
   `launch: source=autostart` が出る。トレイ ON なら窓は出ない
7. タスクマネージャーのスタートアップで無効にし、設定ページを開き直すとチェックが OFF で出る
8. 村を別のプロセスが開いている（`fuseforks-cli serve` で掴む）状態で、トレイ ON のまま ✕ を押すと隠れずに終了する
9. 同じ状態（村を掴まれている）・トレイ ON で `fuseforks.exe --autostart` を起動すると、**窓が出て覆いに失敗の理由が出る**。
   トレイは出ない（D0 の 5）
10. 言語を切り替えると、トレイのメニューの文言が切り替わる

## 裁定（2026-10-20。旧未決 1〜7。7 つとも推奨どおり）

理由はいずれも Goal 4（OFF なら今と何も変わらない）。

1. **トレイのアイコンは `minimizeToTray` が真の間だけ出す** — 承認。作るのは組み立てが成功した後で、作る前はアイコンが
   無い（組み立てに失敗したプロセスがトレイに残らない。D0・D2）
2. **閉じる確認のチェックは「起動と終了」へ移す** — 承認（D6）。付帯: 保存先は `fuseforks.settings.v1` の
   `confirmClose` のまま鍵を変えない / 辞書の `closeWindow.intro` の「常駐しません」は廃止し、トレイの ON / OFF で出し分ける
3. **起動時の一括起動は `{app_data_dir}/startup.json`** — 承認（D1）。村に入れると配った先で開いただけで全員が起動し、
   `batch_start_invariant` が防いでいる事故そのものになる
4. **自動起動はプラグインを使わない** — 条件つきで承認（D4）。条件: Windows の登録は `windows-registry` で自前で書き、
   パスを引用符で包む / `get_autostart` は HKCU と HKLM の両方を読み、`set_autostart(false)` は両方を消しにいく /
   登録名は `fuseforks` で、実機の値名を確かめて大小まで固定する（`productName` は小文字の `fuseforks` を再確認済み。
   値の名前は大文字小文字を区別しない。実機の値名は P4 で読む）
5. **`--autostart` かつトレイ ON のときだけ隠して起こす** — 条件つきで承認（D5）。条件: 組み立てに失敗したら必ず
   窓を出す（D0 の 5）。P0 で隠した窓の webview が `initialize()` を走らせないと分かったら、隠さない側へ倒す余地を残す
6. **MSI のアンインストール後に残る登録は文書で受け、Homebrew の `zap` に plist を足す** — 承認（D11）。
   「アンインストールの前に OFF にする」を DETAIL / PRIVACY / リリースノートの 3 箇所に書く
7. **時間あたりの上限は本 Spec に入れない** — 承認（D10）。Spec 07 の順序（まず実測 → 必要なら日次 / 月次の上限）に従う。
   本 Spec が足すのは `launch: source=` の行と、3 つ全部 ON のときの 1 文だけ

## Notes

### 1. Spec 07 の「トレイ常駐後に重くなる論点」との対応

| 論点 | 本 Spec |
|---|---|
| 11. 無人運用では通知が構造的要件になる | 範囲外（P4 の鐘・通知）。無人で起きた起動は `launch: source=autostart` で数えられるようにする |
| 12. 無人 + 定期 + LLM = 見張りのないコスト | 範囲外（裁定 7）。3 つ全部 ON のときに 1 文 |
| 13. ✕ が終了でなくなる | **本 Spec の D2 / D6** — ✕ は隠す、確認は「終了」へ |

### 2. 2026-08-07〜08 の判断との関係

当時の利用者の言葉は「タスクトレイ常駐ではなく閉じるときに確認ダイアログを出せればいい」。常駐を見送った
理由は `closeConfirm.ts` の doc にある「常駐は『開いただけで課金が始まる作りにしない』を裏返す」。
本 Spec はこの理由を次の形で保つ:

- 3 つとも既定 OFF（開いただけでは何も変わらない）
- 一括起動の設定は村に入らない（配った先で開いただけで起動しない）
- 隠す設定は「閉じると止まる」を「閉じても止まらない」へ変えるので、確認は「終了」の側へ移し、ページの説明文で
  止まらないことを言い切る

### 3. サインイン直後の MCP

個体の起動は MCP の接続に失敗しても止まらない（`connect_all`）。サインイン直後にネットワークが整う前に
一括起動すると、リモートの MCP の道具を持たないまま稼働する。**本 Spec では起動を遅らせる仕組みを作らない**
（頻度を見てから。`launch: source=autostart` の直後の `McpServerStatus` の失敗で数えられる）。

### 4. rev1 の査読の反映（2026-10-20。2 系統 19 点 → 採用 12 / 前提を訂正して採用 5 / 反証 2）

| # | 指摘 | 判定 | rev2 |
|---|---|---|---|
| 1-1 | 隠して起動したまま組み立てに失敗すると失敗に気づけない | 採用 | D0 の 5（失敗なら `show()` + `set_focus()`）・P4-9 |
| 1-2 | トレイを作る時点と隠す判定の順序が決まっていない | 採用 | D0 を新設 |
| 1-3 | トレイ ON で Cmd+Q が `hide()` に吸われて終了できなくなる | 前提を訂正して採用 | ソースの読みで Cmd+Q は `CloseRequested` を通らない（現況）。隠す分岐は `CloseRequested` の 1 箇所だけと D2 に明記。副産物で Cmd+Q が今も閉じる確認を素通りしていると分かった（Notes 5） |
| 1-4 | 旧プラグインが書いた HKLM が残り、`CurrentUser` の `is_enabled` は HKCU しか見ない | 前提を訂正して採用 | 旧プラグインは存在しない（自動起動を配ったことが無い）。`is_enabled` は両方を読む。ただし `disable` は HKCU しか消さないので、HKLM に値があると外せない — `scope` を返して直し方を出す（D4） |
| 1-5 | 隠した窓で webview が走るか / OFF の作り方が変わる | 採用 | D5 に案 A・B を並べ P0 で比べる。走らない場合の設計の分岐を書いた（→ P0 で走ると分かり、案 A に決まった） |
| 1-6 | `auto-launch` は引用符を渡す口が無い。フォークか自前か | 前提を訂正して採用 | `build` はパスを検査しないので、包んだ文字列を渡せば値に入る。macOS は包まない。版を留めて値を P0 で読む（D4） |
| 1-7 | Rust で起動するとフロントが `AgentStatusChanged` を取りこぼす | **反証** | `initialize()` は購読を張ってから `refreshAll()` で全件を取り直す（`useOrchestrator.ts` の「購読を先に張る」）。D3 にその根拠を書いた |
| 1 軽微 | 登録名の大小（`Fuseforks` かもしれない） | **反証（実測）** | `tauri.conf.json` の `productName` は `fuseforks`。現況に書いた |
| 1 軽微 | macOS の plist / Linux の `.desktop` を読む手段が無い | 採用 | `auto-launch` が書く固定の形だけを読む小さな関数（D4・P1） |
| 1 軽微 | 左クリックの記述が同じ文の中で逆 | 採用 | D2 で OS ごとに分けた |
| 1 軽微 | D8 で例外が GUI だけと読めない | 採用 | 「GUI の端末側」「ヘッドレスはこの例外を持たない」 |
| 1 軽微 | 失敗したときにチェックを戻すか | 採用 | D7 — ファイルの失敗は押す前へ、OS の失敗は読み直した値へ |
| 2-1 | 1-1 と同じ（不可視のゾンビ） | 採用 | D0 |
| 2-2a | 一括起動がスイッチの戻しより先に走り、既定の値で動く | 前提を訂正して採用 | 既定は厳しい側で、扉と予定は今も画面より先に動く（新しい窓ではない）。D10 の書き方を正確にした |
| 2-2b | `manage` の前に待つと覆いが外れない | 採用 | D0 の 4・D3 — `manage` の後に別のタスク |
| 2-3 | 画面が `minimizeToTray` を知る経路が無い / `allow-hide` が無い | 採用 | D1・D2・P2 |
| 2-4 | 終了の確認が待ち行列で重なる・後ろに積まれる | 採用 | D2 — `exitConfirmInFlight` で 2 回目を積まない。他の確認は取り消さない |
| 2-5 | 「全般」に案内があると書いている | 前提を訂正して採用 | rev1 の記述は「ユーザー / 言語 / 起動と終了」で、コードと一致している。後半（`VILLAGE_PAGES` に入れず、開くたびに読み直す）は採用（D7） |
| 2-6 | 言語の切り替えでトレイの文言を差し替える配線が無い | 採用 | D2・P1 — `set_language` に `AppHandle` |
| 2-7 | macOS のモードを明示しないと plist のパスと食い違う | 採用 | D4 — `MacOSLaunchMode::LaunchAgent` を明示 |

2-2 は前半（a）と後半（b）を分けて書いたが、数えるときは 1 点（前提を訂正して採用）とした。
査読 2-6 / 2-7 が引用した文（「未確定なら OS ロケール」「パスは `current_exe()`」）は rev1 に無かったが、指摘の実体
（配線の欠け・指定の明示）は rev1 にも当てはまったので採用した。

### 5. 見つけた別件 — macOS の Cmd+Q は閉じる確認を通らない（本 Spec の範囲外）

ソースの読み（現況）では、macOS の Cmd+Q は `terminate:` → `applicationWillTerminate` → 終了へ進み、
`CloseRequested` も止められる `ExitRequested` も出ない。つまり **今の閉じる確認（2026-08-08）は、macOS では ✕ と
赤いボタンにしか掛かっていない**。実機が無いので測っていない。直すなら既定のアプリメニューの「終了」を
自前のメニュー項目へ差し替え、閉じる確認へ通す形になる（別件として起票を検討）。

### 6. 裁定の反映（2026-10-20。7 つとも推奨どおり、条件 2 つ）

裁定の文面は rev1 の査読（Notes 4 の 1 系統目）を下敷きにしており、rev2 の実測と食い違う前提が 3 つあった。
**裁定の結論はそのまま採り、前提の食い違いはここに残す**（結論を覆す理由にはならなかった）:

| 裁定の前提 | rev2 の実測 | rev3 の扱い |
|---|---|---|
| `auto-launch` 0.6.0 には引用符つきのパスを渡す口が無いので、Windows は自前で書く必要がある | `build` はパスを検査しないので、包んだ文字列はそのまま値に入る（フォークも自前も要らない） | **自前で書く（裁定どおり）**。理由を「口が無い」から「値の形が `auto-launch` の内部の `format!` に寄りかからない / 読む側も `windows-registry` なので書く側と揃う」へ置き換えた（D4） |
| 旧版が HKLM に書いた値が残っていると、OS は起動するのに `is_enabled` は偽を返す | Fuseforks は自動起動を配ったことが無い。`auto-launch` の `is_enabled` は HKLM と HKCU の両方を読む | **両方を読み、両方を消しにいく（裁定どおり）**。HKLM の削除が拒否されたら `scope: machine` で返す（D4） |
| `productName` が `Fuseforks` なら NSIS の値名と大小がずれる | `productName` は小文字の `fuseforks`（再確認）。レジストリの値の名前は大文字小文字を区別しない | **`fuseforks` で固定（裁定どおり）**。実機の値名は P4 で読む |

裁定 5 に添えられた判定の 3 行（`initError || !ready` なら出す / 自動起動かつトレイなら隠す / それ以外は出す）は、
1 つの時点で評価すると `setup` では `ready` が必ず偽なので常に出す側になる。rev3 は判定を `setup` と組み立ての結果の
2 つの時点に割って同じ結果にした（D5）。

裁定が P0 の残りとして挙げた 3 点のうち、**Cmd+Q の経路は rev2 でソースを読んで決着している**（`CloseRequested` を
通らない。現況）。実機（Mac）が無いので P0 で測ることはできず、ソースの読みを根拠として記録する。残る 2 点
（隠した窓で `initialize()` が走るか / deb・rpm の AppIndicator の依存）は P0 で測る。
