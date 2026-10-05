# Spec 64: コアを GUI なしで動かす（ホストの切り出しとヘッドレス実行）

- 状態: **Draft rev1**（2026-10-06 起票。査読待ち）
- 起点: 利用者（2026-10-06）—「以前から構想されていた fuseforks-core と GUI の完全分離構想について、
  仕様を作ってください」
- 前提の裁定（2026-09-14 利用者）:「最初は GUI で回して、そのフローが自動化で失敗しないようになったら、
  クラウドに core と workspace をコンテナにして送って実行できるようにしたい。これは最終的な目標」
  — **GUI は設計と安定化の場、コンテナは安定したフローを実行する場**。順番は「GUI で失敗しなくなる」が先
- 材料: CLAUDE.md「設計の材料 3 件 > core 単独実行の構想」（ギャップ 6 つ・2026-08-29）/
  「agent-orchestrator の実読」（2026-09-18。デーモンと GUI の寿命・多重起動の防止・秘密の渡し方）/
  「外から MCP の扉へ繋ぐ形」（2026-09-22）

## Goal

1. **村を GUI なしで動かせる。** Tauri を一切リンクしない実行ファイルが、GUI と同じ村（`world.json` /
   `sessions.redb` / 条例 / 黒板 / 予定）を開き、同じコアで依頼を処理する
2. **使い方は 3 つ。** `check`（この村を GUI なしで回せるかを、LLM を 1 回も呼ばずに判定する）/
   `ask`（依頼を 1 通送り、答えを標準出力へ書いて終わる）/ `serve`（常駐して予定と MCP の扉を回す）
3. **GUI の挙動は 1 つも変わらない。** 起動の手順・ログの行・IPC・画面はそのまま。変わるのは
   「同じ村を 2 つのプロセスが同時に開けなくなる」1 点だけ（Goal 4）
4. **同じ村を 2 つのプロセスが同時に開くことを、構造で拒む。** GUI とヘッドレス、ヘッドレス同士のどちらも
5. **コンテナへ持っていける形にする。** 秘密を OS の資格情報ストア以外から読める / パスを引数で渡せる /
   GUI でしか解けない待ちを起動前に名指しで拒む。**ただしコンテナの像・配布・クラウドへの配置は範囲外**（別 Spec）

**やらないこと（範囲外）**: Docker イメージとその配布 / クラウドへの配置手順 / GUI を常駐プロセスの
クライアントにすること（「採らなかった形」1）/ 扉の bind を 127.0.0.1 以外へ開くこと（D9）/
飛行中のターンを再起動後に再開すること / 1 プロセスで複数の村を開くこと / Release のアセットに
ヘッドレスの実行ファイルを足すこと（D12）

## 前提の実測（2026-10-06）

**GUI 層は 3,865 行。そのうち Tauri に依存するのは IPC の受け口だけで、持ち上げる側の依存は 7 か所。**

| ファイル | 行 | Tauri への依存 | 行き先 |
|---|---|---|---|
| `commands.rs` | 1,570 | 240（`#[tauri::command]` と `State<'_, AppState>`） | **GUI に残る**（IPC の受け口） |
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
  同時 1 本・新しい因果の根・天井は新品・送り手は `Endpoint::External`。**`ask` はこれを呼ぶだけで、
  オーケストレーションの側に新しい経路は 1 本も要らない**
- **版番号**: GUI は `package_info().version`（CI がタグから `tauri.conf.json` を書き換える）。
  workspace の `version` はどのビルドでも `0.1.0` のまま（`failures.md` #112 の処方の注記）
- **MSRV**: workspace の `rust-version` は 1.85。`std::fs::File::try_lock` は 1.89 で安定化（D3）

## Design

### D1. crate の構成 — ホストを 1 つの crate に寄せる

```text
crates/fuseforks-core      … 変更は最小（D4 の EnvSecretStore / D7 の検査 / D6 の予定を止める設定）
crates/fuseforks-host      … 新設。Tauri を知らない。村を開いて組み立て、閉じる
   ├─ paths.rs             … HostPaths（data_dir / workspace）
   ├─ lock.rs              … 村の排他ロック（D3）
   ├─ boot.rs              … build_host（今の build_state の本体）
   ├─ mcp_server.rs        … ← apps/gui-tauri から移動
   ├─ probe_approvals.rs   … ← 同
   ├─ pricing_source.rs    … ← 同
   └─ jev_settings.rs      … ← 同
apps/gui-tauri/src-tauri   … Tauri の Builder・IPC（commands.rs）・イベントの橋だけ
apps/headless              … 新設。実行ファイル（名前は未決 1）。check / ask / serve
```

- **起動の配線は `fuseforks-host::build_host` の 1 実装。** GUI と CLI が別々に組むと、同梱ツールの登録漏れ・
  前判定の承認の差し込み漏れ（`state.rs` の注記「ここを忘れると全部 unapproved という安全側で止まる」）が
  片方だけに起きる。**2 つ目の組み立てを書かない**
- `AppState` は `Host`（ホストが返す構造体）を 1 つ持つ形へ縮む。`commands.rs` は `state.orchestrator` を
  `state.host.orchestrator` へ読み替えるだけで、**IPC の名前・引数・戻り値は 1 つも変えない**
- 移動する 4 ファイルは**中身を変えず、`use` と可視性だけを直すコミット**で動かす（巨大ファイル分割の 6 箇条の 3 —
  差分が「移動」だと読める形に保つ）。`tauri::async_runtime::spawn` は `tokio::spawn` へ（Tauri の非同期ランタイムは
  tokio の上に乗っているので、GUI の挙動は変わらない — P1 で GUI の起動ログを突き合わせて確かめる）

### D2. パス — data_dir を引数で受ける

```rust
pub struct HostPaths {
    /// 端末ごとの棚（mcp_server.json / pricing.json / probe_approvals.json / jev.json の置き場）。
    pub data_dir: PathBuf,
    /// 村。常に data_dir/workspace。
    pub workspace: PathBuf,
}
```

- **GUI は Tauri の `app_data_dir()` を渡す**（今と同じ場所。Windows は `%APPDATA%\jp.outcasts.fuseforks`）
- **CLI は `--data-dir` を必須にする。既定値を持たない**（未決 2）。既定で GUI と同じ場所を開くと、利用者が
  意図せず GUI の村をヘッドレスで触る形が一番起きやすい形になる。D3 のロックが同時は止めるが、
  「GUI を閉じた隙に CLI が予定を消化した」は止めない
- 棚と村の 2 層は今と同じ。**「村を配っても扉は開かない・承認は付いてこない・単価の取得先は付いてこない」が
  そのまま「イメージに秘密や承認を焼かない」になる**（コンテナでは data_dir ごとマウントし、棚のファイルは
  運用者が意図して置く）

### D3. 村の排他ロック — 同じ村を開けるのは 1 プロセスだけ

- `build_host` の**最初の手**として `{workspace}/.fuseforks.lock` を作り（無ければ）、OS の排他ロックを取る
  （`std::fs::File::try_lock`）。取れなければ**組み立てを始めずに**エラーで返す
  - GUI: 起動の覆いに「この村は別のプロセスが開いています（Fuseforks の GUI か、ヘッドレスの実行）」と出す
  - CLI: 終了コード 4（D10）と同じ文を標準エラーへ
- **ロックファイルの存在ではなく OS のロックで判定する。** プロセスが落ちれば OS が外すので、強制終了の後に
  残ったファイルを人が消す手順が要らない（`lib.rs` の doc が「ロックファイルを自作すると強制終了後の残留を
  自分で面倒みることになる」と書いて避けた問題は、OS のロックなら起きない）
- ロックは `Host` が持ち、`Host` が Drop されるまで外さない
- **MSRV を 1.89 へ上げる**（`File::try_lock` の安定化）。開発機は 1.98。CI の toolchain は stable なので影響なし
- 単一インスタンスのプラグインは**残す**。役目が違う — プラグインは「2 つ目の GUI を前面化して閉じる」
  （利用者への見せ方）、ロックは「村を 2 重に開かない」（データの安全）
- **ロックの中身には何も書かない。** Windows の `LockFileEx` はロック中のファイルを他プロセスから読めなくする
  ので、PID を書いても相手は読めない。「誰が持っているか」は文言の 2 択で足りる

### D4. 秘密 — `EnvSecretStore`（読み取り専用）

```rust
/// 環境変数から秘密を読む（Spec 64）。**書けない。** set / delete はエラーを返す。
pub struct EnvSecretStore { /* 起動時に 1 回だけ読んだ写し */ }
```

- 鍵 → 変数名: `FUSEFORKS_SECRET_` + 鍵を大文字にして英数字以外を `_` へ（`claude_sonnet` →
  `FUSEFORKS_SECRET_CLAUDE_SONNET` / `jev_api_token` → `FUSEFORKS_SECRET_JEV_API_TOKEN`）
- **衝突は起動で拒む** — 村のテンプレート ID 2 つが同じ変数名に写るなら、どちらの鍵か決められないので
  組み立てを止める（`world.json` を読んだ後で判定できる）
- **起動時に 1 回だけ環境から読み、以後は環境を見ない**（ターンの途中で値が変わる経路を作らない）
- **値をどこにも出さない**（`SecretStore` の規律そのまま）。`check` が出すのは「どの変数が要るか / 有るか」だけ
- **既定は keyring のまま。** CLI は `--secrets keyring|env` で選ぶ（既定 keyring）。GUI は常に keyring
- `secret.rs` の doc「なぜ環境変数ではないのか」は**デスクトップの話として残し、1 段落足す** —
  「コンテナには資格情報ストアが無い。デプロイ時に注入する環境変数が正しい置き場」。
  **書き込めない実装にしたのは #1 の教訓の側**（画面から設定したキーが環境変数へ消える経路を作らない）

### D5. GUI でしか解けない待ちを、起動前に名指しする（`check` と起動前の検査）

コアに純関数 `headless_preflight(&World, &HostView) -> Vec<Finding>` を置き、`check` は結果を出して終わり、
`ask` / `serve` は**拒否が 1 件でもあれば LLM を 1 回も呼ばずに止まる**。

| 検査 | 重さ | 理由 |
|---|---|---|
| 計画の確認（`planReview`）が ON の個体が、起動する集合に居る | **拒否**（`--bypass-plan-review` で通す） | 波が人の承認を永久に待つ（Spec 43）。予定の「確認を自動で通す」（Spec 53）と同じ欄を立てるだけで、新しい機構は無い |
| 起動する集合のテンプレートに秘密が無い | **拒否** | 1 通目で 401 になり、`echo_on_failure` で偽の応答が返る — ヘッドレスでは誰も画面を見ていない |
| `ask` で窓口（`reception`）が未設定・削除済み | **拒否** | `ask_external` が即座に断るので、起動して MCP を繋いでから断るより前で止める |
| `ask` で窓口の接続先（委譲・転送・`plan` の相手）が起動する集合の外 | 警告 | 窓口は動くが、委譲は `NOT_RUNNING` で返る。窓口だけで答える依頼なら困らないので拒否にはしない。`--start batch` で直る |
| コマンドの承認モードが「承認が必要」で、`run` を持つ個体が居る | 警告（`--run-approval` で変えられる） | 待ちにはならない（拒否文が返ってターンは進む）。ただし `pending` は誰も承認しない |
| 前判定・後判定のコマンドがこの棚で未承認 | 警告 | 予定は発火しても配送しない（`unapproved`）。承認は棚の `probe_approvals.json` を GUI で作ってから一緒に持っていく |
| 判断役があるのに Jev の鍵が無い / ツール結果の圧縮が ON で鍵が無い | 警告 | 判断役は無効で起動し、呼べない（Spec 62 の述語どおり） |
| MCP サーバーが `command` で起動する stdio | 情報 | コンテナにその実行ファイルがあるかは `check` からは分からない（接続は `serve` / `ask` の起動時に試す） |

- **検査の材料はファイルだけ**（`world.json` / `schedules.json` / `probe_approvals.json` / `jev.json` /
  秘密の有無）。**LLM も MCP も 1 回も呼ばない** — `check` は CI やコンテナのビルド時に安く回せる
- 拒否と警告の文は**直し方を書く**（`failures.md` #44 — 何が起きたかと次に何をするかを両方）
- 「起動する集合」は D6 の `--start` が決める

### D6. 起動する個体と、一括起動の不変条件

- **`batch_start_invariant`（アプリを開いた時点では誰も走らない）は GUI では不変。**
  ヘッドレスは `--start batch|none|<id,...>` で**明示したときだけ**起動する。既定は `none`（未決 3）。
  **例外は `ask` の窓口 1 体だけ** — `ask` は窓口へ送る以外の動作を持たないので、窓口が止まっていると必ず失敗する。
  `ask` を打つこと自体が窓口を動かす明示の意図なので、窓口は `--start` に関わらず起動する
  - 不変条件の理由は「開いただけでトークンを払う作りにしない」。CLI の引数で `--start` を書くのは明示の意図で、
    開いただけではない（CLAUDE.md「システム設定への追加候補」で既に書いた線）
- **`ask` では予定のティッカーを止める。** `ask` の数秒〜数分の間に期限の来た予定が発火すると、頼んでいない
  仕事が同じプロセスで走る。`OrchestratorConfig` に `run_schedules: bool`（既定 true）を 1 欄足し、
  `bootstrap` はこれが偽ならティッカーを起こさない。**消化の記録（`lastConsumedDueMs`）にも触れない**
  ので、次に GUI か `serve` で開いたときは今までどおり「再開時に 1 回だけ」が働く
- `serve` は予定を回す（GUI と同じ）

### D7. `ask` — 1 通送って、答えを出して、閉じる

```text
<bin> ask --data-dir <dir> [--secrets env] [--start batch] [--new-session] [--client <名前>]
          [--bypass-plan-review] [--run-approval <mode>] [--events jsonl]  <依頼文 | - で標準入力>
```

1. ロック → 組み立て（`build_host`。扉は開かない — D9）→ D5 の検査 → 窓口と `--start` の個体を起動
2. `Orchestrator::ask_external(client, message)` を 1 回呼ぶ。**送り手は `Endpoint::External`**（既定の名乗りは
   `fuseforks-cli`、`--client` で変えられる。D8 の外部クライアントの呼び名の設定があればそれが勝つ — Spec 25 と同じ）
3. 答えを**標準出力へそのまま**書く。改行は 1 つだけ足す
4. 起動した個体を全部止め、ターンが閉じて `turn:` 行と `Record::Turn` が書かれるのを待ってから終わる

- **送り手を `User` にしない理由は Spec 25 D6 と同じ** — 端末から打つ人と、cron や CI から呼ぶスクリプトを
  プロセスの側から区別できない。人の依頼として封筒に書くと、そうでない場合に封筒が嘘になる
- **待ちの上限は村の `ask_timeout`**（Spec 44。既定 600 秒）。`ask_external` がそのまま使う
- 会話は**最新の会話へ続ける**（MCP の扉と同じ）。`--new-session` で新しい会話を作ってから送る
  （評価の走行を毎回まっさらにしたいとき）。未決 4
- `--events jsonl` を付けると、`CoreEvent` を 1 行 1 イベントの JSON で**標準エラー**へ流す
  （IPC と同じワイヤ形。新しい形を作らない）。標準出力は答えだけに保つ — パイプで次のコマンドへ渡せるように

### D8. `serve` — 常駐して、予定と扉を回す

```text
<bin> serve --data-dir <dir> [--secrets env] [--start batch|none|<ids>] [--bypass-plan-review] [--run-approval <mode>]
```

- ロック → 組み立て → D5 の検査 → `--start` の個体を起動 → 扉は `mcp_server.json` の設定どおり開く
  （GUI と同じ）→ **SIGINT / SIGTERM（Windows は Ctrl+C）まで待つ**
- 終わり方: 新しい配送を止め → 飛行中のターンに打ち切りを送り（Spec 10 の `interrupt_all`）→ 個体を止め →
  ロックを外す。**猶予は 30 秒**で、超えたら打ち切りを待たずに終わる（払いの記録が欠けうることを
  標準エラーに 1 行書く — `failures.md` #103 の「払ったのに記録に出ない」をここで黙って作らない）
- コンテナの外から状態を見る口（`/healthz` 等）は**作らない**。プロセスが生きていること自体が状態で、
  中身は `fuseforks.log` が持つ。要るようになったら別 Spec（agent-orchestrator の `running.json` + `/healthz` が参照実装）

### D9. 扉の bind は 127.0.0.1 のまま — コンテナでは同じネットワーク名前空間のプロキシで受ける

**ギャップ 3（bind 固定）は、コードを変えずに越える。**

- k8s の Pod や `docker run --network container:<id>` では、同じ Pod / コンテナの中のプロキシから
  `127.0.0.1` へ届く。外向きの TLS・認証・回数の上限はプロキシが持つ（2026-09-22 の利用者の見立て
  「外から繋ぐならサーバー認証で仲介する形」と同じ）
- プロキシは `Host` を `127.0.0.1:<port>` へ書き換える（rmcp の既定 `allowed_hosts` が loopback 3 種なので、
  書き換えないと拒まれる）。**これはコンテナの Spec の手順で、本 Spec では文書に 1 段落書くだけ**
- 凍結（Spec 25 `mcp_server_contract` 凍結 4）は動かさない

### D10. 終了コード

| コード | 意味 |
|---|---|
| 0 | `ask`: 答えが返った（予算切れ・打ち切りの定型文でも、答えとして返れば 0）/ `check`: 拒否が 0 件 / `serve`: シグナルで正常に閉じた |
| 1 | コアがエラーを返した（`ask_external` の `CoreError`。標準エラーに `code` と文面を 1 行） |
| 2 | 引数の誤り |
| 3 | D5 の検査で拒否された（`check` は拒否が 1 件以上） |
| 4 | 村のロックが取れない（D3） |
| 5 | 組み立てに失敗した（`world.json` が壊れている・秘密の変数名が衝突した 等） |

- **予算切れを 0 にする理由**: 予算切れ・打ち切りは**ターンの結末**であって、プロセスの失敗ではない。
  ただし答えの本文で分かる（Spec 11 の定型文）。**イベントからは読めない** — `TurnRecorded` は
  `agentId` と `sessionId` しか持たず、結末（`stop` の 7 値）は `sessions.redb` の `Record::Turn` と
  `fuseforks.log` の `turn:` 行にある。区別したい呼び出し側のために終了コードを分けるかは未決 5

### D11. 版番号

- `build_host` は `app_version: &str` を引数で受け、今と同じ `version: app=… profile=…` の行を書く
- GUI は今までどおり `package_info().version`
- CLI は**ビルド時に `git describe --tags --abbrev=0` から取る**（`build.rs`。GUI の画面の版番号が vite で
  同じことをしている）。タグが無ければ `0.0.0`（ステータスバーと同じ規則 — 打っていないリリースを名乗らない）

### D12. 配布はしない（この Spec では）

- ヘッドレスの実行ファイルは**ソースからのビルドだけ**（`cargo build -p <未決 1> --release`）。Release の
  アセット・winget・tap には足さない
- 理由: 配るなら Linux 向けの静的リンク・コンテナの像・署名の扱いが要り、それはコンテナの Spec で一緒に決める。
  先に実行ファイルだけ配ると、像を作るときに配布物の形を 2 回決めることになる

### D13. 外へ送るもの（PRIVACY）

- **新しい送信先は無い。** ヘッドレスは GUI と同じコア・同じ設定で動くので、送るものは GUI と同じ
- PRIVACY 日英に 1 節足す: 「GUI を使わずに実行した場合も、送るものは同じ。秘密を環境変数から読む形を選んだ場合、
  値はプロセスの環境に置かれる（OS の資格情報ストアの保護は効かない）。置き場を選ぶのは実行する人」

## 採らなかった形

1. **GUI を常駐プロセスのクライアントにする**（agent-orchestrator の形 — Electron が Go のデーモンへソケットで繋ぐ）。
   「完全分離」をこの意味で取ると、IPC 117 本をすべてソケット越しの API にし、イベントの橋をネットワークに載せ、
   GUI と常駐の寿命の結び方（`owner` + 握り続けるソケット）を設計することになる。**得るのは「GUI を閉じても
   村が回り続ける」で、それは `serve` が GUI なしで既に与える**。GUI で設計して安定させ、無人では `serve` で回す、
   という利用者の順番（2026-09-14）には、GUI がコアを同じプロセスに持つ今の形で足りる
2. **CLI に既定の data_dir を持たせる** — D2
3. **ロックファイルの存在で判定する** — D3。強制終了の後に残る
4. **扉を `0.0.0.0` で開く設定を足す** — D9。トークンの漏れがそのまま外への穴になる。プロキシで受ければ要らない
5. **`ask` で窓口以外の個体へ直接送る `--to`** — 外部の入口が 2 本になり、Spec 25 の「村の中はブラックボックス」と
   「同時 1 本」の門を 2 か所で守ることになる。窓口を変えたいなら `world.json` の `reception` を変える
6. **検査を警告だけにする** — 計画の確認の待ちはヘッドレスでは**永久に解けない**ので、警告にすると
   「動かないが理由は 1 行だけログにある」を作る

## Tasks

### P0 — 測ってから凍結する

- [ ] GUI の起動ログ（`fuseforks.log` の起動から `jev:` 行まで）を今のビルドで採取し、P1 の比較の基準にする
- [ ] `File::try_lock` の挙動を 3 OS で確かめる（同じプロセス内で 2 回取ったとき / 別プロセス / プロセスを殺した後）。
      Windows は CI のランナーで、macOS / Linux も CI で
- [ ] `tauri::async_runtime::spawn` を `tokio::spawn` へ替えて、扉の開け閉めが GUI で同じに動くか（結合テスト
      `tests/mcp_server_wire.rs` が緑のまま）
- [ ] `data_contract.yaml` に `headless_host_contract` を凍結（D2 / D3 / D4 / D5 の拒否 3 つ / D6 / D7 / D10）
      と、`batch_start_invariant` への注記（GUI では不変・ヘッドレスは明示のときだけ）

### P1 — ホストの切り出し（挙動を 1 つも変えない）

- [ ] `crates/fuseforks-host` を新設し、4 ファイルを**中身を変えずに**移す（1 コミット 1 ファイル）
- [ ] `build_state` の本体を `build_host(paths, app_version, secrets) -> Host` へ移し、GUI は呼ぶだけにする
- [ ] `commands.rs` の読み替え（`state.host.…`）。IPC の名前・形は不変（`tests/ipc_contract.rs` が緑）
- [ ] P0 で採った起動ログと 1 行ずつ一致することを確かめる

### P2 — 村の排他ロック（GUI にも効く唯一の挙動の変化）

- [ ] `lock.rs` + `build_host` の最初の手。MSRV 1.89
- [ ] GUI の起動の覆いの文言（ja / en）
- [ ] 結合テスト: 同じ村を 2 回 `build_host` すると 2 回目が `Locked` / 1 回目を Drop した後は取れる /
      別プロセス（テストから子プロセスを起こす）でも取れない

### P3 — 秘密の読み口と起動前の検査（コア）

- [ ] `EnvSecretStore`（変数名の写像・衝突の検出・set/delete はエラー・値をエラー文に載せない）
- [ ] `headless_preflight`（純関数。D5 の表の 8 行を 1 つずつ単体で）
- [ ] `OrchestratorConfig::run_schedules`（偽ならティッカーを起こさない。既定 true でバイト等価）

### P4 — 実行ファイル

- [ ] `apps/headless`（名前は未決 1）: `check` / `ask` / `serve`、引数の解析、終了コード、`build.rs` の版番号
- [ ] 結合テスト: 秘密の無いテンプレートの村で `check` が 3 を返す / echo のバックエンドで `ask` が答えを標準出力へ
      書いて 0 で終わる / GUI と同じ村をロック中に開くと 4 / `--events jsonl` が標準エラーにだけ出る

### P5 — 台帳

- [ ] DETAIL 3 言語に「GUI なしで動かす」の節 / README 3 言語に 1 行 / PRIVACY 日英（D13）/
      CLAUDE.md の「core 単独実行の構想」のギャップ 6 つに、どれを閉じたかを書き戻す / `secret.rs` の doc

### P6 — 実機

- [ ] 開発機の村を `--data-dir` でコピーし、`check` → `ask` → `serve` を通す
- [ ] GUI を開いたまま同じ村へ `ask` → 4 で止まる / 逆に `serve` 中に GUI を開く → 覆いに文言
- [ ] `--secrets env` で、keyring に何も無い状態から `ask` が通る
- [ ] 計画の確認 ON の進行役が居る村で `serve` → 3 で止まり、`--bypass-plan-review` で通る

## 未決

1. **実行ファイルの名前。** 推奨は `fuseforks-cli`（サブコマンドに `serve` があっても、1 回きりの `ask` と `check` が
   主な使い道なので `d` = デーモンは名前が嘘になる）。候補は `fuseforksd` / `fuseforks-headless`。
   GUI の bin 名が `fuseforks` なので、それと衝突しない名前に限る
2. **CLI の data_dir に既定値を持たせないこと**（D2）。推奨は「持たせない」
3. **`--start` の既定。** 推奨は `none`（`ask` の窓口は D6 の例外で常に起動する。委譲先も要るなら `--start batch`）。
   `batch` を既定にすると、`ask` 1 回で一括起動の全員が MCP に接続する
4. **`ask` の会話は既定で「続ける」か「新しく作る」か**（D7）。推奨は「続ける」（扉と同じ。評価の走行は `--new-session`）
5. **予算切れ・打ち切りを終了コードで分けるか**（D10）。推奨は分けない（0）。分けるなら 6 / 7 を足す

## Notes

### 1. CLAUDE.md のギャップ 6 つとの対応

| ギャップ | 本 Spec |
|---|---|
| 1. ホストの実行ファイル | **閉じる**（D1・P1・P4） |
| 2. `batch_start_invariant` との衝突 | **閉じる**（D6 — GUI は不変、ヘッドレスは明示のときだけ） |
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
