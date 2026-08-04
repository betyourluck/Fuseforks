//! コマンド実行の許容規則（Spec 15 rev4）。
//!
//! ここに置くのは**型と純機構**だけで、ファイルの読み書きは
//! [`crate::tools::run::RunTool`] が [`crate::config_store::ConfigStore`] 経由で行う
//! （`schedule.rs` と同じ「型 + 純機構」／「I/O」の分業）。
//!
//! # allow / deny / 未知 の 3 状態
//!
//! [`CommandPolicy`] は `agents/{id}/command.json` に住み、**`mcp.json` と同じ棚**で
//! 人が直す。allow に一致すれば承認なしで実行、deny に一致すれば拒否して
//! **承認要求も積まない**、どちらにも無ければ拒否して `pending` へ積む。
//!
//! # 照合は argv 配列に対して行う
//!
//! エージェントは `command` と `args` 配列を書く。**シェル文字列を受けないので、
//! `&&` も `|` も `$(...)` も構造的に存在しない** — allow / deny をパターンにしても、
//! 「パースで守る境界は必ず抜けられる」問題は戻ってこない。
//!
//! # ここは安全機構ではない
//!
//! allow に `python *` を入れた時点で任意コード実行を許している。deny も
//! **危険を列挙し切れない**（`rm` を弾いても `python -c` が残る）。
//! **deny の価値は「利用者が一度した判断を憶えておく」ことであって、
//! 敵対的な入力を止めることではない。**

use serde::{Deserialize, Serialize};

/// 壁時計を読む**唯一の場所**。
///
/// [`CommandPolicy::note_pending`] は `now_ms` を引数で受け取る（`schedule.rs` と
/// 同じ規律 — 内部で壁時計を読むと、テストが特定の時刻でだけ落ちるものになる）。
/// その引数を作るのは**呼び出しの縁**の仕事で、ここに 1 つだけ置く。
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// タイムアウトの既定値（秒）。
pub const DEFAULT_TIMEOUT_SECS: u64 = 60;

/// タイムアウトの上限（秒）。`cargo build` / `pytest` を締め出さない長さ。
pub const MAX_TIMEOUT_SECS: u64 = 3600;

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

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
mod tests {
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
