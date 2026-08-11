//! 予定の前判定（Spec 28）を**この端末で実行してよいか**の記録。
//!
//! ## なぜ workspace の外に置くのか
//!
//! `schedules.json` は**村の内容物**なので配布に載る。承認を同じ場所へ書くと
//! **承認ごと配られ**、他人が用意したコマンドが受け取った側で黙って走る。
//! 攻撃者が書けるファイルに承認を書いても防御にならない。
//!
//! ゆえに置き場は `{app_data_dir}`（Spec 25 が合鍵で使った棚と同じ）。
//! **「村を配っても承認は付いてこない」が構造で成立する。**
//!
//! ## 保存するのはハッシュだけ
//!
//! コマンド行の原文は書かない。書くと、このファイルが
//! **「この端末で実行できるコマンドの一覧」**になる。承認ダイアログが表示する
//! 原文は `schedules.json` 側から読めば足りるので、**守る対象そのものを消す。**
//!
//! ## 書くのは人のクリックだけ
//!
//! 承認を足す関数は `tauri::command` の層からしか呼べない（`pub` にせず
//! モジュール内へ閉じ、公開するのは `AppState` 経由の 3 本だけ）。
//! **モデルが `schedules.json` を `file write` で書いても承認は付かない** —
//! 「呼ばない約束」ではなく「呼べない構造」で守る。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use fuseforks_core::orchestrator::ProbeApprovals;
use fuseforks_core::schedule::ScheduledTask;
use serde::{Deserialize, Serialize};

/// 承認ファイルの名前（`{app_data_dir}/probe_approvals.json`）。
pub const APPROVALS_FILE: &str = "probe_approvals.json";

/// ファイルの中身。**欄は 1 つだけ。**
///
/// 増やしたくなったら、それは承認以外の何かを持ち込もうとしている合図。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Stored {
    /// 承認済みの鍵（`SHA-256` の 16 進小文字 64 桁）。
    hashes: Vec<String>,
}

/// 端末側の承認台帳。
pub struct ApprovalStore {
    path: PathBuf,
    hashes: std::sync::RwLock<HashSet<String>>,
    /// 読めなかった理由。**読めない間は書き込みを拒む。**
    ///
    /// 既定を書き戻すと、壊れる前に人が承認した記録を捨てることになる
    /// （`failures.md` #70 — 読みの安全側と書きの安全側は別）。
    blocked: Option<String>,
}

impl ApprovalStore {
    /// 承認ファイルを読み込む。**読めなければ空**（＝どの前判定も走らない）。
    pub fn load(app_data_dir: &Path) -> Self {
        let path = app_data_dir.join(APPROVALS_FILE);
        let (hashes, blocked) = match std::fs::read_to_string(&path) {
            Ok(raw) => match serde_json::from_str::<Stored>(&raw) {
                Ok(stored) => (stored.hashes.into_iter().collect(), None),
                Err(err) => {
                    // 空として扱うのは**安全側**（全部の前判定が unapproved になる
                    // だけで、危険側へは倒れない）。ただし書き戻さない。
                    fuseforks_core::note!(
                        "WARN probe approvals: {APPROVALS_FILE} が読めません（前判定は実行されません）: {err}"
                    );
                    (HashSet::new(), Some(err.to_string()))
                }
            },
            // 未作成は「まだ 1 件も承認していない」— 異常ではない。
            Err(_) => (HashSet::new(), None),
        };
        Self {
            path,
            hashes: std::sync::RwLock::new(hashes),
            blocked,
        }
    }

    /// 承認を足して保存する。**呼べるのは保存 IPC だけ。**
    ///
    /// # Errors
    /// ファイルが読めていない、または書き込みに失敗した場合。
    pub fn approve(&self, key: String) -> Result<(), String> {
        self.mutate(|set| {
            set.insert(key);
        })
    }

    /// いま使われていない承認を落とす。
    ///
    /// 時計で切らないのは、判定に壁時計を持ち込むとテストが時刻に依存するため
    /// （`schedule.rs` の「内部で `Local::now()` を呼ばない」と同じ規律）。
    /// **予定を消せば承認も消える**ので、肥大化はこれで止まる。
    ///
    /// **呼び出し元は 2 つ。総数ではなく列挙する**（`create_schedule` /
    /// `delete_schedule`。増えたら行を足すことが更新になる — `failures.md` #67）。
    /// 初版は「保存のたびに呼ぶ」とだけ書いて `create_schedule` からしか
    /// 呼んでおらず、**doc の「予定を消せば承認も消える」が嘘だった**
    /// （`failures.md` #88）。**この関数の単体テストは呼ばれていることを
    /// 1 ミリも保証しない** ので、配線は `schedule_probe_approval_wiring.rs` が留める。
    ///
    /// # Errors
    /// ファイルが読めていない、または書き込みに失敗した場合。
    pub fn retain_for(&self, tasks: &[ScheduledTask], village_id: &str) -> Result<(), String> {
        let live: HashSet<String> = tasks
            .iter()
            .filter_map(|task| task.probe.as_ref())
            .map(|probe| probe.approval_key(village_id))
            .collect();
        self.mutate(|set| set.retain(|key| live.contains(key)))
    }

    /// 読み書きの共通経路。**メモリを直してからディスクへ書く。**
    fn mutate(&self, edit: impl FnOnce(&mut HashSet<String>)) -> Result<(), String> {
        if let Some(reason) = &self.blocked {
            return Err(format!(
                "{APPROVALS_FILE} が読めないため保存できません。ファイルを直すか削除してください（{reason}）"
            ));
        }
        let snapshot = {
            let mut guard = match self.hashes.write() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            edit(&mut guard);
            // **並びを固定して書く。** 同じ内容で保存し直したときに
            // ファイルが変わると、差分が「何か変わった」と嘘をつく。
            let mut hashes: Vec<String> = guard.iter().cloned().collect();
            hashes.sort_unstable();
            Stored { hashes }
        };
        let raw = serde_json::to_string_pretty(&snapshot).map_err(|err| err.to_string())?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
        }
        std::fs::write(&self.path, raw).map_err(|err| err.to_string())?;
        restrict_permissions(&self.path);
        Ok(())
    }
}

impl ProbeApprovals for ApprovalStore {
    fn is_approved(&self, key: &str) -> bool {
        match self.hashes.read() {
            Ok(guard) => guard.contains(key),
            // 毒されたロックで承認を通さない。**読めないなら走らせない側へ倒す。**
            Err(poisoned) => poisoned.into_inner().contains(key),
        }
    }
}

/// 承認ファイルを本人だけが読める権限にする（Unix のみ）。
///
/// **Windows では何もしない** — `app_data_dir` は既に利用者ごとに分かれており、
/// ACL を自前で組むと「効いているつもりで効いていない」を作りやすい
/// （`mcp_server.rs` と同じ判断）。**中身がハッシュだけなのは、この前提の保険**。
fn restrict_permissions(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(err) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            fuseforks_core::note!("probe approvals: 承認ファイルの権限を絞れませんでした（{err}）");
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

#[cfg(test)]
mod tests {
    use super::*;
    use fuseforks_core::schedule::{Recurrence, ScheduledTask};
    use fuseforks_core::schedule_probe::ScheduleProbe;

    fn temp_dir(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "fuseforks-approvals-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn probe(command: &str) -> ScheduleProbe {
        ScheduleProbe {
            command: command.to_owned(),
            args: vec!["watch.py".to_owned()],
            expect: "CHANGED".to_owned(),
            timeout_secs: 60,
            cwd: None,
        }
    }

    fn task_with(probe: Option<ScheduleProbe>) -> ScheduledTask {
        ScheduledTask {
            id: "t1".to_owned(),
            to: fuseforks_core::model::AgentId::from("agent_01"),
            message: "見張って".to_owned(),
            recurrence: Recurrence::Interval { every_minutes: 5 },
            created_at_ms: 0,
            last_consumed_due_ms: None,
            enabled: true,
            probe,
            session_mode: fuseforks_core::schedule_probe::SessionMode::Continue,
            summarize_after: false,
        }
    }

    #[test]
    fn an_unknown_key_is_not_approved() {
        let dir = temp_dir("unknown");
        let store = ApprovalStore::load(&dir);
        assert!(!store.is_approved("deadbeef"));
    }

    #[test]
    fn an_approval_survives_a_reload() {
        let dir = temp_dir("reload");
        let key = probe("python").approval_key("v1");
        {
            let store = ApprovalStore::load(&dir);
            store.approve(key.clone()).unwrap();
            assert!(store.is_approved(&key));
        }
        let reopened = ApprovalStore::load(&dir);
        assert!(reopened.is_approved(&key), "承認は起動をまたいで残ること");
    }

    #[test]
    fn only_hashes_are_written_to_disk() {
        // **原文が残ると、このファイルが実行可能なコマンドの一覧になる。**
        let dir = temp_dir("hashonly");
        let store = ApprovalStore::load(&dir);
        store.approve(probe("python").approval_key("v1")).unwrap();

        let raw = std::fs::read_to_string(dir.join(APPROVALS_FILE)).unwrap();
        assert!(!raw.contains("python"), "コマンド名が残ってはいけない: {raw}");
        assert!(!raw.contains("watch.py"), "引数が残ってはいけない: {raw}");
        assert!(raw.contains("hashes"), "{raw}");
    }

    #[test]
    fn cleanup_drops_approvals_no_schedule_uses() {
        let dir = temp_dir("cleanup");
        let store = ApprovalStore::load(&dir);
        let live = probe("python").approval_key("v1");
        let stale = probe("curl").approval_key("v1");
        store.approve(live.clone()).unwrap();
        store.approve(stale.clone()).unwrap();

        store
            .retain_for(&[task_with(Some(probe("python")))], "v1")
            .unwrap();

        assert!(store.is_approved(&live), "使われている承認は残ること");
        assert!(
            !store.is_approved(&stale),
            "どの予定も使わない承認は落ちること（肥大化を時計ではなく参照で止める）"
        );
    }

    #[test]
    fn cleanup_with_no_probes_empties_the_ledger() {
        let dir = temp_dir("cleanup-empty");
        let store = ApprovalStore::load(&dir);
        store.approve(probe("python").approval_key("v1")).unwrap();
        store.retain_for(&[task_with(None)], "v1").unwrap();
        assert!(!store.is_approved(&probe("python").approval_key("v1")));
    }

    #[test]
    fn a_broken_file_blocks_writing_instead_of_overwriting_it() {
        // 読めないファイルへ既定を書き戻すと、人が承認した記録を捨てる（#70）。
        let dir = temp_dir("broken");
        std::fs::write(dir.join(APPROVALS_FILE), "{ これは JSON ではない").unwrap();

        let store = ApprovalStore::load(&dir);
        assert!(!store.is_approved("anything"), "読めない間は誰も承認されない");
        let err = store.approve("abc".to_owned()).unwrap_err();
        assert!(err.contains(APPROVALS_FILE), "{err}");

        // ファイルは 1 バイトも変わっていない。
        let raw = std::fs::read_to_string(dir.join(APPROVALS_FILE)).unwrap();
        assert_eq!(raw, "{ これは JSON ではない");
    }

    #[test]
    fn saving_the_same_set_twice_produces_the_same_bytes() {
        // 並びが揺れると、差分が「何か変わった」と嘘をつく。
        let dir = temp_dir("stable");
        let store = ApprovalStore::load(&dir);
        store.approve(probe("python").approval_key("v1")).unwrap();
        store.approve(probe("curl").approval_key("v1")).unwrap();
        let first = std::fs::read_to_string(dir.join(APPROVALS_FILE)).unwrap();

        store.approve(probe("curl").approval_key("v1")).unwrap();
        let second = std::fs::read_to_string(dir.join(APPROVALS_FILE)).unwrap();
        assert_eq!(first, second);
    }
}
