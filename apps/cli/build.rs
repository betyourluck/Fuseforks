//! 版番号をビルド時に取る（Spec 64 D11）。
//!
//! `git describe --tags --abbrev=0`（タグ。先頭の `v` は落とす）と
//! `git rev-parse --short HEAD`（コミット）から `0.4.0+g1b33d9d` の形を作り、
//! `FUSEFORKS_CLI_VERSION` として本体へ渡す。**どちらかが取れないとき（`.git` の無い
//! ソースの zip・shallow clone・git が無い）は panic せずに落とす** — タグが無ければ
//! `0.0.0`（打っていないリリースを名乗らない。ステータスバーと同じ規則）、コミットが
//! 無ければ `+g…` を付けない。
//!
//! **環境変数 `FUSEFORKS_CLI_VERSION` が空でなければ、git より先にそれを使う**（Spec 65 P3）。
//! 像のビルドは `.git` を送らない（`deploy/Dockerfile.dockerignore`）ので、git からは
//! `0.0.0` しか取れない。`deploy/Dockerfile` の `ARG` がこの名前で渡す。空なら渡していないのと同じ
//! （compose の `${FUSEFORKS_CLI_VERSION:-}` は未設定を空文字で渡す）。
//!
//! `+g<hash>` は `failures.md` #112 の「手元のビルド同士は区別できない」をヘッドレスの
//! 側だけ埋める（GUI は `build.rs` の射程の外のまま、`0.1.0` を書く）。

use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_owned())
}

fn main() {
    println!("cargo:rerun-if-env-changed=FUSEFORKS_CLI_VERSION");
    if let Ok(given) = std::env::var("FUSEFORKS_CLI_VERSION")
        && !given.trim().is_empty()
    {
        println!("cargo:rustc-env=FUSEFORKS_CLI_VERSION={}", given.trim());
        println!("cargo:rerun-if-changed=build.rs");
        return;
    }
    let tag = git(&["describe", "--tags", "--abbrev=0"]);
    let base = tag
        .as_deref()
        .map(|t| t.strip_prefix('v').unwrap_or(t).to_owned())
        .unwrap_or_else(|| "0.0.0".to_owned());
    let version = match git(&["rev-parse", "--short", "HEAD"]) {
        Some(hash) => format!("{base}+g{hash}"),
        None => base,
    };
    println!("cargo:rustc-env=FUSEFORKS_CLI_VERSION={version}");

    // コミットやタグが動いたら取り直す。**HEAD だけでは足りない** — HEAD の中身は
    // `ref: refs/heads/main` のままで、コミットで動くのはブランチの ref のほう。
    // 置き場は worktree で変わる（ref は共通の git ディレクトリに住む）ので、
    // `--git-path` で解決する。**無いパスを rerun-if-changed に書かない** — 書くと
    // 毎回のビルドで build.rs が走り直す。
    println!("cargo:rerun-if-changed=build.rs");
    let mut watched = vec![
        "HEAD".to_owned(),
        "packed-refs".to_owned(),
        "refs/tags".to_owned(),
    ];
    if let Some(branch) = git(&["rev-parse", "--symbolic-full-name", "HEAD"])
        && branch.starts_with("refs/")
    {
        watched.push(branch);
    }
    for name in watched {
        if let Some(path) = git(&["rev-parse", "--git-path", &name])
            && std::path::Path::new(&path).exists()
        {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}
