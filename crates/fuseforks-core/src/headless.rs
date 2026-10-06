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

use std::collections::BTreeSet;

use serde::Serialize;

use crate::command::RunApproval;
use crate::model::{AgentId, CredentialSource};
use crate::schedule::ScheduledTask;
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
    /// MCP サーバーが `command` で起動する stdio（情報）。
    pub const MCP_STDIO: &str = "MCP_STDIO";
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
    /// `command` で起動する stdio の MCP サーバー名（共通と個体別を呼び出し側が集める）。
    pub stdio_mcp_servers: &'a [String],
}

/// Jev のトークンの鍵。置き場はホスト層（`jev_settings.rs` の `TOKEN_KEY`）で、
/// ここは直し方の文に変数名を書くためだけに同じ綴りを持つ。
const JEV_TOKEN_KEY: &str = "jev_api_token";

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

    // 9. MCP サーバーが command で起動する stdio — 両方・情報。
    //    その実行ファイルがあるかは検査からは分からない（接続は起動時に試す）。
    for name in view.stdio_mcp_servers {
        out.push(Finding {
            level: FindingLevel::Info,
            code: codes::MCP_STDIO,
            message: lang
                .pick(
                    &format!("MCP サーバー {name} は command で起動する stdio です。実行ファイルがあるかは起動時に接続して初めて分かります"),
                    &format!("MCP server {name} is a stdio server started by a command; whether the executable exists is only known when connecting at boot"),
                )
                .to_owned(),
            fix: lang
                .pick(
                    "コンテナの像にその実行ファイルを入れる（起動時のログ「MCP の初期接続に失敗しました」で確かめる）",
                    "Put the executable in the container image (check the boot log line about the MCP connection failing)",
                )
                .to_owned(),
        });
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

    /// 全部そろった view（秘密あり・承認あり・鍵あり・承認モードは自動承認）。
    struct Base {
        start: BTreeSet<AgentId>,
        stdio: Vec<String>,
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
                stdio_mcp_servers: &self.stdio,
            }
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

    /// 9. stdio の MCP サーバーは情報。
    #[test]
    fn stdio_mcp_servers_are_informational() {
        let mut world = village();
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a", "b"]),
            stdio: vec!["filesystem".to_owned()],
        };
        let view = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(codes_of(&findings), vec![codes::MCP_STDIO]);
        assert_eq!(findings[0].level, FindingLevel::Info);
        assert!(findings[0].message.contains("filesystem"));
    }

    /// 結果は重い順（拒否 → 警告 → 情報）。同じ重さの中は表の順。
    #[test]
    fn findings_are_ordered_heaviest_first() {
        let mut world = village();
        // 表の順と重さが逆転する組を入れる: 4 行目（情報: 窓口の接続先 b が集合の外）は
        // 6 行目（警告: 承認モード）より表の上にある。並べ替えが無いと情報が警告の前に出る。
        // あわせて拒否（1 行目: 計画の確認）と、同じ重さの中の順（情報 4 行目 → 9 行目）も見る。
        world.set_reception(Some(&AgentId::from("a"))).unwrap();
        let base = Base {
            start: ids(&["a"]),
            stdio: vec!["fs".to_owned()],
        };
        let mut view = base.view(HeadlessMode::Ask, &yes_secret, &yes_probe);
        view.jev_token_present = false;
        view.jev_pruning_enabled = true;
        view.run_approval = RunApproval::Required;
        let mut spec = world.agent(&AgentId::from("a")).unwrap().spec.clone();
        spec.enabled_tools = Some(vec!["run".to_owned()]);
        spec.plan_review = true;
        world.update_agent(spec).unwrap();

        let findings = headless_preflight(&world, &[], &view);
        assert_eq!(
            codes_of(&findings),
            vec![
                codes::PLAN_REVIEW_WAITS,
                codes::RUN_APPROVAL_REQUIRED,
                codes::JEV_TOKEN_MISSING,
                codes::RECEPTION_TARGETS_OUTSIDE,
                codes::MCP_STDIO,
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
