//! 起動前検査の材料を集める（Spec 64 D5 / D6）。
//!
//! 検査の本体はコアの純関数 [`headless_preflight`] で、ここは**ファイルと秘密の有無だけを
//! 読んで**材料を揃える層。`fuseforks-cli` の `check` / `ask` / `serve` の 3 つが同じ
//! [`preflight`] を呼ぶ — `check` が見た材料と `ask` / `serve` が見た材料を食い違わせない
//! （CI で `check` が通ってコンテナで落ちる形を作らない）。
//!
//! **村を開かない。** ロックを取らず、`bootstrap` も MCP の接続もしない（`build_host` は
//! 組み立ての途中で MCP サーバーへ繋ぎ、stdio なら子プロセスを起こす）。`ask` と `serve` は
//! この検査を**組み立ての前に**走らせるので、拒否が 1 件でもあれば LLM にも MCP にも
//! 触れずに止まる。読むのは `world.json` / `schedules.json` / `mcp.json`（共通と個体別）/
//! `village_id` / 棚の `probe_approvals.json` と `jev.json` と、選んだ置き場の秘密の有無。
//! **何も書かない**（村の識別子が無くても作らない — [`ConfigStore::read_village_id`]）。
//!
//! Spec 65 で材料が増えた — 起動する個体の `run.json` の `allow` / `headers` の秘密の参照 /
//! パスの有無 / PATH / プロセスの時刻帯 / 棚の `bake.json` の `sourceTimeZone`。

use std::collections::BTreeSet;
use std::path::Path;

use fuseforks_core::command::RunApproval;
use fuseforks_core::headless::{
    headless_preflight, Finding, HeadlessMode, HostView, PathKind, ProcessTimeZone, RunCommand,
    SecretRef, StdioCommand,
};
use fuseforks_core::mcp::McpServerConfig;
use fuseforks_core::model::AgentId;
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::{ConfigStore, EnvSecretStore, World};

use crate::boot::{check_secret_names, secret_store, HostError, SecretSource};
use crate::jev_settings::{stored_token, JevSettingsStore};
use crate::paths::HostPaths;
use crate::probe_approvals::ApprovalStore;

/// 起動する集合の指定（D6。`--start`）。**既定値を持たない** — 呼び出し側が必ず選ぶ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartSpec {
    /// 一括起動の対象（GUI の全体 ▶ と同じ述語。[`World::batch_start_ids`]）。
    Batch,
    /// 窓口だけ（`ask` 専用）。
    Reception,
    /// 名指しした個体。
    Ids(Vec<String>),
}

/// 検査の注文。`fuseforks-cli` の引数をそのまま写す。
#[derive(Debug, Clone)]
pub struct PreflightRequest {
    /// `ask` か `serve` か（`check --for`）。
    pub mode: HeadlessMode,
    /// 起動する集合。
    pub start: StartSpec,
    /// 秘密の置き場。
    pub secrets: SecretSource,
    /// `--bypass-plan-review`。
    pub bypass_plan_review: bool,
    /// `--run-approval`。
    pub run_approval: RunApproval,
    /// `serve --door-port`（Spec 65 D9）。`ask` では `None`。
    pub door_port: Option<u16>,
}

/// 検査の結果。
#[derive(Debug, Clone)]
pub struct PreflightReport {
    /// 解決した起動する集合（一覧の表示順。`ask` では窓口を含む）。
    pub start: Vec<AgentId>,
    /// 指摘（重い順）。
    pub findings: Vec<Finding>,
    /// 環境変数から読めた秘密の変数名（`--secrets env` のときだけ。値は持たない）。
    pub env_variables: Vec<String>,
}

impl PreflightReport {
    /// 拒否が 1 件でもあるか（`ask` / `serve` はここで止まり、`check` は終了コード 3）。
    pub fn rejected(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.level == fuseforks_core::headless::FindingLevel::Reject)
    }
}

/// 検査の材料を揃えられなかった理由。
#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    /// `--start` に村に居ない id がある（引数の誤り — 終了コード 2）。
    #[error("--start の {0} は村にいません（サーヴァントの id を書いてください）")]
    UnknownAgent(String),
    /// `--start reception` を `ask` 以外で使った（引数の誤り — 終了コード 2）。
    #[error("--start reception は ask でだけ使えます（serve では batch か id を書いてください）")]
    ReceptionOnlyForAsk,
    /// `world.json` が読めない・秘密の変数名が衝突する（組み立ての失敗と同じ — 終了コード 5）。
    #[error(transparent)]
    Host(#[from] HostError),
}

/// 検査の材料を揃えて [`headless_preflight`] を走らせる。
///
/// # Errors
/// [`PreflightError`] を参照。
pub async fn preflight(
    paths: &HostPaths,
    req: &PreflightRequest,
) -> Result<PreflightReport, PreflightError> {
    let store = ConfigStore::new(paths.workspace());
    let world = World::from_persisted(store.load_world().await.map_err(HostError::Core)?);

    // 組み立てと同じ置き場・同じ衝突の検査（D4）。衝突は組み立ての失敗として返す。
    let secrets = secret_store(req.secrets);
    let templates = world.templates();
    let all_ids: Vec<AgentId> = world.agent_names().into_iter().map(|(id, _)| id).collect();
    let village_refs = mcp_secret_ref_names(&store, &all_ids).await;
    check_secret_names(req.secrets, templates.iter().map(|t| t.id.as_str()), &village_refs)?;

    let start = resolve_start(&world, req)?;
    let start_set: BTreeSet<AgentId> = start.iter().cloned().collect();

    // 予定。宛先が存在しない予定は `bootstrap` が落とすので、ここでも数えない
    // （警告「宛先が集合の外」を、起動すれば消える予定に出さない）。読めない
    // `schedules.json` は `bootstrap` と同じく起動を止める理由にしない。
    let schedules: Vec<_> = store
        .load_schedules()
        .await
        .map(|loaded| loaded.tasks)
        .unwrap_or_default()
        .into_iter()
        .filter(|task| world.agent(&task.to).is_ok())
        .collect();

    // 前判定・後判定の承認。鍵は村の識別子を含むので、識別子が無ければどれも未承認
    // （実際に開いたときも作り直された識別子では一致しない — 同じ結論）。
    let village_id = store.read_village_id().await;
    let approvals = ApprovalStore::load(paths.data_dir());
    let probe_approved = |probe: &fuseforks_core::schedule_probe::ScheduleProbe| {
        village_id
            .as_deref()
            .is_some_and(|id| approvals.is_approved(&probe.approval_key(id)))
    };

    let jev = JevSettingsStore::load(paths.data_dir());
    let jev_token_present = stored_token(secrets.as_ref()).is_some();

    let (stdio_mcp_commands, mcp_secret_refs) = mcp_materials(&store, &start).await;
    let run_commands = run_commands(&store, &start).await;
    let process_time_zone = read_process_time_zone();
    let baked_time_zone = read_baked_time_zone(paths.data_dir());

    let secret_present = |key: &str| secrets.contains(key).unwrap_or(false);
    let command_on_path = |command: &str| fuseforks_core::resolve_program(command).is_some();
    let view = HostView {
        mode: req.mode,
        start: &start_set,
        secret_present: &secret_present,
        probe_approved: &probe_approved,
        jev_token_present,
        jev_pruning_enabled: jev.config().enabled,
        bypass_plan_review: req.bypass_plan_review,
        run_approval: req.run_approval,
        stdio_mcp_commands: &stdio_mcp_commands,
        mcp_secret_refs: &mcp_secret_refs,
        run_commands: &run_commands,
        path_kind: &path_kind,
        command_on_path: &command_on_path,
        door_port: req.door_port,
        door_token_present: secrets.contains(crate::boot::DOOR_TOKEN_KEY).unwrap_or(false),
        process_time_zone: process_time_zone.as_view(),
        baked_time_zone: baked_time_zone.as_deref(),
    };
    let findings = headless_preflight(&world, &schedules, &view);

    let env_variables = match req.secrets {
        SecretSource::Env => EnvSecretStore::from_env().variable_names(),
        SecretSource::Keyring => Vec::new(),
    };

    Ok(PreflightReport {
        start,
        findings,
        env_variables,
    })
}

/// `--start` を個体の並びへ解決する（D6）。**`ask` はどの値でも窓口を足す。**
///
/// 窓口が未設定・削除済みなら足さない — その指摘（拒否）は検査の側が出す。
fn resolve_start(world: &World, req: &PreflightRequest) -> Result<Vec<AgentId>, PreflightError> {
    let mut start = match &req.start {
        StartSpec::Batch => world.batch_start_ids(),
        StartSpec::Reception => {
            if req.mode != HeadlessMode::Ask {
                return Err(PreflightError::ReceptionOnlyForAsk);
            }
            Vec::new()
        }
        StartSpec::Ids(ids) => {
            let mut out = Vec::new();
            for raw in ids {
                let id = AgentId::from(raw.as_str());
                if world.agent(&id).is_err() {
                    return Err(PreflightError::UnknownAgent(raw.clone()));
                }
                if !out.contains(&id) {
                    out.push(id);
                }
            }
            out
        }
    };
    if req.mode == HeadlessMode::Ask
        && let Some(reception) = world.reception()
        && world.agent(reception).is_ok()
        && !start.contains(reception)
    {
        start.push(reception.clone());
    }
    Ok(start)
}

/// MCP の材料（**有効なサーバーだけ**）: stdio の起動コマンドと、http の `headers` が参照する
/// 秘密の名前。共通の `mcp.json` は名前のまま、個体別は `id:名前`。読めない設定は飛ばす
/// （組み立ても MCP の初期接続の失敗では止まらない）。
async fn mcp_materials(
    store: &ConfigStore,
    start: &[AgentId],
) -> (Vec<StdioCommand>, Vec<SecretRef>) {
    let mut stdio = Vec::new();
    let mut refs = Vec::new();
    let mut collect = |label: String, config: &McpServerConfig| {
        if !config.enabled() {
            return;
        }
        match config {
            McpServerConfig::Stdio(own) => stdio.push(StdioCommand {
                server: label,
                command: own.command.clone(),
            }),
            McpServerConfig::Http(own) => refs.extend(own.secret_ref_names().into_iter().map(
                |name| SecretRef {
                    server: label.clone(),
                    name,
                },
            )),
        }
    };
    if let Ok(common) = store.read_mcp_config().await {
        for (name, config) in &common.servers {
            collect(name.clone(), config);
        }
    }
    for id in start {
        if let Ok(own) = store.read_agent_mcp_config(id).await {
            for (name, config) in &own.servers {
                collect(format!("{id}:{name}"), config);
            }
        }
    }
    (stdio, refs)
}

/// 有効な http の MCP サーバーが `headers` で参照する秘密の名前（共通と、渡した個体の個体別。
/// 重複なし・名前の順）。秘密の変数名の衝突検査（[`check_secret_names`]）が読む — `build_host` と
/// 起動前検査は**村の全個体**を渡して、数える範囲を揃える。
pub(crate) async fn mcp_secret_ref_names(store: &ConfigStore, ids: &[AgentId]) -> Vec<String> {
    let (_, refs) = mcp_materials(store, ids).await;
    let names: BTreeSet<String> = refs.into_iter().map(|r| r.name).collect();
    names.into_iter().collect()
}

/// 起動する個体の `run.json` の `allow` の先頭の語（重複は個体ごとに 1 つ）。`run` を持つかは
/// 検査の側が見る。読めない `run.json` は飛ばす（`run` は読めないとき全部を承認待ちにする）。
async fn run_commands(store: &ConfigStore, start: &[AgentId]) -> Vec<RunCommand> {
    let mut out: Vec<RunCommand> = Vec::new();
    for id in start {
        let Ok(policy) = store.read_command_policy(id).await else {
            continue;
        };
        for pattern in &policy.allow {
            let Some(command) = pattern.split_whitespace().next() else {
                continue;
            };
            if !out.iter().any(|r| &r.agent == id && r.command == command) {
                out.push(RunCommand {
                    agent: id.clone(),
                    command: command.to_owned(),
                });
            }
        }
    }
    out
}

/// パスの種類（起動前検査の [`PathKind`]）。読めないパスは「無い」に数える。
fn path_kind(path: &str) -> PathKind {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_dir() => PathKind::Dir,
        Ok(_) => PathKind::File,
        Err(_) => PathKind::Missing,
    }
}

/// プロセスの時刻帯（持ち主のある形。[`ProcessTimeZone`] は借用なので変換して渡す）。
enum OwnedTimeZone {
    Named(String),
    // 作るのは時刻帯のファイルを確かめられる Unix の経路だけ。
    #[cfg_attr(not(unix), allow(dead_code))]
    Unreadable(String),
    Unknown,
}

impl OwnedTimeZone {
    fn as_view(&self) -> ProcessTimeZone<'_> {
        match self {
            Self::Named(name) => ProcessTimeZone::Named(name),
            Self::Unreadable(tz) => ProcessTimeZone::Unreadable(tz),
            Self::Unknown => ProcessTimeZone::Unknown,
        }
    }
}

/// 時刻帯のファイルの置き場（Debian の tzdata。像に入れてある — Spec 65 D6）。
#[cfg(unix)]
const ZONEINFO: &str = "/usr/share/zoneinfo";

/// プロセスの時刻帯を読む。**`TZ` を先に見る**（Spec 65 契約 11）— Linux の
/// `iana-time-zone` は `TZ` を見ずに `/etc/localtime` を読むので、`TZ=Asia/Tokyo` の
/// コンテナで `Etc/UTC` を返す（P0 実測）。`chrono::Local` は `TZ` に従う。
fn read_process_time_zone() -> OwnedTimeZone {
    if let Ok(raw) = std::env::var("TZ") {
        let value = raw.strip_prefix(':').unwrap_or(&raw).trim();
        if !value.is_empty() {
            return time_zone_from_env(value);
        }
    }
    match iana_time_zone::get_timezone() {
        Ok(name) => OwnedTimeZone::Named(name),
        Err(_) => OwnedTimeZone::Unknown,
    }
}

/// `TZ` の値を名前にする。時刻帯のファイルが無ければ**読めない**（`chrono` は黙って UTC に
/// 落ちる — P0 実測の `TZ=Bogus/Zone`）。絶対パスで書かれていれば置き場の下の部分を名前にする。
#[cfg(unix)]
fn time_zone_from_env(value: &str) -> OwnedTimeZone {
    let (file, name) = if value.starts_with('/') {
        let name = value
            .strip_prefix(ZONEINFO)
            .map(|rest| rest.trim_start_matches('/'))
            .unwrap_or(value);
        (Path::new(value).to_path_buf(), name)
    } else {
        (Path::new(ZONEINFO).join(value), value)
    };
    if file.is_file() {
        OwnedTimeZone::Named(name.to_owned())
    } else {
        OwnedTimeZone::Unreadable(value.to_owned())
    }
}

/// Windows には時刻帯のファイルの置き場が無いので、`TZ` の値をそのまま名前にする
/// （コンテナは Linux。Windows で `TZ` を設定して `serve` する形は検査の対象外）。
#[cfg(not(unix))]
fn time_zone_from_env(value: &str) -> OwnedTimeZone {
    OwnedTimeZone::Named(value.to_owned())
}

/// 棚の `bake.json` の `sourceTimeZone`（Spec 65 D2 の 6）。無ければ `None`（`bake` で作った
/// 写しではない = 比べる相手が無い）。**目録の型の全体は書き手と一緒に P2 で決める** —
/// ここは時刻帯の 1 欄だけを読む。壊れた `bake.json` も `None`（`bake` が書くファイルで、
/// 手で壊したときに起動を止める理由にしない）。
fn read_baked_time_zone(data_dir: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Baked {
        source_time_zone: Option<String>,
    }
    let text = std::fs::read_to_string(data_dir.join("bake.json")).ok()?;
    serde_json::from_str::<Baked>(&text).ok()?.source_time_zone
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `TZ` の値は時刻帯のファイルがあれば名前、無ければ「読めない」（chrono は黙って UTC に
    /// 落ちる — Spec 65 P0）。絶対パスで書かれていれば置き場の下の部分を名前にする。
    #[cfg(unix)]
    #[test]
    fn tz_values_resolve_against_zoneinfo() {
        if !Path::new(ZONEINFO).join("Asia/Tokyo").is_file() {
            eprintln!("tzdata が無いので飛ばす");
            return;
        }
        assert!(matches!(
            time_zone_from_env("Asia/Tokyo"),
            OwnedTimeZone::Named(n) if n == "Asia/Tokyo"
        ));
        assert!(matches!(
            time_zone_from_env("/usr/share/zoneinfo/Asia/Tokyo"),
            OwnedTimeZone::Named(n) if n == "Asia/Tokyo"
        ));
        assert!(matches!(
            time_zone_from_env("Bogus/Zone"),
            OwnedTimeZone::Unreadable(v) if v == "Bogus/Zone"
        ));
    }

    /// 棚の bake.json の sourceTimeZone だけを読む。無い・壊れていれば None。
    #[test]
    fn the_baked_time_zone_is_read_from_the_shelf() {
        let dir = std::env::temp_dir().join(format!("ff-bake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(read_baked_time_zone(&dir), None, "無ければ None");
        let manifest = r#"{"sourceTimeZone":"Asia/Tokyo","maps":[]}"#;
        std::fs::write(dir.join("bake.json"), manifest).unwrap();
        assert_eq!(read_baked_time_zone(&dir).as_deref(), Some("Asia/Tokyo"));
        std::fs::write(dir.join("bake.json"), "{ broken").unwrap();
        assert_eq!(read_baked_time_zone(&dir), None, "壊れていれば None");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
