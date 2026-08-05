//! コマンド実行の許容規則（Spec 15 rev4）。
//!
//! ここに置くのは**型と純機構**だけで、ファイルの読み書きは
//! [`crate::tools::run::RunTool`] が [`crate::config_store::ConfigStore`] 経由で行う
//! （`schedule.rs` と同じ「型 + 純機構」／「I/O」の分業）。
//!
//! # allow / deny / 未知 の 3 状態
//!
//! [`CommandPolicy`] は `agents/{id}/run.json` に住み、**`mcp.json` と同じ棚**で
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

impl PendingCommand {
    /// 承認・却下で `allow` / `deny` へ入れるパターン（Spec 20 D1）。
    ///
    /// - `open == false` … この呼び出しだけ（完全一致）
    /// - `open == true` … **第 1 引数までを固定**して、以降を任意にする
    ///
    /// **末尾 `*` は 0 個以上に一致する**ので、開いた側は完全一致の
    /// スーパーセットになる（`ruff *` は引数なしの `ruff` にも当たる）。
    /// この性質が崩れると「承認したのに通らない」が起きる。
    ///
    /// **「第 1 引数まで」は当てずっぽう**（`ls -la /tmp` は旗を固定する）。
    /// 隠さないために、**画面は結果の文字列そのものを出す**契約になっている。
    pub fn pattern(&self, open: bool) -> String {
        if !open {
            return std::iter::once(self.command.as_str())
                .chain(self.args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ");
        }
        match self.args.first() {
            None => format!("{} *", self.command),
            Some(first) => format!("{} {first} *", self.command),
        }
    }
}

/// 承認・却下の結果（Spec 20）。
///
/// **`NotFound` を黙って捨てない。** 押したのに何も起きない画面は、壊れているのか
/// 成功したのかを利用者が区別できない（`failures.md` #44 の「歯止めの先に道が
/// 無い」と同型）。画面まで運んで「もう一覧にありません」と告げる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalOutcome {
    /// `pending` から消し、`allow` / `deny` へ入れた。
    Applied,
    /// 該当の要求が `pending` に無かった。**許容は 1 行も増えていない。**
    NotFound,
}
/// 承認画面へ渡す 1 体分の投影（Spec 20）。
///
/// **`broken` を持つのは、読めなかったことを画面で言うため。** 既定を返して
/// 「判断待ちゼロ」に見せると、**壊れている事実が画面から消える**。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandPolicyView {
    /// 対象のサーヴァント。
    pub agent_id: crate::model::AgentId,
    /// 表示名（画面は ID ではなく名前で束ねる）。
    pub name: String,
    /// 判断待ちの要求。読めなかった場合は空。
    pub pending: Vec<PendingCommand>,
    /// `run.json` が壊れていて読めなかった。
    pub broken: bool,
}
/// `agents/{id}/run.json` の中身。
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
    /// **`run.json` を持たない個体の既定。**
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
        // 人が手で `allow` / `deny` を足していたら、その時点で決着している。
        self.prune_settled();
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

    /// 決着済みの判断待ちを落とす（Spec 20 P4 の実機指摘）。
    ///
    /// **`pending` に残ってよいのは、いま判定して `Unknown` のものだけ。**
    /// 広いパターンで 1 件承認すると、その `allow` 行が**別の判断待ちを覆う**が、
    /// 覆われた側は誰も消さないので一覧に残り続けていた。実機のログで
    /// `allow=5 のまま decision=Allowed / pending=1` が観測され、`allow` には
    /// `curl -sI *` と `curl -sI https://www.yahoo.co.jp/` が並んだ —
    /// **もう許可済みの要求を承認して、冗長な行を増やしていた。**
    ///
    /// 判定は既存の [`Self::decide`] をそのまま使う。**新しい規則を作らない** —
    /// 「一覧に出るべきか」と「実行してよいか」は同じ問いの裏表なので、
    /// 別の述語を置くと 2 つの答えがずれる余地が生まれる。
    ///
    /// 人が `run.json` を手で編集して `allow` を足した場合も、次に `run` が
    /// 呼ばれた時点で掃除される（`note_pending` からも呼ぶ）。
    fn prune_settled(&mut self) {
        let settled: Vec<usize> = self
            .pending
            .iter()
            .enumerate()
            .filter(|(_, p)| self.decide(&p.command, &p.args) != Decision::Unknown)
            .map(|(i, _)| i)
            .collect();
        for index in settled.into_iter().rev() {
            self.pending.remove(index);
        }
    }

    /// 承認: `pending` の 1 件を消し、パターンを `allow` へ足す（Spec 20）。
    pub fn approve(&mut self, command: &str, args: &[String], open: bool) -> ApprovalOutcome {
        self.resolve(command, args, open, true)
    }

    /// 却下: `pending` の 1 件を消し、パターンを `deny` へ足す（Spec 20）。
    ///
    /// **`pending` から消すだけにしない。** 消すだけだと次の呼び出しでまた積まれ、
    /// 同じものを何度も却下することになる（`Decision::Denied` の doc が正）。
    pub fn reject(&mut self, command: &str, args: &[String], open: bool) -> ApprovalOutcome {
        self.resolve(command, args, open, false)
    }

    /// 承認と却下の共通実装。**入れる先が違うだけ**なので 1 つに畳む。
    fn resolve(
        &mut self,
        command: &str,
        args: &[String],
        open: bool,
        allow_it: bool,
    ) -> ApprovalOutcome {
        // 照合と同じ正規化を通す。`pending` へ手で書かれたパス区切り入りを
        // `allow` へ移すと、**何にも一致しない死んだ行**が増える（D8）。
        if normalize_command(command).is_none() {
            return ApprovalOutcome::NotFound;
        }
        let Some(index) = self
            .pending
            .iter()
            .position(|p| p.command == command && p.args == args)
        else {
            // **`pending` に無いものは許容へ入れない**（D9）。承認画面は
            // 「実際に要求されたものだけ」を許容へ変える装置で、ここが緩むと
            // 閉じた許容が「GUI から任意に書ける許容」になる。
            return ApprovalOutcome::NotFound;
        };
        let pattern = self.pending[index].pattern(open);
        self.pending.remove(index);
        let target = if allow_it { &mut self.allow } else { &mut self.deny };
        if !target.iter().any(|p| p == &pattern) {
            target.push(pattern);
        }
        // **足したパターンが覆う判断待ちを、ここで落とす。** 残すと「もう許可
        // 済みの要求」が一覧に並び、押せば冗長な行が増える（実機で起きた）。
        self.prune_settled();
        ApprovalOutcome::Applied
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

    // **`*` 単独のパターンは何にも一致させない。** 理由は 2 つ:
    //
    // - `parts.len() == 1` だと下の `&parts[1..parts.len() - 1]` が
    //   `&parts[1..0]` になり **panic する**（start > end）。到達条件は
    //   「コマンド自体が `*`」で、モデルが書けば起こる
    // - 仮に通したら「何でも許す」= **開いた許容**になり、閉じた許容という
    //   設計の本体と正面から衝突する。落とすのが正しい挙動でもある
    if parts.len() == 1 && parts[0] == "*" {
        return false;
    }

    let open = *parts.last().unwrap() == "*";
    // `*` は **0 個以上**の引数に一致する（`ruff *` は引数なしの `ruff` にも当たる）。
    // つまり開いたパターンは完全一致のスーパーセットで、承認の 2 択が
    // 「狭い / 広い」として成立する。
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

    /// **`*` は 0 個以上の引数に一致する。**
    ///
    /// ここが 1 個以上だと、`ruff *` を承認したのに `ruff` 自体が通らない。
    /// 承認の 2 択（完全一致 / 末尾 `*`）は**広いほうが狭いほうのスーパーセット**
    /// であることが前提なので、この性質が崩れると 2 択の意味が壊れる。
    #[test]
    fn a_trailing_star_matches_zero_or_more_args() {
        let p = policy(&["ruff *"], &[]);
        assert_eq!(p.decide("ruff", &[]), Decision::Allowed, "引数なしにも当たる");
        assert_eq!(p.decide("ruff", &args(&["check"])), Decision::Allowed);
        assert_eq!(
            p.decide("ruff", &args(&["check", "src/"])),
            Decision::Allowed,
            "個数は問わない"
        );
    }

    /// `*` 単独のパターンは**何にも一致せず、panic もしない**。
    ///
    /// 旧実装は `&parts[1..0]` で落ちた（要素 1 個のとき先頭と末尾が同じになる）。
    /// 落とす先を「一致しない」にしたのは、通せば**開いた許容**になるため。
    #[test]
    fn a_bare_star_pattern_matches_nothing_and_never_panics() {
        let p = policy(&["*"], &[]);
        assert_eq!(p.decide("*", &[]), Decision::Unknown, "自分自身にも当たらない");
        assert_eq!(p.decide("git", &args(&["log"])), Decision::Unknown);

        // deny 側でも同じ（deny が先に評価されるので、こちらも落ちない）。
        let d = policy(&[], &["*"]);
        assert_eq!(d.decide("*", &[]), Decision::Unknown);
    }
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

    /// 承認の 2 択が生む文字列（D1 の表そのもの）。
    #[test]
    fn the_pattern_follows_the_two_choices() {
        let p = |cmd: &str, a: &[&str]| PendingCommand {
            command: cmd.to_owned(),
            args: a.iter().map(|s| (*s).to_owned()).collect(),
            first_requested_at_ms: 0,
            count: 1,
        };
        assert_eq!(p("ruff", &[]).pattern(false), "ruff");
        assert_eq!(p("ruff", &[]).pattern(true), "ruff *");
        assert_eq!(
            p("git", &["log", "--oneline", "-5"]).pattern(false),
            "git log --oneline -5"
        );
        assert_eq!(p("git", &["log", "--oneline", "-5"]).pattern(true), "git log *");
        assert_eq!(p("cargo", &["test", "--", "--nocapture"]).pattern(true), "cargo test *");
    }

    /// 承認は `pending` から消して `allow` へ入れ、却下は `deny` へ入れる。
    #[test]
    fn approving_moves_one_request_and_rejecting_fills_deny() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("git", &args(&["log", "--oneline"]), 1);
        let _ = p.note_pending("rm", &args(&["-rf", "/"]), 2);

        assert_eq!(
            p.approve("git", &args(&["log", "--oneline"]), true),
            ApprovalOutcome::Applied
        );
        assert_eq!(p.allow, vec!["git log *"]);
        assert_eq!(p.pending.len(), 1, "承認した 1 件だけが消える");

        assert_eq!(
            p.reject("rm", &args(&["-rf", "/"]), false),
            ApprovalOutcome::Applied
        );
        assert_eq!(p.deny, vec!["rm -rf /"]);
        assert!(p.pending.is_empty());

        // 却下したものは deny に載るので、次に呼ばれても pending へは積まれない。
        assert_eq!(p.decide("rm", &args(&["-rf", "/"])), Decision::Denied);
    }

    /// **`pending` に無いものは許容へ入れない**（D9）。
    ///
    /// 押し出しや積み替えで消えた後に押されても、`allow` は 1 行も増えない。
    /// ここが緩むと、閉じた許容が「GUI から任意に書ける許容」になる。
    #[test]
    fn an_unknown_request_changes_nothing_and_says_so() {
        let mut p = policy(&["ruff *"], &["rm *"]);
        assert_eq!(p.approve("curl", &args(&["https://example.com"]), true), ApprovalOutcome::NotFound);
        assert_eq!(p.reject("curl", &[], false), ApprovalOutcome::NotFound);
        assert_eq!(p.allow, vec!["ruff *"], "許容は 1 行も増えない");
        assert_eq!(p.deny, vec!["rm *"]);

        // 引数まで含めて一致しないと消せない（`git log` と `git log -5` は別）。
        let _ = p.note_pending("git", &args(&["log"]), 1);
        assert_eq!(p.approve("git", &args(&["log", "-5"]), false), ApprovalOutcome::NotFound);
        assert_eq!(p.pending.len(), 1);
    }

    /// パス区切りを含む `command` は承認できない（D8）。
    ///
    /// 機械は積まないが、人が `run.json` へ手で書ける。通すと**何にも一致しない
    /// 死んだ行**が `allow` に増え、「書いてあるのに拒否され続ける」状態になる。
    #[test]
    fn a_path_like_command_cannot_be_approved() {
        let mut p = CommandPolicy::default();
        p.pending.push(PendingCommand {
            command: "./python".to_owned(),
            args: Vec::new(),
            first_requested_at_ms: 1,
            count: 1,
        });
        assert_eq!(p.approve("./python", &[], false), ApprovalOutcome::NotFound);
        assert!(p.allow.is_empty());
        assert_eq!(p.pending.len(), 1, "消しもしない（判断できないものは残す）");
    }

    /// **広いパターンで承認すると、それが覆う判断待ちも一覧から消える。**
    ///
    /// 実機で踏んだ形（2026-08-05）: `curl -sI https://…` が判断待ちに残ったまま
    /// 別の要求を `curl -sI *` で承認したので、**もう許可済みの要求が一覧に
    /// 並び続け**、押すと `allow` に冗長な行が増えた。
    #[test]
    fn approving_broadly_also_clears_the_requests_it_now_covers() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("curl", &args(&["-sI", "https://a.example"]), 1);
        let _ = p.note_pending("curl", &args(&["-sI", "https://b.example"]), 2);
        let _ = p.note_pending("git", &args(&["log"]), 3);
        assert_eq!(p.pending.len(), 3);

        // 1 件目を「先頭に続く任意の引数」で承認する。
        assert_eq!(
            p.approve("curl", &args(&["-sI", "https://a.example"]), true),
            ApprovalOutcome::Applied
        );
        assert_eq!(p.allow, vec!["curl -sI *"]);
        assert_eq!(
            p.pending.len(),
            1,
            "覆われた curl の要求も消え、無関係な git だけが残る"
        );
        assert_eq!(p.pending[0].command, "git");
    }

    /// 却下でも同じ（`deny` が覆う判断待ちは消える）。
    #[test]
    fn rejecting_broadly_also_clears_the_requests_it_now_covers() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("rm", &args(&["-rf", "/tmp/a"]), 1);
        let _ = p.note_pending("rm", &args(&["-rf", "/tmp/b"]), 2);

        assert_eq!(
            p.reject("rm", &args(&["-rf", "/tmp/a"]), true),
            ApprovalOutcome::Applied
        );
        assert_eq!(p.deny, vec!["rm -rf *"]);
        assert!(p.pending.is_empty(), "覆われた側も消える");
    }

    /// 人が手で `allow` を足していたら、次の `note_pending` で掃除される。
    ///
    /// **画面を開かなくても一覧が自己修復する。** 直接編集の経路を残している以上、
    /// そちらで決着した要求が残り続けると一覧が信用できなくなる。
    #[test]
    fn a_hand_written_allow_settles_the_pending_on_the_next_call() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("ruff", &args(&["check"]), 1);
        assert_eq!(p.pending.len(), 1);

        // 人が run.json を直接編集した、という状況。
        p.allow.push("ruff *".into());

        let _ = p.note_pending("curl", &args(&["https://example.com"]), 2);
        assert_eq!(p.pending.len(), 1, "決着済みの ruff は落ちる");
        assert_eq!(p.pending[0].command, "curl");
    }

    /// 同じパターンを二度承認しても `allow` は増えない。
    #[test]
    fn the_same_pattern_is_not_added_twice() {
        let mut p = CommandPolicy::default();
        let _ = p.note_pending("git", &args(&["log"]), 1);
        assert_eq!(p.approve("git", &args(&["log"]), true), ApprovalOutcome::Applied);
        let _ = p.note_pending("git", &args(&["log", "-5"]), 2);
        assert_eq!(p.approve("git", &args(&["log", "-5"]), true), ApprovalOutcome::Applied);
        assert_eq!(p.allow, vec!["git log *"], "同じ文字列は 1 行だけ");
    }

    #[test]
    fn the_timeout_is_clamped_to_the_contract_range() {
        let too_long = CommandPolicy { timeout_secs: 99_999, ..CommandPolicy::default() };
        assert_eq!(too_long.timeout_secs(), MAX_TIMEOUT_SECS);
        let zero = CommandPolicy { timeout_secs: 0, ..CommandPolicy::default() };
        assert_eq!(zero.timeout_secs(), 1);
    }
}
