//! 終了コード（Spec 64 D10・`headless_host_contract` 凍結 10）。
//!
//! **終了コードは機械が読む結末、標準出力の本文は人が読む結末。** コンテナや CI が
//! 日本語の定型文を解析しなくても結末を読めるようにする。写像は純関数で持ち、
//! 単体で留める（実行経路に散らすと、1 つの結末が 2 つのコードに割れる）。

use fuseforks_core::CoreError;
use fuseforks_core::plan::PlanTaskState;
use fuseforks_host::{HostError, PreflightError};

/// `ask`: 答えが返った / `check`: 拒否 0 件 / `serve`: シグナルで正常に閉じた。
pub const OK: u8 = 0;
/// コアがエラーを返した（`code` と文面を 1 行）。
pub const CORE: u8 = 1;
/// 引数の誤り。
pub const USAGE: u8 = 2;
/// 起動前検査（D5）で拒否された。
pub const REJECTED: u8 = 3;
/// 村のロックが取れない / `sessions.redb` を別のプロセスが開いている。
pub const LOCKED: u8 = 4;
/// 組み立てに失敗した（`world.json` の破損・秘密の変数名の衝突 等）。
pub const BOOT: u8 = 5;
/// `ask`: 答えが返らなかった（`NoAnswer` / `Undeliverable`）。
pub const NO_ANSWER: u8 = 6;
/// `ask`: 待ちの上限を超えた（`TimedOut`）。
pub const TIMED_OUT: u8 = 7;
/// `ask`: 打ち切られた（`Interrupted` — Ctrl+C を含む）。
pub const INTERRUPTED: u8 = 8;
/// `ask`: 予算の天井で止まった（`BudgetExhausted`）。
pub const BUDGET: u8 = 9;
/// `bake`: `headers` に平文の鍵がある（Spec 65 D2。`bake` だけの番号は 10 番台）。
pub const PLAINTEXT_HEADERS: u8 = 10;
/// `bake`: 写し先の状態が食い違う（`--update` なしで空でない / `--update` で `bake.json` が無い）。
pub const OUT_STATE: u8 = 11;

/// `bake` の失敗 → 終了コード（Spec 65 D2 の表）。**Spec 64 の番号と意味を共有する** —
/// 引数の誤り 2・置き換え漏れ 3（検査で拒否）・ロック 4・読み書きの失敗 5。
pub fn for_bake_error(err: &fuseforks_host::bake::BakeError) -> u8 {
    use fuseforks_host::bake::BakeError;
    match err {
        BakeError::Usage(_) => USAGE,
        BakeError::Unmapped(_) => REJECTED,
        BakeError::Lock(_) => LOCKED,
        BakeError::Io(_) => BOOT,
        BakeError::PlaintextHeaders(_) => PLAINTEXT_HEADERS,
        BakeError::OutState(_) => OUT_STATE,
    }
}

/// 配送の結末 → 終了コード。
///
/// `Running` は確定していない状態で、`ask_external_outcome` からは返らないはず。
/// 返ったら「答えが返らなかった」として扱う（0 にすると、確定していないものを成功と読む）。
pub fn for_outcome(state: PlanTaskState) -> u8 {
    match state {
        PlanTaskState::Answered | PlanTaskState::HandedOff => OK,
        PlanTaskState::NoAnswer | PlanTaskState::Undeliverable | PlanTaskState::Running => {
            NO_ANSWER
        }
        PlanTaskState::TimedOut => TIMED_OUT,
        PlanTaskState::Interrupted => INTERRUPTED,
        PlanTaskState::BudgetExhausted => BUDGET,
    }
}

/// 組み立ての失敗 → 終了コード。**ロックの 2 つの網（D3 の OS ロック / 凍結 4 の
/// `sessions.redb`）はどちらも 4**、それ以外は 5。
pub fn for_host_error(err: &HostError) -> u8 {
    match err {
        HostError::Lock(_) | HostError::Core(CoreError::SessionStoreLocked { .. }) => LOCKED,
        HostError::Io(_) | HostError::SecretNameCollision(_) | HostError::Core(_) => BOOT,
    }
}

/// 起動前検査の材料が揃わなかった → 終了コード。`--start` の誤りは引数の誤り（2）、
/// それ以外は組み立ての失敗と同じ。
pub fn for_preflight_error(err: &PreflightError) -> u8 {
    match err {
        PreflightError::UnknownAgent(_) | PreflightError::ReceptionOnlyForAsk => USAGE,
        PreflightError::VillageMissing(_) => BOOT,
        PreflightError::Host(host) => for_host_error(host),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D10 の表をそのまま写す。結末 8 値はすべてどこかの行に落ちる。
    #[test]
    fn outcomes_map_to_the_d10_table() {
        assert_eq!(for_outcome(PlanTaskState::Answered), 0);
        assert_eq!(for_outcome(PlanTaskState::HandedOff), 0);
        assert_eq!(for_outcome(PlanTaskState::NoAnswer), 6);
        assert_eq!(for_outcome(PlanTaskState::Undeliverable), 6);
        assert_eq!(for_outcome(PlanTaskState::Running), 6);
        assert_eq!(for_outcome(PlanTaskState::TimedOut), 7);
        assert_eq!(for_outcome(PlanTaskState::Interrupted), 8);
        assert_eq!(for_outcome(PlanTaskState::BudgetExhausted), 9);
    }

    /// `bake` の失敗は Spec 64 の番号と意味を共有し、`bake` だけのものは 10 番台（Spec 65 D2 の表）。
    #[test]
    fn bake_errors_map_to_the_d2_table() {
        use fuseforks_host::bake::BakeError;
        assert_eq!(for_bake_error(&BakeError::Usage(String::new())), 2);
        assert_eq!(for_bake_error(&BakeError::Unmapped(Vec::new())), 3);
        let held = fuseforks_host::LockError::Held { path: "x".into() };
        assert_eq!(for_bake_error(&BakeError::Lock(held)), 4);
        assert_eq!(for_bake_error(&BakeError::Io(String::new())), 5);
        assert_eq!(for_bake_error(&BakeError::PlaintextHeaders(Vec::new())), 10);
        assert_eq!(for_bake_error(&BakeError::OutState(String::new())), 11);
    }

    /// ロックの 2 つの網はどちらも 4。組み立ての他の失敗は 5、`--start` の誤りは 2。
    #[test]
    fn boot_failures_split_between_locked_and_boot() {
        let held = HostError::Lock(fuseforks_host::LockError::Held {
            path: "x".into(),
        });
        assert_eq!(for_host_error(&held), 4);
        let redb = HostError::Core(CoreError::SessionStoreLocked { path: "x".into() });
        assert_eq!(for_host_error(&redb), 4);
        let collision = HostError::SecretNameCollision(Vec::new());
        assert_eq!(for_host_error(&collision), 5);
        assert_eq!(
            for_preflight_error(&PreflightError::UnknownAgent("ghost".into())),
            2
        );
        assert_eq!(
            for_preflight_error(&PreflightError::ReceptionOnlyForAsk),
            2
        );
        assert_eq!(
            for_preflight_error(&PreflightError::Host(collision)),
            5
        );
        assert_eq!(
            for_preflight_error(&PreflightError::VillageMissing("x".into())),
            5
        );
    }
}
