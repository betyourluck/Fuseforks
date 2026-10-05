# Spec 64: コアを GUI なしで動かす（ホストの切り出しとヘッドレス実行）

- 状態: **Draft rev2・未決 0**（2026-10-06 起票 → 同日、査読 2 系統 26 点を反映して rev2。表は Notes 4。
  rev2 の再査読で未決 1 を rev2 の方針どおりに閉じた）
- 起点: 利用者（2026-10-06）—「以前から構想されていた fuseforks-core と GUI の完全分離構想について、
  仕様を作ってください」
- 前提の裁定（2026-09-14 利用者）:「最初は GUI で回して、そのフローが自動化で失敗しないようになったら、
  クラウドに core と workspace をコンテナにして送って実行できるようにしたい。これは最終的な目標」
  — **GUI は設計と安定化の場、コンテナは安定したフローを実行する場**。順番は「GUI で失敗しなくなる」が先
- 材料: CLAUDE.md「設計の材料 3 件 > core 単独実行の構想」（ギャップ 6 つ・2026-08-29）/
  「agent-orchestrator の実読」（2026-09-18。デーモンと GUI の寿命・多重起動の防止・秘密の渡し方）/
  「外から MCP の扉へ繋ぐ形」（2026-09-22）
- **用語**: 本 Spec の「分離」は **「GUI なしでコアを回せること」** の意味で使う。GUI とコアを別プロセスに
  分ける（GUI を常駐プロセスのクライアントにする）意味ではない（「採らなかった形」1）

## rev1 からの変更（査読の反映。詳細は Notes 4）

- **組み立ての入力を `HostBootOptions` に束ねた**（D1）。予定のティッカーを回すか・扉を開くか・秘密をどこから
  読むかは呼び出し側が決める。rev1 は `build_host(paths, app_version, secrets)` で、D6 と D7 が要る設定を運ぶ口が無かった
- **`--start` を `ask` と `serve` の両方で必須にし、`none` を無くした**（D6）。rev1 の既定 `none` は
  `serve` で誰も起動しない空転を作り、`ask` では委譲が要る村で毎回警告を出していた
- **`check` の構文と、検査が何を対象にするかを書いた**（D5）。`check` は検査したいコマンドと同じ引数を取る
- **`ask` の会話の既定を「新しい会話」へ**（D7）。今の会話へ続けると、GUI で使っている会話の履歴が
  cron の依頼で埋まり、評価の走行も前の文脈を引きずる
- **終了コードを結末ごとに分けた**（D10）。`ask_external` は配送の結末（`PlanTaskState`）を既に受け取って捨てて
  いるので、それを返す口を 1 本足すだけで済む（オーケストレーションの経路は増えない）
- **`ask` も Ctrl+C を拾い、`serve` と同じ手順で閉じる**（D8）
- **`sessions.redb` が「別のプロセスが開いている」で開けないときは、起動を止める**（D3）
- **環境変数名の衝突検査は、`bootstrap` の直後・MCP の接続より前**（D4）。固定の鍵（`jev_api_token`）も数える
- **`--events jsonl` の間は標準エラーの全行を JSON にする**（D7）
- 移動と中身の変更を別コミットに分けた（P1）/ ロックの前に `create_dir_all`（D3）/ `HostPaths` は
  `data_dir` だけを持つ（D2）/ 版番号のフォールバック（D11）

## Goal

1. **村を GUI なしで動かせる。** Tauri を一切リンクしない実行ファイルが、GUI と同じ村（`world.json` /
   `sessions.redb` / 条例 / 黒板 / 予定）を開き、同じコアで依頼を処理する
2. **使い方は 3 つ。** `check`（このコマンドで GUI なしに回せるかを、LLM を 1 回も呼ばずに判定する）/
   `ask`（依頼を 1 通送り、答えを標準出力へ書いて終わる）/ `serve`（常駐して予定と MCP の扉を回す）
3. **GUI の挙動は、同じ村の 2 重オープンを拒む 1 点を除いて変わらない。** 起動の手順・ログの行・IPC・画面はそのまま
4. **同じ村を 2 つのプロセスが同時に開くことを、構造で拒む。** GUI とヘッドレス、ヘッドレス同士のどちらも
5. **コンテナへ持っていける形にする。** 秘密を OS の資格情報ストア以外から読める / パスを引数で渡せる /
   GUI でしか解けない待ちを起動前に名指しで拒む / 結末を終了コードで読める。
   **ただしコンテナの像・配布・クラウドへの配置は範囲外**（別 Spec）

**やらないこと（範囲外）**: Docker イメージとその配布 / クラウドへの配置手順 / GUI を常駐プロセスの
クライアントにすること（「採らなかった形」1）/ 扉の bind を 127.0.0.1 以外へ開くこと（D9）/
飛行中のターンを再起動後に再開すること / 1 プロセスで複数の村を開くこと / Release のアセットに
ヘッドレスの実行ファイルを足すこと（D12）

## 前提の実測（2026-10-06）

**GUI 層は 3,865 行。そのうち Tauri に依存するのは IPC の受け口だけで、持ち上げる側の依存は 7 か所。**

| ファイル | 行 | Tauri への依存 | 行き先 |
|---|---|---|---|
| `commands.rs` | 1,570 | 240（`#[tauri::command]` と `State<'_, AppState>`） | **GUI に残る**（IPC の受け口・117 本） |
| `mcp_server.rs` | 629 | 1（`tauri::async_runtime::spawn`） | ホストへ |
| `jev_settings.rs` | 516 | 0 | ホストへ |
| `probe_approvals.rs` | 338 | 0（doc コメントの 1 か所だけ） | ホストへ |
| `pricing_source.rs` | 328 | 0 | ホストへ |
| `state.rs` | 245 | 6（`app.path().app_data_dir()` ×2 / `package_info()` / `emit` / `Manager` / `AppHandle`） | 組み立ての本体をホストへ、イベントの橋だけ GUI に残す |
| `lib.rs` | 233 | — | GUI に残る（Tauri の Builder と IPC の登録） |

- **`state.rs` の `build_state` が起動の配線のすべて**（ログを開く → 版の行 → 資格情報ストア → `bootstrap` →
  同梱ツール 9 本の登録 → MCP の初期接続 → 扉 → 前判定の承認 → Jev → 単価表の取得元）。
  ここが Tauri の `AppHandle` を受け取っているので、**この関数をそのまま別の実行ファイルから呼べない**
- **コアは既に Tauri を知らない**（`fuseforks-core` の依存に tauri は無い）。分離の工事はコアではなく GUI 層の中で起きる
- **今、同じ村を 2 つのプロセスが開くと黙って壊れる。** 多重起動を止めているのは
  `tauri-plugin-single-instance`（GUI のプロセスが 2 つ並ぶのを止めるだけで、村は見ていない）。
  別の実行ファイルが同じ村を開くと:
  - `sessions.redb` は redb 自身が排他ロックを取る（`DatabaseError::DatabaseAlreadyOpen`）が、
    **コアは開けないときに WARN を 1 行出して「会話を保存しない起動」で続ける**（`bootstrap.rs:132`）
  - `world.json` と `schedules.json` は**両方のプロセスが書く**。予定は両方で発火し、`lastConsumedDueMs` を競う
  - = `lib.rs` の doc が単一インスタンスの理由に挙げた事故が、そのまま別の経路で起きる
- **秘密の鍵の名前**: モデルの API キーはテンプレートの ID（`ModelTemplate.id`）、Jev のトークンは `jev_api_token`。
  `SecretStore` の実装は `KeyringSecretStore` と `InMemorySecretStore` の 2 つだけで、コンテナには keyring が無い
- **外部からの依頼の入口は既にある**（`Orchestrator::ask_external`。Spec 25）— 窓口へ配送して待ち、
  同時 1 本・新しい因果の根・天井は新品・送り手は `Endpoint::External`。**`ask` はこれを呼ぶだけ**
- **`ask_external` は配送の結末を受け取って捨てている** — `deliver_and_wait` は `(answer, PlanTaskState)` を返し、
  `ask_external` は `let (answer, _state) = …` で状態を落としている（`runtime.rs:603`）。状態は
  `Answered / HandedOff / Undeliverable / NoAnswer / TimedOut / Interrupted / BudgetExhausted`（`plan.rs:29`）
- **版番号**: GUI は `package_info().version`（CI がタグから `tauri.conf.json` を書き換える）。
  workspace の `version` はどのビルドでも `0.1.0` のまま（`failures.md` #112 の処方の注記）
- **MSRV**: workspace の `rust-version` は 1.85。`std::fs::File::try_lock` は 1.89 で安定化（D3）

## Design

### D1. crate の構成と、組み立ての入口

```text
crates/fuseforks-core      … 変更は最小（D3 の SessionStoreLocked / D4 の EnvSecretStore / D5 の検査 /
                               D6 の run_schedules / D10 の ask_external_outcome）
crates/fuseforks-host      … 新設。Tauri を知らない。村を開いて組み立て、閉じる
   ├─ paths.rs             … HostPaths
   ├─ lock.rs              … 村の排他ロック（D3）
   ├─ boot.rs              … build_host（今の build_state の本体）
   ├─ mcp_server.rs        … ← apps/gui-tauri から移動
   ├─ probe_approvals.rs   … ← 同
   ├─ pricing_source.rs    … ← 同
   └─ jev_settings.rs      … ← 同
apps/gui-tauri/src-tauri   … Tauri の Builder・IPC（commands.rs）・イベントの橋だけ
apps/cli                   … 新設。実行ファイル fuseforks-cli。check / ask / serve
```

```rust
pub struct HostBootOptions {
    /// `version: app=… profile=…` の行に書く版（D11）。
    pub app_version: String,
    /// 秘密をどこから読むか。GUI は常に Keyring。
    pub secrets: SecretSource,          // Keyring | Env
    /// 予定のティッカーを回すか（D6）。GUI と serve は true、ask と check は false。
    pub run_schedules: bool,
    /// mcp_server.json の設定どおり扉を開くか（D9）。GUI と serve は true、ask と check は false。
    pub open_door: bool,
}

pub async fn build_host(paths: &HostPaths, opts: HostBootOptions) -> Result<Host, HostError>;
```

- **起動の配線は `build_host` の 1 実装。** GUI と CLI が別々に組むと、同梱ツールの登録漏れ・前判定の承認の
  差し込み漏れ（`state.rs` の注記「ここを忘れると全部 unapproved という安全側で止まる」）が片方だけに起きる
- **呼び出しごとに違うのは `HostBootOptions` の 4 欄だけ。** 扉を開くかを `build_host` の外（呼び出し側が
  `host.open_door()` を呼ぶ形）に出さないのは、**GUI の起動ログの行の順序を変えないため**（今は扉の行が
  前判定の承認と Jev の行より先に出る。P1 で 1 行ずつ突き合わせる）
- 組み立ての順序（GUI の今と同じ。D3 と D4 の 2 手が足される）:

  ```text
  create_dir_all(data_dir / workspace) → ロック（D3）→ ログ → 版の行 → 秘密のストア → bootstrap
  → 環境変数名の衝突検査（D4。Env のときだけ）→ 同梱ツール 9 本 → MCP の初期接続 → 扉（open_door）
  → 前判定の承認 → Jev → 単価表の取得元
  ```

- `AppState` は `Host` を 1 つ持つ形へ縮む。`commands.rs` は `state.orchestrator` を `state.host.orchestrator` へ
  読み替えるだけで、**IPC の名前・引数・戻り値は 1 つも変えない**
- 実行ファイルの名前は **`fuseforks-cli`**（未決 1 の推奨を 2 系統とも支持）。GUI の bin 名 `fuseforks` と衝突しない

### D2. パス — data_dir を引数で受ける

```rust
pub struct HostPaths { data_dir: PathBuf }
impl HostPaths {
    pub fn data_dir(&self) -> &Path;          // 端末ごとの棚（mcp_server.json / pricing.json / probe_approvals.json / jev.json）
    pub fn workspace(&self) -> PathBuf;       // 常に data_dir/workspace
}
```

- **村の場所は data_dir から導く 1 通りだけ**（欄を 2 つ持つと、将来どちらかを別に指したくなったときに
  2 つが食い違う形が生まれる）
- **GUI は Tauri の `app_data_dir()` を渡す**（今と同じ場所。Windows は `%APPDATA%\jp.outcasts.fuseforks`）
- **CLI は `--data-dir` を必須にする。既定値を持たない**（未決 2 の推奨を 2 系統とも支持）。既定で GUI と同じ場所を
  開くと、開発機で意図せず GUI の村をヘッドレスで触る形が一番起きやすくなる。D3 のロックは同時を止めるが、
  「GUI を閉じた隙に CLI が予定を消化した」は止めない
- 棚と村の 2 層は今と同じ。**「村を配っても扉は開かない・承認は付いてこない・単価の取得先は付いてこない」が
  そのまま「イメージに秘密や承認を焼かない」になる**（コンテナでは data_dir ごとマウントし、棚のファイルは
  運用者が意図して置く）

### D3. 村の排他ロック — 同じ村を開けるのは 1 プロセスだけ

- `build_host` の最初の手として `data_dir` と `workspace` を `create_dir_all` し（初回起動では無いので）、
  `{workspace}/.fuseforks.lock` を作って OS の排他ロックを取る（`std::fs::File::try_lock`）。
  取れなければ**それ以上何も開かずに** `HostError::Locked` で返す
  - GUI: 起動の覆いに「この村は別のプロセスが開いています（Fuseforks の GUI か、fuseforks-cli）」
  - CLI: 終了コード 4（D10）と同じ文を標準エラーへ
- **ロックファイルの存在ではなく OS のロックで判定する。** プロセスが落ちれば OS が外すので、強制終了の後に
  残ったファイルを人が消す手順が要らない
- ロックは `Host` が持ち、`Host` が Drop されるまで外さない
- **MSRV を 1.89 へ上げる**（`File::try_lock` の安定化）。開発機は 1.98。CI の toolchain は stable なので影響なし
- 単一インスタンスのプラグインは**残す**。役目が違う — プラグインは「2 つ目の GUI を前面化して閉じる」
  （利用者への見せ方）、ロックは「村を 2 重に開かない」（データの安全）
- **ロックの中身には何も書かない。** Windows の `LockFileEx` はロック中のファイルを他プロセスから読めなくする
  ので、PID を書いても相手は読めない。「誰が持っているか」を調べたいときは、Unix は `lsof <path>`、
  Windows は Resource Monitor の「関連付けられたハンドル」でファイル名を引く
- **`sessions.redb` が「別のプロセスが開いている」で開けないときは、起動を止める（二重の網）。**
  今の `bootstrap` はどんな理由で開けなくても WARN を 1 行出して「会話を保存しない起動」で続ける。
  ロックが正しく効いていればこの経路には来ないが、**ロックの実装に穴があったときに、2 重オープンを黙って
  続ける経路がここに残る**。`SessionStore::open` が redb の `DatabaseAlreadyOpen` を
  `CoreError::SessionStoreLocked` として区別して返し、`bootstrap` はそれだけをエラーで返す
  （壊れたファイル・権限など他の理由は今までどおり WARN で続ける — そちらは 2 重オープンではない）

### D4. 秘密 — `EnvSecretStore`（読み取り専用）

```rust
/// 環境変数から秘密を読む（Spec 64）。**書けない。** set / delete はエラーを返す。
pub struct EnvSecretStore { /* 起動時に 1 回だけ読んだ写し */ }
```

- 鍵 → 変数名: `FUSEFORKS_SECRET_` + 鍵を大文字にして英数字以外を `_` へ（`claude_sonnet` →
  `FUSEFORKS_SECRET_CLAUDE_SONNET` / `jev_api_token` → `FUSEFORKS_SECRET_JEV_API_TOKEN`）
- **起動時に 1 回だけ、`FUSEFORKS_SECRET_` で始まる変数を全部読む。** 以後は環境を見ない
  （ターンの途中で値が変わる経路を作らない）。ストアを作るのに `world.json` は要らない
- **衝突は `bootstrap` の直後・MCP の接続より前に拒む**（D1 の順序）。数える鍵は
  **村のテンプレート ID の全部 + コードが持つ固定の鍵（`jev_api_token`）**。2 つ以上の鍵が同じ変数名に写るなら、
  どちらの鍵か決められないので組み立てを止める（`HostError::SecretNameCollision`、終了コード 5）。
  `bootstrap` は LLM も MCP も呼ばないので、ここで止めれば何も外へ出ていない
- **値をどこにも出さない**（`SecretStore` の規律そのまま）。`check` が出すのは「どの変数が要るか / 有るか」だけ
- **既定は keyring のまま。** CLI は `--secrets keyring|env` で選ぶ（既定 keyring）。GUI は常に keyring
- `secret.rs` の doc「なぜ環境変数ではないのか」は**デスクトップの話として残し、1 段落足す** —
  「コンテナには資格情報ストアが無い。デプロイ時に注入する環境変数が正しい置き場。**書き込めない実装にしたのは
  #1 の教訓の側** — 画面から設定したキーが環境変数や平文のファイルへ流れる経路を作らない」

### D5. GUI でしか解けない待ちを、起動前に名指しする

**コマンドの構文**:

```text
fuseforks-cli check --for ask|serve --data-dir <dir> --start <集合> [--secrets keyring|env]
                    [--bypass-plan-review] [--run-approval <mode>] [--json]
```

- `check` は**検査したいコマンドと同じ引数を取る**（`--for` で `ask` か `serve` を名指しする）。起動する集合も
  秘密の読み先も、実際に走らせるときと同じものを検査しないと、CI で `check` が通ってコンテナで落ちる（逆も）
- 出力は人が読む文（既定）か、`--json` で `{ "findings": [ { "level", "code", "message", "fix" } ] }`
- `ask` と `serve` は**自分の起動の途中で同じ検査を走らせ**、拒否が 1 件でもあれば LLM を 1 回も呼ばずに止まる
  （`check` を別に打たなくても安全側に倒れる）

**検査の本体**はコアの純関数:

```rust
pub enum HeadlessMode { Ask, Serve }

pub struct HostView<'a> {
    pub mode: HeadlessMode,
    pub start: &'a BTreeSet<AgentId>,           // 起動する集合（D6 で解決済み。ask では窓口を含む）
    pub secret_present: &'a dyn Fn(&str) -> bool,// 選んだストアにその鍵があるか（値は見ない）
    pub probe_approved: &'a dyn Fn(&ScheduleProbe) -> bool, // 棚の承認で通るか
    pub jev_token_present: bool,
    pub jev_pruning_enabled: bool,               // jev.json
    pub bypass_plan_review: bool,
    pub run_approval: RunApproval,
}

pub fn headless_preflight(world: &World, schedules: &[ScheduledTask], view: &HostView) -> Vec<Finding>;
```

**材料はファイルと秘密の有無だけ。LLM も MCP も 1 回も呼ばない** — `check` は CI やコンテナのビルド時に安く回せる。

| 検査 | 対象 | 重さ | 理由 |
|---|---|---|---|
| 計画の確認（`planReview`）が ON の個体が起動する集合に居る | 両方 | **拒否**（`--bypass-plan-review` で通す） | 波が人の承認を永久に待つ（Spec 43）。通すときは Spec 53 のスイッチを立てるだけで、新しい機構は無い |
| 起動する集合のテンプレートに、選んだストアの秘密が無い | 両方 | **拒否** | 1 通目で 401 になり、`echo_on_failure` で偽の応答が返る — ヘッドレスでは誰も画面を見ていない |
| 窓口（`reception`）が未設定・削除済み | ask | **拒否** | `ask_external` が即座に断る。起動して MCP を繋いでから断るより前で止める |
| 窓口の接続先（委譲・転送・`plan` の相手）が起動する集合の外 | ask | 情報（`check` と `--verbose` のときだけ表示） | 委譲は `NOT_RUNNING` で返り、窓口が自分で答える。`--start reception` は利用者が選んだ形なので、毎回の警告にはしない |
| 予定の宛先が起動する集合の外 | serve | 警告 | その予定は発火しても配送されない（GUI と同じ「停止中なのでスキップ」）|
| コマンドの承認モードが「承認が必要」で、`run` を持つ個体が居る | 両方 | 警告（`--run-approval` で変えられる） | 待ちにはならない（未承認は拒否文が返ってターンは進む — `tools/run.rs`）。ただし `pending` は誰も承認しない |
| 前判定・後判定のコマンドがこの棚で未承認 | serve | 警告 | 予定は発火しても配送しない（`unapproved`）。承認は棚の `probe_approvals.json` を GUI で作ってから一緒に持っていく |
| 判断役があるのに Jev の鍵が無い / ツール結果の圧縮が ON で鍵が無い | 両方 | 警告 | 判断役は無効で起動し、圧縮は走らない（Spec 59 / 62 の述語どおり） |
| MCP サーバーが `command` で起動する stdio | 両方 | 情報 | その実行ファイルがあるかは検査からは分からない（接続は起動時に試す） |

- 拒否と警告の文は**直し方を書く**（`failures.md` #44）

### D6. 起動する個体と、一括起動の不変条件

- **`batch_start_invariant`（アプリを開いた時点では誰も走らない）は GUI では不変。**
- **ヘッドレスは `--start` を必須にする。既定値を持たない。** 値は次の 3 つ:

  | 値 | 起動する集合 |
  |---|---|
  | `batch` | 一括起動の対象（GUI の全体 ▶ と同じ述語 — `batch_start && (無所属 \|\| group.batch_start)`。Spec 51 凍結 7） |
  | `reception` | 窓口だけ（`ask` 専用） |
  | `<id>,<id>,…` | 名指しした個体 |

  - 不変条件の理由は「開いただけでトークンを払う作りにしない」。引数で `--start` を書くのは明示の意図で、開いただけではない
  - **`none` は持たない。** `serve` で誰も起動しないと、扉も予定も何も処理できない空転になる
  - **`ask` は、どの値でも窓口を集合に足す。** `ask` は窓口へ送る以外の動作を持たないので、窓口が止まっていると必ず失敗する
- **`ask` と `check` では予定のティッカーを回さない**（`HostBootOptions.run_schedules = false`）。`ask` の間に期限の来た
  予定が発火すると、頼んでいない仕事が同じプロセスで走る。コアは `OrchestratorConfig` に `run_schedules: bool`
  （既定 true = GUI はバイト等価）を 1 欄持ち、`bootstrap` はこれが偽ならティッカーを起こさない。
  **消化の記録（`lastConsumedDueMs`）にも触れない**ので、次に GUI か `serve` で開いたときは今までどおり
  「再開時に 1 回だけ」が働く
- `serve` は予定を回す。宛先が起動する集合の外にある予定は、GUI と同じく「停止中なのでスキップ」で消化される（D5 の警告）

### D7. `ask` — 1 通送って、答えを出して、閉じる

```text
fuseforks-cli ask --data-dir <dir> --start <集合> [--secrets keyring|env] [--continue-session]
                  [--client <名前>] [--bypass-plan-review] [--run-approval <mode>] [--events jsonl] [--verbose]
                  <依頼文 | - で標準入力>
```

1. 組み立て（`open_door = false` / `run_schedules = false`）→ D5 の検査 → 起動する集合を起動
2. **新しい会話を作る**（`--continue-session` のときは作らず、今の会話へ続ける）
3. `ask_external_outcome(client, message)` を 1 回呼ぶ（D10）。**送り手は `Endpoint::External`**（既定の名乗りは
   `fuseforks-cli`、`--client` で変えられる。村に外部クライアントの呼び名が設定されていればそれが勝つ —
   Spec 25 D8 の `world.json` の `externalName`）
4. 答えを**標準出力へそのまま**書く。改行は 1 つだけ足す
5. 起動した個体を全部止め、ターンが閉じて `turn:` 行と `Record::Turn` が書かれるのを待ってから、結末に応じた終了コードで終わる

- **送り手を `User` にしない理由は Spec 25 D6 と同じ** — 端末から打つ人と、cron や CI から呼ぶスクリプトを
  プロセスの側から区別できない。人の依頼として封筒に書くと、そうでない場合に封筒が嘘になる
- **既定を「新しい会話」にした理由**（rev2。査読 2-5）: 今の会話へ続けると (a) GUI で使っている会話に cron の依頼が
  積まれ、(b) 窓口の履歴（滑る窓 8 往復）に GUI の会話が乗ったまま評価が走る。**扉（MCP サーバー）は今の会話へ
  続けるまま** — 扉は GUI が開いている間の外部の割り込みで、GUI の利用者がその場で見ている
  - **代償**: GUI は起動時に最も新しく更新された会話を開くので、`ask` の後に GUI を開くと `ask` の会話が開く。
    会話の一覧から戻れる。機構は足さない
- **待ちの上限は村の `ask_timeout`**（Spec 44。既定 600 秒）
- **Ctrl+C（SIGINT / SIGTERM）を拾う。** `serve` と同じ閉じ方（D8）を通る。プロセスを即座に落とすと、
  プロバイダ側では払っているのに `turn:` 行も `Record::Turn` も残らない（`failures.md` #103 の形）
- **標準出力は答えだけ。** パイプで次のコマンドへ渡せるように
- **`--events jsonl` の間は、標準エラーの全行が JSON**: `CoreEvent` は IPC と同じワイヤ形で 1 行 1 イベント、
  CLI 自身の行（検査の結果・エラー・閉じ方の注記）は `{"type":"cli","level":…,"code":…,"message":…}`。
  プレーンテキストの行を混ぜない（パーサが壊れる）。`--events` が無いときは人が読む文

### D8. `serve` — 常駐して、予定と扉を回す

```text
fuseforks-cli serve --data-dir <dir> --start <集合> [--secrets keyring|env]
                    [--bypass-plan-review] [--run-approval <mode>] [--events jsonl]
```

- 組み立て（`open_door = true` / `run_schedules = true`）→ D5 の検査 → 起動する集合を起動 →
  **SIGINT / SIGTERM（Windows は Ctrl+C）まで待つ**
- **閉じ方（`ask` と共通）**: 新しい配送を止め → 飛行中のターンに打ち切りを送り（Spec 10 の `interrupt_all`）→
  個体を止め → ロックを外す。**猶予は 30 秒**で、超えたら打ち切りを待たずに終わる。そのときは払いの記録が
  欠けうることを標準エラーに 1 行書く（`failures.md` #103 の「払ったのに記録に出ない」を黙って作らない）。
  2 回目の Ctrl+C は猶予を待たずに終わる
- コンテナの外から状態を見る口（`/healthz` 等）は**作らない**。プロセスが生きていること自体が状態で、
  中身は `fuseforks.log` が持つ。要るようになったら別 Spec（agent-orchestrator の `running.json` + `/healthz` が参照実装）

### D9. 扉の bind は 127.0.0.1 のまま — コンテナでは同じネットワーク名前空間のプロキシで受ける

**ギャップ 3（bind 固定）は、コードを変えずに越える。**

- k8s の Pod や `docker run --network container:<id>` では、同じ Pod / コンテナの中のプロキシから
  `127.0.0.1` へ届く。外向きの TLS・認証・回数の上限はプロキシが持つ（2026-09-22 の利用者の見立て
  「外から繋ぐならサーバー認証で仲介する形」と同じ）
- プロキシは `Host` を `127.0.0.1:<port>` へ書き換える（rmcp の既定 `allowed_hosts` が loopback 3 種なので、
  書き換えないと拒まれる）。**これはコンテナの Spec の手順で、本 Spec では DETAIL に 1 段落書くだけ**
- 凍結（Spec 25 `mcp_server_contract` 凍結 4）は動かさない

### D10. 終了コード — 結末を読める形で返す

| コード | 意味 |
|---|---|
| 0 | `ask`: 答えが返った（`Answered` / `HandedOff`）/ `check`: 拒否が 0 件 / `serve`: シグナルで正常に閉じた |
| 1 | コアがエラーを返した（`CoreError`。`code` と文面を 1 行） |
| 2 | 引数の誤り |
| 3 | D5 の検査で拒否された（`check` は拒否が 1 件以上） |
| 4 | 村のロックが取れない（D3）/ `sessions.redb` を別のプロセスが開いている |
| 5 | 組み立てに失敗した（`world.json` が壊れている・秘密の変数名が衝突した 等） |
| 6 | `ask`: 答えが返らなかった（`NoAnswer` / `Undeliverable`） |
| 7 | `ask`: 待ちの上限を超えた（`TimedOut`） |
| 8 | `ask`: 打ち切られた（`Interrupted` — Ctrl+C を含む） |
| 9 | `ask`: 予算の天井で止まった（`BudgetExhausted`） |

- **`ask_external` は配送の結末を既に受け取って捨てている**（前提の実測）。コアに
  `ask_external_outcome(client, message) -> CoreResult<(String, PlanTaskState)>` を足し、`ask_external` はそれを呼んで
  状態を捨てる 1 行になる。**扉（MCP サーバー）の挙動は 1 つも変わらない**（Spec 25 の
  「失敗も文字列で返る — 会話の事実であって扉の故障ではない」はそのまま）
- 6〜9 でも**答えの本文（定型文）は標準出力へ書く**。終了コードは機械が読む結末、本文は人が読む結末
- rev1 で予算切れを 0 にしていた理由（「プロセスの失敗ではない」）は正しいが、**コンテナと CI が主な使い道で、
  日本語の定型文を解析しないと結末が読めない形は Goal 5 と矛盾する**（査読 2-6）。査読 1 の「分けるには
  `ask_external` のシグネチャを変える必要がある」は、状態が既に手元にあることで当たらない

### D11. 版番号

- `build_host` は `HostBootOptions.app_version` を受け、今と同じ `version: app=… profile=…` の行を書く
- GUI は今までどおり `package_info().version`
- CLI は**ビルド時に `build.rs` で取る**: `git describe --tags --abbrev=0`（タグ）と `git rev-parse --short HEAD`
  （コミット）。**どちらかが取れないとき（`.git` の無いソースの zip・shallow clone・git が無い）は panic せずに落とす** —
  タグが無ければ `0.0.0`、コミットが無ければ付けない。結果は `0.4.0+g1b33d9d` / `0.0.0+g1b33d9d` / `0.0.0`
  - `0.0.0` はステータスバーと同じ規則（打っていないリリースを名乗らない）。`+g<hash>` は `failures.md` #112 の
    「手元のビルド同士は区別できない」をヘッドレスの側だけ埋める（GUI は `build.rs` の射程の外のまま）
  - **GUI と CLI で手元ビルドの版の書き方が違う**（GUI のログは `0.1.0`、CLI は `0.0.0+g…`）。同じ村のログに
    両方が並んだとき、`profile=` と `app=` の形でどちらの実行かが読める。P1 の起動ログの突き合わせは
    GUI 同士の比較なので、この差は当たらない

### D12. 配布はしない（この Spec では）

- `fuseforks-cli` は**ソースからのビルドだけ**（`cargo build -p fuseforks-cli --release`）。Release の
  アセット・winget・tap には足さない
- 理由: 配るなら Linux 向けの静的リンク・コンテナの像・署名の扱いが要り、それはコンテナの Spec で一緒に決める。
  先に実行ファイルだけ配ると、像を作るときに配布物の形を 2 回決めることになる

### D13. 外へ送るもの（PRIVACY）

- **新しい送信先は無い。** ヘッドレスは GUI と同じコア・同じ設定で動くので、送るものは GUI と同じ
- PRIVACY 日英に 1 節足す: 「GUI を使わずに実行した場合も、送るものは同じ。秘密を環境変数から読む形を選んだ場合、
  値はプロセスの環境に置かれる（OS の資格情報ストアの保護は効かない）。置き場を選ぶのは実行する人」

## 採らなかった形

1. **GUI を常駐プロセスのクライアントにする**（agent-orchestrator の形 — Electron が Go のデーモンへソケットで繋ぐ）。
   「分離」をこの意味で取ると、IPC 117 本をすべてソケット越しの API にし、イベントの橋をネットワークに載せ、
   GUI と常駐の寿命の結び方（`owner` + 握り続けるソケット）を設計することになる。**得るのは「GUI を閉じても
   村が回り続ける」で、それは `serve` が GUI なしで既に与える**。GUI で設計して安定させ、無人では `serve` で回す、
   という利用者の順番（2026-09-14）には、GUI がコアを同じプロセスに持つ今の形で足りる（査読 2 系統とも支持）
2. **CLI に既定の data_dir を持たせる** — D2
3. **ロックファイルの存在で判定する** — D3。強制終了の後に残る
4. **扉を `0.0.0.0` で開く設定を足す** — D9。トークンの漏れがそのまま外への穴になる。プロキシで受ければ要らない
5. **`ask` で窓口以外の個体へ直接送る `--to`** — 外部の入口が 2 本になり、Spec 25 の「村の中はブラックボックス」と
   「同時 1 本」の門を 2 か所で守ることになる。窓口を変えたいなら `world.json` の `reception` を変える
6. **検査を警告だけにする** — 計画の確認の待ちはヘッドレスでは**永久に解けない**ので、警告にすると
   「動かないが理由は 1 行だけログにある」を作る
7. **扉を開く処理を `build_host` の外に出す**（査読 1-1 の案の片方）— GUI の起動ログの行の順序が変わる（D1）
8. **`--start` に既定値を持たせる**（`ask` は `none`、`serve` は `batch` に分ける案を含む）— 同じ引数がコマンドで
   違う意味になる。`serve` を既定 `batch` にすると、`ask` と `serve` を打ち間違えたときに一括起動の全員が
   動き出す。必須にすれば、打った引数がそのまま起動する集合になる

## Tasks

### P0 — 測ってから凍結する

- [ ] GUI の起動ログ（`fuseforks.log` の起動から `jev:` 行まで）を今のビルドで採取し、P1 の比較の基準にする
- [ ] `File::try_lock` の挙動を 3 OS で確かめる（同じプロセス内で 2 回取ったとき / 別プロセス / プロセスを殺した後）。CI のランナーで
- [ ] redb 4.1 の `Database::create` が、別プロセスが開いているときに `DatabaseAlreadyOpen` を返すことを確かめる
      （同じプロセス内で 2 回開いたときと区別して）
- [ ] `data_contract.yaml` に `headless_host_contract` を凍結（D2 / D3 / D4 / D5 の拒否 3 つ / D6 / D7 / D10）
      と、`batch_start_invariant` への注記（GUI では不変・ヘッドレスは `--start` を必須にして明示のときだけ）

### P1 — ホストの切り出し（挙動を 1 つも変えない）

- [ ] `crates/fuseforks-host` を新設し、4 ファイルを**中身を変えずに**移す（1 コミット 1 ファイル。`use` と可視性だけ）
- [ ] **別のコミットで** `mcp_server.rs` の `tauri::async_runtime::spawn` を `tokio::spawn` へ替える。
      `tests/mcp_server_wire.rs` が緑のまま（移動の差分と中身の差分を混ぜない — 巨大ファイル分割の 6 箇条の 3）
- [ ] `build_state` の本体を `build_host(paths, HostBootOptions)` へ移し、GUI は呼ぶだけにする
- [ ] `commands.rs` の読み替え（`state.host.…`）。IPC の名前・形は不変（`tests/ipc_contract.rs` が緑）
- [ ] P0 で採った起動ログと 1 行ずつ一致することを確かめる

### P2 — 村の排他ロック（GUI にも効く唯一の挙動の変化）

- [ ] `lock.rs` + `build_host` の最初の手（`create_dir_all` の後）。MSRV 1.89
- [ ] `CoreError::SessionStoreLocked` と、`bootstrap` がそれだけを止める分岐
- [ ] GUI の起動の覆いの文言（ja / en）
- [ ] 結合テスト: 同じ村を 2 回 `build_host` すると 2 回目が `Locked` / 1 回目を Drop した後は取れる /
      別プロセス（テストから子プロセスを起こす）でも取れない / ロックを迂回して `sessions.redb` だけを
      開いておくと `bootstrap` が `SessionStoreLocked` で止まる

### P3 — コア

- [ ] `EnvSecretStore`（変数名の写像・set/delete はエラー・値をエラー文に載せない）と衝突の検査（固定の鍵を含む）
- [ ] `headless_preflight`（純関数。D5 の表の 9 行を 1 つずつ単体で、`Ask` と `Serve` の両方で）
- [ ] `OrchestratorConfig::run_schedules`（偽ならティッカーを起こさない。既定 true でバイト等価）
- [ ] `ask_external_outcome`。`ask_external` はそれを呼ぶ 1 行へ（扉の結合テストが緑のまま）

### P4 — 実行ファイル

- [ ] `apps/cli`（`fuseforks-cli`）: `check` / `ask` / `serve`、引数の解析、終了コード、Ctrl+C、`build.rs` の版番号
- [ ] 結合テスト: 秘密の無いテンプレートの村で `check --for ask` が 3 / echo のバックエンドで `ask` が答えを標準出力へ
      書いて 0 / ロック中に開くと 4 / `--events jsonl` のとき標準エラーの全行が JSON として読める /
      `ask` の天井を小さくした村で 9 / `--start` を省くと 2

### P5 — 台帳

- [ ] DETAIL 3 言語に「GUI なしで動かす」の節（D9 のプロキシの 1 段落を含む）/ README 3 言語に 1 行 /
      PRIVACY 日英（D13）/ CLAUDE.md の「core 単独実行の構想」のギャップ 6 つに、どれを閉じたかを書き戻す /
      `secret.rs` の doc

### P6 — 実機

- [ ] 開発機の村を `--data-dir` でコピーし、`check` → `ask` → `serve` を通す
- [ ] GUI を開いたまま同じ村へ `ask` → 4 で止まる / 逆に `serve` 中に GUI を開く → 覆いに文言
- [ ] `--secrets env` で、keyring に何も無い状態から `ask` が通る
- [ ] 計画の確認 ON の進行役が居る村で `serve --start batch` → 3 で止まり、`--bypass-plan-review` で通る
- [ ] `ask` の途中で Ctrl+C → 8 で終わり、`turn:` 行が残る

## 未決

**ゼロ。**

~~1. `--start reception` のとき、窓口の接続先が止まっていることを `ask` で表示するか~~ → **rev2 の方針で確定**
（2026-10-06 の再査読）。「情報」として `check` と `--verbose` のときだけ出す。`--start reception` は
「窓口 1 体で済ませる」という利用者の明示で、毎回の警告は秘密の欠落や未承認のような本当の警告を埋もれさせる。

- **再査読の前提を 1 つ訂正して記録する** — 「委譲が `NOT_RUNNING` になった事実はログとイベントに正確に残る」は
  半分だけ正しい。残るのは `tool: … name=ask_agent_N ok=… body_chars=…` の行（`--events jsonl` では `toolInvoked`）で、
  **止まっていたという理由は残らない**。理由は `ask` の結果の本文に書かれ、本文はログに 1 字も書かない規律
  （`failures.md` #71）なので `fuseforks.log` からは読めない。読めるのは GUI の会話ペインのツール行を開いたとき
  （Spec 57）だけで、ヘッドレスには画面が無い。**ゆえに事前の `check` が、この情報を読める唯一の場所になる** —
  結論（`check` で出す）は同じで、理由がより強くなった

## Notes

### 1. CLAUDE.md のギャップ 6 つとの対応

| ギャップ | 本 Spec |
|---|---|
| 1. ホストの実行ファイル | **閉じる**（D1・P1・P4） |
| 2. `batch_start_invariant` との衝突 | **閉じる**（D6 — GUI は不変、ヘッドレスは `--start` 必須） |
| 3. 扉の bind 127.0.0.1 固定 | **コードを変えずに越える**（D9 — 同じ名前空間のプロキシ）。手順はコンテナの Spec |
| 4. `probe_approvals.json` の焼き込み | **既に解けていた** — 承認鍵は `villageId` で村に結び付いており、端末には結び付いていない。棚のファイルを一緒に持っていけば効く。D5 が未承認を名指しする |
| 5. 村の排他所有 | **閉じる**（D3）。設計の状態と実行の状態を bake の段で分ける話（`world.json` と `sessions.redb`）はコンテナの Spec |
| 6. GUI 前提機能の検査 | **閉じる**（D5） |

### 2. 次の Spec（コンテナ）へ渡す材料

- 像: Linux の静的リンク（`rustls` 前提なので C の TLS は要らない。`keyring` は Linux で Secret Service を引くので、
  `--secrets env` しか使わない像では feature を落とせるか確かめる）
- 起動: `serve --data-dir /data --secrets env --start batch`。`/data` はボリューム
- 外向き: 同じ Pod のプロキシ（D9）
- MCP の stdio サーバーは像に同梱するか、リモート MCP（Spec 47）へ寄せる
- 参照実装: agent-orchestrator の「鍵を 1 ターンごとにコマンドの環境変数にだけ入れる」形（`run` の子プロセスに
  秘密を継承させない規律は、この村は `env_clear` で既に持っている）

### 3. 主流の形との対照（2026-09-14 の調べ）

見た 7 実装の多くは「1 回きりの実行を単位にし、常駐はそれを受ける口として被せる」2 層だった（LangGraph /
CrewAI / OpenHands / Claude Code `-p` / Codex `exec` / orca-cli / stablyai/orca）。本 Spec の `ask` と `serve` は
この形に合わせてある。常駐させるのは待つもの（予定・外部からの依頼）があるときだけ。

### 4. rev1 の査読の反映（2026-10-06。2 系統 26 点）

| # | 指摘 | 判定 | 反映 |
|---|---|---|---|
| 1-① | 採らなかった形 1 の線引き | 支持（変更なし） | 用語の定義を冒頭へ（2-用語） |
| 1-② | D5 の重さ | 支持（`run` が待ちにならないことを `tools/run.rs` で確認済み） | — |
| 1-③ | `EnvSecretStore` と #1 | 支持 | — |
| 1-矛盾 1 | `build_host` に予定と扉の設定を運ぶ口が無い | **採用** | D1 `HostBootOptions`。扉を外に出す案は不採用（採らなかった形 7） |
| 1-矛盾 2 | `serve --start none` は空転 | **採用** | D6 — `--start` 必須・`none` を削除。コマンドごとに既定を分ける案は不採用（採らなかった形 8） |
| 1-矛盾 3 | `check` の構文が無い | **採用** | D5 — 検査したいコマンドと同じ引数 + `--for` + `--json` |
| 1-矛盾 4 | 検査がモードを知らない / `HostView` が未定義 | **採用** | D5 — `HeadlessMode` と `HostView` を定義 |
| 1-矛盾 5 | D7 の「D8」は誤参照 | **採用・前提を訂正** | Spec 25 D8 の意。呼び名の置き場は `mcp_server.json` ではなく `world.json` の `externalName` |
| 1-矛盾 6 | `ask` の Ctrl+C | **採用** | D7・D8 — 同じ閉じ方。終了コード 8 |
| 1-未決 5 | 分けるには `ask_external` のシグネチャを変える必要がある | **反証** | 結末は `deliver_and_wait` から既に返っており、`ask_external` が捨てているだけ（`runtime.rs:603`）。新しい口を 1 本足すだけ（D10） |
| 1-細 1 | 固定の鍵との衝突 | **採用** | D4 |
| 1-細 2 | `.git` が無いときの版番号 | **採用** | D11 |
| 2-1 | ロックと `bootstrap` の WARN 続行が両立しない | **採用（範囲を絞って）** | D3 — `DatabaseAlreadyOpen` だけを止める。他の理由の WARN 続行は 2 重オープンではないので残す |
| 2-2 | 「中身を変えずに移す」と `tokio::spawn` が矛盾 | **採用** | P1 — 別コミット |
| 2-3 | 衝突検査に `world.json` が要るがストアは先に作る | **採用（形を変えて）** | D4 — ストアは環境だけで作れる。衝突検査を `bootstrap` の直後へ置く（`SecretSource` は `HostBootOptions` に入れた） |
| 2-4 | `ask` が委譲する村で毎回警告 | **採用** | D5 — 情報へ下げた。未決 1 |
| 2-5 | 今の会話へ続けると GUI の履歴を汚す | **採用** | D7 — 既定を新しい会話へ。扉は続けるまま。代償（GUI が `ask` の会話を開く）を明記 |
| 2-6 | 終了コード 0 で予算切れを握りつぶす | **採用** | D10 — 6〜9 |
| 2-7 | `check` が `--secrets` を見ない | **採用** | D5 — `check` は同じ引数を取る |
| 2-8 | 標準エラーの多重化 | **採用** | D7 — `--events jsonl` の間は全行 JSON |
| 2-9 | 手元ビルドで GUI `0.1.0` と CLI `0.0.0` が食い違い、P1 の検証が不一致になる | **一部採用・前提を訂正** | P1 の比較は GUI 同士なので当たらない。CLI に `+g<hash>` を足した（D11） |
| 2-細 1 | ロックの前に `create_dir_all` | **採用** | D3 |
| 2-細 2 | ティッカーと起動 0 体 | 採用（`none` 削除で消えた） | D6 — 宛先が止まっている予定の扱いを明記 |
| 2-細 3 | Unix の `lsof` | **採用** | D3 |
| 2-細 4 | `HostPaths` の冗長 | **採用** | D2 |
| 2-細 5 | 用語の定義 | **採用** | 冒頭（README ではなく Spec に置く。README は本 Spec の着地まで触らない） |
