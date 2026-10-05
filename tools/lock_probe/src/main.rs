//! Spec 64 P0 の測定 — `std::fs::File::try_lock` と redb 4.1 の `Database::create` が、
//! 同じプロセス・別プロセス・殺したプロセスの 3 通りでどう振る舞うかを 3 OS で確かめる。
//!
//! **予測を先に書いてから撃つ**（Spec 48 P0 と同じ作法）。各場面の `expected` は
//! 観測の前に書いた値で、外れた行は `UNEXPECTED` と出る。
//!
//! 使い方: 引数なしで全場面を回す。`hold <lock|db> <path> <ready>` は自分自身を
//! 子プロセスとして起こすときの入口（資源を取って `ready` を作り、標準入力が閉じるか
//! 1 行届くまで持ち続ける）。

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use redb::{Database, DatabaseError};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("hold") {
        hold(&args[2], Path::new(&args[3]), Path::new(&args[4]));
        return;
    }
    let unexpected = probe();
    if unexpected > 0 {
        std::process::exit(1);
    }
}

// ---------------------------------------------------------------- 子プロセス側

fn hold(kind: &str, path: &Path, ready: &Path) {
    let _lock_file;
    let _db;
    match kind {
        "lock" => {
            let file = open_rw(path);
            // ロック中のファイルを他プロセスが読めるかを測るため、中身を 1 行置く
            // （D3 の「PID を書いても相手は読めない」の検証用。本番では書かない）。
            (&file).write_all(b"holder\n").expect("write marker");
            if let Err(err) = file.try_lock() {
                eprintln!("holder: try_lock failed: {}", describe_lock(Err(err)));
                std::process::exit(2);
            }
            _lock_file = Some(file);
            _db = None;
        }
        "db" => match Database::create(path) {
            Ok(db) => {
                _db = Some(db);
                _lock_file = None;
            }
            Err(err) => {
                eprintln!("holder: Database::create failed: {err}");
                std::process::exit(2);
            }
        },
        other => {
            eprintln!("holder: unknown kind {other}");
            std::process::exit(2);
        }
    }
    File::create(ready).expect("create ready marker");
    // 親が標準入力へ 1 行書くか閉じるまで持ち続ける。殺される場面ではここで死ぬ。
    let mut line = String::new();
    let _ = std::io::stdin().lock().read_line(&mut line);
}

// ---------------------------------------------------------------- 親プロセス側

struct Report {
    unexpected: usize,
}

impl Report {
    fn line(&mut self, name: &str, result: &str, expected: &str) {
        let verdict = if result == expected { "OK" } else { "UNEXPECTED" };
        if verdict == "UNEXPECTED" {
            self.unexpected += 1;
        }
        println!("{name}: result={result} expected={expected} [{verdict}]");
    }
    fn info(&self, name: &str, value: &str) {
        println!("{name}: {value}");
    }
}

fn probe() -> usize {
    let dir = std::env::temp_dir().join(format!("lock_probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    let mut r = Report { unexpected: 0 };
    r.info("os", std::env::consts::OS);
    r.info("dir", &dir.display().to_string());

    let win = cfg!(windows);

    // ---- L: ロックファイル --------------------------------------------------
    let lock_path = dir.join("probe.lock");

    // L1. 同じハンドルで try_lock を 2 回。
    //     予測: Unix は flock の再取得で Ok / Windows は同じ範囲の再ロックが
    //     ERROR_LOCK_VIOLATION → WouldBlock。std の doc も「未規定・OS 依存」と書く。
    {
        let f = open_rw(&lock_path);
        let first = describe_lock(f.try_lock());
        let second = describe_lock(f.try_lock());
        r.line("L1a same-handle first", &first, "Ok");
        r.line(
            "L1b same-handle second",
            &second,
            if win { "WouldBlock" } else { "Ok" },
        );
        drop(f);
    }

    // L2. 同じプロセスの別ハンドル。予測: 3 OS とも WouldBlock
    //     （flock は open file description ごと / LockFileEx はハンドルごと）。
    {
        let a = open_rw(&lock_path);
        let b = open_rw(&lock_path);
        r.line("L2a two-handles first", &describe_lock(a.try_lock()), "Ok");
        r.line("L2b two-handles second", &describe_lock(b.try_lock()), "WouldBlock");
        // L6. 持ち主を Drop すると取れる。予測: Ok（Drop で close → 解放）。
        drop(a);
        r.line("L6 after-drop same-process", &describe_lock(b.try_lock()), "Ok");
        drop(b);
    }

    // L3. 別プロセスが持っている。予測: WouldBlock。
    // L7. その間に読めるか。予測: Unix は open も read も Ok（advisory）/
    //     Windows は open は Ok・read は Err（LockFileEx は読みも拒む）。
    {
        let ready = dir.join("ready-lock-1");
        let mut child = spawn_holder("lock", &lock_path, &ready);
        let f = open_rw(&lock_path);
        r.line("L3 other-process holds", &describe_lock(f.try_lock()), "WouldBlock");
        drop(f);
        match File::open(&lock_path) {
            Ok(mut g) => {
                r.line("L7a open while locked", "Ok", "Ok");
                let mut s = String::new();
                let read = match g.read_to_string(&mut s) {
                    Ok(n) => format!("Ok({n} bytes)"),
                    Err(e) => format!("Err(kind={:?} os={:?})", e.kind(), e.raw_os_error()),
                };
                let expected = if win { "Err" } else { "Ok(7 bytes)" };
                let folded = if read.starts_with("Err") { "Err".to_string() } else { read.clone() };
                r.line("L7b read while locked", &folded, expected);
                if folded != read {
                    r.info("L7b detail", &read);
                }
            }
            Err(e) => r.line(
                "L7a open while locked",
                &format!("Err(kind={:?} os={:?})", e.kind(), e.raw_os_error()),
                "Ok",
            ),
        }
        // L4. 持ち主が普通に終わる。予測: Ok（すぐに）。
        release(&mut child);
        let (first, ms) = lock_after(&lock_path, Duration::from_secs(2));
        r.line("L4 after-holder-exit", &first, "Ok");
        r.info("L4 ms-until-ok", &fmt_ms(ms));
    }

    // L5. 持ち主を殺す（SIGKILL / TerminateProcess）。予測: Ok（OS が外す）。
    //     ロックファイルは残っている（存在ではなく OS のロックで判定する根拠）。
    {
        let ready = dir.join("ready-lock-2");
        let mut child = spawn_holder("lock", &lock_path, &ready);
        child.kill().expect("kill");
        let _ = child.wait();
        let (first, ms) = lock_after(&lock_path, Duration::from_secs(2));
        r.line("L5 after-holder-killed", &first, "Ok");
        r.info("L5 ms-until-ok", &fmt_ms(ms));
        r.line(
            "L5b lockfile-still-exists",
            &lock_path.exists().to_string(),
            "true",
        );
    }

    // ---- R: redb ---------------------------------------------------------------
    let db_path = dir.join("probe.redb");

    // R1. 同じプロセスで 2 回 create。予測: 2 回目は DatabaseAlreadyOpen（別ハンドル
    //     の try_lock が WouldBlock → redb がそれを写す）。Drop 後は Ok。
    {
        let first = Database::create(&db_path);
        r.line("R1a same-process first", &describe_db(&first), "Ok");
        let second = Database::create(&db_path);
        r.line(
            "R1b same-process second",
            &describe_db(&second),
            "DatabaseAlreadyOpen",
        );
        drop(second);
        drop(first);
        let third = Database::create(&db_path);
        r.line("R1c after-drop", &describe_db(&third), "Ok");
        drop(third);
    }

    // R2. 別プロセスが開いている。予測: DatabaseAlreadyOpen（R1b と同じ値 —
    //     redb は同じプロセスと別プロセスを区別しない）。R4. 普通に閉じた後は Ok。
    {
        let ready = dir.join("ready-db-1");
        let mut child = spawn_holder("db", &db_path, &ready);
        let attempt = Database::create(&db_path);
        r.line(
            "R2 other-process holds",
            &describe_db(&attempt),
            "DatabaseAlreadyOpen",
        );
        drop(attempt);
        release(&mut child);
        let (first, ms) = db_after(&db_path, Duration::from_secs(2));
        r.line("R4 after-holder-exit", &first, "Ok");
        r.info("R4 ms-until-ok", &fmt_ms(ms));
    }

    // R3. 持ち主を殺す。予測: Ok（未クリーンな終了は redb が開くときに直す）。
    {
        let ready = dir.join("ready-db-2");
        let mut child = spawn_holder("db", &db_path, &ready);
        child.kill().expect("kill");
        let _ = child.wait();
        let (first, ms) = db_after(&db_path, Duration::from_secs(2));
        r.line("R3 after-holder-killed", &first, "Ok");
        r.info("R3 ms-until-ok", &fmt_ms(ms));
    }

    let _ = std::fs::remove_dir_all(&dir);
    println!("summary: unexpected={} os={}", r.unexpected, std::env::consts::OS);
    r.unexpected
}

// ---------------------------------------------------------------- 道具

fn open_rw(path: &Path) -> File {
    // Windows は append だけで開いたファイルをロックできない（std の doc）。
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .expect("open lock file")
}

fn describe_lock(result: Result<(), TryLockError>) -> String {
    match result {
        Ok(()) => "Ok".to_string(),
        Err(TryLockError::WouldBlock) => "WouldBlock".to_string(),
        Err(TryLockError::Error(e)) => {
            format!("Error(kind={:?} os={:?})", e.kind(), e.raw_os_error())
        }
    }
}

fn describe_db(result: &Result<Database, DatabaseError>) -> String {
    match result {
        Ok(_) => "Ok".to_string(),
        Err(DatabaseError::DatabaseAlreadyOpen) => "DatabaseAlreadyOpen".to_string(),
        Err(e) => format!("Err({e})"),
    }
}

/// 取れるまで 10 ms おきに試し、最初の結果と Ok までの時間を返す。
fn lock_after(path: &Path, max: Duration) -> (String, Option<u128>) {
    let started = Instant::now();
    let mut first: Option<String> = None;
    loop {
        let f = open_rw(path);
        let res = describe_lock(f.try_lock());
        first.get_or_insert_with(|| res.clone());
        if res == "Ok" {
            return (first.unwrap(), Some(started.elapsed().as_millis()));
        }
        drop(f);
        if started.elapsed() > max {
            return (first.unwrap(), None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn db_after(path: &Path, max: Duration) -> (String, Option<u128>) {
    let started = Instant::now();
    let mut first: Option<String> = None;
    loop {
        let res = describe_db(&Database::create(path));
        first.get_or_insert_with(|| res.clone());
        if res == "Ok" {
            return (first.unwrap(), Some(started.elapsed().as_millis()));
        }
        if started.elapsed() > max {
            return (first.unwrap(), None);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn fmt_ms(ms: Option<u128>) -> String {
    match ms {
        Some(ms) => format!("{ms}"),
        None => "never".to_string(),
    }
}

fn spawn_holder(kind: &str, path: &Path, ready: &Path) -> Child {
    let exe: PathBuf = std::env::current_exe().expect("current_exe");
    let child = Command::new(exe)
        .arg("hold")
        .arg(kind)
        .arg(path)
        .arg(ready)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn holder");
    let started = Instant::now();
    while !ready.exists() {
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "holder did not become ready ({kind})"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    child
}

/// 標準入力へ 1 行書いて、普通に終わらせる。
fn release(child: &mut Child) {
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(b"\n");
    }
    let _ = child.wait();
}
