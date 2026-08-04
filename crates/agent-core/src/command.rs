//! コマンド実行の許容規則（Spec 15）。
//!
//! ここに置くのは**型と純機構**だけで、ファイルの読み書きは
//! [`crate::config_store::ConfigStore`] が持つ（`schedule.rs` と同じ分業）。
//!
//! # 2 つの機構が同居している（移行中）
//!
//! - **rev4（新）**: [`CommandPolicy`] — エージェント別の allow / deny / pending。
//!   `agents/{id}/command.json` に住み、`mcp.json` と同じ棚で人が直す
//! - **rev3（旧）**: [`CommandRegistry`] 系 — 村で 1 つの登録簿。**P3' で撤去する**
//!
//! 同居させているのは、配線の差し替えを段階に分けるため。**新しい機構を先に
//! 置いて単体で固め、配線を次の Phase で移す** — 1 コミットで全部を動かすと、
//! どこで壊れたかが読めなくなる。
//!
//! 以下の節は**旧機構（rev3）の設計記録**で、`CommandPolicy` には掛からない。
//!
//! # 閉じた許容（rev3。rev4 で撤回済み）
//!
//! エージェントは**実行ファイル名を書けない。書けるのは登録名だけ。**
//! これが `command_tool_contract` の中核で、コマンド名の許可リスト（開いた許容）を
//! 採らなかった理由でもある（`failures.md` #56）。副産物として PATH 解決の曖昧性と
//! Windows のコマンド名正規化（`python` / `python.exe` / `PATHEXT` / 大文字小文字）が
//! 問題ごと消える。
//!
//! # 無効化は削除ではない
//!
//! `program` が実在しない登録は**メモリ上で無効印を付けるだけ**で、ファイルは
//! 書き換えない。書き換えると、配布した村を別の端末で一度開いて閉じただけで
//! 登録が消える（元の端末へ戻しても失われている）。
//!
//! # 実在検査は注入する
//!
//! [`CommandRegistry::mark_availability`] は述語を受け取り、自分では `fs` を触らない
//! （`AgentRoleDefaults::apply_to` が `template_exists` を受けるのと同じ形）。
//! テストが実ファイルを置かずに書ける。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::model::AgentId;

/// 壁時計を読む**唯一の場所**。
///
/// [`CommandRequestLog::record`] は `now_ms` を引数で受け取る（`schedule.rs` と
/// 同じ規律 — 内部で壁時計を読むと、テストが特定の時刻でだけ落ちるものになる）。
/// その引数を作るのは**呼び出しの縁**の仕事で、ここに 1 つだけ置く。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// 登録要求の保持上限。超えたら `first_requested_at_ms` の**古い順**に捨てる。
///
/// `run` では RepeatGuard の効きが落ちる（同じコマンドが違う出力を返す）ので、
/// 歯止めの効かないエージェントがファイルを無限に太らせる経路を塞ぐ。
pub const MAX_COMMAND_REQUESTS: usize = 50;

/// タイムアウトの既定値（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// タイムアウトの上限（秒）。`cargo build` / `pytest` を締め出さない長さ。
pub const MAX_TIMEOUT_SECS: u64 = 3600;

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

/// 1 件の登録。`{workspace}/commands.json` に入る形そのもの。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRegistration {
    /// エージェントが呼ぶときの名前。登録簿の中で一意。
    pub name: String,
    /// モデルへ提示する説明。**いつ呼ぶべきかを書く**（`AgentTool` と同じ規律）。
    #[serde(default)]
    pub description: String,
    /// 実行ファイルの**絶対パス**。登録時に解決して記録し、実行時に `PATH` を
    /// 引き直さない。
    pub program: PathBuf,
    /// 常に渡す引数。エージェントの `extraArgs` は**この後ろへ足される**
    /// （置き換えは起こらない）。
    #[serde(default)]
    pub args: Vec<String>,
    /// エージェントが引数を足せるか。`false` なら足すと拒否。
    ///
    /// **汎用インタプリタ（`python` / `node` / `sh`）で `true` にすると
    /// 任意コード実行になる。** 機構では止めず、UI が警告を出す
    /// （警告リストは除外リストではない = 許容の判定に一切関与しない）。
    #[serde(default)]
    pub allow_extra_args: bool,
    /// このコマンドのタイムアウト（秒）。
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// 実行時の作業ディレクトリ。`None` なら呼んだエージェントの `work_dir`。
    #[serde(default)]
    pub cwd: Option<PathBuf>,
}

/// 登録が使えない理由。**理由ごとに次の手が違う**ので 1 つに畳まない
/// （`failures.md` #44 — 歯止めの先に道を書く）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// `program` が絶対パスではない。手で書いた登録に起きる。
    ProgramNotAbsolute,
    /// `program` の指す先にファイルが無い。別の端末で村を開いたときに起きる。
    ProgramMissing,
}

impl Unavailable {
    /// 利用者・モデルへ見せる理由。
    pub fn reason_ja(self) -> &'static str {
        match self {
            Self::ProgramNotAbsolute => {
                "実行ファイルが絶対パスで登録されていません（登録し直してください）"
            }
            Self::ProgramMissing => {
                "実行ファイルが見つかりません（別の端末の村を開いた可能性があります）"
            }
        }
    }
}

/// 登録 1 件と、その実行時の状態。
#[derive(Debug, Clone, PartialEq)]
pub struct RegisteredCommand {
    /// ファイルに書かれている内容そのまま。
    pub spec: CommandRegistration,
    /// 使えない理由。`None` なら使える。**ファイルには書き戻さない。**
    pub unavailable: Option<Unavailable>,
}

impl RegisteredCommand {
    /// 実行時の作業ディレクトリを決める。登録の `cwd` が優先、無ければ個体の
    /// `work_dir`。**どちらも無ければ実行できない。**
    pub fn resolve_cwd(&self, agent_work_dir: Option<&Path>) -> Option<PathBuf> {
        self.spec
            .cwd
            .clone()
            .or_else(|| agent_work_dir.map(Path::to_path_buf))
    }

    /// この個体から**今すぐ実行できるか**。
    ///
    /// 提示の判定はこの述語を単位にする — 実行できない登録の名前を見せると、
    /// モデルはそれを呼び、拒否され、他に手が無ければ同じ呼び出しを繰り返す。
    pub fn is_runnable(&self, agent_work_dir: Option<&Path>) -> bool {
        self.unavailable.is_none() && self.resolve_cwd(agent_work_dir).is_some()
    }

    /// 実際に走る引数列。`spec.args` の後ろへ `extra` を足す（置き換えない）。
    pub fn full_args(&self, extra: &[String]) -> Vec<String> {
        let mut args = self.spec.args.clone();
        args.extend_from_slice(extra);
        args
    }

    /// 上限で丸めたタイムアウト。
    pub fn timeout_secs(&self) -> u64 {
        self.spec.timeout_secs.clamp(1, MAX_TIMEOUT_SECS)
    }
}

/// 登録簿。`{workspace}/commands.json` の投影。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandRegistry {
    entries: Vec<RegisteredCommand>,
}

impl CommandRegistry {
    /// 読み込んだ登録から作る。**同名は最初の 1 件だけを採り、後を落とす** —
    /// 後勝ちにすると、どちらが効いているかを利用者がファイルから読めない。
    ///
    /// 戻りは落とした理由。空でなければ呼び出し側が WARN を出す。
    #[must_use = "落とした登録を捨てると「入れたはずの登録が無い」が見えなくなる"]
    pub fn from_registrations(rows: Vec<CommandRegistration>) -> (Self, Vec<String>) {
        let mut entries: Vec<RegisteredCommand> = Vec::new();
        let mut dropped = Vec::new();

        for spec in rows {
            if spec.name.trim().is_empty() {
                dropped.push("名前が空の登録を 1 件落としました".to_owned());
                continue;
            }
            if entries.iter().any(|e| e.spec.name == spec.name) {
                dropped.push(format!("登録名が重複しています: `{}`（後の 1 件を落としました）", spec.name));
                continue;
            }
            if spec.timeout_secs == 0 || spec.timeout_secs > MAX_TIMEOUT_SECS {
                dropped.push(format!(
                    "`{}` のタイムアウト {} 秒は範囲外です（1〜{MAX_TIMEOUT_SECS} 秒へ丸めました）",
                    spec.name, spec.timeout_secs
                ));
            }
            entries.push(RegisteredCommand { spec, unavailable: None });
        }

        (Self { entries }, dropped)
    }

    /// `program` の実在を検査して無効印を付ける。**ファイルは書き換えない。**
    ///
    /// 述語を受け取るのは、この関数が `fs` を知らないため（テストが実ファイルを
    /// 置かずに書ける。`AgentRoleDefaults::apply_to` と同じ形）。
    ///
    /// 戻りは無効化した登録の説明。空でなければ呼び出し側が WARN を出す。
    #[must_use = "無効化を黙って行うと「登録したのに使えない」が見えなくなる"]
    pub fn mark_availability(&mut self, program_exists: impl Fn(&Path) -> bool) -> Vec<String> {
        let mut notes = Vec::new();
        for entry in &mut self.entries {
            let reason = if !entry.spec.program.is_absolute() {
                Some(Unavailable::ProgramNotAbsolute)
            } else if !program_exists(&entry.spec.program) {
                Some(Unavailable::ProgramMissing)
            } else {
                None
            };
            entry.unavailable = reason;
            if let Some(reason) = reason {
                notes.push(format!(
                    "`{}` を無効にしました: {}（{}）",
                    entry.spec.name,
                    reason.reason_ja(),
                    entry.spec.program.display()
                ));
            }
        }
        notes
    }

    /// 名前で引く。無効な登録も返す — **「登録されていない」と「登録はあるが
    /// 使えない」は別の答え**で、モデルへ返す文面が違う。
    pub fn get(&self, name: &str) -> Option<&RegisteredCommand> {
        self.entries.iter().find(|e| e.spec.name == name)
    }

    /// この個体から実行できる登録。**モデルへ列挙するのはこれだけ。**
    pub fn runnable(&self, agent_work_dir: Option<&Path>) -> Vec<&RegisteredCommand> {
        self.entries
            .iter()
            .filter(|e| e.is_runnable(agent_work_dir))
            .collect()
    }

    /// この個体から実行できない登録の件数。**名前は返さない** — 使えないものに
    /// スキーマ分のトークンを払わず、件数と次の手だけを 1 行で伝える。
    pub fn unrunnable_count(&self, agent_work_dir: Option<&Path>) -> usize {
        self.entries.len() - self.runnable(agent_work_dir).len()
    }

    /// 登録の総数（有効・無効を問わない）。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 登録が 1 件も無いか。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 保存用に生の登録へ戻す。**無効印は落ちる**（ファイルへ書き戻さない）。
    pub fn to_registrations(&self) -> Vec<CommandRegistration> {
        self.entries.iter().map(|e| e.spec.clone()).collect()
    }
}

/// エージェントが呼んだが登録が無かった記録。`{workspace}/command_requests.json`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandRequest {
    /// 呼ばれた登録名。
    pub name: String,
    /// **エージェントが渡そうとした追加引数だけ。**
    ///
    /// `args` にすると、利用者が登録画面で「これを登録の `args` に入れるのか、
    /// `allowExtraArgs: true` にして追加で渡させるのか」を決められない。
    #[serde(default)]
    pub attempted_extra_args: Vec<String>,
    /// 呼んだエージェント。
    pub agent_id: AgentId,
    /// 最初に要求された時刻。**畳んでも更新しない**（いつから欲しがっているかが
    /// 消える）。
    pub first_requested_at_ms: u64,
    /// 要求された回数。
    pub count: u32,
}

/// 登録要求の帳面。`{workspace}/command_requests.json` の投影。
///
/// **`commands.json` とは別ファイル。** 同居させると、登録簿が壊れた JSON に
/// なったときの「空として起動 WARN」で**登録要求まで一緒に消える**。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandRequestLog {
    requests: Vec<CommandRequest>,
}

impl CommandRequestLog {
    /// 読み込んだ要求から作る。
    pub fn from_requests(requests: Vec<CommandRequest>) -> Self {
        Self { requests }
    }

    /// 要求を 1 件積む。
    ///
    /// **重複は `(name, attempted_extra_args)` の完全一致で畳む** — `name` だけで
    /// 畳むと、利用者が「何を登録すればよいか」を決める材料（実際に呼ばれた引数）が
    /// 消える。
    ///
    /// 上限を超えたら `first_requested_at_ms` の**古い順**に捨てる。
    /// 「`count` の少ない古いもの」にしないのは、2 つの軸を混ぜるとどちらが効いたかを
    /// 利用者が読めないため。
    ///
    /// `now_ms` は**引数で受け取る**（`schedule.rs` と同じ規律。内部で壁時計を読むと、
    /// テストが特定の時刻でだけ落ちるものになる）。
    ///
    /// 戻りは捨てた登録要求の説明。空でなければ呼び出し側が WARN を出す。
    #[must_use = "捨てた要求を黙って消すと「頼んだのに画面に出ない」が見えなくなる"]
    pub fn record(
        &mut self,
        name: &str,
        attempted_extra_args: &[String],
        agent_id: &AgentId,
        now_ms: u64,
    ) -> Option<String> {
        if let Some(existing) = self
            .requests
            .iter_mut()
            .find(|r| r.name == name && r.attempted_extra_args == attempted_extra_args)
        {
            existing.count = existing.count.saturating_add(1);
            return None;
        }

        self.requests.push(CommandRequest {
            name: name.to_owned(),
            attempted_extra_args: attempted_extra_args.to_vec(),
            agent_id: agent_id.clone(),
            first_requested_at_ms: now_ms,
            count: 1,
        });

        if self.requests.len() <= MAX_COMMAND_REQUESTS {
            return None;
        }

        // 最も古いものを 1 件捨てる。押し出しは 1 件ずつしか起きない
        // （1 回の record で増えるのは 1 件だけ）。
        let oldest = self
            .requests
            .iter()
            .enumerate()
            .min_by_key(|(_, r)| r.first_requested_at_ms)
            .map(|(index, _)| index)?;
        let removed = self.requests.remove(oldest);
        Some(format!(
            "登録要求が上限 {MAX_COMMAND_REQUESTS} 件を超えたため、最も古い `{}` を捨てました",
            removed.name
        ))
    }

    /// 登録が済んだ要求を取り除く。
    pub fn forget(&mut self, name: &str) {
        self.requests.retain(|r| r.name != name);
    }

    /// 保存・表示用。
    pub fn requests(&self) -> &[CommandRequest] {
        &self.requests
    }

    /// 要求が 1 件も無いか。
    pub fn is_empty(&self) -> bool {
        self.requests.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(name: &str, program: &str) -> CommandRegistration {
        CommandRegistration {
            name: name.to_owned(),
            description: String::new(),
            program: PathBuf::from(program),
            args: Vec::new(),
            allow_extra_args: false,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            cwd: None,
        }
    }

    #[cfg(windows)]
    const ABS: &str = r"C:\bin\python.exe";
    #[cfg(not(windows))]
    const ABS: &str = "/usr/bin/python";

    #[test]
    fn duplicate_names_keep_the_first_and_report_the_rest() {
        let (registry, dropped) =
            CommandRegistry::from_registrations(vec![reg("test", ABS), reg("test", ABS)]);
        assert_eq!(registry.len(), 1);
        assert_eq!(dropped.len(), 1, "落としたことを言うこと: {dropped:?}");
    }

    #[test]
    fn an_empty_name_is_dropped() {
        let (registry, dropped) = CommandRegistry::from_registrations(vec![reg("  ", ABS)]);
        assert!(registry.is_empty());
        assert_eq!(dropped.len(), 1);
    }

    #[test]
    fn a_missing_program_is_marked_unavailable_without_touching_the_file() {
        let (mut registry, _) = CommandRegistry::from_registrations(vec![reg("test", ABS)]);
        let notes = registry.mark_availability(|_| false);

        assert_eq!(notes.len(), 1, "無効化を黙って行わないこと: {notes:?}");
        assert_eq!(
            registry.get("test").unwrap().unavailable,
            Some(Unavailable::ProgramMissing)
        );
        assert_eq!(
            registry.to_registrations().len(),
            1,
            "書き戻す内容から登録が消えないこと（消すと配布した村を開いただけで失われる）"
        );
    }

    #[test]
    fn a_relative_program_is_a_different_reason_from_a_missing_one() {
        let (mut registry, _) = CommandRegistry::from_registrations(vec![reg("test", "python")]);
        let _ = registry.mark_availability(|_| true);
        assert_eq!(
            registry.get("test").unwrap().unavailable,
            Some(Unavailable::ProgramNotAbsolute),
            "実在検査より先に絶対パスを見ること（理由ごとに次の手が違う）"
        );
    }

    #[test]
    fn availability_is_recomputed_so_a_fixed_path_recovers() {
        let (mut registry, _) = CommandRegistry::from_registrations(vec![reg("test", ABS)]);
        let _ = registry.mark_availability(|_| false);
        let notes = registry.mark_availability(|_| true);
        assert!(notes.is_empty(), "直したら無効印が消えること: {notes:?}");
        assert!(registry.get("test").unwrap().unavailable.is_none());
    }

    #[test]
    fn runnable_needs_both_availability_and_a_cwd() {
        let (mut registry, _) = CommandRegistry::from_registrations(vec![reg("test", ABS)]);
        let _ = registry.mark_availability(|_| true);

        assert!(
            registry.runnable(None).is_empty(),
            "cwd が決まらなければ実行可能にしないこと"
        );
        assert_eq!(registry.unrunnable_count(None), 1);
        assert_eq!(registry.runnable(Some(Path::new("/w"))).len(), 1);
        assert_eq!(registry.unrunnable_count(Some(Path::new("/w"))), 0);
    }

    #[test]
    fn a_registration_cwd_wins_over_the_agent_work_dir() {
        let mut spec = reg("test", ABS);
        spec.cwd = Some(PathBuf::from("/fixed"));
        let (registry, _) = CommandRegistry::from_registrations(vec![spec]);
        let entry = registry.get("test").unwrap();
        assert_eq!(
            entry.resolve_cwd(Some(Path::new("/w"))),
            Some(PathBuf::from("/fixed"))
        );
        assert_eq!(entry.resolve_cwd(None), Some(PathBuf::from("/fixed")));
    }

    #[test]
    fn extra_args_are_appended_never_substituted() {
        let mut spec = reg("test", ABS);
        spec.args = vec!["-m".into(), "pytest".into()];
        let (registry, _) = CommandRegistry::from_registrations(vec![spec]);
        assert_eq!(
            registry.get("test").unwrap().full_args(&["-q".to_string()]),
            vec!["-m", "pytest", "-q"],
            "登録の args を置き換えないこと"
        );
    }

    #[test]
    fn an_out_of_range_timeout_is_clamped_and_reported() {
        let mut spec = reg("test", ABS);
        spec.timeout_secs = 99_999;
        let (registry, dropped) = CommandRegistry::from_registrations(vec![spec]);
        assert_eq!(dropped.len(), 1, "黙って丸めないこと: {dropped:?}");
        assert_eq!(registry.get("test").unwrap().timeout_secs(), MAX_TIMEOUT_SECS);
    }

    #[test]
    fn an_unavailable_registration_is_still_findable_by_name() {
        let (mut registry, _) = CommandRegistry::from_registrations(vec![reg("test", ABS)]);
        let _ = registry.mark_availability(|_| false);
        assert!(
            registry.get("test").is_some(),
            "「登録が無い」と「登録はあるが使えない」は別の答え（文面が違う）"
        );
    }

    // -- 登録要求 -------------------------------------------------------------

    fn agent() -> AgentId {
        AgentId::from("agent_1")
    }

    #[test]
    fn the_same_call_is_folded_and_counted() {
        let mut log = CommandRequestLog::default();
        let args = vec!["-q".to_string()];
        assert!(log.record("pytest", &args, &agent(), 100).is_none());
        assert!(log.record("pytest", &args, &agent(), 200).is_none());

        assert_eq!(log.requests().len(), 1);
        assert_eq!(log.requests()[0].count, 2);
        assert_eq!(
            log.requests()[0].first_requested_at_ms, 100,
            "畳んでも最初の時刻を保つこと（いつから欲しがっているかが消える）"
        );
    }

    #[test]
    fn different_extra_args_are_separate_requests() {
        let mut log = CommandRequestLog::default();
        let _ = log.record("pytest", &["-q".to_string()], &agent(), 100);
        let _ = log.record("pytest", &["-v".to_string()], &agent(), 100);
        assert_eq!(
            log.requests().len(),
            2,
            "name だけで畳むと「何を登録すればよいか」の材料が消える"
        );
    }

    #[test]
    fn the_oldest_request_is_evicted_at_the_cap() {
        let mut log = CommandRequestLog::default();
        for i in 0..MAX_COMMAND_REQUESTS {
            // 新しいものほど大きい時刻。最初の 1 件がいちばん古い。
            let _ = log.record(&format!("cmd{i}"), &[], &agent(), 1_000 + i as u64);
        }
        assert_eq!(log.requests().len(), MAX_COMMAND_REQUESTS);

        let warn = log.record("overflow", &[], &agent(), 9_999);
        assert_eq!(log.requests().len(), MAX_COMMAND_REQUESTS);
        assert!(
            warn.is_some_and(|w| w.contains("cmd0")),
            "最も古いものを捨て、捨てたことを言うこと"
        );
        assert!(log.requests().iter().any(|r| r.name == "overflow"));
        assert!(!log.requests().iter().any(|r| r.name == "cmd0"));
    }

    #[test]
    fn folding_does_not_evict() {
        let mut log = CommandRequestLog::default();
        for i in 0..MAX_COMMAND_REQUESTS {
            let _ = log.record(&format!("cmd{i}"), &[], &agent(), 1_000 + i as u64);
        }
        let warn = log.record("cmd0", &[], &agent(), 9_999);
        assert!(warn.is_none(), "畳みで押し出しを起こさないこと");
        assert_eq!(log.requests().len(), MAX_COMMAND_REQUESTS);
    }

    #[test]
    fn registering_forgets_the_request() {
        let mut log = CommandRequestLog::default();
        let _ = log.record("pytest", &["-q".to_string()], &agent(), 100);
        let _ = log.record("pytest", &["-v".to_string()], &agent(), 100);
        log.forget("pytest");
        assert!(log.is_empty(), "同じ名前の要求はまとめて消えること");
    }
}

// ---------------------------------------------------------------------------
// rev4: エージェント別の allow / deny（Spec 15 rev4）
//
// 上の登録制（`CommandRegistry` 系）は **P3' で撤去する**。ここでは新しい機構を
// 並べて置き、配線の差し替えを段階に分ける。
// ---------------------------------------------------------------------------

/// `pending` の保持上限（1 体あたり）。
///
/// rev3 は村全体で 50 件だったが、per-agent になったので小さくてよい。
pub const MAX_PENDING: usize = 20;

/// 1 回の照合の答え。**理由ごとに次の手が違う**ので 1 つに畳まない
/// （`failures.md` #44）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// `allow` に一致。承認なしで実行する。
    Allowed,
    /// `deny` に一致。実行せず、**承認要求も積まない**。
    ///
    /// 一度「要らない」と決めたものが毎回画面へ並ぶと、承認の一覧がノイズで埋まり、
    /// 本当に判断すべき新規の要求が埋もれる。
    Denied,
    /// どちらにも無い。実行せず、`pending` へ積む。
    Unknown,
    /// `command` にパス区切りが含まれる。**照合前に拒否**する。
    ///
    /// `./python` と `python` を別物として数えると、`allow` が指す対象が曖昧になる。
    /// 解決は OS の `PATH` に任せる。
    Malformed,
}

/// エージェントが呼んだが `allow` にも `deny` にも無かった記録。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingCommand {
    /// 呼ばれた実行ファイル名。
    pub command: String,
    /// 渡された引数。
    #[serde(default)]
    pub args: Vec<String>,
    /// 最初に要求された時刻。**畳んでも更新しない**（いつから欲しがっているかが消える）。
    pub first_requested_at_ms: u64,
    /// 要求された回数。
    pub count: u32,
}

/// `agents/{id}/command.json` の中身。
///
/// **`mcp.json` と同じ棚**に置き、同じ編集手段（`ConfigFileKind`）で人が直す。
/// **人と機械の両方が書く唯一のファイル**なので、書き込みは全文上書きにせず
/// `pending` だけを差分適用する（`command_tool_contract`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicy {
    /// 形式版数。
    #[serde(default = "policy_version")]
    pub version: u32,
    /// 承認なしで実行してよいパターン。
    #[serde(default)]
    pub allow: Vec<String>,
    /// 拒否するパターン。**`allow` に優先する。**
    #[serde(default)]
    pub deny: Vec<String>,
    /// どちらにも無かった呼び出し。人がここから `allow` か `deny` へ動かす。
    #[serde(default)]
    pub pending: Vec<PendingCommand>,
    /// このファイル全体のタイムアウト（秒）。
    ///
    /// **パターンごとには持たない** — 対応表がもう 1 つ増えると
    /// 「書いたとおりに効く」が壊れる（D8）。
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn policy_version() -> u32 {
    1
}

impl Default for CommandPolicy {
    /// **`command.json` を持たない個体の既定。**
    ///
    /// `allow` が空なので**何も実行できず、`run` はモデルへ提示すらされない** —
    /// これが fail closed（D10）。初回の手間は欠点ではなく仕様。
    fn default() -> Self {
        Self {
            version: policy_version(),
            allow: Vec::new(),
            deny: Vec::new(),
            pending: Vec::new(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

impl CommandPolicy {
    /// 1 回の呼び出しを判定する。**`deny` が `allow` に優先する。**
    pub fn decide(&self, command: &str, args: &[String]) -> Decision {
        let Some(head) = normalize_command(command) else {
            return Decision::Malformed;
        };
        let mut tokens = Vec::with_capacity(args.len() + 1);
        tokens.push(head);
        tokens.extend(args.iter().cloned());

        if self.deny.iter().any(|p| pattern_matches(p, &tokens)) {
            return Decision::Denied;
        }
        if self.allow.iter().any(|p| pattern_matches(p, &tokens)) {
            return Decision::Allowed;
        }
        Decision::Unknown
    }

    /// 上限で丸めたタイムアウト。
    pub fn timeout_secs(&self) -> u64 {
        self.timeout_secs.clamp(1, MAX_TIMEOUT_SECS)
    }

    /// `run` をモデルへ提示するか（2 段ゲートの 2 段目）。
    pub fn offers_anything(&self) -> bool {
        !self.allow.is_empty()
    }

    /// `pending` へ 1 件積む。
    ///
    /// 同一の `(command, args)` は `count++` で畳み、`first_requested_at_ms` は
    /// 最初のまま。上限を超えたら**最も古いものから**捨てる。
    ///
    /// `now_ms` は引数で受け取る（`schedule.rs` と同じ規律）。
    ///
    /// 戻りは捨てた要求の説明。空でなければ呼び出し側が WARN を出す。
    #[must_use = "捨てた要求を黙って消すと「頼んだのに画面に出ない」が見えなくなる"]
    pub fn note_pending(&mut self, command: &str, args: &[String], now_ms: u64) -> Option<String> {
        if let Some(existing) = self
            .pending
            .iter_mut()
            .find(|p| p.command == command && p.args == args)
        {
            existing.count = existing.count.saturating_add(1);
            return None;
        }

        self.pending.push(PendingCommand {
            command: command.to_owned(),
            args: args.to_vec(),
            first_requested_at_ms: now_ms,
            count: 1,
        });

        if self.pending.len() <= MAX_PENDING {
            return None;
        }
        let oldest = self
            .pending
            .iter()
            .enumerate()
            .min_by_key(|(_, p)| p.first_requested_at_ms)
            .map(|(index, _)| index)?;
        let removed = self.pending.remove(oldest);
        Some(format!(
            "登録要求が上限 {MAX_PENDING} 件を超えたため、最も古い `{}` を捨てました",
            removed.command
        ))
    }

    /// 人が編集した内容へ、機械が持っている `pending` だけを移す。
    ///
    /// **全文上書きにしない**ための経路。`allow` / `deny` / `timeout_secs` は
    /// **読み直した側（人の編集）を必ず勝たせる** — 機械が持っている古い値で
    /// 人の変更を潰さない。
    pub fn merge_pending_into(&self, fresh: &mut Self) {
        fresh.pending = self.pending.clone();
    }
}

/// `command` を照合できる形へ正規化する。**できなければ `None`。**
///
/// 3 段（`command_tool_contract`）:
/// 1. パス区切りを含むなら拒否
/// 2. 小文字化（Windows のファイル名は大文字小文字を区別しない）
/// 3. Windows では `PATHEXT` の拡張子を剥がす（`python` と `python.exe` を
///    別物にしない）
pub fn normalize_command(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() || trimmed.contains(['/', '\\', ':']) {
        return None;
    }
    let lowered = trimmed.to_lowercase();
    Some(strip_path_ext(&lowered))
}

/// `PATHEXT` の拡張子を剥がす（Windows のみ）。
fn strip_path_ext(lowered: &str) -> String {
    if !cfg!(windows) {
        return lowered.to_owned();
    }
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
    for ext in exts.split(';').filter(|e| !e.is_empty()) {
        let ext = ext.to_lowercase();
        if let Some(stem) = lowered.strip_suffix(&ext)
            && !stem.is_empty()
        {
            return stem.to_owned();
        }
    }
    lowered.to_owned()
}

/// パターンが argv トークン列に一致するか。
///
/// 構文は 2 つだけ — **完全一致**と**末尾のワイルドカード**。
/// `"ruff"` は引数なしにしか一致しない（任意引数を許すなら `"ruff *"`）。
///
/// 中間のワイルドカードは持たない。何個の引数に対応するかの規則が要り、
/// **書いた人の意図と照合結果がずれる場所が増える**。
fn pattern_matches(pattern: &str, tokens: &[String]) -> bool {
    let parts: Vec<&str> = pattern.split_whitespace().collect();
    if parts.is_empty() || tokens.is_empty() {
        return false;
    }
    // パターン側の先頭も同じ規則で正規化する（`Python.exe` と書かれても効く）。
    let Some(head) = normalize_command(parts[0]) else {
        return false;
    };
    if head != tokens[0] {
        return false;
    }

    let open = *parts.last().unwrap() == "*";
    let fixed = if open { &parts[1..parts.len() - 1] } else { &parts[1..] };

    if open {
        if tokens.len() - 1 < fixed.len() {
            return false;
        }
    } else if tokens.len() - 1 != fixed.len() {
        return false;
    }
    fixed.iter().zip(&tokens[1..]).all(|(p, t)| *p == t.as_str())
}

#[cfg(test)]
mod policy_tests {
    use super::*;

    fn policy(allow: &[&str], deny: &[&str]) -> CommandPolicy {
        CommandPolicy {
            allow: allow.iter().map(|s| (*s).to_owned()).collect(),
            deny: deny.iter().map(|s| (*s).to_owned()).collect(),
            ..CommandPolicy::default()
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn a_bare_pattern_matches_only_the_bare_call() {
        let p = policy(&["ruff"], &[]);
        assert_eq!(p.decide("ruff", &[]), Decision::Allowed);
        assert_eq!(
            p.decide("ruff", &args(&["check"])),
            Decision::Unknown,
            "引数なしにしか一致しないこと（任意引数を許すなら末尾のワイルドカード）"
        );
    }

    #[test]
    fn a_trailing_star_accepts_any_remaining_args() {
        let p = policy(&["ruff *"], &[]);
        assert_eq!(p.decide("ruff", &args(&["check", "src"])), Decision::Allowed);
        assert_eq!(p.decide("ruff", &[]), Decision::Allowed, "0 個でも一致する");
    }

    #[test]
    fn a_prefix_pattern_pins_the_leading_args() {
        let p = policy(&["ruff check *"], &[]);
        assert_eq!(p.decide("ruff", &args(&["check", "src"])), Decision::Allowed);
        assert_eq!(
            p.decide("ruff", &args(&["format", "src"])),
            Decision::Unknown,
            "固定部が違えば一致しない"
        );
        assert_eq!(p.decide("ruff", &args(&["check"])), Decision::Allowed);
    }

    #[test]
    fn deny_wins_over_allow() {
        let p = policy(&["git *"], &["git push *"]);
        assert_eq!(p.decide("git", &args(&["status"])), Decision::Allowed);
        assert_eq!(
            p.decide("git", &args(&["push", "origin"])),
            Decision::Denied,
            "両方に一致したら deny"
        );
    }

    #[test]
    fn an_unknown_call_is_neither_allowed_nor_denied() {
        assert_eq!(
            policy(&["ruff *"], &[]).decide("cargo", &args(&["test"])),
            Decision::Unknown
        );
    }

    #[test]
    fn a_command_with_path_separators_is_rejected_before_matching() {
        let p = policy(&["python *"], &[]);
        for bad in ["./python", "/usr/bin/python", "bin\\python", "C:python"] {
            assert_eq!(
                p.decide(bad, &[]),
                Decision::Malformed,
                "照合前に拒否すること（allow が指す対象が曖昧になる）"
            );
        }
    }

    #[test]
    fn matching_ignores_case() {
        let p = policy(&["Ruff *"], &[]);
        assert_eq!(p.decide("RUFF", &args(&["check"])), Decision::Allowed);
    }

    #[test]
    #[cfg(windows)]
    fn the_executable_extension_is_stripped_on_windows() {
        let p = policy(&["python *"], &[]);
        assert_eq!(p.decide("python.exe", &args(&["-V"])), Decision::Allowed);
        assert_eq!(p.decide("PYTHON.EXE", &args(&["-V"])), Decision::Allowed);
    }

    #[test]
    fn args_are_compared_literally_including_spaces() {
        let p = policy(&["echo a b"], &[]);
        assert_eq!(
            p.decide("echo", &args(&["a b"])),
            Decision::Unknown,
            "1 トークンの引数はパターンの 2 語と一致しない（シェルの引用規則を持ち込まない）"
        );
        assert_eq!(p.decide("echo", &args(&["a", "b"])), Decision::Allowed);
    }

    #[test]
    fn an_empty_policy_allows_nothing_and_offers_nothing() {
        let p = CommandPolicy::default();
        assert_eq!(p.decide("ruff", &[]), Decision::Unknown);
        assert!(
            !p.offers_anything(),
            "allow が空なら run は提示されない（fail closed）"
        );
    }

    #[test]
    fn the_same_call_is_folded_and_keeps_the_first_timestamp() {
        let mut p = CommandPolicy::default();
        assert!(p.note_pending("cargo", &args(&["test"]), 100).is_none());
        assert!(p.note_pending("cargo", &args(&["test"]), 900).is_none());
        assert_eq!(p.pending.len(), 1);
        assert_eq!(p.pending[0].count, 2);
        assert_eq!(p.pending[0].first_requested_at_ms, 100);
    }

    #[test]
    fn different_args_are_separate_pending_entries() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("cargo", &args(&["test"]), 100);
        let _ = p.note_pending("cargo", &args(&["build"]), 100);
        assert_eq!(p.pending.len(), 2, "何を許可すればよいかの材料を消さない");
    }

    #[test]
    fn the_oldest_pending_is_evicted_at_the_cap() {
        let mut p = CommandPolicy::default();
        for i in 0..MAX_PENDING {
            let _ = p.note_pending(&format!("cmd{i}"), &[], 1_000 + i as u64);
        }
        let warn = p.note_pending("overflow", &[], 9_999);
        assert_eq!(p.pending.len(), MAX_PENDING);
        assert!(
            warn.is_some_and(|w| w.contains("cmd0")),
            "最も古いものを捨てて、捨てたことを言うこと"
        );
        assert!(p.pending.iter().any(|x| x.command == "overflow"));
    }

    #[test]
    fn merging_keeps_the_human_edits_and_only_moves_pending() {
        let mut machine = CommandPolicy::default();
        let _ = machine.note_pending("cargo", &args(&["test"]), 100);
        machine.allow.push("古い".into());

        // 人が手で編集した側（読み直した結果）。
        let mut fresh = CommandPolicy { timeout_secs: 300, ..policy(&["ruff *"], &["rm *"]) };
        machine.merge_pending_into(&mut fresh);

        assert_eq!(fresh.allow, vec!["ruff *"], "人の編集を機械の古い値で潰さないこと");
        assert_eq!(fresh.deny, vec!["rm *"]);
        assert_eq!(fresh.timeout_secs, 300);
        assert_eq!(fresh.pending.len(), 1, "pending だけが移ること");
    }

    #[test]
    fn the_timeout_is_clamped_to_the_contract_range() {
        let too_long = CommandPolicy { timeout_secs: 99_999, ..CommandPolicy::default() };
        assert_eq!(too_long.timeout_secs(), MAX_TIMEOUT_SECS);
        let zero = CommandPolicy { timeout_secs: 0, ..CommandPolicy::default() };
        assert_eq!(zero.timeout_secs(), 1);
    }
}
