[English](README.md) | **日本語**

# 村をコンテナで回す

GUI で作って安定させた村を、Docker のコンテナで GUI なしに回すための参照構成です（Spec 65）。
**GUI は設計と安定化の場、コンテナは安定したフローを回す場**という分担で、村の設定を直すのは
いつも GUI の側です。

> この構成は配布物ではありません。像（イメージ）は公開しておらず、`fuseforks-cli` も配っていません。
> どちらもこのリポジトリのソースからビルドします。

## 置き場

| ホストのフォルダ | コンテナ | 中身 |
|---|---|---|
| `deploy/village/` | `/data` | `bake` が作った村の写し。会話・`Memory.md`・ログはここに溜まる |
| `deploy/work/` | `/work` | 作業フォルダの**親**。リポジトリはこの下にクローンする |
| `deploy/.env` | 環境変数 | 時刻帯と秘密（`FUSEFORKS_SECRET_*`） |

3 つとも git には入りません（`.gitignore`）。像は村を持たないので、同じ像でいくつの村でも回せます。

## 手順

### 1. `fuseforks-cli` をビルドする（GUI の端末で）

```powershell
cargo build -p fuseforks-cli --release
```

`bake` は GUI の村を読んでパスを置き換えた写しを作るコマンドで、**GUI の端末で動かします**
（元の村のロックと、元の端末の時刻帯を読むため）。

### 2. GUI を閉じて `bake` する

```powershell
target\release\fuseforks-cli.exe bake --data-dir "$env:APPDATA\jp.outcasts.fuseforks" --out deploy\village --map "D:\Github=/work"
```

- `--map <元>=<先>` は作業フォルダ・`rag` の宣言・前判定の `cwd` の 3 つの欄だけを置き換えます。
  最長の前方一致が勝ちます。置き換え漏れが 1 つでもあれば、写しを書かずに 3 で止まります
- `mcp.json` の `headers` に平文の鍵があると 10 で止まります。`Authorization: Bearer ${secret:NAME}` の形に直してください
  （値は `.env` の `FUSEFORKS_SECRET_MCP_NAME`）
- `Construct.md` や `SKILL.md` の本文、`run.json` の許可コマンドに Windows のパスが残っていれば警告で名指しします（写しは作ります）
- 書いたものは標準出力に出ます。`deploy/village/bake.json` の `sourceTimeZone` を次の手順で使います

### 3. 作業フォルダを用意する

```bash
git clone <リポジトリ> deploy/work/<名前>
```

`--map` の先（例: `/work/mathlab`）とフォルダの名前を合わせます。**`/work` の直下にリポジトリ自体を置かない**でください。
ファイルを消すとき、ごみ箱がマウントの頂点（`/work/.Trash-10001`）に作られるので、リポジトリをマウントすると
`git status` に出ます。ごみ箱は自動では空になりません。

成果の戻り道は作業フォルダの側です。個体は `run` の `git` で push します（`bake` も compose も作業フォルダを写しません）。

### 4. `.env` を書く

```bash
cp deploy/.env.example deploy/.env
```

- `TZ` に `bake.json` の `sourceTimeZone` を書く（例: `Asia/Tokyo`）。書かないとコンテナは UTC で動き、
  `daily` / `weekly` の予定が GUI とずれた時刻に発火します
- 秘密はテンプレート ID を大文字にした名前で書く（`claude_sonnet` → `FUSEFORKS_SECRET_CLAUDE_SONNET`）

### 5. 起動前に確かめる

```bash
cd deploy
docker compose build
docker compose run --rm fuseforks check --for serve --data-dir /data --start batch --secrets env
```

`check` は村を開かずに検査だけします。拒否があれば 3 で、直し方が 1 行ずつ出ます。主なもの:

| 識別子 | 直し方 |
|---|---|
| `SECRET_MISSING` | `.env` に出ている変数名を書く |
| `WORK_DIR_MISSING` | `deploy/work/` にクローンする（手順 3） |
| `PLAN_REVIEW_WAITS` | GUI でその個体の「計画の確認」を OFF にするか、`command` に `--bypass-plan-review` を足す（ヘッドレスでは誰も承認しない） |
| `TIMEZONE_MISMATCH` | `.env` の `TZ` を `sourceTimeZone` に合わせる |
| `MCP_COMMAND_NOT_FOUND` | stdio の MCP は像の中に無い。リモート MCP（`type: "http"`）にするか、派生した像に入れる |
| `RUN_COMMAND_NOT_FOUND` | 許可したコマンドが像に無い。派生した像に入れる |

### 6. 起動する

```bash
docker compose up -d
docker compose logs -f
```

起動するのは「一括起動」が ON の個体です（`--start batch`）。予定（スケジュール）はコンテナの中で発火します。

## 村を直したとき（再 `bake`）

GUI で `Construct.md` や設定を直したら、写しを作り直します。

```powershell
docker compose -f deploy\compose.yaml stop
target\release\fuseforks-cli.exe bake --data-dir "$env:APPDATA\jp.outcasts.fuseforks" --out deploy\village --update
docker compose -f deploy\compose.yaml up -d
```

- **必ずコンテナを止めてから `--update` してください。** `bake --update` は写し先のロックを取りますが、
  **Docker Desktop の bind mount ではロックが Windows とコンテナの間を越えません**（実測。コンテナが村を
  開いたまま、Windows の側からも同じ村を開けた）。止めずに作り直すと、走っている村の下でファイルが置き換わります
- 置き換わるのは設計のファイル（`world.json`・条例・`Construct.md`・`SKILL.md`・`mcp.json` など）だけです。
  **会話・`Memory.md`・予定の消化の記録・承認待ちのコマンドはコンテナの側が残ります**
- コンテナの「自動承認して許可」が `run.json` の `allow` に書き足した行は、再 `bake` で消えます（GUI が `allow` の持ち主）。
  消える行は標準エラーに名指しされます。残したければ GUI の村へ写してください
- `--map` を省くと前回のものを引き継ぎます

## 扉を外へ出す（任意）

外から MCP で村へ依頼するときだけ、Caddy を重ねます。

```bash
# .env に FUSEFORKS_DOMAIN と FUSEFORKS_SECRET_DOOR_TOKEN を書く
cd deploy
docker compose -f compose.yaml -f compose.door.yaml up -d
```

- Caddy の設定は `deploy/Caddyfile`（ドメインは `.env` から読む）。証明書の取り方を変えるならこのファイルを直す
- 扉は `127.0.0.1:39641` に bind したままで、Caddy が同じネットワーク名前空間から受けます。TLS は Caddy が終端します
- 合鍵（`Authorization: Bearer <FUSEFORKS_SECRET_DOOR_TOKEN>`）は扉が検査します。Caddy に秘密は渡しません
- **回数の上限はありません。** 外部依頼は 1 件ごとに新しい予算（`tokenBudget`）で走るので、合鍵が漏れたときの損失は
  「1 依頼の天井 × 回数」です。要るなら Caddy にレート制限を足してください
- 呼び出し側のタイムアウトは、村の「委譲の待ち時間」（既定 600 秒）より長くしてください

## 知っておくこと

- **止め方**: `docker compose stop`（`down`）。`stop_grace_period: 40s` で、飛行中のターンを待ってから閉じます。
  Docker の既定の 10 秒にすると、ターンが SIGKILL で切れて払いの記録（`turn:` 行）が残りません
- **Linux の bind mount**: ホストの所有者のままなので、像のユーザー（UID 10001）が書けるようにします。
  ```bash
  sudo chown -R 10001:10001 deploy/village deploy/work
  ```
  書けないと起動が 5（組み立ての失敗）で止まります。`bake` より前に `docker compose up` すると、
  Docker が `deploy/village` を root の所有で作ってしまうので、順番は `bake` → `up` です
- **同じ名前の別プログラム**: 像の Debian には `sg`（別のグループで実行するコマンド）が入っています。
  ast-grep の `sg` を許可している村では、`check` が「ある」と判定しても別物が走ります。ast-grep を入れた
  派生の像を作り、許可を `ast-grep` の名前にしてください
- **道具の追加**: `lake` / `node` などは、この像を `FROM` にした派生の像で足します
  ```dockerfile
  FROM fuseforks:local
  USER root
  RUN apt-get update && apt-get install -y --no-install-recommends nodejs && rm -rf /var/lib/apt/lists/*
  USER 10001
  ```
- **版番号**: 像には `.git` を送らないので、版番号はビルドの引数で渡します。渡さなければ `fuseforks-cli --version` は `0.0.0` です
  ```bash
  FUSEFORKS_CLI_VERSION="$(git describe --tags --abbrev=0 | sed 's/^v//')+g$(git rev-parse --short HEAD)" docker compose build
  ```
- **同じ村の写しを 2 つのコンテナで回さない**でください。前判定の承認が村の ID に結び付いているので、写しは同じ村を名乗ります
- 送る先は GUI と同じです（LLM・MCP・Jev・単価表）。コンテナで回して新しく外へ送るものはありません
