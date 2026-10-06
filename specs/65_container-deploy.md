# Spec 65: 村をコンテナで回す（bake・像・参照構成）

- 状態: **Draft rev2・未決ゼロ**（2026-10-07 起票 → 同日、査読 2 系統 27 点を反映して rev2。表は Notes 3 →
  同日、再査読が未決 4 つの推奨を支持し、利用者がそれを回答として転送して未決ゼロ。次は P0）
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

## rev1 からの変更（査読の反映。詳細は Notes 3）

- **Goal 1 を「構造化されたパスの欄」へ狭めた**。自由記述（`Construct.md`・`env`・`args`・`allow`）のパスは警告で名指しする
- **パスを持つ欄は 3 つ**（`workDir` / `ragSources` / 予定の `ScheduleProbe.cwd`）。**rev1 の「MCP の stdio の `cwd`」は
  存在しない欄だった**（`mcp.rs` の stdio は `command` / `args` / `env` だけ）— 起票者の誤り。`user/` / `external/` /
  `judges/` にもパスの欄は無い（中身は `icon.webp` と `judge.toml`）
- **`--map` は最長前方一致**（D2）
- **`bake` の終了コードを表にした**（D2）。平文の鍵は置き換え漏れ（3）と別の **10**、写し先の状態の食い違いは **11**
- **平文の鍵は「写しを作らない」**（そのファイルだけ写さない、ではない）。rev1 の D10 の文言を直した
- **D1 の表**: `probe_approvals.json` は「変換して写す」/ `bake.json` を棚に置く / `Memory.md` の初回は seed の例外 /
  `schedules.json` は GUI に無い予定の消化記録を落とす / `run.json` の `pending` は新しい `allow` / `deny` で決着したものを
  既存の `prune_settled` で落とす
- **扉の開き方を決めた**（D9。未決 2 を閉じた）— `serve --door-port <N>`、合鍵は秘密の鍵 `door_token`。
  rev1 は `mcp_server.json` を写さないと書きながら、`mcp_server.json` が無いと扉が開かない実装と組み合わせていた
- **秘密の変数名の衝突検査に `mcp:NAME` と `door_token` を足す**（D3）/ `SECRET_MISSING` と `DOOR_TOKEN_MISSING` を D5 の表へ /
  `HostView` に参照の名前を渡す口
- **`bake` は GUI の端末で動かす**と書いた（D2。ソースからのビルド・未決 5）
- **compose**: `ports:` は `fuseforks` 側 / `depends_on` / `/work` は bind mount の例 / ボリュームの所有者（D8）
- `--update` の `--map` の引き継ぎ・「空」の判定 / ヘッダー名の判定規則 / `TIMEZONE_MISMATCH` は有効な予定だけ・読めないとき /
  `bake.json` の置き場

## Goal

1. **GUI で育てた村を、コンテナで回せる写しにできる。** `fuseforks-cli bake` が、**構造化されたパスの欄**
   （`workDir` / `ragSources` / 予定の前判定・後判定の `cwd`）にある Windows の絶対パスをコンテナの中のパスへ置き換えた
   写しを作る。**この欄に置き換え忘れたパスが 1 つでもあれば写しを作らない。** 自由記述の中のパスは書き換えず、名指しで警告する
2. **写しを 2 回目以降に作り直しても、コンテナの側で育ったもの（会話・Memory・予定の消化・コマンドの承認待ち）を
   消さない。** 置き換えるのは GUI が真実を持つ「設計」のファイルだけ
3. **リポジトリに像の定義と compose の参照構成がある。** `docker compose up` で `serve` が常駐し、予定が現地時刻で
   発火し、扉は同じネットワーク名前空間のプロキシ越しに外から届く。`docker compose down` で払いの記録が欠けずに閉じる
4. **リモート MCP の鍵と扉の合鍵を、村のファイルに平文で置かずに済む。** `mcp.json` の `headers` の値に秘密の参照を書け、
   GUI では資格情報ストア、コンテナでは環境変数から読む（Spec 64 の `SecretStore` と同じ 2 つの読み先）
5. **コンテナで回らない設定を、起動前に名指しする。** 存在しない作業フォルダ・PATH に無い MCP の起動コマンドと
   `run` の許可コマンド・タイムゾーンの食い違い・欠けた秘密を、LLM を 1 回も呼ばずに `check` が返す

**やらないこと（範囲外）**: 特定のクラウド（Cloud Run / Fly.io / k8s）の手順 / 像の公開配布（未決 3）/
GUI の村とコンテナの村の双方向の同期 / 会話（`sessions.redb`）の持ち出しと持ち帰り / 作業フォルダ（リポジトリ）の
写し（クローンは運用者が行う）/ 複数のコンテナで 1 つの村を回すこと / 扉の bind を 127.0.0.1 以外へ開くこと
（Spec 64 D9 のまま）/ Windows のコンテナ / コンテナの中で `bake` すること（D2）

## 前提の実測（2026-10-07）

**開発機の村（`%APPDATA%\jp.outcasts.fuseforks`）を読んで数えた。値は鍵の名前だけを見た。**

- **パス**: 10 体すべての `workDir` が `D:\Github\Outcasts-MathLab`。`ragSources` は 6 体が `D:\ManualeRAG`、
  1 体が `D:\Outcasts.jp\draft` も持つ。予定 2 件は前判定・後判定に `cwd` を持たない
- **パスを持つ欄は型の上で 3 つだけ**（`model.rs` の `AgentSpec.work_dir: Option<String>` / `rag_sources: Vec<String>` と
  `schedule_probe.rs` の `ScheduleProbe.cwd: Option<String>`。`AgentSnapshot` の同名の欄は投影で保存されない）。
  MCP の stdio は `command` / `args` / `env` を持ち、`cwd` は持たない（`mcp.rs:81-89`）
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
- **扉は `mcp_server.json` が `enabled` で合鍵があるときだけ開く**（`boot.rs:308` の `start_if_enabled`。ポートの既定は 39641）。
  ファイルが無ければ既定の `enabled: false` で、`serve` でも開かない
- **秘密の変数名の衝突検査が数える鍵**は、村のテンプレート ID の全部と `jev_api_token`（`boot.rs` の `check_secret_names`）
- **`run` の許可コマンド**（`allow` の先頭の語）: `bash` / `curl` / `dyff` / `elan` / `git` / `lake` / `pwd` / `python` /
  `rg` / `sg` / `ssh`。`run` は `env_clear` した子を起こすので、像に入っていないコマンドは実行時に「見つからない」で返る
- **予定の時刻**: ティッカーは `chrono::Local::now()`（`orchestrator/schedules.rs:41`）。`Recurrence` は
  `interval` / `daily` / `weekly` の 3 値で、後ろの 2 つが現地時刻。コンテナの既定は UTC なので、村の「火曜 16:38」は
  **日本時間 火曜 01:38 に発火する**。`iana-time-zone 0.1.65` は既に依存の木に居る（chrono の clock）
- **Linux 向けの C 依存が 2 つ**（`cargo tree --target x86_64-unknown-linux-gnu -i`）:
  - `libdbus-sys` ← `dbus` ← `dbus-secret-service` ← `keyring`（`sync-secret-service`）
  - `aws-lc-sys` ← `aws-lc-rs` ← `rustls` ← `reqwest 0.12`（core）と `reqwest 0.13`（rmcp）
  - `ring` も同じ木に居る（rustls の別の provider）
- **1 つのファイルに設計と実行が同居しているものが 2 つある**:
  - `schedules.json` — 予定の定義（GUI が書く）と `lastConsumedDueMs`（ティッカーが書く）
  - `agents/<id>/run.json` — `allow` / `deny`（人が書く）と `pending`（`run` が積む）、さらに Spec 61 の
    「自動承認して許可」は `allow` へ機械が書き足す。**`pending` は承認待ちの記録で、実行を止めない**（未承認の呼び出しは
    拒否文が返ってターンは進む）。新しい `allow` / `deny` で決着した行を落とす `prune_settled` が既にある（`command.rs:368`）
- **Docker**: 開発機に 29.8.1（server linux/amd64）。D: の空きは 51 GB（Spec 64 の日にフルビルド 1 回で約 30 GB 使った）

## Design

### D1. 村のファイルを 3 つの置き場に分ける

**`bake` と再 `bake` の規則は、すべてこの表から出る。** 表に無いファイルは写さない。

| 置き場 | ファイル | 真実の持ち主 | 初回の `bake` | 再 `bake`（`--update`） |
|---|---|---|---|---|
| **設計** | `world.json` / `Ordinance.md` / `village_id` / `mcp.json`（共通）/ `agents/<id>/{Construct.md, SKILL.md, mcp.json, icon.webp}` / `judges/<id>/judge.toml` / `user/icon.webp` / `external/icon.webp` | GUI | 写す（パスの欄を置き換える） | **置き換える** |
| **同居** | `schedules.json` | 定義 = GUI / `lastConsumedDueMs` = コンテナ | 定義を写す・消化の記録は捨てる | 定義を置き換え、**消化の記録は予定の id ごとにコンテナの側を残す**。GUI に無くなった予定の記録は落として名指しする |
| **同居** | `agents/<id>/run.json` | `allow` / `deny` = GUI / `pending` = コンテナ | `allow` / `deny` を写す・`pending` は空 | `allow` / `deny` を置き換え、**`pending` はコンテナの側を残し、新しい `allow` / `deny` で決着した行を `prune_settled` で落とす** |
| **実行** | `agents/<id>/Memory.md` | コンテナ（**初回だけ GUI から seed する**） | **写す**（未決 4） | **触らない** |
| **実行** | `sessions.redb` / `attachments/` / `exports/` / `fuseforks.log` / `.fuseforks.lock` | コンテナ | 写さない | 触らない |
| **棚** | `bake.json`（`{data_dir}` の直下） | `bake` | 作る | 作り直す（`bakedAt` などを更新） |
| **棚** | `probe_approvals.json` | GUI の人の承認 | **変換して写す**（D4） | **変換して置き換える**（D4） |
| **棚** | `jev.json` / `pricing.json` | GUI | 写す（秘密を含まない） | 置き換える |
| **棚** | `mcp_server.json`（扉の設定） | — | **写さない**（コンテナの扉は D9） | 触らない |

- **`Memory.md` の「真実の持ち主」が 2 つに見えるのは、初回だけの seed の例外**。写した瞬間から持ち主はコンテナで、
  再 `bake` は触らない（Goal 2）。GUI で書き足した Memory を後からコンテナへ届ける経路は無い（範囲外: 同期）
- **黒板は作業フォルダの中**（`<work_dir>/blackboard/`）なので村の写しに入らない。作業フォルダは運用者がクローンする
- **「自動承認して許可」がコンテナで `allow` へ書き足した行は、再 `bake` で消える。** GUI が `allow` の真実の持ち主で、
  残したいなら GUI の村へ写し戻す。再 `bake` は**写し先にあって新しい `allow` に無い行**を標準エラーに名指しする
  （黙って消さない）。`allow` は置き換えの対象ではない（パスの欄ではない）ので、両側とも置き換え前の文字列で比べられる
- **会話を持ち出さない理由**: GUI の会話は GUI の画面で読むもの、コンテナの会話はコンテナで起きたこと。混ぜると
  「どちらで起きた会話か」が記録から読めなくなる。`ask` は Spec 64 D7 で既定が新しい会話なので、空で始めて困る経路は無い
- 写しの `world.json` は**コアの読み書きを通して**書く（`UnknownFields` を保つ = #112 の処方。手で JSON を組まない）。
  **帰結として、写しの `world.json` は元とバイトでは一致しない**（欄の順序・整形）。`bake` の正しさは「読み直して
  置き換えた欄以外が値として等しい」で確かめる（契約に書く）

### D2. `bake` — パスを置き換えた写しを作る

```text
fuseforks-cli bake --data-dir <GUI の data_dir> --out <写しの data_dir>
                   [--map <元>=<先> …] [--update] [--source-time-zone <IANA>]
                   [--allow-plaintext-headers] [--json]
```

**`bake` は GUI の端末で動かす。** 元の村のロック（Windows の `LockFileEx`）を取り、元の端末の時刻帯を読む必要があるため。
コンテナの中から元の村をマウントして `bake` する形は採らない（Docker Desktop のマウント越しに Linux の `flock` が
Windows のロックと噛み合う保証は無く、元の端末の時刻帯もコンテナからは読めない）。`fuseforks-cli` は Spec 64 D12 のとおり
ソースからのビルド（`cargo build -p fuseforks-cli --release`）。配り方は未決 5。

1. **元の村のロックを取る**（Spec 64 D3 と同じ `.fuseforks.lock`）。GUI が開いていれば 4 で止まる —
   書きかけの `world.json` を読まないため。**GUI を閉じてから `bake` する**
2. **写し先の状態を確かめる**（判定は `{out}` = 写しの `data_dir` で行う）:
   - `--update` なし: `{out}` が存在しないか空のフォルダでなければ **11** で止まる（上書きの事故を作らない）
   - `--update` あり: `{out}/bake.json` が無ければ **11** で止まる（`bake` が作った写しだけを作り直す）。
     あれば**写し先のロックも取る**（コンテナを止めてから作り直す。取れなければ 4）
3. **パスの置き換え**。対象は**構造化されたパスの欄の 3 つだけ**:

   | 欄 | 例 |
   |---|---|
   | `world.json` の `agents[].workDir` | `D:\Github\Outcasts-MathLab` → `/work/mathlab` |
   | `world.json` の `agents[].ragSources[]` | `D:\ManualeRAG` → `/work/manuale-rag` |
   | `schedules.json` の前判定・後判定の `ScheduleProbe.cwd` | 同上 |

   - **`--map` は最長前方一致**（`--map D:\Github=/work --map D:\Github\Outcasts-MathLab=/work/mathlab` なら後者が勝つ）。
     元は区切りの手前で終わる完全なパス成分（`D:\Git` は `D:\Github` に当たらない）。元が Windows 形（ドライブ文字か `\`）なら
     大文字小文字を無視し、残りの部分の `\` を `/` へ変える。同じ元が 2 回あれば 2（引数の誤り）
   - **置き換えなかった絶対パスが上の欄に 1 つでも残れば、写しを 1 バイトも書かずに止める**（終了コード **3**）。
     名指しは欄の位置と値（`agents[agent_3].ragSources[0] = D:\Outcasts.jp\draft`）。**除外リストではなく閉じた許容** —
     欄の集合は型から決まる（前提の実測）。欄が増えたらこの表と契約を同時に直す
   - **自由記述は置き換えない**: `Construct.md` / `SKILL.md` の本文・MCP の stdio の `command` / `args` / `env`・
     `run.json` の `allow` / `deny`・予定の `ScheduleProbe.command` / `args` と依頼文。機械が書き換えると、意味が変わったことを
     誰も見ていない。**Windows の絶対パスを含むものは警告で名指しする**（写しは作る）。`ScheduleProbe.command` / `args` を
     書き換えないことは D4 の前提でもある
   - `--update` で `--map` を省くと、`{out}/bake.json` の `maps` を引き継ぐ。`--map` を 1 つでも渡すと**集合ごと置き換え**
     （足し合わせない）、前回との差を標準出力に名指しする
4. **平文の鍵**: `mcp.json`（共通と個体別）の `headers` に、名前が鍵らしく値に `${secret:` を含まないものがあれば、
   **写しを 1 バイトも書かずに止める**（終了コード **10**。`--allow-plaintext-headers` で通す）。
   鍵らしい名前 = 小文字にして `authorization` / `proxy-authorization` / `cookie` / `x-api-key` / `x-auth-token` / `api-key` の
   どれかに一致するか、`-key` / `-token` / `-secret` / `-password` で終わる。**これは推測の規則で保証ではない**
   （保証は `${secret:}` で書くこと）。止める理由は #1 と同じ形 — 写しはボリュームやバックアップへ流れる
5. **時刻帯**: 元の端末の IANA 名を `iana-time-zone` で読む。読めなければ `--source-time-zone` を求めて **2** で止まる
   （`bake.json` に推測の値を書かない）。`--source-time-zone` は読めたときも優先する
6. `{out}/bake.json` を書く: `{ bakedAt, appVersion, sourceVillageId, sourceTimeZone, maps: [{from, to}] }`
7. 結果を標準出力に書く — 写したファイル・置き換えた欄の数・運んだ承認の数・警告。`--json` で機械向け

**終了コード**（Spec 64 D10 の番号と意味を共有し、`bake` だけのものに 10 番台を使う）:

| コード | 意味 |
|---|---|
| 0 | 写しを作った（警告があっても 0） |
| 2 | 引数の誤り（`--map` の構文・同じ元の重複・時刻帯が読めず `--source-time-zone` も無い） |
| 3 | パスの欄に置き換えなかった絶対パスが残る |
| 4 | 元の村か写し先のロックが取れない |
| 5 | 元の村が読めない（`world.json` が壊れている 等。Spec 64 の「組み立てに失敗した」と同じ） |
| 10 | 平文の鍵が `headers` にある |
| 11 | 写し先の状態が食い違う（`--update` なしで空でない / `--update` で `bake.json` が無い） |

- **`bake` は LLM も MCP も呼ばない。** ファイルを読んで書くだけ
- **止まるときは 1 バイトも書かない。** 写しは一時フォルダに組んでから入れ替える（`--update` は写し先の設計と棚のファイルを
  1 つずつ `write_atomic` で置き換える。実行のファイルには触れないので途中で止まっても会話と Memory は無傷）
- **`village_id` を写す**（村の同一性。前判定の承認がこれに結び付いている — D4）。同じ村の写しを 2 つの
  コンテナで回さないのは運用者の責任で、構造では止めない（範囲外: 複数のコンテナ）

### D3. 秘密の参照 — `headers` の値に `${secret:名前}` を書ける

- `mcp.json` の `headers` の**値の中**に `${secret:NAME}` を書ける（`"Authorization": "Bearer ${secret:OUTCASTS_TOKEN}"`）。
  `NAME` は `[A-Z0-9_]+` だけ。1 つの値に複数あってよい
- 接続の直前に `SecretStore` で引く — 鍵は `mcp:NAME`。GUI は資格情報ストア、`--secrets env` は
  `FUSEFORKS_SECRET_MCP_NAME`（Spec 64 D4 の写し方そのまま）
- **引けなければ接続しない**（プレースホルダの文字列をそのまま送らない）。その MCP サーバーは「接続できない」として
  今の失敗の経路に乗り、理由は**名前だけ**を書く（値は書かない — Spec 47 D7）
- **展開は `headers` の値だけ。** `env` / `args` / `url` には広げない（`env` に秘密を書かせない、は Spec 47 の凍結。
  広げると「どこに秘密を書いてよいか」の規則が欄の数だけ増える）
- **変数名の衝突検査が数える鍵を広げる**（`check_secret_names`）: 村のテンプレート ID の全部 + `jev_api_token` +
  `door_token`（D9）+ **起動する集合の `mcp.json`（共通と個体別）が参照する `mcp:NAME` の全部**。テンプレート ID が
  `mcp_outcasts` で参照が `OUTCASTS` なら、どちらも `FUSEFORKS_SECRET_MCP_OUTCASTS` に写るので組み立てを止める（Spec 64 の 5）
- GUI: MCP の設定画面が `mcp.json` の本文から参照の名前を拾い、名前ごとに「値を保存」の欄を出す
  （資格情報ストアへ書く。画面に値を戻さない — モデルの API キーと同じ扱い）
- **参照を書かない今の `mcp.json` はそのまま動く**（バイト等価。`$` を含む値が既にあっても、`${secret:` で
  始まる部分がなければ展開しない）

### D4. 前判定・後判定の承認を写しへ運ぶ

- 承認の鍵は `SHA-256(canonical_json({args, command, cwd, villageId}))`（Spec 28 D10）。`bake` が `cwd` を置き換えると
  **鍵が変わり、写しの中で「未承認」になる**
- `bake` は、**元の棚で承認されている前判定と後判定（`acceptance.probe`）だけ**について、置き換えた後の鍵を写しの
  `probe_approvals.json` に書く。元で未承認のものは写しでも未承認のまま。`cwd` を持たない予定は鍵が変わらないので
  そのまま写る（今の村の 2 件）
- 再 `bake` では写しの `probe_approvals.json` を**元から作り直す**（写し先の古い鍵は残さない）。コンテナの中で承認が
  増える経路は無い（承認を書くのは GUI の IPC と `bake` だけ）ので、作り直して失うものは無い
- これは「承認を書くのは GUI の IPC だけ」（`probe_approvals.rs` の doc / 契約）を 1 点広げる。
  **広げても「人がこのコマンドを見て承認した」は保たれる** — 運ぶのは人が既に承認した `command` と `args` で、
  変わるのは人が `--map` で明示した `cwd` だけ。コマンド行は 1 文字も変えない（D2 の 3 で `command` / `args` を置き換えない）
- **未決 1**（契約の拡張なので利用者の確認が要る）

### D5. 起動前検査に足すもの

Spec 64 の `headless_preflight` に足す。**材料はファイルと PATH と環境だけで、LLM も MCP も呼ばない**のは同じ。
純関数を保つため、ファイルシステム・PATH・参照の名前は `HostView` で渡す:

```rust
pub struct HostView<'a> {
    // …Spec 64 の欄…
    pub path_kind: &'a dyn Fn(&str) -> PathKind,          // Dir / File / Missing
    pub command_on_path: &'a dyn Fn(&str) -> bool,         // run と同じ PATH で解決できるか
    pub stdio_mcp_commands: &'a [(String, String)],        // (サーバー名, command)。今の stdio_mcp_servers を置き換える
    pub mcp_secret_refs: &'a [(String, String)],           // (サーバー名, NAME)。起動する集合の共通と個体別から
    pub door_port: Option<u16>,                            // --door-port
    pub door_token_present: bool,
    pub process_time_zone: Option<&'a str>,                // 読めなければ None
    pub baked_time_zone: Option<&'a str>,                  // bake.json が無ければ None
}
```

| 識別子 | 対象 | 重さ | 理由 |
|---|---|---|---|
| `SECRET_MISSING`（既存の識別子を広げる） | `headers` の `${secret:NAME}` が選んだストアに無い | **拒否** | その MCP サーバーは接続しない。`message` が「MCP の headers」と書き分け、`fix` に変数名を書く |
| `DOOR_TOKEN_MISSING` | `--door-port` があるのに `door_token` が無い | **拒否** | 扉を開くと言ったのに開けない（D9） |
| `WORK_DIR_MISSING` | 起動する集合の `workDir` が設定されていて、存在しない・フォルダでない | **拒否** | ファイル系のツールが全部「作業フォルダが存在しません」を返す。ヘッドレスでは誰も直さない |
| `RAG_SOURCE_MISSING` | `ragSources` の 1 つが存在しない | 警告 | `rag` はその宣言を飛ばして動く |
| `MCP_COMMAND_NOT_FOUND` | stdio の `command` が絶対パスなら存在しない、名前なら PATH に無い | 警告（**今の情報 `MCP_STDIO` を置き換える**） | その MCP サーバーは繋がらず、個体はそのツール無しで動く。直し方はリモート MCP（Spec 47）か派生した像 |
| `RUN_COMMAND_NOT_FOUND` | `run.json` の `allow` の先頭の語が PATH に無い | 警告 | 許可しても実行時に「見つからない」 |
| `TIMEZONE_MISMATCH` | **有効な**（`enabled`）`daily` / `weekly` の予定があり、プロセスの IANA 名が `bake.json` の `sourceTimeZone` と違うか、プロセスの側が読めない | 警告 | 予定が GUI で決めた時刻と違う時刻に発火する。直し方は `TZ=<sourceTimeZone>` |

- **`MCP_STDIO`（情報）の撤去は契約の破壊的変更**。`headless_host_contract` の識別子の列挙を直し、`--json` の `code` を読む
  側（Spec 64 は配布していないので利用者はいない）へ向けて契約に記録する
- `bake.json` が無い村（GUI の村を `--data-dir` で直接開いた場合）では `TIMEZONE_MISMATCH` を出さない（比べる相手が無い）
- **`WORK_DIR_MISSING` は `bake.json` の有無に関わらず拒否**。GUI の村を直接開いた場合でも、存在しない作業フォルダの個体は
  ツールが全部失敗する。Spec 64 の CLI は配布していないので、挙動の変化で困る利用者はいない
- **PATH の解決は `run` と同じ規則**（`env_clear` した子に渡す `PATH`）。`check` の側だけ別の PATH を見ると、
  `check` が通って実行時に見つからない形が生まれる

### D6. 時刻

- 像は `tzdata` を持つ。**`TZ` を既定で設定しない** — 既定を `Asia/Tokyo` にすると、他の地域の運用者の予定が黙ってずれる
- `bake` は `deploy/.env.example` を写しの横に置かない。`deploy/README.md` が「`TZ` に `bake.json` の `sourceTimeZone` を書く」と
  手順を書き、食い違えば D5 の警告が名指しする
- `interval` の予定は時刻帯に依らない（今の村の 1 件目）

### D7. 像

```text
deploy/Dockerfile    多段ビルド
  builder: rust:<MSRV 以上>-bookworm + cmake + clang + libdbus-1-dev + pkg-config
           cargo build -p fuseforks-cli --release --locked
  runtime: debian:bookworm-slim
           + ca-certificates tzdata tini libdbus-1-3
           + git curl bash python3 python-is-python3 openssh-client ripgrep
           RUN useradd -u 10001 -m fuseforks && mkdir -p /data /work && chown 10001:10001 /data /work
           USER 10001 / HOME=/home/fuseforks
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
- **非 root（UID 10001）で動かす。ボリュームの所有者が要る:**
  - 名前付きボリュームは、初めてマウントするときに像の中のフォルダの所有者を引き継ぐ（Docker の仕様）ので、
    像の `/data` / `/work` を 10001 にしておけば足りる（P0 で確かめる）
  - **bind mount はホストの所有者のまま**。`deploy/README.md` に `chown 10001:10001` の手順を書く。書けなければ
    `build_host` の `create_dir_all` が Permission denied で 5（組み立ての失敗）になる
- **`trash` はごみ箱を `$HOME/.local/share/Trash` か、ボリュームの頂点の `.Trash-<uid>` に作る**（freedesktop の規則。
  ボリュームをまたぐ移動はできないので、作業フォルダが `/home` と別のボリュームなら頂点の側）。頂点に書けないと
  `file remove` と黒板の削除が失敗する — P0 で測る
- **像に村を焼かない。** `/data` はボリューム。像は村を知らない — 同じ像で何個の村でも回せる
- 道具の追加（`lake` / `elan` / `dyff` / `sg` / node など）は**派生した像**（`FROM`）で行う。基本の像に積むほど、
  使わない人の像が大きくなる

### D8. compose の参照構成

```text
deploy/compose.yaml
  fuseforks:
    build: { context: .., dockerfile: deploy/Dockerfile }
    volumes:
      - data:/data              # 名前付き（像の所有者を引き継ぐ）
      - ./work:/work            # bind（運用者がホストでクローンしたリポジトリ。所有者は README の手順で）
    env_file: .env              # TZ / FUSEFORKS_SECRET_*（git に入れない）
    command: [serve, --data-dir, /data, --secrets, env, --start, batch]   # 扉を開くときは --door-port 39641 を足す
    ports: ["443:443"]          # proxy が受ける口。network_mode: service: の側には ports を書けないのでこちらに置く
    stop_grace_period: 40s      # Spec 64 D8 の猶予 30 秒 + 余裕
  proxy:                        # 扉を外へ出すときだけ（profiles: [door]）
    image: caddy
    network_mode: "service:fuseforks"   # 同じネットワーク名前空間 = 127.0.0.1 が届く（Spec 64 D9）
    depends_on: [fuseforks]
    volumes: [./Caddyfile:/etc/caddy/Caddyfile:ro, caddy_data:/data]
deploy/Caddyfile.example   TLS / Bearer の検査 / reverse_proxy 127.0.0.1:39641 { header_up Host 127.0.0.1:39641 }
deploy/.env.example
```

- **`stop_grace_period` を書く理由**: Docker の既定は 10 秒で、超えると SIGKILL が来る。Spec 64 D8 の猶予は 30 秒なので、
  既定のままだと飛行中のターンの `turn:` 行と `Record::Turn` が欠ける（#103 の形）
- **止める順序**: `depends_on` があるので、`docker compose down` は `proxy` を先に止めてから `fuseforks` を止める
  （依存の逆順）。`proxy` が名前空間の持ち主より後まで残る形にはならない
- **`ports:` を `fuseforks` の側に置く理由**: `network_mode: "service:…"` のサービスは自分では `ports:` を持てない（Docker の
  仕様）。公開するのは Caddy の 443 で、扉の 39641 は 127.0.0.1 に bind しているので公開しても外からは届かない
- 外向きの TLS・認証・回数の上限はプロキシが持つ（2026-09-22 の利用者の見立て「サーバー認証で仲介する形」）。
  Caddyfile は雛形で、**証明書の取り方と認証の方式は運用者が決める**（範囲外: 特定のクラウド）
- **成果の戻り道は作業フォルダの側。** `/work/<name>` は運用者がクローンしたリポジトリで、個体は `run` の `git` で push する。
  `bake` も compose も作業フォルダを写さない
- 起動の手順は README ではなく `deploy/README.md`（日英）に置く（README の 160 行の上限は既に超えている）

### D9. コンテナの扉 — `--door-port` と秘密の `door_token`

- `serve --door-port <N>` を足す。**渡したときだけ**、`mcp_server.json` を読まずに 127.0.0.1:N で扉を開き、合鍵は
  `SecretStore` の鍵 `door_token`（`--secrets env` なら `FUSEFORKS_SECRET_DOOR_TOKEN`）から読む
- **渡さないときは Spec 64 のまま**（`mcp_server.json` が `enabled` なら開く）。GUI は変わらない
- 合鍵が無ければ D5 の `DOOR_TOKEN_MISSING` で起動前に止まる（開くと言って開かない、を作らない）
- **環境変数があるだけで扉を開く形にしない** — 扉を開くかどうかは引数で書く（Spec 64 D6 の `--start` と同じ考え方。
  明示の意図だけが開く）
- 合鍵も他の秘密と同じ置き場（`.env` 1 つ）に揃う。D1 の `mcp_server.json` は「写さない」のまま

### D10. 像を確かめる経路

- `.github/workflows/verify-image.yml`（**手動**）— 像をビルドし、`check` を同梱の小さな村（テストの fixture）で回して
  0 を確かめる。**タグの CI には入れない**（3 OS のビルドの横に像のビルドを足すと待ちが延びる。`verify-cask.yml` と同じ扱い）
- 像の公開（GHCR）は未決 3

### D11. 外へ送るもの（PRIVACY）

- **アプリが新しく外へ送るものは無い。** コンテナで回しても送り先は GUI と同じ（LLM・MCP・Jev・単価表）
- PRIVACY 日英に 1 段落: 「コンテナで回す場合、村の写しと秘密を置くのは運用者のホスト。`bake` は `headers` に平文の鍵が
  あると写しを作らずに止まる（`--allow-plaintext-headers` で通した場合は写しに平文で入る）」

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
9. **自由記述の中の Windows パスも 3 で止める**（査読 1-1 の案の片方）— `Construct.md` に過去の作業の記録としてパスが
   書かれているのは普通で、止めると写しが作れない。警告で名指しする
10. **未決着の `pending` を再 `bake` で消す**（査読 1-4 の案）— `pending` は「人がまだ決めていない」の記録で実行を止めない。
    消すとコンテナで何を承認待ちにしたかが失われる。決着したものだけを既存の `prune_settled` で落とす
11. **`FUSEFORKS_DOOR_TOKEN` があるだけで扉を開く** — D9
12. **コンテナの中で `bake` する** — D2

## Tasks

### P0 — 測ってから凍結する

- [ ] 開発機の Docker で、`deploy/Dockerfile` の試作をビルドする（所要・像の大きさ・`aws-lc-sys` が bookworm で通るか）
- [ ] 像の中で測る: (a) UID 10001 で `trash::delete` が、`/home` と別の名前付きボリュームの上のファイルで動くか・bind mount の上で
  どうなるか (b) 名前付きボリュームが像の `/data` の所有者 10001 を引き継ぐか (c) `TZ=Asia/Tokyo` で `chrono::Local` と
  `iana-time-zone` が `Asia/Tokyo` を返すか・`TZ` 無しで UTC / 読めないか (d) `tini` の下で `docker stop` が SIGTERM を届け、
  `ask` / `serve` が Spec 64 の終了コードで閉じるか (e) `network_mode: service:` の Caddy から `127.0.0.1:39641` へ届くか
- [ ] `iana-time-zone` が Windows の開発機で `Asia/Tokyo` を返すか
- [ ] 開発機の村の写しを scratchpad に手で作り（パスを手で置き換え）、像の中で `check --for serve` を回して、
  D5 の各識別子が何件ずつ出るかの予測を先に書いてから突き合わせる
- [ ] `data_contract.yaml` に `container_contract` を凍結（D1 の表 / D2 の閉じた許容・欄の集合・終了コード / D3 の展開規則と
  衝突検査の鍵 / D4 / D5 の識別子 / D9）。`headless_host_contract` の `MCP_STDIO` を置き換え、`probe_approvals` の
  「GUI の IPC だけが書く」に `bake` を足す。写しの `world.json` は値として等しい（バイトではない）を書く

### P1 — コア

- [ ] `mcp.rs`: `headers` の値の `${secret:NAME}` の展開（接続の直前・`SecretStore` 経由・引けなければ接続しない）
- [ ] `headless.rs`: D5 の表。`HostView` の欄（`stdio_mcp_servers` を `stdio_mcp_commands` へ置き換え）
- [ ] 単体: 展開（参照なし = バイト等価 / 部分展開 / 複数 / 未定義 / 不正な名前）・D5 の各行・`bake.json` が無いとき
  `TIMEZONE_MISMATCH` が出ない・無効な予定では出ない

### P2 — ホストと `bake`

- [ ] `check_secret_names` に `door_token` と `mcp:NAME` を足す
- [ ] `serve --door-port`（`mcp_server.rs` が合鍵を `SecretStore` から受けて開く口）
- [ ] `fuseforks-host` に写しの組み立て（D1 の表を 1 か所の定数に。ファイルごとの規則はそこから引く）
- [ ] パスの置き換え（最長前方一致・成分の境界・Windows 形の大文字小文字）と、置き換え漏れ・自由記述のパスの名指し
- [ ] 同居のファイルの合流（`schedules.json` の消化と孤児 / `run.json` の `pending` と `prune_settled`）と、消える `allow` の名指し
- [ ] 承認の運搬（D4。後判定を含む）
- [ ] `fuseforks-cli bake` と終了コード（D2 の表）
- [ ] 結合: 初回 / 再 `bake` で Memory と消化と `pending` が残る / 置き換え漏れで 1 バイトも書かない（3）/ 平文の鍵（10）/
  写し先の状態（11）/ GUI が開いていると 4 / `--map` の最長一致 / `--update` で `maps` を引き継ぐ

### P3 — 像と参照構成

- [ ] `deploy/Dockerfile` / `deploy/compose.yaml` / `deploy/Caddyfile.example` / `deploy/.env.example` / `deploy/README.md`（日英）
- [ ] `.github/workflows/verify-image.yml`（手動）と、像で回す小さな村の fixture

### P4 — GUI

- [ ] MCP の設定画面: `${secret:…}` の名前を拾って値を保存する欄（資格情報ストア。値を画面に戻さない）

### P5 — 台帳

- [ ] DETAIL 3 言語 / README 3 言語（1 行）/ PRIVACY 日英 / CLAUDE.md（Spec の状態・ギャップの書き戻し）/
  Spec 47 の「headers は平文」の注記 / Spec 64 の D12・Notes 2・`MCP_STDIO` へ行き先 / `.github/workflows/build.yml` の
  「members = 2 つ」のコメント（Spec 64 で 4 つになっていた）

### P6 — 実機

- [ ] 開発機の村を `bake` → `docker compose --profile door up` → 扉へプロキシ越しに 1 件依頼して答えが返る
- [ ] 壁時計の予定が `TZ` どおりの時刻に発火する
- [ ] `docker compose down` で `turn:` 行が欠けない（飛行中のターンがあるとき）
- [ ] GUI で Construct を直して再 `bake --update` → コンテナの Memory・会話・予定の消化が残り、Construct だけ変わる
- [ ] 平文の `Authorization` を持つ村の `bake` が 10 で止まり、`${secret:…}` に直すと通る

## 未決

**ゼロ。**（2026-10-07。rev2 の再査読が未決 1・3・4・5 の推奨をすべて支持し、利用者がそれを回答として転送した。
4 つとも推奨どおりに閉じる。以下は閉じた経緯の記録）

1. ~~前判定の承認を `bake` が運ぶか~~ → **運ぶ**（D4 のとおり。後判定も含む）。「承認を書くのは GUI の IPC だけ」の契約を 1 点広げる。推奨は運ぶ
   （コマンド行は変えず、変わるのは人が `--map` で明示した `cwd` だけ）。運ばない場合、`cwd` を持つ前判定は写しの中で
   永久に未承認になり、直す経路が無い（GUI はその写しを開けない）。**査読 2 系統とも推奨を支持**
2. ~~コンテナの扉の合鍵をどう作るか~~ → **D9 で閉じた**（`--door-port` + 秘密の `door_token`。rev1 の推奨 (b) を、
   扉を開くかどうかを引数で書く形にして採った）
3. ~~像を GHCR で公開するか~~ → **この Spec では公開しない**（D10）— 公開すると配布の経路が 4 つ目になり、版を出すたびに更新が要る
   （CLAUDE.md「タグを打った後にもう 2 つ仕事がある」が 3 つになる）。推奨は**この Spec では公開しない**
   （Dockerfile をリポジトリに置き、運用者がビルドする）。需要が出たら別に決める。**査読 2 系統とも推奨を支持**
4. ~~初回の `bake` で `Memory.md` を写すか~~ → **写す**（D1 の seed の例外）— 推奨は写す（個体が GUI で覚えたことはコンテナでも効いてほしい）。
   写さないと、コンテナの個体は `remember` の記録を持たずに始まる。**査読 2 系統とも推奨を支持**
5. ~~Windows 向けの `fuseforks-cli` を配るか~~ → **この Spec では配らない**（D2。ソースからのビルド）— `bake` は GUI の端末で動かすので、配らなければ
   `bake` する人は Rust の toolchain でビルドする。推奨は**この Spec では配らない**（Spec 64 D12 のまま。コンテナを運用する人は
   ソースをビルドできる前提に置く）。広く使われ始めたら、GUI に「コンテナ用の写しを作る」を足すか Release に CLI を
   足すかを別に決める

## Notes

### 1. Spec 64 のギャップとの対応

| Spec 64 で残したもの | 本 Spec |
|---|---|
| ギャップ 3（扉の bind）— 手順はコンテナの Spec | D8・D9 — compose の `network_mode: service:` と Caddy の `header_up Host`、`--door-port` |
| ギャップ 5 の後半（設計と実行の状態を bake で分ける） | D1・D2 |
| Notes 2「静的リンク」 | D7 — 採らない（像で配るので要らない） |
| Notes 2「`keyring` の feature を落とせるか」 | D7 — 落とさない（`libdbus-1-3` を像に入れる） |
| Notes 2「MCP の stdio は像に同梱するか、リモートへ寄せる」 | 起票前の裁定 3・D3・D5 |
| D12「配布はしない」 | D10・未決 3・未決 5 — 像の定義は置くが、像も CLI も公開は未決 |

### 2. この村の今の設定でコンテナに持っていけないもの（P0 の予測の材料）

- MCP: `docker` の gateway（5 体）/ Windows の exe 2 つ（memoria 5 体・manuale 1 体）/ `npx` の browsermcp / lorelei（127.0.0.1）
  → 写しで動くのは http の 3 つ（outcasts / elyth / alphaxiv）だけ。memoria と manuale はリモート MCP 化か Linux 版の派生像
- `run` の許可: `lake` / `elan` / `dyff` / `sg` は基本の像に無い（D7 の派生した像）
- `headers`: 4 体ぶんの `Authorization` を `${secret:…}` へ直すまで `bake` は 10 で止まる（D2 の 4）
- 計画の確認（`planReview`）が ON の個体がザリ・ルナに居る（Spec 64 P6）— 無人で回すなら `--bypass-plan-review` か予定の
  「計画の確認を自動で通す」（Spec 53）

### 3. rev1 の査読の反映（2026-10-07。2 系統 27 点）

| # | 指摘 | 判定 | 反映 |
|---|---|---|---|
| 1-1 | Goal 1 と D2 の 3 の矛盾 | **採用** | Goal 1 を構造化された欄へ狭めた。自由記述を止める案は不採用（採らなかった形 9） |
| 1-2 | 平文の鍵の扱いが 2 通り・終了コードの共用 | **採用（番号は変えて）** | 写しを作らない、に統一。終了コードは 10（査読の案 5 は Spec 64 で「組み立ての失敗」なので使わない） |
| 1-3 | `probe_approvals.json` は変換して書く | **採用** | D1 の表 / D4 に再 `bake` の作り直し |
| 1-4 | 孤児（予定の消化記録・`pending`） | **採用（`pending` は前提を訂正して）** | 予定は落として名指し。`pending` は実行を止めないので、決着した行だけ既存の `prune_settled` で落とす（採らなかった形 10） |
| 1-5 | D5 の表に `SECRET_MISSING` が無い・`MCP_STDIO` の撤去は破壊的変更 | **採用** | D5 |
| 1-6 | 非 root とボリュームの所有者・`trash` | **採用** | D7・P0 (a)(b) |
| 1-7 | compose の起動順・止める順・`/work` | **採用** | D8。止める順は `depends_on` の逆順で `proxy` が先 |
| 1-A | `--map` の最長一致 | **採用** | D2 の 3 |
| 1-B | `user/` / `external/` / `judges/` のパス | **確認（変更なし）** | 中身は `icon.webp` と `judge.toml` でパスの欄は無い。欄の集合は型から決めた（前提の実測）。**同時に起票者の誤りを 1 つ直した** — rev1 の「MCP の stdio の `cwd`」は存在しない欄 |
| 1-C | `Memory.md` の真実の持ち主 | **採用** | D1 の脚注（seed の例外） |
| 1-D | 未決 2 の決め方で D1 が変わる | **採用** | D9 で閉じた。D1 は「写さない」のまま |
| 1-細 1 | `WORK_DIR_MISSING` の互換 | **不採用** | GUI の村を直接開いても作業フォルダが無ければツールは全部失敗する。Spec 64 の CLI は配布していない（D5） |
| 1-細 2 | 写しの `world.json` の整形 | **採用** | D1 — 値として等しい、を契約に |
| 1-細 3 | 上書き防止は `data_dir` で判定 | **採用** | D2 の 2 |
| 1-細 4 | 時刻帯が読めないとき | **採用** | D2 の 5（`--source-time-zone`） |
| 2-(1) | 扉が開かない・ポート・Tasks の欠落・`ports:` | **採用** | D9・D8・P2 |
| 2-(2) | `SECRET_MISSING` と `HostView` | **採用** | D5 |
| 2-(3) | 変数名の衝突に `mcp:NAME` | **採用** | D3（`door_token` も） |
| 2-(4) | `bake` をどこで動かすか | **採用** | D2 冒頭・未決 5 |
| 2-(5) | ボリュームの所有者 | **採用** | 1-6 と同じ |
| 2-(6) | 再 `bake` の `--map`・「空」の判定 | **採用** | D2 の 2・3 |
| 2-(7) | `bake.json` の置き場 | **採用** | `{data_dir}` の直下（棚）。D1 の表 |
| 2-(8) | `allow` の差分とパス | **前提を訂正して採用** | `allow` は置き換えの対象ではないので、両側とも置き換え前の文字列で比べられ誤検知は起きない。`allow` の中の Windows パスは自由記述として警告する（D2 の 3） |
| 2-(9) | 鍵らしいヘッダー名の判定 | **採用** | D2 の 4（推測の規則だと明記） |
| 2-(10) | `TIMEZONE_MISMATCH` の対象・読めないとき | **採用** | D5（有効な予定だけ・読めなければ警告） |
| 2-(11) | `bake` の終了コード | **採用** | D2 の表 |
| 2-未決 | 未決 1・3・4 の推奨を支持 / 後判定も運ぶ | **採用** | D4 に後判定を明記 |
