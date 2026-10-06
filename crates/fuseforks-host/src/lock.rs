//! 村の排他ロック（Spec 64 D3）— 同じ村を開けるのは 1 プロセスだけ。
//!
//! **ファイルの存在ではなく OS のロックで判定する。** `{workspace}/.fuseforks.lock` を
//! 作って `std::fs::File::try_lock` を取る。プロセスが落ちれば OS が外すので、強制終了の
//! 後に残ったファイルを人が消す手順が要らない（P0 実測: kill の後 3 OS とも 1 ms 以内に
//! 取れ、ファイルは残る）。
//!
//! **中身には何も書かない。** Windows の `LockFileEx` はロック中のファイルを他プロセスから
//! 読めなくする（P0 実測: open は通り read が os error 33）ので、PID を書いても相手は
//! 読めない。「誰が持っているか」は Unix なら `lsof <path>`、Windows なら Resource Monitor の
//! 「関連付けられたハンドル」でファイル名を引く。
//!
//! **ファイルは消さない。** Drop で unlink すると、同じパスを開いて待っていた相手が消えた
//! inode のロックを取り、その隙に新しいプロセスが新しいファイルを作って別のロックを取る —
//! 2 つのプロセスが両方「持っている」と思う形になる（lock file の unlink 競合）。
//! 解放は `File` の Drop（close）だけに任せる。
//!
//! `sessions.redb` も同じ機構（redb 自身が `File::try_lock`）で自分を守っている。この
//! ロックはその手前に置く 1 枚で、ロックを持たない `world.json` と `schedules.json` まで
//! 1 プロセスに閉じるためのもの。

use std::fs::{File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};

/// ロックファイルの名前（`{workspace}/.fuseforks.lock`）。
pub const LOCK_FILE: &str = ".fuseforks.lock";

/// 取れたロック。**Drop するまで村を持つ。**
#[derive(Debug)]
pub struct VillageLock {
    /// 握っている間だけロックが生きる。読み書きはしない。
    _file: File,
    path: PathBuf,
}

/// ロックが取れなかった理由。
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    /// 別のプロセス（Fuseforks の GUI か、fuseforks-cli）が同じ村を開いている。
    #[error("この村は別のプロセスが開いています（Fuseforks の GUI か、fuseforks-cli）: {path}")]
    Held {
        /// ロックファイルのパス。
        path: String,
    },
    /// ロックファイルを作れない・開けない・この OS がファイルロックを持たない。
    #[error("村のロック `{path}` を取れません: {source}")]
    Io {
        /// ロックファイルのパス。
        path: String,
        /// OS からの理由。
        #[source]
        source: std::io::Error,
    },
}

impl VillageLock {
    /// `{workspace}/.fuseforks.lock` の排他ロックを取る。**取れなければ待たずに返す。**
    ///
    /// # Errors
    /// 別のプロセスが持っていれば [`LockError::Held`]、ファイルを開けない・ロックが
    /// 使えない OS なら [`LockError::Io`]。どちらも村を開かない（fail closed）。
    pub fn acquire(workspace: &Path) -> Result<Self, LockError> {
        let path = workspace.join(LOCK_FILE);
        let shown = path.display().to_string();
        // Windows は append だけで開いたファイルをロックできない（std の doc）ので
        // read + write で開く。truncate はしない — 中身は無いが、相手が持っている
        // 間に切り詰めへ行かない。
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| LockError::Io {
                path: shown.clone(),
                source,
            })?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file, path }),
            Err(TryLockError::WouldBlock) => Err(LockError::Held { path: shown }),
            Err(TryLockError::Error(source)) => Err(LockError::Io {
                path: shown,
                source,
            }),
        }
    }

    /// ロックファイルのパス。
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "fuseforks-lock-{tag}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).expect("一時フォルダ");
        dir
    }

    /// 同じプロセスの別ハンドルでも取れない（P0 実測: 3 OS とも WouldBlock）。
    #[test]
    fn a_second_acquire_in_the_same_process_is_held() {
        let ws = temp_workspace("twice");
        let first = VillageLock::acquire(&ws).expect("1 回目");
        assert_eq!(first.path(), ws.join(LOCK_FILE));
        let second = VillageLock::acquire(&ws);
        assert!(
            matches!(second, Err(LockError::Held { .. })),
            "2 回目は Held: {second:?}"
        );
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// Drop で解放される（close に任せる。unlink はしない）。
    #[test]
    fn dropping_the_lock_releases_it_and_keeps_the_file() {
        let ws = temp_workspace("drop");
        let first = VillageLock::acquire(&ws).expect("1 回目");
        drop(first);
        let again = VillageLock::acquire(&ws).expect("Drop の後は取れる");
        assert!(ws.join(LOCK_FILE).exists(), "ファイルは残る");
        drop(again);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 中身を書かない（Windows では相手が読めないので、書いても意味が無い）。
    #[test]
    fn the_lock_file_stays_empty() {
        let ws = temp_workspace("empty");
        let lock = VillageLock::acquire(&ws).expect("取れる");
        let len = std::fs::metadata(lock.path()).expect("metadata").len();
        assert_eq!(len, 0);
        drop(lock);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 開けない場所は Io（Held と混ぜない — 直し方が違う）。
    #[test]
    fn an_unopenable_path_is_io_not_held() {
        let ws = temp_workspace("io");
        let not_a_dir = ws.join("file");
        std::fs::write(&not_a_dir, b"x").expect("ファイル");
        let result = VillageLock::acquire(&not_a_dir);
        assert!(matches!(result, Err(LockError::Io { .. })), "{result:?}");
        let _ = std::fs::remove_dir_all(&ws);
    }
}
