# Spec 65: 村をコンテナで回す（bake・像・参照構成）

- 状態: **Draft rev1**（2026-10-07 起票）
- 起点: 利用者（2026-10-07）—「コンテナの SPEC を起票しましょう」。Spec 64 D12 が「配るなら Linux 向けの
  静的リンク・コンテナの像・署名の扱いが要り、それはコンテナの Spec で一緒に決める」と送った先
- 前提の裁定（2026-09-14 利用者）:「最初は GUI で回して、そのフローが自動化で失敗しないようになったら、
  クラウドに core と workspace をコンテナにして送って実行できるようにしたい。これは最終的な目標」
- **起票前の裁定 3 点**（2026-10-07 利用者。3 つとも推奨案を採用）:
  1. **配置先は汎用の Docker ホスト** — OCI の像と docker compose の参照構成。永続ボリュームを持つ任意の
     Linux ホストで動く。特定のクラウドの手順は書かない
  2. **村は `bake` で写しを作って持っていく** — パスを置き換えた写しを作り、以後の会話・Memory・予定の消化は
     コンテナの側のもの。GUI の村とは同期しない。成果は作業フォルダの側（git など）で戻す
  3. **MCP はリモートを主にし、鍵は参照で持つ** — 像に入れるのは `fuseforks-cli` と基本の道具だけ。
     stdio の MCP は起動前検査で名指しする。`headers` の値に秘密の参照を書けるようにし、鍵を村から外す
- 材料: Spec 64 の Notes 1（ギャップ 6 つ）/ Notes 2（次の Spec へ渡す材料）/ D9（扉はプロキシで受ける）/
  CLAUDE.md「設計の材料 3 件 > core 単独実行の構想」/「agent-orchestrator の実読」/「外から MCP の扉へ繋ぐ形」

## Goal

1. **GUI で育てた村を、コンテナで回せる写しにできる。** `fuseforks-cli bake` が、Windows の絶対パスを
   コンテナの中のパスへ置き換えた写しを作る。置き換え忘れたパスが 1 つでもあれば写しを作らない
2. **写しを 2 回目以降に作り直しても、コンテナの側で育ったもの（会話・Memory・予定の消化・コマンドの承認待ち）を
   消さない。** 置き換えるのは GUI が真実を持つ「設計」のファイルだけ
3. **リポジトリに像の定義と compose の参照構成がある。** `docker compose up` で `serve` が常駐し、予定が現地時刻で
   発火し、扉は同じネットワーク名前空間のプロキシ越しに外から届く。`docker compose down` で払いの記録が欠けずに閉じる
4. **リモート MCP の鍵を村のファイルに平文で置かずに済む。** `mcp.json` の `headers` の値に秘密の参照を書け、
   GUI では資格情報ストア、コンテナでは環境変数から読む（Spec 64 の `SecretStore` と同じ 2 つの読み先）
5. **コンテナで回らない設定を、起動前に名指しする。** 存在しない作業フォルダ・PATH に無い MCP の起動コマンドと
   `run` の許可コマンド・タイムゾーンの食い違いを、LLM を 1 回も呼ばずに `check` が返す

**やらないこと（範囲外）**: 特定のクラウド（Cloud Run / Fly.io / k8s）の手順 / 像の公開配布（未決 3）/
GUI の村とコンテナの村の双方向の同期 / 会話（`sessions.redb`）の持ち出しと持ち帰り / 作業フォルダ（リポジトリ）の
写し（クローンは運用者が行う）/ 複数のコンテナで 1 つの村を回すこと / 扉の bind を 127.0.0.1 以外へ開くこと
（Spec 64 D9 のまま）/ Windows のコンテナ

## 前提の実測（2026-10-07）

**開発機の村（`%APPDATA%\jp.outcasts.fuseforks`）を読んで数えた。値は鍵の名前だけを見た。**

- **パス**: 10 体すべての `workDir` が `D:\Github\Outcasts-MathLab`。`ragSources` は 6 体が `D:\ManualeRAG`、
  1 体が `D:\Outcasts.jp\draft` も持つ。予定 2 件は前判定・後判定に `cwd` を持たない
- **起動前検査は作業フォルダの存在を見ていない**（`headless.rs` の識別子 11 個に該当なし）。
  作業フォルダの実在は**ツール実行時**の `resolve_in_work_dir`（canonicalize + 前方一致）だけが見ており、
  保存時も起動時も検査しない（Spec 29 の判断）。コンテナで `D:\…` の村を開くと、`serve` は普通に起動し、
  `file` / `fd` / `grep` を呼んだ時点で初めて「作業フォルダが存在しません」が返る
- **MCP（5 体ぶんの個体別 `mcp.json`）**:

  | 種類 | 中身 | コンテナでの扱い |
  |---|---|---|
  | stdio | `docker mcp gateway run`（5 体） | 像の中に docker は無い |
  | stdio | `D:\memoria\MemoriaAeterna.exe` / `D:\memoria\manuale.exe` | Windows の実行ファイル。Linux では動かない |
  | stdio | `npx @browsermcp/mcp@latest` | node が要る。しかもブラウザ拡張が相手 |
  | http | `https://outcasts.jp/mcp` / `elythworld.com` / `api.alphaxiv.org` | 届く。**`headers.Authorization` が平文**（4 体ぶん） |
  | http | `http://127.0.0.1:39642/mcp`（lorelei） | 開発機のローカルのサービス。クラウドからは届かない |

  stdio の `env` にもパス（`MEMORIA_DB_PATH` / `MANUALE_ROOT`）と Windows の変数（`LOCALAPPDATA` / `ProgramData`）が入っている
- **`headers` の値は展開されない**（`mcp.rs:709` が文字列をそのまま送る）。Spec 47 の契約は「headers は平文の
  `mcp.json` に保存され、村と一緒に配られる」と書いており、**村を写すと鍵も写る**
- **`run` の許可コマンド**（`allow` の先頭の語）: `bash` / `curl` / `dyff` / `elan` / `git` / `lake` / `pwd` / `python` /
  `rg` / `sg` / `ssh`。`run` は `env_clear` した子を起こすので、像に入っていないコマンドは実行時に「見つからない」で返る
- **予定の時刻**: ティッカーは `chrono::Local::now()`（`orchestrator/schedules.rs:41`）。コンテナの既定は UTC なので、
  村の「火曜 16:38」は**日本時間 火曜 01:38 に発火する**。`iana-time-zone 0.1.65` は既に依存の木に居る（chrono の clock）
- **Linux 向けの C 依存が 2 つ**（`cargo tree --target x86_64-unknown-linux-gnu -i`）:
  - `libdbus-sys` ← `dbus` ← `dbus-secret-service` ← `keyring`（`sync-secret-service`）
  - `aws-lc-sys` ← `aws-lc-rs` ← `rustls` ← `reqwest 0.12`（core）と `reqwest 0.13`（rmcp）
  - `ring` も同じ木に居る（rustls の別の provider）
- **1 つのファイルに設計と実行が同居しているものが 2 つある**:
  - `schedules.json` — 予定の定義（GUI が書く）と `lastConsumedDueMs`（ティッカーが書く）
  - `agents/<id>/run.json` — `allow` / `deny`（人が書く）と `pending`（`run` が積む）、さらに Spec 61 の
    「自動承認して許可」は `allow` へ機械が書き足す
- **Docker**: 開発機に 29.8.1（server linux/amd64）。D: の空きは 51 GB（Spec 64 の日にフルビルド 1 回で約 30 GB 使った）

## Design

### D1. 村のファイルを 3 つの置き場に分ける

**`bake` と再 `bake` の規則は、すべてこの表から出る。** 表に無いファイルは写さない。

| 置き場 | ファイル | 真実の持ち主 | 初回の `bake` | 再 `bake`（`--update`） |
|---|---|---|---|---|
| **設計** | `world.json` / `Ordinance.md` / `village_id` / `mcp.json`（共通）/ `agents/<id>/{Construct.md, SKILL.md, mcp.json, icon.webp}` / `judges/<id>/judge.toml` / `user/` / `external/` | GUI | 写す（パスを置き換える） | **置き換える** |
| **同居** | `schedules.json` | 定義 = GUI / `lastConsumedDueMs` = コンテナ | 定義を写す・消化の記録は捨てる | 定義を置き換え、**消化の記録は予定の id ごとにコンテナの側を残す** |
| **同居** | `agents/<id>/run.json` | `allow` / `deny` = GUI / `pending` = コンテナ | `allow` / `deny` を写す・`pending` は空 | `allow` / `deny` を置き換え、**`pending` はコンテナの側を残す** |
| **実行** | `agents/<id>/Memory.md` | コンテナ（初回だけ GUI） | **写す**（個体の連続性。未決 4） | **触らない** |
| **実行** | `sessions.redb` / `attachments/` / `exports/` / `fuseforks.log` / `.fuseforks.lock` | コンテナ | 写さない | 触らない |
| **棚** | `probe_approvals.json` | GUI の人の承認 | 写す（D4） | 置き換える（D4） |
| **棚** | `jev.json` / `pricing.json` | GUI | 写す（秘密を含まない） | 置き換える |
| **棚** | `mcp_server.json`（扉の合鍵） | 運用者 | **写さない**（未決 2） | 触らない |

- **黒板は作業フォルダの中**（`<work_dir>/blackboard/`）なので村の写しに入らない。作業フォルダは運用者がクローンする
- **「自動承認して許可」がコンテナで `allow` へ書き足した行は、再 `bake` で消える。** GUI が `allow` の真実の持ち主で、
  残したいなら GUI の村へ写し戻す。再 `bake` は消える行を標準エラーに名指しする（黙って消さない）
- **会話を持ち出さない理由**: GUI の会話は GUI の画面で読むもの、コンテナの会話はコンテナで起きたこと。混ぜると
  「どちらで起きた会話か」が記録から読めなくなる。`ask` は Spec 64 D7 で既定が新しい会話なので、空で始めて困る経路は無い

### D2. `bake` — パスを置き換えた写しを作る

```text
fuseforks-cli bake --data-dir <GUI の data_dir> --out <写しの data_dir>
                   --map <元>=<先> [--map …] [--update]
                   [--allow-plaintext-headers] [--json]
```

1. **元の村のロックを取る**（Spec 64 D3 と同じ `.fuseforks.lock`）。GUI が開いていれば 4 で止まる —
   書きかけの `world.json` を読まないため。**GUI を閉じてから `bake` する**
2. `--update` のときは**写し先のロックも取る**（コンテナを止めてから作り直す）。`--update` が無いのに写し先に
   `workspace/` があれば止まる（上書きの事故を作らない）。`--update` なのに写し先が空でも止まる
3. 設計のファイルを読み、**構造化されたパスの欄だけ**を `--map` で置き換える:

   | 欄 | 例 |
   |---|---|
   | `agents[].workDir` | `D:\Github\Outcasts-MathLab` → `/work/mathlab` |
   | `agents[].ragSources[]` | `D:\ManualeRAG` → `/work/manuale-rag` |
   | 予定の `probe.cwd` / `acceptance.cwd` | 同上 |
   | `mcp.json` の stdio の `cwd` | 同上 |

   - 照合は**前方一致**。`--map` の元は区切りの手前で終わる完全なパス成分（`D:\Git` は `D:\Github` に当たらない）。
     元が Windows 形（ドライブ文字か `\`）なら大文字小文字を無視し、残りの部分の `\` を `/` へ変える
   - **置き換えなかった絶対パスが上の欄に 1 つでも残れば、写しを 1 バイトも書かずに止める**（終了コード 3）。
     名指しは欄の位置と値（`agents[agent_3].ragSources[0] = D:\Outcasts.jp\draft`）。**除外リストではなく閉じた許容** —
     「写しに Windows のパスが紛れ込む」を、気づいた人の注意ではなく構造で止める
   - **自由記述の欄は置き換えない**（`Construct.md` の本文・`env` の値・`args`・依頼文）。そこに書かれたパスを
     機械が書き換えると、意味が変わったことを誰も見ていない。`env` / `args` に Windows のパスがあれば**警告**で名指しする
4. `headers` の値に平文の `Authorization` / `Cookie` / `X-Api-Key` 系があり、秘密の参照（D3）で書かれていなければ
   **止める**（`--allow-plaintext-headers` で通す）。止める理由は #1 と同じ形 — 写しはボリュームやバックアップへ流れる
5. 写しの `world.json` は**コアの読み書きを通して**書く（`UnknownFields` を保つ = #112 の処方。手で JSON を組まない）
6. 写しの先頭に `bake.json` を置く:
   `{ bakedAt, appVersion, sourceVillageId, sourceTimeZone, maps: [{from, to}] }`。
   `sourceTimeZone` は元の端末の IANA 名（`iana-time-zone`。Windows の時刻帯から写す）で、D6 が読む
7. 結果を標準出力に書く — 写したファイル・置き換えた欄の数・警告。`--json` で機械向け

- **`bake` は LLM も MCP も呼ばない。** ファイルを読んで書くだけ
- **`village_id` を写す**（村の同一性。前判定の承認がこれに結び付いている — D4）。同じ村の写しを 2 つの
  コンテナで回さないのは運用者の責任で、構造では止めない（範囲外: 複数のコンテナ）

### D3. 秘密の参照 — `headers` の値に `${secret:名前}` を書ける

- `mcp.json` の `headers` の**値の中**に `${secret:NAME}` を書ける（`"Authorization": "Bearer ${secret:OUTCASTS_TOKEN}"`）。
  `NAME` は `[A-Z0-9_]+` だけ
- 接続の直前に `SecretStore` で引く — GUI は資格情報ストアの鍵 `mcp:NAME`、`--secrets env` は
  `FUSEFORKS_SECRET_MCP_NAME`（Spec 64 D4 の写し方そのまま）
- **引けなければ接続しない**（プレースホルダの文字列をそのまま送らない）。その MCP サーバーは「接続できない」として
  今の失敗の経路に乗り、理由は**名前だけ**を書く（値は書かない — Spec 47 D7）
- 検査（D5）: 起動する集合の `mcp.json` が参照する名前が選んだストアに無ければ `SECRET_MISSING`（拒否）。
  Spec 64 の秘密の行と同じ識別子で、`message` が「MCP の headers」と書き分ける
- **展開は `headers` の値だけ。** `env` / `args` / `url` には広げない（`env` に秘密を書かせない、は Spec 47 の凍結。
  広げると「どこに秘密を書いてよいか」の規則が欄の数だけ増える）
- GUI: MCP の設定画面が `mcp.json` の本文から参照の名前を拾い、名前ごとに「値を保存」の欄を出す
  （資格情報ストアへ書く。画面に値を戻さない — モデルの API キーと同じ扱い）
- **参照を書かない今の `mcp.json` はそのまま動く**（バイト等価。`$` を含む値が既にあっても、`${secret:` で
  始まらなければ展開しない）

### D4. 前判定の承認を写しへ運ぶ

- 承認の鍵は `SHA-256(canonical_json({args, command, cwd, villageId}))`（Spec 28 D10）。`bake` が `cwd` を置き換えると
  **鍵が変わり、写しの中で全部「未承認」になる**
- `bake` は、**元の棚で承認されている前判定・後判定だけ**について、置き換えた後の鍵を写しの `probe_approvals.json` に書く。
  元で未承認のものは写しでも未承認のまま
- これは「承認を書くのは GUI の IPC だけ」（`probe_approvals.rs` の doc / 契約）を 1 点広げる。
  **広げても「人がこのコマンドを見て承認した」は保たれる** — 運ぶのは人が既に承認した `command` と `args` で、
  変わるのは人が `--map` で明示した `cwd` だけ。コマンド行は 1 文字も変えない
- 運んだ承認は標準出力に件数で書く。`cwd` を持たない予定（今の村の 2 件）は鍵が変わらないので、運ぶ必要もない
- **未決 1**（契約の拡張なので利用者の確認が要る）

### D5. 起動前検査に足す 5 つ

Spec 64 の `headless_preflight` に足す。**材料はファイルと PATH と環境だけで、LLM も MCP も呼ばない**のは同じ。
純関数を保つため、ファイルシステムと PATH は `HostView` の関数として渡す（`secret_present` と同じ形）。

| 識別子 | 対象 | 重さ | 理由 |
|---|---|---|---|
| `WORK_DIR_MISSING` | 起動する集合の `workDir` が存在しない・フォルダでない | **拒否** | ファイル系のツールが全部「作業フォルダが存在しません」を返す。ヘッドレスでは誰も直さない |
| `RAG_SOURCE_MISSING` | `ragSources` の 1 つが存在しない | 警告 | `rag` はその宣言を飛ばして動く |
| `MCP_COMMAND_NOT_FOUND` | stdio の `command` が絶対パスなら存在しない、名前なら PATH に無い | 警告（今の情報 `MCP_STDIO` を置き換える） | その MCP サーバーは繋がらず、個体はそのツール無しで動く。直し方はリモート MCP（Spec 47）か派生した像 |
| `RUN_COMMAND_NOT_FOUND` | `run.json` の `allow` の先頭の語が PATH に無い | 警告 | 許可しても実行時に「見つからない」 |
| `TIMEZONE_MISMATCH` | 壁時計の予定（`daily` / `weekly`。`Recurrence` の 3 値のうち `interval` 以外）があり、プロセスの IANA 名が `bake.json` の `sourceTimeZone` と違う | 警告 | 予定が GUI で決めた時刻と違う時刻に発火する。直し方は `TZ=<sourceTimeZone>` |

- `bake.json` が無い村（GUI の村を `--data-dir` で直接開いた場合）では `TIMEZONE_MISMATCH` を出さない
  （比べる相手が無い。Spec 64 の使い方を変えない）
- **PATH の解決は `run` と同じ規則**（`env_clear` した子に渡す `PATH`）。`check` の側だけ別の PATH を見ると、
  `check` が通って実行時に見つからない形が生まれる
- 情報 `MCP_STDIO` は消える（置き換え）。Spec 64 の `headless_host_contract` の識別子の列挙を直す

### D6. 時刻

- 像は `tzdata` を持つ。**`TZ` を既定で設定しない** — 既定を `Asia/Tokyo` にすると、他の地域の運用者の予定が黙ってずれる
- compose の参照構成は `TZ` を `.env` から取り、`bake` が `bake.json` の `sourceTimeZone` を `.env` の雛形へ書く
  （D8）。食い違えば D5 の警告
- `interval` の予定は時刻帯に依らない（今の村の 1 件目）

### D7. 像

```text
deploy/Dockerfile    多段ビルド
  builder: rust:<MSRV 以上>-bookworm + cmake + clang + libdbus-1-dev + pkg-config
           cargo build -p fuseforks-cli --release --locked
  runtime: debian:bookworm-slim
           + ca-certificates tzdata tini libdbus-1-3
           + git curl bash python3 python-is-python3 openssh-client ripgrep
           USER 10001（fuseforks）/ HOME=/home/fuseforks
           ENTRYPOINT ["tini", "--", "fuseforks-cli"]
           CMD ["serve", "--data-dir", "/data", "--secrets", "env", "--start", "batch"]
```

- **静的リンク（musl）にしない。** 配るのは実行ファイルではなく像なので、glibc と `libdbus-1-3` を像に入れれば足りる。
  musl にすると `aws-lc-sys` の C のビルドと `libdbus` の静的版を用意する手間だけが増え、得るものが無い
  （Spec 64 Notes 2 の「静的リンク」は実行ファイル単体を配る前提の話だった）
- **distroless にしない。** `run` の許可コマンド（git / bash / python）と前判定のコマンドが動く必要がある
- **`keyring` の feature は落とさない。** `--secrets env` だけを使う像でも、`libdbus-1-3` を入れれば `--secrets keyring` の
  経路はリンクできる（Secret Service が居ないので実行時に失敗するだけ）。feature を割ると GUI とコアのビルドが 2 通りになる
- **`tini` を PID 1 にする。** `run` と MCP の stdio が子を起こすので、孫のゾンビを刈る役が要る。
  シグナルは tini が `fuseforks-cli` へ渡し、閉じ方は Spec 64 D8 のまま
- **非 root で動かす。** `trash` はごみ箱を `$HOME/.local/share/Trash` か、ボリュームの頂点の `.Trash-<uid>` に作る —
  どちらで動くかは P0 で測る（Linux の freedesktop の規則。ボリュームをまたぐ移動はできないので、作業フォルダが
  `/home` と別のボリュームなら頂点の側になる）
- **像に村を焼かない。** `/data` はボリューム。像は村を知らない — 同じ像で何個の村でも回せる
- 道具の追加（`lake` / `elan` / `dyff` / `sg` / node など）は**派生した像**（`FROM`）で行う。基本の像に積むほど、
  使わない人の像が大きくなる

### D8. compose の参照構成

```text
deploy/compose.yaml
  fuseforks:
    image: fuseforks:local（ビルドは deploy/Dockerfile）
    volumes: [data:/data, work:/work]
    env_file: .env          # TZ / FUSEFORKS_SECRET_*（git に入れない）
    stop_grace_period: 40s  # Spec 64 D8 の猶予 30 秒 + 余裕
  proxy:                    # 扉を外へ出すときだけ（profiles: [door]）
    image: caddy
    network_mode: "service:fuseforks"   # 同じネットワーク名前空間 = 127.0.0.1 が届く（Spec 64 D9）
    Caddyfile: TLS / Bearer の検査 / reverse_proxy 127.0.0.1:<port> { header_up Host 127.0.0.1:<port> }
deploy/.env.example
```

- **`stop_grace_period` を書く理由**: Docker の既定は 10 秒で、超えると SIGKILL が来る。Spec 64 D8 の猶予は 30 秒なので、
  既定のままだと飛行中のターンの `turn:` 行と `Record::Turn` が欠ける（#103 の形）
- 外向きの TLS・認証・回数の上限はプロキシが持つ（2026-09-22 の利用者の見立て「サーバー認証で仲介する形」）。
  Caddyfile は雛形で、**証明書の取り方と認証の方式は運用者が決める**（範囲外: 特定のクラウド）
- **成果の戻り道は作業フォルダの側。** `/work/<name>` は運用者がクローンしたリポジトリで、個体は `run` の `git` で push する。
  `bake` も compose も作業フォルダを写さない
- 起動の手順は README ではなく `deploy/README.md`（日英）に置く（README の 160 行の上限は既に超えている）

### D9. 像を確かめる経路

- `.github/workflows/verify-image.yml`（**手動**）— 像をビルドし、`check` を同梱の小さな村（テストの fixture）で回して
  0 を確かめる。**タグの CI には入れない**（3 OS のビルドの横に像のビルドを足すと待ちが延びる。`verify-cask.yml` と同じ扱い）
- 像の公開（GHCR）は未決 3

### D10. 外へ送るもの（PRIVACY）

- **アプリが新しく外へ送るものは無い。** コンテナで回しても送り先は GUI と同じ（LLM・MCP・Jev・単価表）
- PRIVACY 日英に 1 段落: 「コンテナで回す場合、村の写しと秘密を置くのは運用者のホスト。`bake` は平文の
  `Authorization` を含む `mcp.json` を既定で写さない」

## 採らなかった形

1. **起動時にパスを読み替える**（村はそのまま、メモリの中だけで置き換える）— `world.json` はコアが書き戻すので、
   置き換えた後のパスがファイルへ残る経路を全部塞ぐ必要がある。`bake` なら写しは最初からコンテナのパスで、書き戻しても同じ
2. **GUI の村とコンテナの村を同期する** — 交互に開くと版の違うプロセスが欄を消す（#112）。会話と Memory は
   両方で育つので、どちらが正かの規則が要る。D1 の表で真実の持ち主を 1 つに決めたほうが単純
3. **像に村を焼く** — 秘密と承認が像に入り、像を押すたびに村が配られる。Spec 64 D2 の「村を配っても扉は開かない」と逆
4. **Docker-in-Docker で `docker mcp gateway` を動かす** — 像に Docker の特権が要る。リモート MCP へ寄せる裁定（起票前 3）
5. **`${env:NAME}` で任意の環境変数を展開する** — GUI では資格情報ストアから読めない。`SecretStore` を通せば GUI と
   コンテナで同じ書き方になる（D3）
6. **CLI に `approve` を足す**（写しの中で人が承認し直す）— 承認を書く経路が 2 つになる。運ぶのは既に承認されたものだけ（D4）
7. **`bake` が自由記述のパスも書き換える** — D2 の 3
8. **基本の像に開発機の道具を全部入れる**（`lake` / `elan` / node）— D7

## Tasks

### P0 — 測ってから凍結する

- [ ] 開発機の Docker で、`deploy/Dockerfile` の試作をビルドする（所要・像の大きさ・`aws-lc-sys` が bookworm で通るか）
- [ ] 像の中で測る: (a) 非 root で `trash::delete` がボリュームの上のファイルで動くか（`/home` と別のボリューム）
  (b) `TZ=Asia/Tokyo` で `chrono::Local` と `iana-time-zone` が `Asia/Tokyo` を返すか・`TZ` 無しで UTC か
  (c) `tini` の下で `docker stop` が SIGTERM を届け、`ask` / `serve` が Spec 64 の終了コードで閉じるか
  (d) `iana-time-zone` が Windows の開発機で `Asia/Tokyo` を返すか
- [ ] 開発機の村の写しを scratchpad に手で作り（パスを手で置き換え）、像の中で `check --for serve` を回して、
  D5 の 5 つが何件ずつ出るかの予測を先に書いてから突き合わせる
- [ ] `data_contract.yaml` に `container_contract` を凍結（D1 の表 / D2 の閉じた許容 / D3 の展開規則 / D4 / D5 の識別子）。
  `headless_host_contract` の `MCP_STDIO` を置き換え、`probe_approvals` の「GUI の IPC だけが書く」に `bake` を足す

### P1 — コア

- [ ] `mcp.rs`: `headers` の値の `${secret:NAME}` の展開（接続の直前・`SecretStore` 経由・引けなければ接続しない）
- [ ] `headless.rs`: D5 の 5 つ。`HostView` に `path_kind` / `command_on_path` / `process_time_zone` / `baked_time_zone`
- [ ] 単体: 展開（参照なし = バイト等価 / 部分展開 / 未定義 / 不正な名前）・検査 5 つ・`bake.json` が無いとき出ない

### P2 — `bake`

- [ ] `fuseforks-host` に写しの組み立て（D1 の表を 1 か所の定数に。ファイルごとの規則はそこから引く）
- [ ] パスの置き換え（前方一致・成分の境界・Windows 形の大文字小文字）と、置き換え漏れの名指し
- [ ] 同居のファイルの合流（`schedules.json` の消化 / `run.json` の `pending`）と、消える `allow` の名指し
- [ ] 承認の運搬（D4）
- [ ] `fuseforks-cli bake` と終了コード（3 = 置き換え漏れ・平文の鍵 / 4 = ロック）
- [ ] 結合: 初回 / 再 `bake` で Memory と消化と `pending` が残る / 置き換え漏れで 1 バイトも書かない / GUI が開いていると 4

### P3 — 像と参照構成

- [ ] `deploy/Dockerfile` / `deploy/compose.yaml` / `deploy/Caddyfile.example` / `deploy/.env.example` / `deploy/README.md`（日英）
- [ ] `.github/workflows/verify-image.yml`（手動）と、像で回す小さな村の fixture

### P4 — GUI

- [ ] MCP の設定画面: `${secret:…}` の名前を拾って値を保存する欄（資格情報ストア。値を画面に戻さない）

### P5 — 台帳

- [ ] DETAIL 3 言語 / README 3 言語（1 行）/ PRIVACY 日英 / CLAUDE.md（Spec の状態・ギャップの書き戻し）/
  Spec 47 の「headers は平文」の注記 / Spec 64 の D12 と Notes 2 へ行き先

### P6 — 実機

- [ ] 開発機の村を `bake` → `docker compose up` → 扉へプロキシ越しに 1 件依頼して答えが返る
- [ ] 壁時計の予定が `TZ` どおりの時刻に発火する
- [ ] `docker compose down` で `turn:` 行が欠けない（飛行中のターンがあるとき）
- [ ] GUI で Construct を直して再 `bake --update` → コンテナの Memory・会話・予定の消化が残り、Construct だけ変わる
- [ ] 平文の `Authorization` を持つ村の `bake` が 3 で止まり、`${secret:…}` に直すと通る

## 未決

1. **前判定の承認を `bake` が運ぶか**（D4）— 「承認を書くのは GUI の IPC だけ」の契約を 1 点広げる。推奨は運ぶ
   （コマンド行は変えず、変わるのは人が `--map` で明示した `cwd` だけ）。運ばない場合、`cwd` を持つ前判定は写しの中で
   永久に未承認になり、直す経路が無い（GUI はその写しを開けない）
2. **コンテナの扉の合鍵をどう作るか** — 今の `mcp_server.json` は GUI の画面で ON にしたときに合鍵を作る。コンテナには
   画面が無い。案は (a) `bake --door-port <N>` が新しい合鍵を作って写しの棚へ書き、標準出力に 1 回だけ出す
   (b) 環境変数 `FUSEFORKS_DOOR_TOKEN` から読む（`SecretStore` の鍵 `door_token`）。推奨は (b) — 合鍵も他の秘密と同じ
   置き場になり、`.env` 1 つで揃う
3. **像を GHCR で公開するか** — 公開すると配布の経路が 4 つ目になり、版を出すたびに更新が要る
   （CLAUDE.md「タグを打った後にもう 2 つ仕事がある」が 3 つになる）。推奨は**この Spec では公開しない**
   （Dockerfile をリポジトリに置き、運用者がビルドする）。需要が出たら別に決める
4. **初回の `bake` で `Memory.md` を写すか**（D1）— 推奨は写す（個体が GUI で覚えたことはコンテナでも効いてほしい）。
   写さないと、コンテナの個体は `remember` の記録を持たずに始まる

## Notes

### 1. Spec 64 のギャップとの対応

| Spec 64 で残したもの | 本 Spec |
|---|---|
| ギャップ 3（扉の bind）— 手順はコンテナの Spec | D8 — compose の `network_mode: service:` と Caddy の `header_up Host` |
| ギャップ 5 の後半（設計と実行の状態を bake で分ける） | D1・D2 |
| Notes 2「静的リンク」 | D7 — 採らない（像で配るので要らない） |
| Notes 2「`keyring` の feature を落とせるか」 | D7 — 落とさない（`libdbus-1-3` を像に入れる） |
| Notes 2「MCP の stdio は像に同梱するか、リモートへ寄せる」 | 起票前の裁定 3・D3・D5 |
| D12「配布はしない」 | D9・未決 3 — 像の定義は置くが、公開は未決 |

### 2. この村の今の設定でコンテナに持っていけないもの（P0 の予測の材料）

- MCP: `docker` の gateway（5 体）/ Windows の exe 2 つ（memoria 5 体・manuale 1 体）/ `npx` の browsermcp / lorelei（127.0.0.1）
  → 写しで動くのは http の 3 つ（outcasts / elyth / alphaxiv）だけ。memoria と manuale はリモート MCP 化か Linux 版の派生像
- `run` の許可: `lake` / `elan` / `dyff` / `sg` は基本の像に無い（D7 の派生した像）
- `headers`: 4 体ぶんの `Authorization` を `${secret:…}` へ直すまで `bake` は 3 で止まる（D2 の 4）
