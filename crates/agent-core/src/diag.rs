//! 診断ログの出口。
//!
//! # なぜ stderr だけでは足りなかったか
//!
//! `[fuseforks]` 行は `eprintln!` で stderr にだけ出ていた。stderr は
//! `tauri dev` を起こした端末にしか残らないため、**利用者が貼らない限り
//! 誰も読めない**。実際、1 ターンで入力 730,406 トークンを使った経路の診断が、
//! ログを手で貼ってもらうまで進まなかった（2026-07-31）。
//!
//! ここは同じ行を**ファイルにも**落とす。stderr への出力は変えない
//! （端末で見ている側の見え方を変えない）。
//!
//! # 載せるもの・載せないもの
//!
//! 載せるのは `[fuseforks]` 行だけ — ターンの集計・ツール 1 本ごとの実測・
//! 予定の発火・plan の波。**プロンプト本文・ツール結果の本文・資格情報は
//! 載せない**。ログは平文で、ワークスペースを開けば誰でも読める場所に置く
//! （`world.json` と同じ扱い）。秘密の置き場は OS の資格情報ストアだけ、
//! という境界をここで崩さない（failures.md #1）。
//!
//! # 失敗しても呼び出し元を止めない
//!
//! 書き込みの失敗（ディスク満杯・権限・ファイルの削除）は握り潰す。
//! 診断のための機構がターンを落とすなら本末転倒で、しかも落ちるのは
//! **困っているとき**に限られる。

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// 1 ファイルの上限（バイト）。超えたら `.old` へ 1 世代だけ回す。
///
/// 世代を 1 つに固定するのは、**上限が「2 ファイルぶん」で言い切れる**ため。
/// 世代数を増やすと、消えないログが増えるのと同じことになる。
const MAX_BYTES: u64 = 8 * 1024 * 1024;

/// 開いているログファイル。
struct Sink {
    /// 書き込み先。回転のたびに開き直すので保持する。
    path: PathBuf,
    /// 追記で開いたハンドル。
    file: File,
    /// 現在のファイルサイズ（回転の判定に使う）。
    written: u64,
}

impl Sink {
    /// 追記で開く。親フォルダが無ければ作る。
    ///
    /// # Errors
    /// 親フォルダを作れない、またはファイルを開けない場合。
    fn open(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_owned(),
            file,
            written,
        })
    }

    /// 1 行書く。上限を超えていたら**書く前に**回転する。
    ///
    /// 回転に失敗したら同じファイルへ書き続ける。上限は行儀であって保証ではない
    /// — ここで諦めると、回転できない状況（ロック・権限）で診断が丸ごと消える。
    fn write_line(&mut self, line: &str) {
        if self.written >= MAX_BYTES {
            self.rotate();
        }
        let bytes = line.len() as u64 + 1;
        if writeln!(self.file, "{line}").is_ok() {
            self.written += bytes;
        }
    }

    /// `.old` へ 1 世代だけ回す。既存の `.old` は上書きされる。
    fn rotate(&mut self) {
        let old = PathBuf::from(format!("{}.old", self.path.display()));
        if std::fs::rename(&self.path, &old).is_err() {
            return;
        }
        match Self::open(&self.path) {
            Ok(fresh) => *self = fresh,
            // 開き直せなければ回転前のハンドルを使い続ける（rename 済みなので
            // 書き込み先は `.old` になるが、消えるよりはよい）。
            Err(_) => self.written = 0,
        }
    }
}

/// プロセスで 1 つだけ持つ出口。未設定なら stderr だけに出る。
static SINK: OnceLock<Mutex<Sink>> = OnceLock::new();

/// ログファイルを開く。**アプリ起動時に 1 回だけ呼ぶ。**
///
/// 2 回目以降は何もしない（最初に決まった宛先が勝つ）。テストや CLI から
/// 呼ばなければファイル出力は起きず、stderr の見え方も変わらない。
///
/// 開いた直後に区切りの 1 行を書く。1 つのファイルに複数回の起動が混ざるので、
/// **どこからが今回の起動か**が読めないと、古い行を現在の症状と読み違える。
///
/// # Errors
/// 親フォルダを作れない、またはファイルを開けない場合。
pub fn open_log(path: &Path) -> std::io::Result<()> {
    let sink = Sink::open(path)?;
    if SINK.set(Mutex::new(sink)).is_err() {
        return Ok(());
    }
    note("起動しました");
    Ok(())
}

/// 1 行出す。stderr へは必ず、ファイルへは開いていれば。
///
/// 呼び出し側は `[fuseforks]` を付けない（ここで付ける）。
/// 通常は [`note!`](crate::note) マクロ経由で呼ぶ。
pub fn note(line: &str) {
    eprintln!("[fuseforks] {line}");
    let Some(sink) = SINK.get() else {
        return;
    };
    // 毒された Mutex でも書き続ける。診断の出口が、別スレッドの panic を
    // 理由に黙るのは筋が悪い。
    let mut sink = match sink.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
    sink.write_line(&format!("{stamp} [fuseforks] {line}"));
}

/// 診断の 1 行を出す。`format!` と同じ書式を取る。
///
/// ```
/// agent_core::note!("turn: agent={} rounds={}", "agent_01", 3);
/// ```
#[macro_export]
macro_rules! note {
    ($($arg:tt)*) => {
        $crate::diag::note(&format!($($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// テスト用の一時ファイル。終了時に本体と `.old` を消す。
    struct TempLog(PathBuf);

    impl TempLog {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir()
                .join(format!("fuseforks-diag-{tag}-{}", std::process::id()))
                .join("fuseforks.log");
            Self(path)
        }
        fn read(&self) -> String {
            std::fs::read_to_string(&self.0).unwrap_or_default()
        }
        fn read_old(&self) -> String {
            std::fs::read_to_string(format!("{}.old", self.0.display())).unwrap_or_default()
        }
    }

    impl Drop for TempLog {
        fn drop(&mut self) {
            if let Some(parent) = self.0.parent() {
                let _ = std::fs::remove_dir_all(parent);
            }
        }
    }

    #[test]
    fn lines_are_appended_and_the_directory_is_created() {
        let temp = TempLog::new("append");
        let mut sink = Sink::open(&temp.0).expect("開けること");

        sink.write_line("1 行目");
        sink.write_line("2 行目");

        let body = temp.read();
        assert!(body.contains("1 行目") && body.contains("2 行目"), "{body}");
        assert_eq!(body.lines().count(), 2, "追記であって上書きではない");
    }

    /// 再度開いても既存の行を消さない（起動のたびに履歴が飛ぶと、
    /// 「落ちた直前」を読む用途で必ず空になる）。
    #[test]
    fn reopening_keeps_the_existing_lines() {
        let temp = TempLog::new("reopen");
        Sink::open(&temp.0).unwrap().write_line("前回の起動");
        Sink::open(&temp.0).unwrap().write_line("今回の起動");

        let body = temp.read();
        assert!(body.contains("前回の起動"), "{body}");
        assert!(body.contains("今回の起動"), "{body}");
    }

    /// 上限を超えたら `.old` へ 1 世代だけ回す。**世代は増やさない** —
    /// 上限が「2 ファイルぶん」で言い切れることが、この機構の唯一の保証。
    #[test]
    fn the_file_rotates_once_at_the_cap() {
        let temp = TempLog::new("rotate");
        let mut sink = Sink::open(&temp.0).unwrap();

        sink.write_line("回転前");
        // 実際に 8MB 書かずに上限へ到達させる（回転の判定は written だけを見る）。
        sink.written = MAX_BYTES;
        sink.write_line("回転後");

        assert_eq!(temp.read().trim(), "回転後", "新しいファイルは回転後の行だけ");
        assert_eq!(temp.read_old().trim(), "回転前", "古い行は .old に残る");
    }

    /// 出口を開いていなくても [`note`] は落ちない（テスト・CLI 経路）。
    #[test]
    fn note_without_a_sink_does_not_panic() {
        note("出口が無いときの 1 行");
    }
}
