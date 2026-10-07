//! 村をコンテナへ持っていく写しを作る（Spec 65 D1 / D2 / D4・`container_contract` 1〜9）。
//!
//! **GUI の端末で動かす。** 元の村のロック（GUI が開いていれば止まる）と、元の端末の時刻帯を
//! 読むため。コンテナの中から元の村をマウントして動かす形は採らない（Spec 65 D2）。
//!
//! ## 2 段に分けて、止まるときは 1 バイトも書かない
//!
//! 1. **計画** — 元の村と（再 `bake` なら）写し先を全部読み、パスの置き換え・置き換え漏れ・平文の鍵・
//!    同居のファイルの合流・承認の運搬をメモリの中で済ませ、**書くもの・消すものの一覧**を作る。
//!    止まる理由（終了コード 2 / 3 / 4 / 5 / 10 / 11）は全部この段で出る
//! 2. **適用** — 一覧を書く。初回は一時フォルダに組んでから入れ替え、再 `bake` は 1 ファイルずつ
//!    原子的に置き換える（実行のファイル — 会話・Memory — には触れないので、途中で止まっても無傷）
//!
//! ## 置き場の表（D1）がすべての規則を決める
//!
//! 設計（GUI が真実を持つ・置き換える）/ 同居（`schedules.json` と `run.json` — 欄ごとに持ち主が違う）/
//! 実行（コンテナが持つ・触らない。`Memory.md` だけは初回の seed）/ 棚（`bake.json` を作る・承認は
//! 変換して写す・`jev.json` と `pricing.json` は写す・`mcp_server.json` は写さない）。**表に無いファイルは
//! 写さない**（会話・添付・ログ・書き出し・ロック）。

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use fuseforks_core::command::CommandPolicy;
use fuseforks_core::config_store::{
    AGENTS_DIR, EXTERNAL_DIR, ICON_FILE, JUDGES_DIR, JUDGE_FILE, MCP_FILE, ORDINANCE_FILE,
    SCHEDULES_FILE, USER_DIR, VILLAGE_ID_FILE, WORLD_FILE,
};
use fuseforks_core::mcp::{McpConfig, McpServerConfig};
use fuseforks_core::model::{AgentId, ConfigFileKind};
use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::ScheduledTask;
use fuseforks_core::schedule_probe::ScheduleProbe;
use fuseforks_core::world::PersistedWorld;
use fuseforks_core::ConfigStore;
use serde::{Deserialize, Serialize};

use crate::lock::{LockError, VillageLock};
use crate::paths::HostPaths;
use crate::probe_approvals::{carried_file, restrict_permissions, ApprovalStore, APPROVALS_FILE};

/// 写しの目録の名前（`{data_dir}/bake.json`。棚に置く）。
pub const BAKE_FILE: &str = "bake.json";

/// 棚のうち、丸ごと写すもの（D1。秘密を含まない）。`mcp_server.json`（扉の合鍵）は**写さない** —
/// コンテナの扉は `serve --door-port` と秘密の `door_token` で開く（D9）。
const SHELF_COPIED: [&str; 2] = [
    crate::jev_settings::CONFIG_FILE,
    crate::pricing_source::CONFIG_FILE,
];

/// 村の直下の設計のファイルのうち、中身を変えずに写すもの（D1）。`world.json` と
/// `schedules.json` は欄を置き換えるので別に組む。
const WORKSPACE_DESIGN: [&str; 3] = [ORDINANCE_FILE, VILLAGE_ID_FILE, MCP_FILE];

/// 個体のフォルダの設計のファイル（D1）。`run.json` は同居、`Memory.md` は実行なので別に組む。
fn agent_design_files() -> [&'static str; 4] {
    [
        ConfigFileKind::Construct.file_name(),
        ConfigFileKind::Skill.file_name(),
        ConfigFileKind::Mcp.file_name(),
        ICON_FILE,
    ]
}

/// パスの置き換え 1 組（`--map <元>=<先>`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathMap {
    /// 元の端末のパス（Windows 形なら大文字小文字を無視して照合する）。
    pub from: String,
    /// コンテナの中のパス（`/` で始まる）。
    pub to: String,
}

/// 写しの目録（`{data_dir}/bake.json`）。起動前検査は `sourceTimeZone` を読む（D5）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BakeManifest {
    /// 写しを作った時刻（UNIX エポックからのミリ秒）。
    pub baked_at_ms: u64,
    /// 写しを作った `fuseforks-cli` の版。
    pub app_version: String,
    /// 元の村の識別子（`village_id`。承認の鍵がこれに結び付いている）。
    pub source_village_id: String,
    /// 元の端末の時刻帯（IANA 名）。
    pub source_time_zone: String,
    /// 使った置き換え。
    pub maps: Vec<PathMap>,
}

/// `bake` の注文。
#[derive(Debug, Clone)]
pub struct BakeRequest {
    /// GUI の `data_dir`（元の村）。
    pub source: PathBuf,
    /// 写しの `data_dir`。
    pub out: PathBuf,
    /// 置き換え。`--update` で空なら `bake.json` の `maps` を引き継ぐ。
    pub maps: Vec<PathMap>,
    /// 写しを作り直す。
    pub update: bool,
    /// 元の端末の時刻帯（`--source-time-zone`）。`None` なら `iana-time-zone` で読む。
    pub source_time_zone: Option<String>,
    /// `--allow-plaintext-headers`。
    pub allow_plaintext_headers: bool,
    /// `bake.json` の `appVersion`。
    pub app_version: String,
}

/// 警告 1 件（写しは作る）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BakeWarning {
    /// 閉じた識別子。
    pub code: &'static str,
    /// 何が起きるか（**値は書かない** — 欄の位置とファイルだけ）。
    pub message: String,
}

/// 警告の識別子。
pub mod warning_codes {
    /// 自由記述の中に Windows の絶対パスがある（書き換えていない）。
    pub const FREE_TEXT_WINDOWS_PATH: &str = "FREE_TEXT_WINDOWS_PATH";
    /// `--allow-plaintext-headers` で、平文の鍵を写しに入れた。
    pub const PLAINTEXT_HEADER_COPIED: &str = "PLAINTEXT_HEADER_COPIED";
    /// 再 `bake` で、GUI に無くなった予定の消化の記録を落とした。
    pub const CONSUMED_RECORD_DROPPED: &str = "CONSUMED_RECORD_DROPPED";
    /// 再 `bake` で、写し先の `allow` にあって GUI の `allow` に無い行が消えた。
    pub const ALLOW_LINE_DROPPED: &str = "ALLOW_LINE_DROPPED";
}

/// 写しの結果。
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BakeReport {
    /// 再 `bake` だったか。
    pub update: bool,
    /// 書いたファイル（写しの `data_dir` からの相対パス・`/` 区切り）。
    pub written: Vec<String>,
    /// 消したファイル（GUI で消えた設計のファイル）。
    pub removed: Vec<String>,
    /// 置き換えたパスの欄の数。
    pub mapped_fields: usize,
    /// 写しへ運んだ承認の数（D4）。
    pub carried_approvals: usize,
    /// 初回の seed として写した `Memory.md` の持ち主。
    pub seeded_memories: Vec<String>,
    /// 使った置き換え。
    pub maps: Vec<PathMap>,
    /// 元の端末の時刻帯。
    pub source_time_zone: String,
    /// 警告。
    pub warnings: Vec<BakeWarning>,
}

/// 写しを作れなかった理由。終了コードは CLI が写す（D2 の表）。
#[derive(Debug, thiserror::Error)]
pub enum BakeError {
    /// 引数の誤り（2）。
    #[error("{0}")]
    Usage(String),
    /// パスの欄に置き換えなかった絶対パスが残る（3）。値はパスなので名指しする。
    #[error("置き換えなかったパスがあります（--map を足してください）: {}", .0.join("; "))]
    Unmapped(Vec<String>),
    /// 元の村か写し先のロックが取れない（4）。
    #[error(transparent)]
    Lock(#[from] LockError),
    /// 元の村が読めない・写しを書けない（5）。
    #[error("{0}")]
    Io(String),
    /// `headers` に平文の鍵がある（10）。**値は載せない**（ファイル・サーバー・ヘッダーの名前だけ）。
    #[error(
        "headers に平文の鍵があります（${{secret:NAME}} で書くか、--allow-plaintext-headers で通してください）: {}",
        .0.join("; ")
    )]
    PlaintextHeaders(Vec<String>),
    /// 写し先の状態が食い違う（11）。
    #[error("{0}")]
    OutState(String),
}

/// 書くもの（`Some`）・消すもの（`None`）。パスは写しの `data_dir` からの相対。
type Plan = BTreeMap<PathBuf, Option<Vec<u8>>>;

/// 写しを作る（計画 → 適用）。
///
/// # Errors
/// [`BakeError`] を参照。止まるときは写しへ 1 バイトも書かない。
pub async fn bake(req: &BakeRequest) -> Result<BakeReport, BakeError> {
    validate_maps(&req.maps)?;
    let source = HostPaths::new(&req.source);
    let source_ws = source.workspace();
    if !source_ws.join(WORLD_FILE).is_file() {
        return Err(BakeError::Io(format!(
            "元の村がありません: {}（GUI の data_dir を --data-dir に書いてください）",
            source_ws.display()
        )));
    }
    // 1. 元の村のロック。GUI が開いていれば止まる（書きかけの world.json を読まない）。
    let _source_lock = VillageLock::acquire(&source_ws)?;

    // 2. 写し先の状態（判定は写しの data_dir で）。
    let out = HostPaths::new(&req.out);
    let out_ws = out.workspace();
    let previous: Option<BakeManifest> = if req.update {
        let text = std::fs::read_to_string(req.out.join(BAKE_FILE)).map_err(|_| {
            BakeError::OutState(format!(
                "--update ですが {} がありません（bake が作った写しだけを作り直せます）",
                req.out.join(BAKE_FILE).display()
            ))
        })?;
        Some(serde_json::from_str(&text).map_err(|err| {
            BakeError::OutState(format!("{BAKE_FILE} が読めません: {err}"))
        })?)
    } else {
        if req.out.exists() && !is_empty_dir(&req.out) {
            return Err(BakeError::OutState(format!(
                "{} は空ではありません（作り直すなら --update。上書きはしません）",
                req.out.display()
            )));
        }
        None
    };
    let _out_lock = if req.update {
        Some(VillageLock::acquire(&out_ws)?)
    } else {
        None
    };

    // 3. 置き換え（--update で省けば前回のものを引き継ぐ。渡せば集合ごと置き換え）。
    let maps = match (&previous, req.maps.is_empty()) {
        (Some(manifest), true) => manifest.maps.clone(),
        _ => req.maps.clone(),
    };
    if maps.is_empty() {
        return Err(BakeError::Usage(
            "--map がありません（<元>=<先> を 1 つ以上。--update なら前回のものを引き継ぎます）".to_owned(),
        ));
    }
    let source_time_zone = match &req.source_time_zone {
        Some(name) => name.clone(),
        None => iana_time_zone::get_timezone().map_err(|_| {
            BakeError::Usage(
                "この端末の時刻帯が読めません。--source-time-zone <IANA 名>（例: Asia/Tokyo）を書いてください"
                    .to_owned(),
            )
        })?,
    };

    let mut report = BakeReport {
        update: req.update,
        maps: maps.clone(),
        source_time_zone: source_time_zone.clone(),
        ..BakeReport::default()
    };

    // 4. 元の村を読む。
    let store = ConfigStore::new(&source_ws);
    let mut world = store
        .load_world()
        .await
        .map_err(|err| BakeError::Io(format!("{WORLD_FILE} が読めません: {err}")))?;
    let mut tasks = store
        .load_schedules()
        .await
        .map_err(|err| BakeError::Io(format!("{SCHEDULES_FILE} が読めません: {err}")))?
        .tasks;
    let village_id = store.read_village_id().await.ok_or_else(|| {
        BakeError::Io(format!(
            "{VILLAGE_ID_FILE} がありません（GUI で一度開いた村から bake してください）"
        ))
    })?;
    let agent_ids: Vec<AgentId> = world.agents.iter().map(|a| a.id.clone()).collect();

    // 5. 承認の運搬に使う元の鍵は、置き換える前の予定で数える（D4）。
    let approvals = ApprovalStore::load(source.data_dir());
    let originally_approved: Vec<bool> = tasks
        .iter()
        .flat_map(task_probes)
        .map(|probe| approvals.is_approved(&probe.approval_key(&village_id)))
        .collect();

    // 6. パスの欄を置き換える。置き換え漏れは集めて 3 で止める。
    let mut unmapped = Vec::new();
    map_world_paths(&mut world, &maps, &mut report.mapped_fields, &mut unmapped);
    map_schedule_paths(&mut tasks, &maps, &mut report.mapped_fields, &mut unmapped);
    if !unmapped.is_empty() {
        return Err(BakeError::Unmapped(unmapped));
    }

    // 7. 承認の運搬（元で承認済みのものだけ、置き換えた後の鍵で）。
    let carried: Vec<String> = tasks
        .iter()
        .flat_map(task_probes)
        .zip(&originally_approved)
        .filter(|(_, approved)| **approved)
        .map(|(probe, _)| probe.approval_key(&village_id))
        .collect();
    report.carried_approvals = carried.len();

    // 8. MCP の設定を読む（平文の鍵と、自由記述のパスの検査に）。
    let mut mcp_files: Vec<(String, McpConfig)> = Vec::new();
    mcp_files.push((
        MCP_FILE.to_owned(),
        store
            .read_mcp_config()
            .await
            .map_err(|err| BakeError::Io(format!("{MCP_FILE} が読めません: {err}")))?,
    ));
    for id in &agent_ids {
        let label = format!("{AGENTS_DIR}/{id}/{MCP_FILE}");
        let config = store
            .read_agent_mcp_config(id)
            .await
            .map_err(|err| BakeError::Io(format!("{label} が読めません: {err}")))?;
        mcp_files.push((label, config));
    }
    let plaintext = plaintext_headers(&mcp_files);
    if !plaintext.is_empty() {
        if !req.allow_plaintext_headers {
            return Err(BakeError::PlaintextHeaders(plaintext));
        }
        for found in plaintext {
            report.warnings.push(BakeWarning {
                code: warning_codes::PLAINTEXT_HEADER_COPIED,
                message: format!("平文の鍵を写しに入れました: {found}"),
            });
        }
    }

    // 9. run.json（元）。壊れていれば止める（`run` は壊れた run.json を全部承認待ちにするが、
    //    写しで規則を黙って失わない）。
    let mut source_policies: BTreeMap<AgentId, Option<CommandPolicy>> = BTreeMap::new();
    for id in &agent_ids {
        let path = agent_dir(&source_ws, id).join(ConfigFileKind::Run.file_name());
        let policy = if path.is_file() {
            Some(store.read_command_policy(id).await.map_err(|err| {
                BakeError::Io(format!("{AGENTS_DIR}/{id}/run.json が読めません: {err}"))
            })?)
        } else {
            None
        };
        source_policies.insert(id.clone(), policy);
    }

    // 10. 自由記述の中の Windows パス（警告。書き換えない）。
    free_text_warnings(&source_ws, &agent_ids, &mcp_files, &source_policies, &tasks, &mut report);

    // 11. 写し先（再 bake のときだけ）を読み、同居のファイルを合流する。
    let out_store = ConfigStore::new(&out_ws);
    if req.update {
        let target_tasks = out_store
            .load_schedules()
            .await
            .map_err(|err| BakeError::OutState(format!("写し先の {SCHEDULES_FILE} が読めません: {err}")))?
            .tasks;
        merge_consumed(&mut tasks, &target_tasks, &mut report);
    } else {
        for task in &mut tasks {
            task.last_consumed_due_ms = None;
        }
    }

    let mut plan: Plan = BTreeMap::new();
    let ws_rel = Path::new(crate::paths::WORKSPACE_DIR);
    put(&mut plan, ws_rel.join(WORLD_FILE), json_bytes(&world));
    put(&mut plan, ws_rel.join(SCHEDULES_FILE), json_bytes(&tasks));
    for name in WORKSPACE_DESIGN {
        mirror(&mut plan, &source_ws.join(name), ws_rel.join(name))?;
    }
    for (dir, file) in [(USER_DIR, ICON_FILE), (EXTERNAL_DIR, ICON_FILE)] {
        mirror(&mut plan, &source_ws.join(dir).join(file), ws_rel.join(dir).join(file))?;
    }
    for judge in &world.judges {
        let rel = Path::new(JUDGES_DIR).join(judge.id.as_str()).join(JUDGE_FILE);
        mirror(&mut plan, &source_ws.join(&rel), ws_rel.join(&rel))?;
    }
    for id in &agent_ids {
        let rel = Path::new(AGENTS_DIR).join(id.as_str());
        for name in agent_design_files() {
            mirror(&mut plan, &source_ws.join(&rel).join(name), ws_rel.join(&rel).join(name))?;
        }
        // run.json（同居）: 規則は GUI、判断待ちはコンテナ。
        let run_rel = ws_rel.join(&rel).join(ConfigFileKind::Run.file_name());
        let source_policy = source_policies.get(id).cloned().flatten();
        let target_run = out_ws.join(&rel).join(ConfigFileKind::Run.file_name());
        let target_policy = if req.update && target_run.is_file() {
            Some(out_store.read_command_policy(id).await.map_err(|err| {
                BakeError::OutState(format!("写し先の {AGENTS_DIR}/{id}/run.json が読めません: {err}"))
            })?)
        } else {
            None
        };
        match (source_policy, target_policy) {
            (None, None) => {}
            (Some(mut fresh), None) => {
                fresh.pending.clear();
                put(&mut plan, run_rel, json_bytes(&fresh));
            }
            (source_policy, Some(mut kept)) => {
                let rules = source_policy.unwrap_or_default();
                let dropped: Vec<&String> =
                    kept.allow.iter().filter(|line| !rules.allow.contains(line)).collect();
                if !dropped.is_empty() {
                    report.warnings.push(BakeWarning {
                        code: warning_codes::ALLOW_LINE_DROPPED,
                        message: format!(
                            "{AGENTS_DIR}/{id}/run.json の allow から {} 行が消えます（GUI の allow に無い — 残すなら GUI の村へ書き戻す）: {}",
                            dropped.len(),
                            dropped.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" / ")
                        ),
                    });
                }
                kept.adopt_rules_from(&rules);
                put(&mut plan, run_rel, json_bytes(&kept));
            }
        }
        // Memory.md（実行）: 写し先に無いときだけ seed する。
        let memory = ConfigFileKind::Memory.file_name();
        let source_memory = source_ws.join(&rel).join(memory);
        if source_memory.is_file() && !out_ws.join(&rel).join(memory).exists() {
            put(&mut plan, ws_rel.join(&rel).join(memory), read(&source_memory)?);
            report.seeded_memories.push(id.to_string());
        }
    }
    for name in SHELF_COPIED {
        mirror(&mut plan, &source.data_dir().join(name), PathBuf::from(name))?;
    }
    put(&mut plan, PathBuf::from(APPROVALS_FILE), carried_file(carried).into_bytes());
    let manifest = BakeManifest {
        baked_at_ms: now_ms(),
        app_version: req.app_version.clone(),
        source_village_id: village_id,
        source_time_zone,
        maps,
    };
    put(&mut plan, PathBuf::from(BAKE_FILE), json_bytes(&manifest));

    // ---- ここから適用。上で止まる理由は全部出し終えている。 ----
    apply(&plan, &req.out, req.update, &mut report)?;
    Ok(report)
}

/// `--map` の検査（2）。元は空でない、先は `/` で始まり `\` を含まない、同じ元は 2 度書かない。
fn validate_maps(maps: &[PathMap]) -> Result<(), BakeError> {
    let mut seen = HashSet::new();
    for map in maps {
        if map.from.trim().is_empty() || map.to.trim().is_empty() {
            return Err(BakeError::Usage(format!("--map の元と先は空にできません: {}={}", map.from, map.to)));
        }
        if !map.to.starts_with('/') || map.to.contains('\\') {
            return Err(BakeError::Usage(format!(
                "--map の先はコンテナの中の絶対パス（/ で始まる）です: {}",
                map.to
            )));
        }
        let key = normalized(&map.from);
        if !seen.insert(key.trim_end_matches('/').to_owned()) {
            return Err(BakeError::Usage(format!("--map の元が 2 回あります: {}", map.from)));
        }
    }
    Ok(())
}

/// 元が Windows 形か（ドライブ文字か `\` で始まる）。
fn windows_form(path: &str) -> bool {
    let bytes = path.as_bytes();
    (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path.starts_with('\\')
}

/// 照合用の形。Windows 形は区切りを `/` へ・ASCII を小文字へ（どちらもバイト数を変えない）。
fn normalized(path: &str) -> String {
    if windows_form(path) {
        path.replace('\\', "/").to_ascii_lowercase()
    } else {
        path.to_owned()
    }
}

/// パスを置き換える（D2 の 3）。**最長前方一致・パス成分の境界で切る**。当たらなければ `None`。
pub fn map_path(value: &str, maps: &[PathMap]) -> Option<String> {
    let target = normalized(value);
    let mut best: Option<(&PathMap, usize)> = None;
    for map in maps {
        // 照合は「元の形」で決める（値が Windows 形でも、元が Unix 形なら当たらない）。
        if windows_form(&map.from) != windows_form(value) {
            continue;
        }
        let from = normalized(&map.from);
        let from = from.trim_end_matches('/');
        let hit = target == from
            || (target.starts_with(from) && target[from.len()..].starts_with('/'));
        if hit && best.is_none_or(|(_, len)| from.len() > len) {
            best = Some((map, from.len()));
        }
    }
    let (map, len) = best?;
    let rest = value[len..].replace('\\', "/");
    let mapped = format!("{}{}", map.to.trim_end_matches('/'), rest);
    Some(if mapped.is_empty() { "/".to_owned() } else { mapped })
}

fn map_world_paths(
    world: &mut PersistedWorld,
    maps: &[PathMap],
    mapped: &mut usize,
    unmapped: &mut Vec<String>,
) {
    for agent in &mut world.agents {
        if let Some(dir) = agent.work_dir.as_mut().filter(|d| !d.trim().is_empty()) {
            match map_path(dir, maps) {
                Some(next) => {
                    *dir = next;
                    *mapped += 1;
                }
                None => unmapped.push(format!("agents[{}].workDir = {dir}", agent.id)),
            }
        }
        for (i, source) in agent.rag_sources.iter_mut().enumerate() {
            match map_path(source, maps) {
                Some(next) => {
                    *source = next;
                    *mapped += 1;
                }
                None => unmapped.push(format!("agents[{}].ragSources[{i}] = {source}", agent.id)),
            }
        }
    }
}

/// 予定の前判定と後判定（この順）。承認の数え方（D4）と置き換えで同じ並びを使う。
fn task_probes(task: &ScheduledTask) -> impl Iterator<Item = &ScheduleProbe> {
    task.probe
        .iter()
        .chain(task.acceptance.as_ref().map(|acceptance| &acceptance.probe))
}

fn map_schedule_paths(
    tasks: &mut [ScheduledTask],
    maps: &[PathMap],
    mapped: &mut usize,
    unmapped: &mut Vec<String>,
) {
    for task in tasks {
        let id = task.id.clone();
        let probes = task
            .probe
            .iter_mut()
            .map(|p| ("probe", p))
            .chain(task.acceptance.as_mut().map(|a| ("acceptance", &mut a.probe)));
        for (which, probe) in probes {
            let Some(cwd) = probe.cwd.as_mut().filter(|c| !c.trim().is_empty()) else {
                continue;
            };
            match map_path(cwd, maps) {
                Some(next) => {
                    *cwd = next;
                    *mapped += 1;
                }
                None => unmapped.push(format!("schedules[{id}].{which}.cwd = {cwd}")),
            }
        }
    }
}

/// 鍵らしいヘッダーの名前（D2 の 4）。**推測の規則で保証ではない**。
fn secret_like_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "authorization"
            | "proxy-authorization"
            | "cookie"
            | "x-api-key"
            | "x-auth-token"
            | "api-key"
    ) || ["-key", "-token", "-secret", "-password"]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

/// `headers` の平文の鍵（**有効と無効の両方** — ファイルの中身は有効に関わらず写る）。
fn plaintext_headers(files: &[(String, McpConfig)]) -> Vec<String> {
    let mut found = Vec::new();
    for (label, config) in files {
        for (server, entry) in &config.servers {
            let McpServerConfig::Http(http) = entry else {
                continue;
            };
            for (name, value) in &http.headers {
                if secret_like_header(name) && !value.contains("${secret:") {
                    found.push(format!("{label}: {server} / {name}"));
                }
            }
        }
    }
    found
}

/// 文字列に Windows の絶対パスらしい並びがあるか（`X:\` / `X:/` / `\\`）。
fn has_windows_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    if text.contains("\\\\") {
        return true;
    }
    bytes.windows(3).enumerate().any(|(i, w)| {
        w[0].is_ascii_alphabetic()
            && w[1] == b':'
            && (w[2] == b'\\' || w[2] == b'/')
            && (i == 0 || !bytes[i - 1].is_ascii_alphanumeric())
    })
}

fn free_text_warnings(
    source_ws: &Path,
    agent_ids: &[AgentId],
    mcp_files: &[(String, McpConfig)],
    policies: &BTreeMap<AgentId, Option<CommandPolicy>>,
    tasks: &[ScheduledTask],
    report: &mut BakeReport,
) {
    let mut places: Vec<String> = Vec::new();
    for id in agent_ids {
        for kind in [ConfigFileKind::Construct, ConfigFileKind::Skill] {
            let path = agent_dir(source_ws, id).join(kind.file_name());
            if std::fs::read_to_string(&path).is_ok_and(|text| has_windows_path(&text)) {
                places.push(format!("{AGENTS_DIR}/{id}/{}", kind.file_name()));
            }
        }
    }
    for (label, config) in mcp_files {
        for (server, entry) in &config.servers {
            let McpServerConfig::Stdio(stdio) = entry else {
                continue;
            };
            if has_windows_path(&stdio.command) {
                places.push(format!("{label}: {server}.command"));
            }
            if stdio.args.iter().any(|a| has_windows_path(a)) {
                places.push(format!("{label}: {server}.args"));
            }
            for (key, value) in &stdio.env {
                if has_windows_path(value) {
                    places.push(format!("{label}: {server}.env.{key}"));
                }
            }
        }
    }
    for (id, policy) in policies {
        let Some(policy) = policy else { continue };
        if policy.allow.iter().chain(&policy.deny).any(|p| has_windows_path(p)) {
            places.push(format!("{AGENTS_DIR}/{id}/run.json: allow / deny"));
        }
    }
    for task in tasks {
        for probe in task_probes(task) {
            if has_windows_path(&probe.command) || probe.args.iter().any(|a| has_windows_path(a)) {
                places.push(format!("schedules[{}]: command / args", task.id));
            }
        }
    }
    for place in places {
        report.warnings.push(BakeWarning {
            code: warning_codes::FREE_TEXT_WINDOWS_PATH,
            message: format!("{place} に Windows の絶対パスがあります（自由記述なので書き換えていません）"),
        });
    }
}

/// 写し先の消化の記録を予定の id ごとに残す。GUI に無くなった予定の記録は落として名指しする。
fn merge_consumed(tasks: &mut [ScheduledTask], target: &[ScheduledTask], report: &mut BakeReport) {
    let kept: BTreeMap<&str, Option<u64>> = target
        .iter()
        .map(|t| (t.id.as_str(), t.last_consumed_due_ms))
        .collect();
    let live: BTreeSet<String> = tasks.iter().map(|t| t.id.clone()).collect();
    for task in tasks.iter_mut() {
        task.last_consumed_due_ms = kept.get(task.id.as_str()).copied().flatten();
    }
    for old in target.iter().filter(|t| !live.contains(&t.id)) {
        if old.last_consumed_due_ms.is_some() {
            report.warnings.push(BakeWarning {
                code: warning_codes::CONSUMED_RECORD_DROPPED,
                message: format!("予定 {} は GUI に無いので、写し先の消化の記録を落としました", old.id),
            });
        }
    }
}

fn agent_dir(workspace: &Path, id: &AgentId) -> PathBuf {
    workspace.join(AGENTS_DIR).join(id.as_str())
}

fn json_bytes<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_string_pretty(value)
        .expect("村の型は必ず直列化できる")
        .into_bytes()
}

fn put(plan: &mut Plan, rel: PathBuf, bytes: Vec<u8>) {
    plan.insert(rel, Some(bytes));
}

fn read(path: &Path) -> Result<Vec<u8>, BakeError> {
    std::fs::read(path).map_err(|err| BakeError::Io(format!("{} が読めません: {err}", path.display())))
}

/// 設計のファイルを写す。元に無ければ写し先から消す（GUI で消えたものは写しでも消える）。
fn mirror(plan: &mut Plan, source: &Path, rel: PathBuf) -> Result<(), BakeError> {
    if source.is_file() {
        plan.insert(rel, Some(read(source)?));
    } else {
        plan.insert(rel, None);
    }
    Ok(())
}

fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).is_ok_and(|mut entries| entries.next().is_none())
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// 計画を書く。初回は一時フォルダに組んでから入れ替える（途中で止まっても写し先に半端な村を
/// 残さない）。再 `bake` は 1 ファイルずつ一時ファイル + rename。
fn apply(plan: &Plan, out: &Path, update: bool, report: &mut BakeReport) -> Result<(), BakeError> {
    let io = |what: &str, path: &Path, err: std::io::Error| {
        BakeError::Io(format!("写しを書けませんでした（{what} {}）: {err}", path.display()))
    };
    let root = if update {
        out.to_path_buf()
    } else {
        let name = out
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "bake".to_owned());
        let staging = out.with_file_name(format!(".{name}.baking-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        staging
    };
    for (rel, bytes) in plan {
        let path = root.join(rel);
        let shown = rel.to_string_lossy().replace('\\', "/");
        match bytes {
            Some(bytes) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|e| io("mkdir", parent, e))?;
                }
                let tmp = path.with_file_name(format!(
                    ".{}.bake-tmp",
                    path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
                ));
                std::fs::write(&tmp, bytes).map_err(|e| io("write", &tmp, e))?;
                std::fs::rename(&tmp, &path).map_err(|e| io("rename", &path, e))?;
                if rel == Path::new(APPROVALS_FILE) {
                    restrict_permissions(&path);
                }
                report.written.push(shown);
            }
            None => {
                if path.is_file() {
                    std::fs::remove_file(&path).map_err(|e| io("remove", &path, e))?;
                    report.removed.push(shown);
                }
            }
        }
    }
    if !update {
        if out.exists() {
            // 空のフォルダだけがここへ来る（空でなければ計画の段で 11）。
            std::fs::remove_dir(out).map_err(|e| io("rmdir", out, e))?;
        }
        std::fs::rename(&root, out).map_err(|e| io("rename", out, e))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maps(pairs: &[(&str, &str)]) -> Vec<PathMap> {
        pairs
            .iter()
            .map(|(from, to)| PathMap { from: (*from).to_owned(), to: (*to).to_owned() })
            .collect()
    }

    /// 最長前方一致・成分の境界・Windows 形は大文字小文字と区切りを無視する。
    #[test]
    fn paths_map_by_the_longest_prefix_on_component_boundaries() {
        let m = maps(&[
            ("D:\\Github", "/work"),
            ("D:\\Github\\Outcasts-MathLab", "/work/mathlab"),
            ("D:\\ManualeRAG", "/work/manuale-rag/"),
        ]);
        assert_eq!(map_path("D:\\Github\\Outcasts-MathLab", &m).as_deref(), Some("/work/mathlab"));
        assert_eq!(
            map_path("d:\\github\\outcasts-mathlab\\sub\\x", &m).as_deref(),
            Some("/work/mathlab/sub/x")
        );
        assert_eq!(map_path("D:/Github/Other", &m).as_deref(), Some("/work/Other"));
        assert_eq!(map_path("D:\\ManualeRAG", &m).as_deref(), Some("/work/manuale-rag"));
        assert_eq!(map_path("D:\\GithubX", &m), None, "成分の途中では当てない");
        assert_eq!(map_path("E:\\Github", &m), None);
        // Unix 形は大文字小文字を区別し、Windows 形の元には当たらない。
        let unix = maps(&[("/home/me/work", "/work")]);
        assert_eq!(map_path("/home/me/work/a", &unix).as_deref(), Some("/work/a"));
        assert_eq!(map_path("/home/me/Work/a", &unix), None);
        assert_eq!(map_path("/home/me/work", &maps(&[("D:\\x", "/x")])), None);
        // 先が / なら値そのものは / になる。
        assert_eq!(map_path("D:\\root", &maps(&[("D:\\root", "/")])).as_deref(), Some("/"));
    }

    /// `--map` の検査: 空・先が絶対パスでない・同じ元の重複は 2。
    #[test]
    fn maps_are_validated() {
        assert!(validate_maps(&maps(&[("D:\\a", "/a"), ("D:\\b", "/b")])).is_ok());
        for bad in [
            maps(&[("", "/a")]),
            maps(&[("D:\\a", "a")]),
            maps(&[("D:\\a", "/a\\b")]),
            maps(&[("D:\\a", "/a"), ("d:/A/", "/b")]),
        ] {
            assert!(matches!(validate_maps(&bad), Err(BakeError::Usage(_))), "{bad:?}");
        }
    }

    /// 鍵らしいヘッダーの名前（推測の規則）。
    #[test]
    fn secret_like_header_names() {
        for name in [
            "Authorization",
            "COOKIE",
            "X-Api-Key",
            "x-auth-token",
            "Api-Key",
            "X-Service-Token",
            "my-secret",
            "db-password",
        ] {
            assert!(secret_like_header(name), "{name}");
        }
        for name in ["Accept", "Content-Type", "X-Request-Id", "User-Agent"] {
            assert!(!secret_like_header(name), "{name}");
        }
    }

    /// Windows の絶対パスらしい並び。URL のスキームや時刻は当てない。
    #[test]
    fn windows_paths_in_free_text() {
        for text in ["D:\\Github\\x", "see C:/Users/me", "(E:\\a)", "\\\\server\\share"] {
            assert!(has_windows_path(text), "{text}");
        }
        for text in ["https://example.com/a", "12:30", "/work/mathlab", "ab:\\x", "Note: done"] {
            assert!(!has_windows_path(text), "{text}");
        }
    }
}
