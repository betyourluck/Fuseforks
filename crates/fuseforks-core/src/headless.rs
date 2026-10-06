//! ヘッドレス実行の起動前検査（Spec 64 D5）。
//!
//! GUI でしか解けない待ち（計画の確認）と、起動しても 1 通目で必ず失敗する設定
//! （秘密が無い・窓口が無い）を、**LLM も MCP も 1 回も呼ばずに**名指しする。
//! 材料はファイル（`world.json` / `schedules.json` / `mcp.json`）と秘密の有無だけ
//! なので、`fuseforks-cli check` は CI やコンテナのビルド時に安く回せる。
//! `ask` と `serve` は自分の起動の途中で同じ検査を走らせ、拒否が 1 件でもあれば
//! LLM を呼ばずに止まる（`check` を別に打たなくても安全側に倒れる）。
//!
//! **拒否と警告の文は直し方を書く**（`failures.md` #44）。検査を警告だけにしない —
//! 計画の確認の待ちはヘッドレスでは永久に解けないので、警告にすると「動かないが
//! 理由は 1 行だけログにある」を作る（Spec 64「採らなかった形」6）。
//!
//! 表（D5）の 9 行をそのまま写した。行ごとの重さは `Reject` / `Warn` / `Info` の
//! 閉じた 3 値で、結果は重い順に並ぶ。
//!
//! **Spec 65（コンテナ）で 6 行を足し、情報 `MCP_STDIO` を警告 `MCP_COMMAND_NOT_FOUND` へ
//! 置き換えた**（`container_contract` 10）。足した行の材料（パスの有無・PATH・`run.json` の
//! `allow`・`headers` の参照・時刻帯）も `HostView` の関数と値で受け、純関数のまま保つ。
//! パスとコマンドの指摘は**パスやコマンドごとに 1 件**へまとめ、該当する個体を列挙する。

use std::collections::BTreeSet;

use serde::Serialize;

use crate::command::RunApproval;
use crate::mcp::mcp_secret_key;
use crate::model::{AgentId, CredentialSource};
use crate::schedule::{Recurrence, ScheduledTask};
use crate::schedule_probe::ScheduleProbe;
use crate::secret::env_secret_name;
use crate::world::{Language, World};

/// 検査の対象になるコマンド。`check --for` で名指しする。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadlessMode {
    /// 1 通送って答えを出して閉じる（D7）。窓口が要る。
    Ask,
    /// 常駐して予定と扉を回す（D8）。予定の宛先と前判定の承認が要る。
    Serve,
}

/// 指摘の重さ。重い順に並ぶ（`Ord`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingLevel {
    /// 起動しない。`ask` / `serve` は LLM を 1 回も呼ばずに止まる。
    Reject,
    /// 起動はするが、設定どおりには動かない部分がある。
    Warn,
    /// 検査からは分からないこと・利用者が選んだ形の確認。
    Info,
}

/// 指摘 1 件。`--json` ではこの形のまま出る。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Finding {
    /// 重さ。
    pub level: FindingLevel,
    /// 閉じた識別子（[`codes`]）。機械はこれで読む。
    pub code: &'static str,
    /// 何が起きるか（村の言語）。
    pub message: String,
    /// 直し方（村の言語）。
    pub fix: String,
}

/// 指摘の識別子。**閉じた集合** — 足すときはここと D5 の表の両方へ。
pub mod codes {
    /// 計画の確認 ON の個体が起動する集合に居る（拒否）。
    pub const PLAN_REVIEW_WAITS: &str = "PLAN_REVIEW_WAITS";
    /// 起動する集合のテンプレートに、選んだストアの秘密が無い（拒否）。
    pub const SECRET_MISSING: &str = "SECRET_MISSING";
    /// 起動する個体のテンプレートが村に無い（拒否）。
    pub const MODEL_TEMPLATE_MISSING: &str = "MODEL_TEMPLATE_MISSING";
    /// 窓口が未設定（拒否。`ask`）。
    pub const RECEPTION_UNSET: &str = "RECEPTION_UNSET";
    /// 窓口が削除済み（拒否。`ask`）。
    pub const RECEPTION_MISSING: &str = "RECEPTION_MISSING";
    /// 窓口の接続先が起動する集合の外（情報。`ask`）。
    pub const RECEPTION_TARGETS_OUTSIDE: &str = "RECEPTION_TARGETS_OUTSIDE";
    /// 予定の宛先が起動する集合の外（警告。`serve`）。
    pub const SCHEDULE_TARGET_OUTSIDE: &str = "SCHEDULE_TARGET_OUTSIDE";
    /// 承認モードが「承認が必要」で `run` を持つ個体が居る（警告）。
    pub const RUN_APPROVAL_REQUIRED: &str = "RUN_APPROVAL_REQUIRED";
    /// 前判定・後判定がこの棚で未承認（警告。`serve`）。
    pub const PROBE_UNAPPROVED: &str = "PROBE_UNAPPROVED";
    /// 判断役か圧縮があるのに Jev の鍵が無い（警告）。
    pub const JEV_TOKEN_MISSING: &str = "JEV_TOKEN_MISSING";
    /// 有効な stdio の MCP サーバーの `command` が見つからない（警告。Spec 65 —
    /// Spec 64 の情報 `MCP_STDIO` を置き換えた）。
    pub const MCP_COMMAND_NOT_FOUND: &str = "MCP_COMMAND_NOT_FOUND";
    /// `serve --door-port` があるのに扉の合鍵が無い（拒否。Spec 65）。
    pub const DOOR_TOKEN_MISSING: &str = "DOOR_TOKEN_MISSING";
    /// 起動する個体の作業フォルダが存在しない・フォルダでない（拒否。Spec 65）。
    pub const WORK_DIR_MISSING: &str = "WORK_DIR_MISSING";
    /// 起動する個体の `ragSources` の 1 つが存在しない（警告。Spec 65）。
    pub const RAG_SOURCE_MISSING: &str = "RAG_SOURCE_MISSING";
    /// `run.json` の `allow` の先頭の語が PATH に無い（警告。Spec 65）。
    pub const RUN_COMMAND_NOT_FOUND: &str = "RUN_COMMAND_NOT_FOUND";
    /// 壁時計の予定があり、時刻帯が `bake.json` と違う・読めない（警告。Spec 65）。
    pub const TIMEZONE_MISMATCH: &str = "TIMEZONE_MISMATCH";
}

/// パスの種類（[`HostView::path_kind`] が返す）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind {
    /// フォルダ。
    Dir,
    /// ファイル（フォルダを期待した場所にあれば「フォルダでない」）。
    File,
    /// 存在しない（読めない場合も含む）。
    Missing,
}

/// 有効な stdio の MCP サーバー 1 台（共通は名前、個体別は `id:名前`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioCommand {
    /// 表示用のサーバー名。
    pub server: String,
    /// `mcp.json` の `command` そのまま。
    pub command: String,
}

/// 有効な http の MCP サーバーの `headers` が参照する秘密 1 つ（Spec 65 D3）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    /// 表示用のサーバー名。
    pub server: String,
    /// `${secret:NAME}` の NAME。
    pub name: String,
}

/// 起動する個体の `run.json` の `allow` の先頭の語 1 つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunCommand {
    /// 持ち主。
    pub agent: AgentId,
    /// 先頭の語（コマンド名）。
    pub command: String,
}

/// プロセスの時刻帯（Spec 65 契約 11）。**`TZ` を先に見る** — Linux の `iana-time-zone` は
/// `TZ` を見ずに `/etc/localtime` を読む（P0 実測: `TZ=Asia/Tokyo` でも `Etc/UTC`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessTimeZone<'a> {
    /// 名前が分かった（`TZ` の値で時刻帯のファイルがあるか、`TZ` が無く OS から読めた）。
    Named(&'a str),
    /// `TZ` は設定されているが時刻帯のファイルが無い — `chrono` は**黙って UTC で動く**。
    Unreadable(&'a str),
    /// `TZ` が無く、OS からも読めなかった。
    Unknown,
}

/// 検査の材料のうち、`World` と予定の外にあるもの。
///
/// 秘密と承認は**有無だけ**を閉じた関数で渡す（値は検査に要らない）。
pub struct HostView<'a> {
    /// `ask` か `serve` か。
    pub mode: HeadlessMode,
    /// 起動する集合（D6 で解決済み。`ask` では窓口を含む）。
    pub start: &'a BTreeSet<AgentId>,
    /// 選んだストアにその鍵（テンプレート ID）があるか。値は見ない。
    pub secret_present: &'a dyn Fn(&str) -> bool,
    /// この棚（`probe_approvals.json`）の承認で通るか。
    pub probe_approved: &'a dyn Fn(&ScheduleProbe) -> bool,
    /// Jev の API トークンが選んだストアにあるか。
    pub jev_token_present: bool,
    /// ツール結果の圧縮（Spec 59）が `jev.json` で ON か。
    pub jev_pruning_enabled: bool,
    /// `--bypass-plan-review`（Spec 53 のスイッチを立てる）。
    pub bypass_plan_review: bool,
    /// コマンドの承認モード（Spec 61）。
    pub run_approval: RunApproval,
    /// 有効な stdio の MCP サーバー（共通と、起動する個体の個体別。呼び出し側が集める）。
    pub stdio_mcp_commands: &'a [StdioCommand],
    /// 有効な http の MCP サーバーの `headers` が参照する秘密（同上）。
    pub mcp_secret_refs: &'a [SecretRef],
    /// 起動する個体の `run.json` の `allow` の先頭の語（`run` を持たない個体の分も
    /// 渡してよい — 数えるのは `run` を持つ個体だけ）。
    pub run_commands: &'a [RunCommand],
    /// パスの種類。値（パス）だけを見る。
    pub path_kind: &'a dyn Fn(&str) -> PathKind,
    /// `run` と同じ規則で PATH から見つかるか（絶対パスならその存在）。
    pub command_on_path: &'a dyn Fn(&str) -> bool,
    /// `serve --door-port`（Spec 65 D9）。`None` なら扉の合鍵は見ない。
    pub door_port: Option<u16>,
    /// 扉の合鍵（秘密 `door_token`）が選んだストアにあるか。
    pub door_token_present: bool,
    /// プロセスの時刻帯。
    pub process_time_zone: ProcessTimeZone<'a>,
    /// `{data_dir}/bake.json` の `sourceTimeZone`。`bake.json` が無ければ `None`。
    pub baked_time_zone: Option<&'a str>,
}

/// Jev のトークンの鍵。置き場はホスト層（`jev_settings.rs` の `TOKEN_KEY`）で、
/// ここは直し方の文に変数名を書くためだけに同じ綴りを持つ。
const JEV_TOKEN_KEY: &str = "jev_api_token";

/// 扉の合鍵の鍵（Spec 65 D9）。置き場はホスト層で、ここは直し方の文に変数名を書くため。
pub const DOOR_TOKEN_KEY: &str = "door_token";

/// 値を最初に現れた順のまま、鍵ごとに束ねる（指摘をパスやコマンドごとに 1 件にする）。
fn group_in_order<K: PartialEq, V>(
    items: impl IntoIterator<Item = (K, V)>,
) -> Vec<(K, Vec<V>)> {
    let mut out: Vec<(K, Vec<V>)> = Vec::new();
    for (key, value) in items {
        match out.iter_mut().find(|(k, _)| *k == key) {
            Some((_, values)) => values.push(value),
            None => out.push((key, vec![value])),
        }
    }
    out
}

/// 起動前検査の本体。純関数 — ファイルも環境も読まず、LLM も MCP も呼ばない。
///
/// 返り値は重い順（拒否 → 警告 → 情報）。同じ重さの中は検査の順（D5 の表の順）。
pub fn headless_preflight(
    world: &World,
    schedules: &[ScheduledTask],
    view: &HostView<'_>,
) -> Vec<Finding> {
    let lang = world.language().unwrap_or(Language::Ja);
    let mut out = Vec::new();

    // 1. 計画の確認（planReview）が ON の個体が起動する集合に居る — 両方・拒否。
    //    波が人の承認を永久に待つ（Spec 43）。通すときは Spec 53 のスイッチを立てるだけ。
    if !view.bypass_plan_review {
        for id in view.start {
            let Ok(record) = world.agent(id) else {
                continue;
            };
            if record.spec.plan_review {
                out.push(Finding {
                    level: FindingLevel::Reject,
                    code: codes::PLAN_REVIEW_WAITS,
                    message: lang
                        .pick(
                            &format!(
                                "{}（{}）は「計画の確認」が ON です。ヘッドレスでは波が人の承認を永久に待ちます",
                                record.spec.name, id
                            ),
                            &format!(
                                "{} ({}) has plan review on; headless runs would wait for a human approval forever",
                                record.spec.name, id
                            ),
                        )
                        .to_owned(),
                    fix: lang
                        .pick(
                            "--bypass-plan-review を付ける（Spec 53 のスイッチを立てるのと同じ）か、この個体の設定で計画の確認を OFF にする",
                            "Pass --bypass-plan-review (the same as the Spec 53 switch) or turn plan review off for this servant",
                        )
                        .to_owned(),
                });
            }
        }
    }

    // 2. 起動する集合のテンプレートに、選んだストアの秘密が無い — 両方・拒否。
    //    1 通目で 401 になり echo_on_failure が偽の応答を返す — ヘッドレスでは誰も画面を見ていない。
    let mut seen_templates = BTreeSet::new();
    for id in view.start {
        let Ok(record) = world.agent(id) else {
            continue;
        };
        let template_id = record.spec.model_template_id.clone();
        if !seen_templates.insert(template_id.clone()) {
            continue;
        }
        let Ok(template) = world.template(&template_id) else {
            out.push(Finding {
                level: FindingLevel::Reject,
                code: codes::MODEL_TEMPLATE_MISSING,
                message: lang
                    .pick(
                        &format!("{}（{}）のモデルテンプレート {} が村にありません", record.spec.name, id, template_id),
                        &format!("Model template {} of {} ({}) is not in the village", template_id, record.spec.name, id),
                    )
                    .to_owned(),
                fix: lang
                    .pick(
                        "この個体の設定で存在するテンプレートを選ぶ",
                        "Pick an existing template in this servant's settings",
                    )
                    .to_owned(),
            });
            continue;
        };
        let missing = match template.credential {
            CredentialSource::NotRequired => false,
            CredentialSource::Unset => true,
            CredentialSource::Keyring => !(view.secret_present)(template_id.as_str()),
        };
        if missing {
            let variable = env_secret_name(template_id.as_str());
            out.push(Finding {
                level: FindingLevel::Reject,
                code: codes::SECRET_MISSING,
                message: lang
                    .pick(
                        &format!(
                            "テンプレート {}（{}）のキーが選んだストアにありません。1 通目で 401 になり、偽の応答が返ります",
                            template_id, template.model
                        ),
                        &format!(
                            "Template {} ({}) has no key in the chosen secret store; the first request would fail with 401 and a fake reply",
                            template_id, template.model
                        ),
                    )
                    .to_owned(),
                fix: lang
                    .pick(
                        &format!("keyring なら GUI の「モデルテンプレートを管理」でキーを登録する。env なら {variable} を設定する"),
                        &format!("With keyring, register the key in the GUI (Manage model templates). With env, set {variable}"),
                    )
                    .to_owned(),
            });
        }
    }

    // 3. 窓口（reception）が未設定・削除済み — ask・拒否。
    //    ask_external が即座に断る。起動して MCP を繋いでから断るより前で止める。
    if view.mode == HeadlessMode::Ask {
        match world.reception() {
            None => out.push(Finding {
                level: FindingLevel::Reject,
                code: codes::RECEPTION_UNSET,
                message: lang
                    .pick(
                        "窓口が設定されていません。ask は窓口へ送る以外の動作を持ちません",
                        "No reception is set; ask does nothing but deliver to the reception",
                    )
                    .to_owned(),
                fix: lang
                    .pick(
                        "GUI のシステム設定 ＞ 外部連携 ＞ MCP サーバーで窓口を選ぶ（world.json の reception）",
                        "Pick a reception in the GUI: System settings > External > MCP server (reception in world.json)",
                    )
                    .to_owned(),
            }),
            Some(reception) if world.agent(reception).is_err() => out.push(Finding {
                level: FindingLevel::Reject,
                code: codes::RECEPTION_MISSING,
                message: lang
                    .pick(
                        &format!("窓口 {reception} は削除されています"),
                        &format!("The reception {reception} has been deleted"),
                    )
                    .to_owned(),
                fix: lang
                    .pick(
                        "窓口を選び直す（削除しても reception は掃除しない — 未設定と区別するため）",
                        "Pick the reception again (deleting a servant leaves reception as is, to keep it distinct from unset)",
                    )
                    .to_owned(),
            }),
            Some(reception) => {
                // 4. 窓口の接続先が起動する集合の外 — ask・情報。
                //    委譲は NOT_RUNNING で返り、窓口が自分で答える。--start reception は
                //    利用者が選んだ形なので毎回の警告にはしない。
                //    **判断役は数えない**（Spec 64 P6 の実機で混ざっていた）— 判断役は起動する
                //    個体ではなく、呼び出し元のツール呼び出しの中で動くので「集合の外」に居ても
                //    NOT_RUNNING にならない。判断役の行き先（judge.toml の to）はこの検査の材料に
                //    入っていない（ファイルを読まない純関数のまま）。
                let outside: Vec<String> = world
                    .connections_of(reception)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|target| world.judge(target).is_none())
                    .filter(|target| !view.start.contains(target))
                    .map(|target| target.to_string())
                    .collect();
                if !outside.is_empty() {
                    let listed = outside.join(", ");
                    out.push(Finding {
                        level: FindingLevel::Info,
                        code: codes::RECEPTION_TARGETS_OUTSIDE,
                        message: lang
                            .pick(
                                &format!("窓口の接続先 {listed} は起動する集合の外です。委譲は NOT_RUNNING で返り、窓口が自分で答えます"),
                                &format!("Reception targets {listed} are outside the start set; delegations return NOT_RUNNING and the reception answers by itself"),
                            )
                            .to_owned(),
                        fix: lang
                            .pick(
                                "委譲させたいなら --start にその個体を足す（batch か id の列挙）",
                                "To allow delegation, add those servants to --start (batch or an id list)",
                            )
                            .to_owned(),
                    });
                }
            }
        }
    }

    // 5. 予定の宛先が起動する集合の外 — serve・警告。
    //    発火しても配送されない（GUI と同じ「停止中なのでスキップ」）。
    if view.mode == HeadlessMode::Serve {
        for task in schedules.iter().filter(|t| t.enabled) {
            if !view.start.contains(&task.to) {
                out.push(Finding {
                    level: FindingLevel::Warn,
                    code: codes::SCHEDULE_TARGET_OUTSIDE,
                    message: lang
                        .pick(
                            &format!("予定 {}（宛先 {}）は宛先が起動する集合の外なので、発火しても配送されません", task.id, task.to),
                            &format!("Schedule {} (to {}) targets a servant outside the start set; it fires but is never delivered", task.id, task.to),
                        )
                        .to_owned(),
                    fix: lang
                        .pick(
                            "--start に宛先を足すか、予定を一時停止する",
                            "Add the target to --start, or pause the schedule",
                        )
                        .to_owned(),
                });
            }
        }
    }

    // 6. 承認モードが「承認が必要」で run を持つ個体が居る — 両方・警告。
    //    待ちにはならない（未承認は拒否文が返ってターンは進む）が、pending は誰も承認しない。
    if view.run_approval == RunApproval::Required {
        let with_run: Vec<String> = view
            .start
            .iter()
            .filter(|id| {
                world.agent(id).is_ok_and(|record| {
                    record
                        .spec
                        .enabled_tools
                        .as_deref()
                        .is_some_and(|tools| tools.iter().any(|t| t == "run"))
                })
            })
            .map(|id| id.to_string())
            .collect();
        if !with_run.is_empty() {
            let listed = with_run.join(", ");
            out.push(Finding {
                level: FindingLevel::Warn,
                code: codes::RUN_APPROVAL_REQUIRED,
                message: lang
                    .pick(
                        &format!("コマンドの承認モードが「承認が必要」で、run を持つ個体（{listed}）が居ます。未承認のコマンドは拒否され、誰も承認しません"),
                        &format!("Command approval is \"required\" and servants with run ({listed}) are starting; unapproved commands are refused and nobody approves them"),
                    )
                    .to_owned(),
                fix: lang
                    .pick(
                        "--run-approval auto-approve か no-approval を選ぶか、GUI で run.json の allow を育ててから持っていく",
                        "Choose --run-approval auto-approve or no-approval, or grow run.json's allow list in the GUI first",
                    )
                    .to_owned(),
            });
        }
    }

    // 7. 前判定・後判定のコマンドがこの棚で未承認 — serve・警告。
    //    発火しても配送しない（unapproved）。承認は棚の probe_approvals.json を GUI で作ってから一緒に持っていく。
    if view.mode == HeadlessMode::Serve {
        for task in schedules.iter().filter(|t| t.enabled) {
            let probes: [(Option<&ScheduleProbe>, &str, &str); 2] = [
                (task.probe.as_ref(), "前判定", "pre-check"),
                (task.acceptance.as_ref().map(|a| &a.probe), "後判定", "acceptance"),
            ];
            for (probe, ja, en) in probes {
                let Some(probe) = probe else {
                    continue;
                };
                if (view.probe_approved)(probe) {
                    continue;
                }
                out.push(Finding {
                    level: FindingLevel::Warn,
                    code: codes::PROBE_UNAPPROVED,
                    message: lang
                        .pick(
                            &format!("予定 {} の{}（{}）はこの棚で未承認です。発火しても配送されません（unapproved）", task.id, ja, probe.command),
                            &format!("The {} of schedule {} ({}) is not approved on this shelf; it fires but is never delivered (unapproved)", en, task.id, probe.command),
                        )
                        .to_owned(),
                    fix: lang
                        .pick(
                            "GUI の予定の画面で承認し、棚の probe_approvals.json を一緒に持っていく",
                            "Approve it in the GUI schedule dialog and bring the shelf's probe_approvals.json along",
                        )
                        .to_owned(),
                });
            }
        }
    }

    // 8. 判断役があるのに Jev の鍵が無い / ツール結果の圧縮が ON で鍵が無い — 両方・警告。
    //    判断役は無効で起動し、圧縮は走らない（Spec 59 / 62 の述語どおり）。
    if !view.jev_token_present {
        let variable = env_secret_name(JEV_TOKEN_KEY);
        let fix = lang
            .pick(
                &format!("Jev の API トークンを選んだストアに入れる（keyring なら GUI のシステム設定 ＞ 外部連携、env なら {variable}）"),
                &format!("Put the Jev API token in the chosen secret store (keyring: GUI System settings > External; env: {variable})"),
            )
            .to_owned();
        if world.judges().iter().any(|judge| judge.enabled) {
            out.push(Finding {
                level: FindingLevel::Warn,
                code: codes::JEV_TOKEN_MISSING,
                message: lang
                    .pick(
                        "判断役がありますが Jev の鍵がありません。判断役は無効のまま起動します",
                        "There are judges but no Jev token; judges start disabled",
                    )
                    .to_owned(),
                fix: fix.clone(),
            });
        }
        if view.jev_pruning_enabled {
            out.push(Finding {
                level: FindingLevel::Warn,
                code: codes::JEV_TOKEN_MISSING,
                message: lang
                    .pick(
                        "ツール結果の圧縮が ON ですが Jev の鍵がありません。圧縮は走りません",
                        "Tool-result pruning is on but there is no Jev token; pruning does not run",
                    )
                    .to_owned(),
                fix,
            });
        }
    }

    // 9. 有効な stdio の MCP サーバーの command が見つからない — 両方・警告（Spec 65。
    //    Spec 64 の情報 MCP_STDIO を置き換えた）。そのサーバーは繋がらず、個体はそのツール
    //    無しで動く。コマンドごとに 1 件。
    let missing_commands = group_in_order(
        view.stdio_mcp_commands
            .iter()
            .filter(|s| !(view.command_on_path)(&s.command))
            .map(|s| (s.command.as_str(), s.server.as_str())),
    );
    for (command, servers) in missing_commands {
        let listed = servers.join(", ");
        out.push(Finding {
            level: FindingLevel::Warn,
            code: codes::MCP_COMMAND_NOT_FOUND,
            message: lang
                .pick(
                    &format!("MCP サーバー {listed} の起動コマンド {command} が見つかりません。そのサーバーは繋がらず、個体はそのツール無しで動きます"),
                    &format!("The command {command} of MCP server {listed} is not found; the server will not connect and the servants run without its tools"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    "リモート MCP（type: http）へ寄せるか、そのコマンドを入れた像を作る（FROM で派生させる）。使わないなら mcp.json で enabled: false",
                    "Move to a remote MCP server (type: http) or build an image that contains the command (derive with FROM). If unused, set enabled: false in mcp.json",
                )
                .to_owned(),
        });
    }

    // 10. headers が参照する秘密が選んだストアに無い — 両方・拒否（Spec 65 D3）。
    //     そのサーバーは接続しない（プレースホルダを送らない）。名前ごとに 1 件。
    let missing_refs = group_in_order(
        view.mcp_secret_refs
            .iter()
            .filter(|r| !(view.secret_present)(&mcp_secret_key(&r.name)))
            .map(|r| (r.name.as_str(), r.server.as_str())),
    );
    for (name, servers) in missing_refs {
        let listed = servers.join(", ");
        let key = mcp_secret_key(name);
        let variable = env_secret_name(&key);
        out.push(Finding {
            level: FindingLevel::Reject,
            code: codes::SECRET_MISSING,
            message: lang
                .pick(
                    &format!("MCP の headers が参照する秘密 {name}（{listed}）が選んだストアにありません。そのサーバーには接続しません"),
                    &format!("The secret {name} referenced by MCP headers ({listed}) is not in the chosen secret store; the server will not connect"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    &format!("keyring なら GUI の MCP の設定で値を保存する（鍵 {key}）。env なら {variable} を設定する"),
                    &format!("With keyring, save the value in the GUI MCP settings (key {key}). With env, set {variable}"),
                )
                .to_owned(),
        });
    }

    // 11. serve --door-port があるのに扉の合鍵が無い — serve・拒否（Spec 65 D9）。
    //     開くと言ったのに開けない、を作らない。
    if view.mode == HeadlessMode::Serve
        && let Some(port) = view.door_port
        && !view.door_token_present
    {
        let variable = env_secret_name(DOOR_TOKEN_KEY);
        out.push(Finding {
            level: FindingLevel::Reject,
            code: codes::DOOR_TOKEN_MISSING,
            message: lang
                .pick(
                    &format!("--door-port {port} で扉を開く指定ですが、扉の合鍵（{DOOR_TOKEN_KEY}）が選んだストアにありません"),
                    &format!("--door-port {port} asks to open the door, but the door token ({DOOR_TOKEN_KEY}) is not in the chosen secret store"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    &format!("env なら {variable} を設定する。扉を開かないなら --door-port を外す"),
                    &format!("With env, set {variable}. To keep the door closed, drop --door-port"),
                )
                .to_owned(),
        });
    }

    // 12. 起動する個体の作業フォルダが存在しない・フォルダでない — 両方・拒否（Spec 65）。
    //     ファイル系のツールが全部「作業フォルダが存在しません」を返す。ヘッドレスでは誰も
    //     直さない。bake.json の有無に関わらない。パスごとに 1 件。
    let mut work_dirs = Vec::new();
    let mut rag_sources = Vec::new();
    for id in view.start {
        let Ok(record) = world.agent(id) else {
            continue;
        };
        if let Some(dir) = record.spec.work_dir.as_deref()
            && (view.path_kind)(dir) != PathKind::Dir
        {
            work_dirs.push((dir.to_owned(), id.to_string()));
        }
        for source in &record.spec.rag_sources {
            if (view.path_kind)(source) != PathKind::Dir {
                rag_sources.push((source.clone(), id.to_string()));
            }
        }
    }
    for (dir, agents) in group_in_order(work_dirs) {
        let listed = agents.join(", ");
        out.push(Finding {
            level: FindingLevel::Reject,
            code: codes::WORK_DIR_MISSING,
            message: lang
                .pick(
                    &format!("作業フォルダ {dir}（{listed}）がありません。ファイル系のツールが全部失敗します"),
                    &format!("The work folder {dir} ({listed}) does not exist; every file tool would fail"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    "そのパスにフォルダを用意する（コンテナなら /work の下へクローンしてマウントする）か、bake の --map で置き換え先を直す",
                    "Provide the folder at that path (in a container, clone under /work and mount it), or fix the target of bake's --map",
                )
                .to_owned(),
        });
    }

    // 13. ragSources の 1 つが存在しない — 両方・警告（Spec 65）。rag はその宣言を飛ばして動く。
    for (source, agents) in group_in_order(rag_sources) {
        let listed = agents.join(", ");
        out.push(Finding {
            level: FindingLevel::Warn,
            code: codes::RAG_SOURCE_MISSING,
            message: lang
                .pick(
                    &format!("rag の宣言フォルダ {source}（{listed}）がありません。rag はその宣言を飛ばして動きます"),
                    &format!("The rag source {source} ({listed}) does not exist; rag skips that declaration"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    "そのパスにフォルダを用意するか、bake の --map で置き換え先を直す",
                    "Provide the folder at that path, or fix the target of bake's --map",
                )
                .to_owned(),
        });
    }

    // 14. run.json の allow の先頭の語が PATH に無い — 両方・警告（Spec 65）。
    //     許可しても実行時に「見つからない」。数えるのは run を持つ起動する個体だけ。
    //     **PATH にあるかしか見ない** — 同じ名前の別のプログラムは見分けない（P0: Debian の sg）。
    let with_run = |agent: &AgentId| {
        view.start.contains(agent)
            && world.agent(agent).is_ok_and(|record| {
                record
                    .spec
                    .enabled_tools
                    .as_deref()
                    .is_some_and(|tools| tools.iter().any(|t| t == "run"))
            })
    };
    let missing_run = group_in_order(
        view.run_commands
            .iter()
            .filter(|r| with_run(&r.agent) && !(view.command_on_path)(&r.command))
            .map(|r| (r.command.as_str(), r.agent.to_string())),
    );
    for (command, agents) in missing_run {
        let listed = agents.join(", ");
        out.push(Finding {
            level: FindingLevel::Warn,
            code: codes::RUN_COMMAND_NOT_FOUND,
            message: lang
                .pick(
                    &format!("run の許可コマンド {command}（{listed}）が PATH にありません。許可しても実行時に見つかりません"),
                    &format!("The allowed run command {command} ({listed}) is not on PATH; it will not be found at run time"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    "そのコマンドを入れた像を作る（FROM で派生させる）か、run.json の allow から外す",
                    "Build an image that contains the command (derive with FROM), or remove it from run.json's allow",
                )
                .to_owned(),
        });
    }

    // 15. 壁時計の予定があり、時刻帯が bake.json と違う・読めない — serve・警告（Spec 65 契約 11）。
    //     予定が GUI で決めた時刻と違う時刻に発火する。bake.json が無い村では比べる相手が無いので出さない。
    let wall_clock = schedules
        .iter()
        .any(|t| t.enabled && !matches!(t.recurrence, Recurrence::Interval { .. }));
    if view.mode == HeadlessMode::Serve
        && wall_clock
        && let Some(baked) = view.baked_time_zone
    {
        let message = match view.process_time_zone {
            ProcessTimeZone::Named(name) if name == baked => None,
            ProcessTimeZone::Named(name) => Some(lang.pick(
                &format!("このプロセスの時刻帯は {name} で、村を作った端末（{baked}）と違います。毎日・毎週の予定が GUI で決めた時刻と違う時刻に発火します"),
                &format!("This process runs in {name}, not in the village's time zone ({baked}); daily and weekly schedules fire at different times than set in the GUI"),
            ).to_owned()),
            ProcessTimeZone::Unreadable(tz) => Some(lang.pick(
                &format!("TZ={tz} の時刻帯が読めません。時刻は黙って UTC で動き、毎日・毎週の予定が村の時刻帯（{baked}）と違う時刻に発火します"),
                &format!("TZ={tz} cannot be read; time silently runs in UTC and daily and weekly schedules fire at different times than in the village's time zone ({baked})"),
            ).to_owned()),
            ProcessTimeZone::Unknown => Some(lang.pick(
                &format!("このプロセスの時刻帯が読めません。毎日・毎週の予定が村の時刻帯（{baked}）どおりに発火するか分かりません"),
                &format!("This process's time zone cannot be read; daily and weekly schedules may not fire in the village's time zone ({baked})"),
            ).to_owned()),
        };
        if let Some(message) = message {
            out.push(Finding {
                level: FindingLevel::Warn,
                code: codes::TIMEZONE_MISMATCH,
                message,
                fix: lang
                    .pick(
                        &format!("TZ={baked} を設定する（像には tzdata が入っている）"),
                        &format!("Set TZ={baked} (the image includes tzdata)"),
                    )
                    .to_owned(),
            });
        }
    }

    // 重い順に並べる。同じ重さの中は検査の順のまま（stable）。
    out.sort_by_key(|f| f.level);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AgentSpec, JudgeSpec, ModelTemplate, UnknownFields};
    use crate::schedule::{Acceptance, Recurrence, ScheduledTask, Weekday};
    use crate::schedule_probe::SessionMode;

    fn template(id: &str, credential: CredentialSource) -> ModelTemplate {
        let mut t = ModelTemplate::new(id, id, "model-x");
        t.credential = credential;
        t
    }

    /// テンプレート `tpl`（keyring）と個体 `a` / `b`、`a` → `b` の線を持つ村。
    fn village() -> World {
        let mut world = World::new();
        world.set_language(Language::Ja);
        world.upsert_template(template("tpl", CredentialSource::Keyring));
        world
            .register_agent(AgentSpec::new("a", "エー", "tpl"))
            .unwrap();
        world
            .register_agent(AgentSpec::new("b", "ビー", "tpl"))
            .unwrap();
        world
            .set_connections(&AgentId::from("a"), vec![AgentId::from("b")])
            .unwrap();
        world
    }

    fn ids(list: &[&str]) -> BTreeSet<AgentId> {
        list.iter().map(|s| AgentId::from(*s)).collect()
    }

    fn task(id: &str, to: &str) -> ScheduledTask {
        ScheduledTask {
            id: id.to_owned(),
            to: AgentId::from(to),
            message: "見張って".to_owned(),
            recurrence: Recurrence::Weekly {
                weekday: Weekday::Thu,
                hour: 17,
                minute: 0,
            },
            created_at_ms: 0,
            last_consumed_due_ms: None,
            enabled: true,
            probe: None,
            session_mode: SessionMode::Continue,
            summarize_after: false,
            acceptance: None,
            auto_approve_plans: false,
        }
    }

    fn probe(command: &str) -> ScheduleProbe {
        ScheduleProbe {
            command: command.to_owned(),
            args: Vec::new(),
            expect: "ok".to_owned(),
            timeout_secs: 10,
            cwd: None,
        }
    }

    /// 全部そろった view（秘密あり・承認あり・鍵あり・承認モードは自動承認・パスは全部
    /// フォルダ・コマンドは全部 PATH にある・扉は開かない・`bake.json` は無い）。
    struct Base {
        start: BTreeSet<AgentId>,
        stdio: Vec<StdioCommand>,
    }

    impl Base {
        fn view<'a>(
            &'a self,
            mode: HeadlessMode,
            secret_present: &'a dyn Fn(&str) -> bool,
            probe_approved: &'a dyn Fn(&ScheduleProbe) -> bool,
        ) -> HostView<'a> {
            HostView {
                mode,
                start: &self.start,
                secret_present,
                probe_approved,
                jev_token_present: true,
                jev_pruning_enabled: false,
                bypass_plan_review: false,
                run_approval: RunApproval::AutoApprove,
                stdio_mcp_commands: &self.stdio,
                mcp_secret_refs: &[],
                run_commands: &[],
                path_kind: &all_dirs,
                command_on_path: &everywhere,
                door_port: None,
                door_token_present: false,
                process_time_zone: ProcessTimeZone::Named("Asia/Tokyo"),
                baked_time_zone: None,
            }
        }
    }

    fn all_dirs(_: &str) -> PathKind {
        PathKind::Dir
    }
    fn everywhere(_: &str) -> bool {
        true
    }
    fn nowhere(_: &str) -> bool {
        false
    }
    fn nothing_exists(_: &str) -> PathKind {
        PathKind::Missing
    }

    fn stdio(server: &str, command: &str) -> StdioCommand {
        StdioCommand {
            server: server.to_owned(),
            command: command.to_owned(),
        }
    }

    fn yes_secret(_: &str) -> bool {
        true
    }
    fn no_secret(_: &str) -> bool {
        false
    }
    fn yes_probe(_: &ScheduleProbe) -> bool {
        true
    }
    fn no_probe(_: &ScheduleProbe) -> bool {
        false
    }

    fn codes_of(findings: &[Finding]) -> Vec<&'static str> {
        findings.iter().map(|f| f.code).collect()
    }

    /// 全部そろっていれば Ask も Serve も指摘ゼロ。
    #[test]
    fn a_complete_village_passes_in_both_modes() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        for mode in [HeadlessMode::Ask, HeadlessMode::Serve] {
            let view = base.view(mode, &yes_secret, &yes_probe);
            assert!(
                headless_preflight(&world, &[task("t1", "b")], &view).is_empty(),
                "{mode:?}"
            );
        }
    }

    /// 1. 計画の確認 ON は両方で拒否。--bypass-plan-review で消える。集合の外の個体は数えない。
    #[test]
    fn plan_review_on_a_starting_servant_is_rejected_unless_bypassed() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let mut spec = world.agent(&AgentId::from("b")).unwrap().spec.clone();
        spec.plan_review = true;
        world.update_agent(spec).unwrap();

        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        for mode in [HeadlessMode::Ask, HeadlessMode::Serve] {
            let view = base.view(mode, &yes_secret, &yes_probe);
            let findings = headless_preflight(&world, &[], &view);
            assert_eq!(codes_of(&findings), vec![codes::PLAN_REVIEW_WAITS], "{mode:?}");
            assert_eq!(findings[0].level, FindingLevel::Reject);
            assert!(findings[0].message.contains("ビー（b）"), "{}", findings[0].message);
            assert!(findings[0].fix.contains("--bypass-plan-review"));

            let mut bypass = base.view(mode, &yes_secret, &yes_probe);
            bypass.bypass_plan_review = true;
            assert!(headless_preflight(&world, &[], &bypass).is_empty(), "{mode:?}");
        }

        // b を起動しなければ b の設定は関係ない。
        let only_a = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let view = only_a.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        assert!(headless_preflight(&world, &[], &view).is_empty());
    }

    /// 2. 秘密が無いテンプレートは拒否。Unset も拒否。NotRequired は見ない。同じテンプレートは 1 件。
    #[test]
    fn a_missing_secret_is_rejected_once_per_template() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };

        let view = base.view(HeadlessMode::Serve, &no_secret, &yes_probe);
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::SECRET_MISSING], "a と b は同じ tpl なので 1 件");
        assert!(findings[0].fix.contains("FUSEFORKS_SECRET_TPL"), "{}", findings[0].fix);

        world.upsert_template(template("tpl", CredentialSource::Unset));
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::SECRET_MISSING], "Unset も無い側");

        world.upsert_template(template("tpl", CredentialSource::NotRequired));
        assert!(headless_preflight(&world, &[], &view).is_empty(), "NotRequired は見ない");
    }

    /// 2'. テンプレートが村に無い個体は拒否（検査できないことを黙らない）。
    ///
    /// `register_agent` も `remove_template` もこの形を作らせないが、**読み込み
    /// （`from_persisted`）はテンプレートを確かめずに個体を入れる** — 手で直した
    /// `world.json` や別の版が書いた村で起こりうるので、その経路で作る。
    #[test]
    fn a_servant_whose_template_is_gone_is_rejected() {
        let mut persisted = village().to_persisted();
        let mut ghost = AgentSpec::new("c", "シー", "tpl");
        ghost.model_template_id = "ghost-tpl".into();
        persisted.agents.push(ghost);
        let world = World::from_persisted(persisted);
        let base = Base {
            start: ids(&["c"]),
            stdio: Vec::new(),
        };
        let view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::MODEL_TEMPLATE_MISSING]);
    }

    /// 3. 窓口が未設定・削除済みは ask だけで拒否。serve は見ない。
    #[test]
    fn reception_unset_or_missing_is_rejected_only_for_ask() {
        let mut world = village();
        let base = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let ask = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        let serve = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);

        assert_eq!(codes_of(&headless_preflight(&world, &[], &ask)), vec![codes::RECEPTION_UNSET]);
        assert!(headless_preflight(&world, &[], &serve).is_empty());

        world.set_reception(Some(&AgentId::from("b"))).unwrap();
        world.remove_agent(&AgentId::from("b")).unwrap();
        assert_eq!(
            codes_of(&headless_preflight(&world, &[], &ask)),
            vec![codes::RECEPTION_MISSING],
            "削除済みは未設定と畳まない"
        );
    }

    /// 4. 窓口の接続先が集合の外 — ask だけ・情報。
    #[test]
    fn reception_targets_outside_the_start_set_are_informational_for_ask() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let ask = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        let findings = headless_preflight(&world, &[], &ask);
        assert_eq!(codes_of(&findings), vec![codes::RECEPTION_TARGETS_OUTSIDE]);
        assert_eq!(findings[0].level, FindingLevel::Info);
        assert!(findings[0].message.contains('b'));

        // 判断役は起動しないので「集合の外」に数えない（P6 の実機で混ざっていた）。
        world
            .register_judge(JudgeSpec {
                id: AgentId::from("judge_1"),
                name: "判断役".to_owned(),
                order: 0,
                enabled: true,
                unknown: UnknownFields::default(),
            })
            .unwrap();
        world
            .set_connections(
                &AgentId::from("a"),
                vec![AgentId::from("b"), AgentId::from("judge_1")],
            )
            .unwrap();
        let findings = headless_preflight(&world, &[], &ask);
        assert_eq!(codes_of(&findings), vec![codes::RECEPTION_TARGETS_OUTSIDE]);
        assert!(!findings[0].message.contains("judge_1"), "{}", findings[0].message);
        let with_b = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let ask_b = with_b.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        assert!(
            headless_preflight(&world, &[], &ask_b).is_empty(),
            "判断役だけが外なら黙る"
        );

        let serve = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        assert!(headless_preflight(&world, &[], &serve).is_empty());
    }

    /// 5. 予定の宛先が集合の外 — serve だけ・警告。一時停止中の予定は見ない。
    #[test]
    fn schedule_targets_outside_the_start_set_warn_only_for_serve() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let mut paused = task("t2", "b");
        paused.enabled = false;
        let schedules = [task("t1", "b"), paused];

        let serve = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        let findings = headless_preflight(&world, &schedules, &serve);
        assert_eq!(codes_of(&findings), vec![codes::SCHEDULE_TARGET_OUTSIDE]);
        assert!(findings[0].message.contains("t1"));

        // ask は予定を回さないので予定の宛先は見ない（窓口 a の接続先 b が集合の外という
        // 情報は別の検査 4 の指摘として出る）。
        let ask = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        assert_eq!(
            codes_of(&headless_preflight(&world, &schedules, &ask)),
            vec![codes::RECEPTION_TARGETS_OUTSIDE]
        );
    }

    /// 6. 承認が必要 + run を持つ個体 — 両方・警告。モードを変えれば消える。
    #[test]
    fn required_approval_with_a_run_servant_warns() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let mut spec = world.agent(&AgentId::from("b")).unwrap().spec.clone();
        spec.enabled_tools = Some(vec!["grep".to_owned(), "run".to_owned()]);
        world.update_agent(spec).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };

        for mode in [HeadlessMode::Ask, HeadlessMode::Serve] {
            let mut view = base.view(mode, &yes_secret, &yes_probe);
            view.run_approval = RunApproval::Required;
            let findings = headless_preflight(&world, &[], &view);
            assert_eq!(codes_of(&findings), vec![codes::RUN_APPROVAL_REQUIRED], "{mode:?}");
            assert_eq!(findings[0].level, FindingLevel::Warn);
            assert!(findings[0].message.contains('b'));

            view.run_approval = RunApproval::NoApproval;
            assert!(headless_preflight(&world, &[], &view).is_empty());
        }
    }

    /// 7. 前判定・後判定が未承認 — serve だけ・警告（1 予定で 2 件になりうる）。
    #[test]
    fn unapproved_probes_warn_only_for_serve() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let mut t = task("t1", "b");
        t.probe = Some(probe("python"));
        t.acceptance = Some(Acceptance {
            probe: probe("pytest"),
            max_attempts: 2,
        });

        let serve = base.view(HeadlessMode::Serve, &yes_secret, &no_probe);
        let findings = headless_preflight(&world, std::slice::from_ref(&t), &serve);
        assert_eq!(
            codes_of(&findings),
            vec![codes::PROBE_UNAPPROVED, codes::PROBE_UNAPPROVED]
        );
        assert!(findings[0].message.contains("前判定") && findings[0].message.contains("python"));
        assert!(findings[1].message.contains("後判定") && findings[1].message.contains("pytest"));

        let approved = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        assert!(headless_preflight(&world, std::slice::from_ref(&t), &approved).is_empty());
        let ask = base.view(HeadlessMode::Ask, &yes_secret, &no_probe);
        assert!(headless_preflight(&world, std::slice::from_ref(&t), &ask).is_empty());
    }

    /// 8. Jev の鍵が無い — 判断役（有効なもの）か圧縮 ON のときだけ警告。
    #[test]
    fn a_missing_jev_token_warns_only_when_something_needs_it() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };

        let mut view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        view.jev_token_present = false;
        assert!(headless_preflight(&world, &[], &view).is_empty(), "要るものが無ければ黙る");

        view.jev_pruning_enabled = true;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::JEV_TOKEN_MISSING]);
        assert!(findings[0].message.contains("圧縮"));
        assert!(findings[0].fix.contains("FUSEFORKS_SECRET_JEV_API_TOKEN"));

        world
            .register_judge(JudgeSpec {
                id: AgentId::from("judge_1"),
                name: "判断役".to_owned(),
                order: 0,
                enabled: true,
                unknown: UnknownFields::default(),
            })
            .unwrap();
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![codes::JEV_TOKEN_MISSING, codes::JEV_TOKEN_MISSING],
            "判断役の分と圧縮の分"
        );

        view.jev_token_present = true;
        assert!(headless_preflight(&world, &[], &view).is_empty());
    }

    /// 9. stdio の MCP の command が見つからなければ警告（コマンドごとに 1 件・サーバーを列挙）。
    ///    見つかれば黙る（Spec 64 の情報 MCP_STDIO は撤去した）。
    #[test]
    fn missing_mcp_commands_warn_once_per_command() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: vec![
                stdio("a:docker", "docker"),
                stdio("a:memoria", "D:\\memoria\\m.exe"),
                stdio("b:docker", "docker"),
            ],
        };
        let mut view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        assert!(headless_preflight(&world, &[], &view).is_empty(), "見つかれば黙る");

        view.command_on_path = &nowhere;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![codes::MCP_COMMAND_NOT_FOUND, codes::MCP_COMMAND_NOT_FOUND],
            "docker と m.exe の 2 件（docker の 2 台は 1 件にまとまる）"
        );
        assert_eq!(findings[0].level, FindingLevel::Warn);
        assert!(findings[0].message.contains("a:docker, b:docker"), "{}", findings[0].message);
        assert!(findings[1].message.contains("D:\\memoria\\m.exe"));
    }

    /// 10. headers の参照が選んだストアに無ければ拒否（名前ごとに 1 件）。鍵は mcp:NAME。
    #[test]
    fn a_missing_header_secret_is_rejected_once_per_name() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let refs = [
            SecretRef { server: "a:outcasts".to_owned(), name: "OUTCASTS".to_owned() },
            SecretRef { server: "b:outcasts".to_owned(), name: "OUTCASTS".to_owned() },
        ];
        // テンプレートの鍵（tpl）はあり、mcp:OUTCASTS だけが無い置き場。
        let only_template = |key: &str| key == "tpl";
        let mut view = base.view(HeadlessMode::Ask, &only_template, &yes_probe);
        view.mcp_secret_refs = &refs;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::SECRET_MISSING]);
        assert_eq!(findings[0].level, FindingLevel::Reject);
        assert!(findings[0].message.contains("a:outcasts, b:outcasts"), "{}", findings[0].message);
        assert!(findings[0].fix.contains("FUSEFORKS_SECRET_MCP_OUTCASTS"), "{}", findings[0].fix);
        assert!(findings[0].fix.contains("mcp:OUTCASTS"));

        let with_ref = |key: &str| key == "tpl" || key == "mcp:OUTCASTS";
        let mut view = base.view(HeadlessMode::Ask, &with_ref, &yes_probe);
        view.mcp_secret_refs = &refs;
        assert!(headless_preflight(&world, &[], &view).is_empty());
    }

    /// 11. --door-port があって合鍵が無ければ serve だけ拒否。--door-port が無ければ見ない。
    #[test]
    fn a_door_port_without_a_token_is_rejected_for_serve() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let mut view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        assert!(headless_preflight(&world, &[], &view).is_empty(), "扉を開かないなら見ない");

        view.door_port = Some(39641);
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::DOOR_TOKEN_MISSING]);
        assert!(findings[0].fix.contains("FUSEFORKS_SECRET_DOOR_TOKEN"), "{}", findings[0].fix);

        view.door_token_present = true;
        assert!(headless_preflight(&world, &[], &view).is_empty());

        let mut ask = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        ask.door_port = Some(39641);
        assert!(headless_preflight(&world, &[], &ask).is_empty(), "ask は扉を開かない");
    }

    /// 12・13. 作業フォルダが無い・ファイルなら拒否、rag の宣言が無ければ警告（パスごとに 1 件）。
    ///         未設定の作業フォルダと、起動しない個体のパスは見ない。
    #[test]
    fn missing_work_dirs_are_rejected_and_rag_sources_warn() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        for (id, dir, rag) in [
            ("a", Some("/work/mathlab"), vec!["/work/rag".to_owned()]),
            ("b", Some("/work/mathlab"), vec!["/work/rag".to_owned(), "/work/ok".to_owned()]),
        ] {
            let mut spec = world.agent(&AgentId::from(id)).unwrap().spec.clone();
            spec.work_dir = dir.map(str::to_owned);
            spec.rag_sources = rag;
            world.update_agent(spec).unwrap();
        }
        let only_ok = |path: &str| match path {
            "/work/ok" => PathKind::Dir,
            "/work/rag" => PathKind::File,
            _ => PathKind::Missing,
        };
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let mut view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        view.path_kind = &only_ok;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![codes::WORK_DIR_MISSING, codes::RAG_SOURCE_MISSING]
        );
        assert_eq!(findings[0].level, FindingLevel::Reject);
        assert!(findings[0].message.contains("/work/mathlab（a, b）"), "{}", findings[0].message);
        assert!(findings[1].message.contains("/work/rag（a, b）"), "{}", findings[1].message);

        // フォルダの代わりにファイルがあっても拒否（「無い」と同じ扱い）。
        let all_files = |_: &str| PathKind::File;
        view.path_kind = &all_files;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![codes::WORK_DIR_MISSING, codes::RAG_SOURCE_MISSING, codes::RAG_SOURCE_MISSING]
        );

        // 作業フォルダが未設定なら見ない。起動しない個体のパスも見ない。
        let mut spec = world.agent(&AgentId::from("a")).unwrap().spec.clone();
        spec.work_dir = None;
        spec.rag_sources = Vec::new();
        world.update_agent(spec).unwrap();
        let only_a = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let mut view = only_a.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        view.path_kind = &only_ok;
        assert!(headless_preflight(&world, &[], &view).is_empty());
    }

    /// 14. run の許可コマンドが PATH に無ければ警告。数えるのは run を持つ起動する個体だけ。
    #[test]
    fn missing_run_commands_warn_only_for_starting_run_servants() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let mut spec = world.agent(&AgentId::from("b")).unwrap().spec.clone();
        spec.enabled_tools = Some(vec!["run".to_owned()]);
        world.update_agent(spec).unwrap();
        let runs = [
            RunCommand { agent: AgentId::from("a"), command: "lake".to_owned() },
            RunCommand { agent: AgentId::from("b"), command: "lake".to_owned() },
            RunCommand { agent: AgentId::from("b"), command: "git".to_owned() },
        ];
        let only_git = |c: &str| c == "git";
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let mut view = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        view.run_commands = &runs;
        view.command_on_path = &only_git;
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::RUN_COMMAND_NOT_FOUND]);
        assert!(findings[0].message.contains("lake（b）"), "a は run を持たないので数えない: {}", findings[0].message);
    }

    /// 15. 時刻帯 — serve で、有効な壁時計の予定があり、bake.json があるときだけ比べる。
    #[test]
    fn the_time_zone_is_compared_only_when_it_matters() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: Vec::new(),
        };
        let weekly = [task("t1", "b")];
        let mut view = base.view(HeadlessMode::Serve, &yes_secret, &yes_probe);
        view.process_time_zone = ProcessTimeZone::Named("Etc/UTC");
        assert!(headless_preflight(&world, &weekly, &view).is_empty(), "bake.json が無ければ比べない");

        view.baked_time_zone = Some("Asia/Tokyo");
        let findings = headless_preflight(&world, &weekly, &view);
        assert_eq!(codes_of(&findings), vec![codes::TIMEZONE_MISMATCH]);
        assert!(findings[0].message.contains("Etc/UTC") && findings[0].message.contains("Asia/Tokyo"));
        assert!(findings[0].fix.contains("TZ=Asia/Tokyo"));

        view.process_time_zone = ProcessTimeZone::Unreadable("Bogus/Zone");
        let findings = headless_preflight(&world, &weekly, &view);
        assert_eq!(codes_of(&findings), vec![codes::TIMEZONE_MISMATCH]);
        assert!(findings[0].message.contains("TZ=Bogus/Zone"), "{}", findings[0].message);

        view.process_time_zone = ProcessTimeZone::Unknown;
        assert_eq!(codes_of(&headless_preflight(&world, &weekly, &view)), vec![codes::TIMEZONE_MISMATCH]);

        view.process_time_zone = ProcessTimeZone::Named("Asia/Tokyo");
        assert!(headless_preflight(&world, &weekly, &view).is_empty(), "同じなら黙る");

        // 無効な予定・interval だけの村・ask では比べない。
        view.process_time_zone = ProcessTimeZone::Named("Etc/UTC");
        let mut paused = task("t2", "b");
        paused.enabled = false;
        let mut interval = task("t3", "b");
        interval.recurrence = Recurrence::Interval { every_minutes: 5 };
        assert!(headless_preflight(&world, &[paused, interval], &view).is_empty());
        let mut ask = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        ask.process_time_zone = ProcessTimeZone::Named("Etc/UTC");
        ask.baked_time_zone = Some("Asia/Tokyo");
        assert!(headless_preflight(&world, &weekly, &ask).is_empty());
    }

    /// 結果は重い順（拒否 → 警告 → 情報）。同じ重さの中は表の順。
    #[test]
    fn findings_are_ordered_heaviest_first() {
        let mut world = village();
        // 表の順と重さが逆転する組を入れる: 4 行目（情報: 窓口の接続先 b が集合の外）は
        // 6 行目（警告: 承認モード）より表の上にある。並べ替えが無いと情報が警告の前に出る。
        // あわせて拒否（1 行目: 計画の確認 → 12 行目: 作業フォルダ）と、同じ重さの中の順
        // （警告 6 行目 → 8 行目 → 9 行目）も見る。12 行目の拒否は表の下にあるので、
        // 並べ替えが無いと警告の後に出る（Spec 65 で表の下に拒否が足された）。
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a"]),
            stdio: vec![stdio("fs", "fs-server")],
        };
        let mut view = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        view.jev_token_present = false;
        view.jev_pruning_enabled = true;
        view.run_approval = RunApproval::Required;
        view.command_on_path = &nowhere;
        view.path_kind = &nothing_exists;
        let mut spec = world.agent(&AgentId::from("a")).unwrap().spec.clone();
        spec.enabled_tools = Some(vec!["run".to_owned()]);
        spec.plan_review = true;
        spec.work_dir = Some("/work/x".to_owned());
        world.update_agent(spec).unwrap();

        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![
                codes::PLAN_REVIEW_WAITS,
                codes::WORK_DIR_MISSING,
                codes::RUN_APPROVAL_REQUIRED,
                codes::JEV_TOKEN_MISSING,
                codes::MCP_COMMAND_NOT_FOUND,
                codes::RECEPTION_TARGETS_OUTSIDE,
            ]
        );
    }

    /// 文言は村の言語（英語の村なら英語）。構造は同じ。
    #[test]
    fn messages_follow_the_village_language() {
        let mut world = village();
        world.set_language(Language::En);
        let base = Base {
            start: ids(&["a"]),
            stdio: Vec::new(),
        };
        let view = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::RECEPTION_UNSET]);
        assert!(findings[0].message.starts_with("No reception"), "{}", findings[0].message);
    }

    /// `--json` の形: level は snake_case・欄は camelCase。
    #[test]
    fn findings_serialise_for_json_output() {
        let finding = Finding {
            level: FindingLevel::Reject,
            code: codes::RECEPTION_UNSET,
            message: "m".to_owned(),
            fix: "f".to_owned(),
        };
        let json = serde_json::to_value(&finding).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "level": "reject", "code": "RECEPTION_UNSET", "message": "m", "fix": "f" })
        );
    }
}
