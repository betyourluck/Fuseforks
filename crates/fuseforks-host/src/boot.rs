//! 村を開いて組み立てる — [`build_host`]（Spec 64 D1）。
//!
//! **起動の配線はここ 1 実装。** GUI（`apps/gui-tauri` の `state.rs`）と
//! `fuseforks-cli` はこれを呼ぶだけで、別々に組まない — 別々に組むと、同梱ツールの
//! 登録漏れ・前判定の承認の差し込み漏れが片方だけに起きる。呼び出しごとに違うのは
//! [`HostBootOptions`] だけ。
//!
//! 順序は GUI がそれまで `build_state` で持っていたものに、村の排他ロック（D3）が
//! 最初の手として足されただけ:
//! `create_dir_all` → **ロック** → ログ → `version:` 行 → 秘密のストア → `bootstrap` →
//! 同梱ツール 9 本 → MCP の初期接続 → 扉 → 前判定の承認 → Jev → 単価表の取得元。
//! 起動ログの並び（P0 で採った基準。P1 で一致を確認）は `起動しました`（`open_log` が出す）→
//! `version:` → `session:` → `attachment gc:` → `mcp server:` → `jev:` で、
//! **扉を開く処理を外へ出さない**のはこの並びを変えないため。

use std::path::PathBuf;
use std::sync::Arc;

use fuseforks_core::secret::{secret_name_collisions, SecretNameCollision};
use fuseforks_core::{
    BlackboardTool, ConfigStore, DiffTool, EnvSecretStore, FdTool, FileTool, GrepTool,
    HttpBackendFactory, KeyringSecretStore, Orchestrator, OrchestratorConfig, RagTool,
    RememberTool, RunTool, SdTool, SecretStore, YqTool,
};

use crate::jev_settings::JevSettingsStore;
use crate::lock::{LockError, VillageLock};
use crate::mcp_server::McpServerManager;
use crate::paths::HostPaths;
use crate::pricing_source::PricingSourceStore;
use crate::probe_approvals::ApprovalStore;

/// 秘密をどこから読むか（Spec 64 D4）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretSource {
    /// OS の資格情報ストア。**GUI は常にこれ。** CLI の既定もこれ。
    Keyring,
    /// 環境変数（`FUSEFORKS_SECRET_*`）。読み取り専用。コンテナ向け。
    Env,
}

/// 呼び出し側（GUI / CLI）ごとに違う、組み立ての選択。
///
/// 欄は Spec 64 D1 の 4 つ。**欄はその機構が着地した Phase で足した**（P1 で
/// `app_version` / `open_door`、P3 で `secrets` / `run_schedules`）— 読まれない欄を
/// 先に置かない。
#[derive(Debug, Clone)]
pub struct HostBootOptions {
    /// `version: app=… profile=…` の行に書く版（D11）。GUI は `package_info().version`、
    /// CLI は `build.rs` が取った `0.4.0+g1b33d9d` の形。
    pub app_version: String,
    /// 秘密をどこから読むか（D4）。GUI は常に [`SecretSource::Keyring`]。
    pub secrets: SecretSource,
    /// 予定のティッカーを回すか（D6）。GUI と `serve` は真、`ask` と `check` は偽。
    pub run_schedules: bool,
    /// `mcp_server.json` の設定どおり扉を開くか（D9）。GUI と `serve` は真、
    /// `ask` と `check` は偽。偽でも棚（設定）は読む — 開かないだけ。
    pub open_door: bool,
}

impl HostBootOptions {
    /// GUI の組み立て（keyring・予定を回す・扉を設定どおりに開く）。
    pub fn gui(app_version: impl Into<String>) -> Self {
        Self {
            app_version: app_version.into(),
            secrets: SecretSource::Keyring,
            run_schedules: true,
            open_door: true,
        }
    }
}

/// 開いた村。組み立てが済んだ部品の束で、IPC と CLI はここを読む。
///
/// **`Drop` されるまで村を持つ** — 排他ロック（D3）は `Host` と同じ寿命で、落とすと
/// 別のプロセスが同じ村を開けるようになる。
pub struct Host {
    /// 村の排他ロック（D3）。読まないが、`Host` が生きている間は握り続ける。
    _lock: VillageLock,
    /// オーケストレーター本体。
    pub orchestrator: Arc<Orchestrator>,
    /// ワークスペースのルート（`HostPaths::workspace`）。「フォルダを開く」導線で使う。
    pub workspace: PathBuf,
    /// 外の LLM から依頼を受ける扉（Spec 25）。
    ///
    /// **設定は workspace の外**（`{data_dir}/mcp_server.json`）に住むので、
    /// `ConfigStore` ではなくこちらが持つ。開け閉めは 1 本の Mutex で直列化する
    /// （ポートの bind と解放が交差すると「開いているのに繋がらない」が出る）。
    pub mcp_server: tokio::sync::Mutex<McpServerManager>,
    /// 直近の扉の起動失敗（ポート衝突など）。画面へそのまま出す。
    pub mcp_server_error: std::sync::Mutex<Option<String>>,
    /// 単価表の取得元（Spec 41）。
    ///
    /// **workspace の外**（`{data_dir}/pricing.json`）に住む — 村の中に置くと
    /// 取得先ごと配布され、**受け取った人の村が、その人の知らない URL へ
    /// 取りに行ける状態**になる。**単価そのものは `ModelTemplate` に住み、
    /// 村と一緒に配られる**（公開情報でテンプレートの属性）。
    pub pricing_source: tokio::sync::Mutex<PricingSourceStore>,
    /// 予定の前判定をこの端末で実行してよいかの記録（Spec 28）。
    ///
    /// **workspace の外**（`{data_dir}/probe_approvals.json`）に住む —
    /// 村の中に置くと承認ごと配布され、他人が用意したコマンドが受け取った側で
    /// 黙って走る。コアへは `ProbeApprovals` として差し込んであり、
    /// **書き戻す口は IPC の層にしかない**。
    pub probe_approvals: Arc<ApprovalStore>,
    /// ツール結果の即時圧縮の設定（Spec 59）。
    ///
    /// **workspace の外**（`{data_dir}/jev.json`）に住む — `pricing.json` と
    /// 同じ理由で、**村を配ったときに、受け取った人の村が知らない送信先へ
    /// ツール結果を送る状態を作らない**。API トークンは資格情報ストア。
    pub jev: tokio::sync::Mutex<JevSettingsStore>,
    /// 資格情報ストア。**Jev のトークンの読み書きに使う**（モデルのキーは
    /// `Orchestrator` 側の口を通るので、ここを読むのは Jev だけ）。
    pub secrets: Arc<dyn SecretStore>,
}

/// 組み立ての失敗理由。
///
/// 文面は中の型のまま出す（`transparent`）— GUI の起動の覆いと CLI の標準エラーが
/// 読むのは `to_string()` で、包み直すと「どこで落ちたか」が 1 段ぼやける。
#[derive(Debug, thiserror::Error)]
pub enum HostError {
    /// `data_dir` / `workspace` を作れない。
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// 村を別のプロセスが開いている（[`LockError::Held`]）か、ロックファイルを
    /// 作れない（[`LockError::Io`]）。どちらも村を 1 つも開かずに返る（D3）。
    #[error(transparent)]
    Lock(#[from] LockError),
    /// 環境変数から読むとき、2 つ以上の鍵が同じ変数名に写る（D4）。どちらの鍵の値か
    /// 決められないので組み立てを止める。`bootstrap` の直後・MCP の接続より前で止まるので、
    /// 何も外へ出ていない。**値は載せない**（変数名と鍵だけ）。
    #[error("秘密の環境変数名が衝突しています（どちらの鍵か決められません）: {}", describe_collisions(.0))]
    SecretNameCollision(Vec<SecretNameCollision>),
    /// `bootstrap` が失敗した（保存済み `world.json` が壊れている等）。
    #[error(transparent)]
    Core(#[from] fuseforks_core::CoreError),
}

fn describe_collisions(found: &[SecretNameCollision]) -> String {
    found
        .iter()
        .map(|c| format!("{} ← {}", c.variable, c.keys.join(" / ")))
        .collect::<Vec<_>>()
        .join("; ")
}

/// 村を開いて組み立てる。
///
/// バックエンドは [`HttpBackendFactory::echo_on_failure`] で構築する。API キーが
/// 未設定でもアプリは動くが、退避したことと理由は `BackendDegraded` イベントと
/// 応答本文の両方に現れる。`strict` にすると、キーを入れるまで画面が沈黙し、
/// 設定不備なのか実装不具合なのか切り分けられなくなる。
///
/// # Errors
/// ワークスペースのディレクトリを作成できない場合、別のプロセスが同じ村を開いている
/// 場合（[`HostError::Lock`]）、または保存済み `world.json` が壊れている場合。
pub async fn build_host(paths: &HostPaths, opts: HostBootOptions) -> Result<Host, HostError> {
    let workspace = paths.workspace();
    tokio::fs::create_dir_all(&workspace).await?;

    // 村の排他ロック（D3）。**最初の手** — 取れなければログもファイルも 1 つも
    // 開かずに返す。相手がログへ書いている最中に、こちらが同じファイルを開いて
    // `起動しました` を混ぜ込む形を作らない。
    let lock = VillageLock::acquire(&workspace)?;

    // 診断ログの出口を開く。**失敗しても起動は止めない** — ログが書けないことは
    // アプリが動かない理由にならず、stderr への出力は残る。
    // 置き場をワークスペース直下にするのは、「フォルダを開く」導線でそのまま
    // 辿り着けるから（不具合の報告時に場所を説明せずに済む）。
    if let Err(err) = fuseforks_core::open_log(&workspace.join("fuseforks.log")) {
        eprintln!("[fuseforks] ログファイルを開けませんでした（stderr のみ）: {err}");
    }
    // **どの配布物が村を触ったかを、ログだけで読めるようにする**（2026-08-20）。
    //
    // 起点は事故 — 旧い版（単価の欄をまだ知らない世代）で村を開くと、`world.json` の
    // 未知の欄が**黙って落ちて書き戻される**。単価が消えて統計の金額が出なくなったが、
    // 起動の区切りが「起動しました」だけだったので、**3 回の起動のどれが古い版か**を
    // ログから判別できなかった（`failures.md` #112）。
    //
    // 版は呼び出し側が渡す。**`CARGO_PKG_VERSION` は使えない** — CI がタグから
    // 書き換えるのは `tauri.conf.json` の `version` だけで、workspace の version は
    // どのビルドでも `0.1.0` のまま。GUI は書き換えられる側（`package_info`）を渡し、
    // CLI は `build.rs` で取った版を渡す（Spec 64 D11）。
    //
    // **判別できる範囲**: 配布物（`0.1.8` 等）と手元のビルド（`0.1.0`）は分かれる。
    // GUI の手元のビルド同士は区別できない — 全部 `0.1.0` になる。
    // `profile` はその半分を埋める（`tauri dev` = debug / `tauri build` = release）。
    fuseforks_core::note!(
        "version: app={} profile={}",
        opts.app_version,
        if cfg!(debug_assertions) { "debug" } else { "release" }
    );

    // 秘密は OS の資格情報ストアにだけ置く。ワークスペースの `world.json` は
    // 平文で保存されるため、そちらへ秘密が入る経路を持たせない。コンテナでは
    // デプロイ時に注入する環境変数を読む（読み取り専用。D4）。
    let secrets: Arc<dyn SecretStore> = match opts.secrets {
        SecretSource::Keyring => Arc::new(KeyringSecretStore::new()),
        SecretSource::Env => Arc::new(EnvSecretStore::from_env()),
    };
    let factory = Arc::new(HttpBackendFactory::echo_on_failure(Arc::clone(&secrets)));

    let store = ConfigStore::new(&workspace);
    let orchestrator = Orchestrator::bootstrap(
        store.clone(),
        factory,
        Arc::clone(&secrets),
        OrchestratorConfig {
            run_schedules: opts.run_schedules,
            ..OrchestratorConfig::default()
        },
    )
    .await?;

    // 環境変数名の衝突（D4）。**`bootstrap` の直後・MCP の接続より前** — `bootstrap` は
    // LLM も MCP も呼ばないので、ここで止めれば何も外へ出ていない。数える鍵は村の
    // テンプレート ID の全部 + コードが持つ固定の鍵（Jev のトークン）。
    if opts.secrets == SecretSource::Env {
        let templates = orchestrator.templates().await;
        let found = secret_name_collisions(
            templates
                .iter()
                .map(|t| t.id.as_str())
                .chain(std::iter::once(crate::jev_settings::TOKEN_KEY)),
        );
        if !found.is_empty() {
            return Err(HostError::SecretNameCollision(found));
        }
    }

    // 同梱ツール。grep / diff の探索範囲（作業フォルダ）は各エージェントの設定から
    // 実行時に解決されるため、ここでは登録するだけでよい。
    orchestrator
        .register_tool(Arc::new(RememberTool::new(store.clone())))
        .await;
    orchestrator.register_tool(Arc::new(GrepTool)).await;
    orchestrator.register_tool(Arc::new(FdTool)).await;
    orchestrator.register_tool(Arc::new(DiffTool)).await;
    orchestrator.register_tool(Arc::new(SdTool)).await;
    orchestrator.register_tool(Arc::new(YqTool)).await;
    orchestrator.register_tool(Arc::new(FileTool)).await;

    // 見出し索引（Spec 18）。宣言フォルダは各エージェントの rag_sources から
    // 呼び出しの瞬間に解決される。**宣言が空でも登録しておく** — 提示するかは
    // spec_for が個体ごとに決める（run と同じで、ここで出し分けない）。
    orchestrator.register_tool(Arc::new(RagTool)).await;

    // 村の黒板（Spec 55）。`enabled_tools` の対象外で、提示するかは spec_for が個体ごとに決める
    // （作業フォルダがある && 黒板を使う設定）。**`file` / `sd` / `yq` は `blackboard/` の下へ
    // 書けない**（囲い）ので、これを登録しないと黒板へ書く経路が 1 本も無くなる。
    orchestrator.register_tool(Arc::new(BlackboardTool)).await;

    // コマンド実行（Spec 15 rev4）。**ポリシーはエージェント別の
    // `agents/{id}/run.json` に住み、呼び出しの瞬間に読む** — 起動時に
    // 読み込んで保持しない（利用者が手で直したら次のターンから効いてほしい）。
    // **登録が 0 件でも登録しておく。** 提示するかは `spec_for` が個体ごとに
    // 決める（`allow` が空なら自分を落とす）ので、ここで出し分けない。
    orchestrator
        .register_tool(Arc::new(RunTool::new(store.clone())))
        .await;

    // MCP サーバーへ接続する。**失敗してもアプリの起動は止めない。**
    // MCP サーバーは外部コマンドで、未インストール・パス違い・権限で普通に落ちる。
    // そこで起動しなくなるのは筋が悪い（各サーバーの結果は list_mcp_servers で読める）。
    // `mcp.json` 自体が壊れている場合もここで握る — 設定を直す画面へ到達できないと
    // 利用者は詰む。
    if let Err(err) = orchestrator.reload_mcp().await {
        fuseforks_core::note!("MCP の初期接続に失敗しました: {err}");
    }

    let orchestrator = Arc::new(orchestrator);

    // 外の LLM から依頼を受ける扉（Spec 25）。**設定は workspace の外**
    // （`{data_dir}/mcp_server.json`）— 村を配っても扉は開かない、を
    // 置き場で成立させている。既定は OFF なので、多くの村ではここは何もしない。
    //
    // **開けなくても起動は止めない**（MCP クライアントの初期接続と同じ判断）。
    // `open_door` が偽なら設定を読むだけで開かない（`ask` / `check`）。
    let data_dir = paths.data_dir();
    let mut mcp_server = McpServerManager::load(data_dir, Arc::clone(&orchestrator));
    let mcp_server_error = if opts.open_door {
        mcp_server.start_if_enabled().await
    } else {
        None
    };

    // 前判定の承認（Spec 28）。**同じ棚（data_dir）に置く理由も同じ** —
    // 村を配っても承認は付いてこない。差し込むまで前判定は 1 本も走らないので、
    // **ここを忘れると「全部 unapproved」という安全側で止まる**。
    let probe_approvals = Arc::new(ApprovalStore::load(data_dir));
    orchestrator
        .set_probe_approvals(
            Arc::clone(&probe_approvals) as Arc<dyn fuseforks_core::orchestrator::ProbeApprovals>
        )
        .await;

    // ツール結果の即時圧縮（Spec 59）。**設定と鍵が揃っている村でだけ採点器が
    // 差し込まれる** — 揃っていなければ `Shared.paragraph_scorer` は `None` のままで、
    // 圧縮の経路そのものが走らない（既定 OFF）。
    //
    // **ここでは 1 バイトも外へ出ない。** `apply` は HTTP クライアントを組むだけで、
    // 送信が起きるのはツールが 4,000 字以上を返したときから（Spec 59 D10）。
    // 「接続を確かめる」は画面のボタンからだけ呼ぶ（この関数は `probe` を持たない）。
    let jev = JevSettingsStore::load(data_dir);
    let jev_active = crate::jev_settings::apply(
        &orchestrator,
        jev.config(),
        jev.blocked().is_some(),
        secrets.as_ref(),
    )
    .await;
    fuseforks_core::note!(
        "jev: enabled={} active={jev_active} blocked={}",
        jev.config().enabled,
        jev.blocked().is_some()
    );

    Ok(Host {
        _lock: lock,
        orchestrator,
        workspace,
        mcp_server: tokio::sync::Mutex::new(mcp_server),
        mcp_server_error: std::sync::Mutex::new(mcp_server_error),
        // **読むだけ**。ここでは 1 度も取りに行かない（Spec 41 の凍結）。
        pricing_source: tokio::sync::Mutex::new(PricingSourceStore::load(data_dir)),
        probe_approvals,
        jev: tokio::sync::Mutex::new(jev),
        secrets,
    })
}
