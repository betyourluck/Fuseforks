**English** | [日本語](README_jp.md)

# Running a village in a container

A reference setup for running a village that you built and stabilised in the GUI inside a Docker
container, without the GUI (Spec 65). **The GUI is where you design and stabilise; the container is
where a stable flow runs.** You always change the village's settings on the GUI side.

> This setup is not a distributed product. The image is not published and `fuseforks-cli` is not
> shipped. You build both from the source in this repository.

## Where things live

| Host folder | Container | Contents |
|---|---|---|
| `deploy/village/` | `/data` | The copy of the village made by `bake`. Conversations, `Memory.md` and logs accumulate here |
| `deploy/work/` | `/work` | The **parent** of the work folders. Clone repositories underneath it |
| `deploy/.env` | Environment | Time zone and secrets (`FUSEFORKS_SECRET_*`) |

None of the three go into git (`.gitignore`). The image holds no village, so one image can run any number of villages.

## Steps

### 1. Build `fuseforks-cli` (on the GUI machine)

```powershell
cargo build -p fuseforks-cli --release
```

`bake` reads the GUI's village and writes a copy with its paths rewritten. **Run it on the GUI machine**
(it takes the village lock and reads the source machine's time zone).

### 2. Close the GUI and `bake`

```powershell
target\release\fuseforks-cli.exe bake --data-dir "$env:APPDATA\jp.outcasts.fuseforks" --out deploy\village --map "D:\Github=/work"
```

- `--map <from>=<to>` rewrites exactly three fields: work folders, `rag` declarations and the `cwd` of
  pre-checks. The longest prefix wins. If any absolute path is left unmapped, nothing is written and it exits 3
- A plaintext key in the `headers` of an `mcp.json` stops it with 10. Rewrite it as
  `Authorization: Bearer ${secret:NAME}` (the value goes in `.env` as `FUSEFORKS_SECRET_MCP_NAME`)
- Windows paths left in the text of `Construct.md` / `SKILL.md` or in `run.json`'s allowed commands are
  named as warnings (the copy is still written)
- What was written is printed. You need `sourceTimeZone` from `deploy/village/bake.json` in the next steps

### 3. Prepare the work folders

```bash
git clone <repository> deploy/work/<name>
```

Match the folder name to the target of `--map` (for example `/work/mathlab`). **Do not put a repository
directly at `/work`**: when a file is deleted, the trash is created at the top of the mount
(`/work/.Trash-10001`), so mounting a repository makes it show up in `git status`. The trash is never emptied automatically.

Results travel back through the work folder: servants push with `git` via `run`. Neither `bake` nor compose copies work folders.

### 4. Write `.env`

```bash
cp deploy/.env.example deploy/.env
```

- Set `TZ` to the `sourceTimeZone` in `bake.json` (for example `Asia/Tokyo`). Without it the container runs
  in UTC and `daily` / `weekly` schedules fire at different times than in the GUI
- Name each secret after its template ID in upper case (`claude_sonnet` → `FUSEFORKS_SECRET_CLAUDE_SONNET`)

### 5. Check before starting

```bash
cd deploy
docker compose build
docker compose run --rm fuseforks check --for serve --data-dir /data --start batch --secrets env
```

`check` only inspects; it does not open the village. Rejections exit 3, each with a one-line fix. The usual ones:

| Code | Fix |
|---|---|
| `SECRET_MISSING` | Add the variable it names to `.env` |
| `WORK_DIR_MISSING` | Clone into `deploy/work/` (step 3) |
| `PLAN_REVIEW_WAITS` | Turn off "Review plans" for that servant in the GUI, or add `--bypass-plan-review` to `command` (nobody approves headless) |
| `TIMEZONE_MISMATCH` | Set `TZ` in `.env` to `sourceTimeZone` |
| `MCP_COMMAND_NOT_FOUND` | stdio MCP servers are not in the image. Use a remote MCP server (`type: "http"`) or add it to a derived image |
| `RUN_COMMAND_NOT_FOUND` | An allowed command is not in the image. Add it to a derived image |

### 6. Start

```bash
docker compose up -d
docker compose logs -f
```

Servants with "batch start" turned on are started (`--start batch`). Schedules fire inside the container.

## When you change the village (re-`bake`)

After editing `Construct.md` or settings in the GUI, rebuild the copy.

```powershell
docker compose -f deploy\compose.yaml stop
target\release\fuseforks-cli.exe bake --data-dir "$env:APPDATA\jp.outcasts.fuseforks" --out deploy\village --update
docker compose -f deploy\compose.yaml up -d
```

- **Always stop the container before `--update`.** `bake --update` takes the copy's lock, but **on Docker
  Desktop the lock does not cross a bind mount between Windows and the container** (measured: with the
  container holding the village, a Windows process opened the same village too). Rebuilding without stopping
  replaces files underneath a running village
- Only design files are replaced (`world.json`, the ordinance, `Construct.md`, `SKILL.md`, `mcp.json`, …).
  **Conversations, `Memory.md`, consumed schedule records and pending command approvals stay as the container left them**
- Lines that "auto-approve and allow" added to `run.json`'s `allow` inside the container disappear on re-`bake`
  (the GUI owns `allow`). They are named on stderr; copy them into the GUI's village if you want to keep them
- Omitting `--map` reuses the previous mappings

## Exposing the door (optional)

Only when something outside should send requests to the village over MCP, add Caddy on top.

```bash
# put FUSEFORKS_DOMAIN and FUSEFORKS_SECRET_DOOR_TOKEN in .env
cd deploy
docker compose -f compose.yaml -f compose.door.yaml up -d
```

- Caddy's configuration is `deploy/Caddyfile` (the domain comes from `.env`). Edit it to change how certificates are obtained
- The door stays bound to `127.0.0.1:39641`; Caddy reaches it from the same network namespace and terminates TLS
- The door checks the key (`Authorization: Bearer <FUSEFORKS_SECRET_DOOR_TOKEN>`). Caddy is given no secrets
- **There is no request limit.** Each external request runs with a fresh budget (`tokenBudget`), so a leaked key
  costs "one request's ceiling × the number of requests". Add rate limiting to Caddy if you need it
- Give the caller a timeout longer than the village's delegation wait (600 seconds by default)

## Things to know

- **Stopping**: `docker compose stop` (or `down`). `stop_grace_period: 40s` waits for in-flight turns before closing.
  With Docker's default of 10 seconds a turn is cut by SIGKILL and its spend record (the `turn:` line) is lost
- **Bind mounts on Linux** keep the host's owner, so let the image user (UID 10001) write:
  ```bash
  sudo chown -R 10001:10001 deploy/village deploy/work
  ```
  If it cannot write, startup fails with 5. Running `docker compose up` before `bake` makes Docker create
  `deploy/village` owned by root, so the order is `bake`, then `up`
- **Same name, different program**: the image's Debian ships `sg` (run as another group). In a village that
  allows ast-grep's `sg`, `check` reports it as present but the other program runs. Build a derived image with
  ast-grep and allow it under the name `ast-grep`
- **Adding tools**: add `lake`, `node` and the like in an image derived from this one
  ```dockerfile
  FROM fuseforks:local
  USER root
  RUN apt-get update && apt-get install -y --no-install-recommends nodejs && rm -rf /var/lib/apt/lists/*
  USER 10001
  ```
- **Version number**: `.git` is not sent to the image build, so pass the version as a build argument.
  Without it `fuseforks-cli --version` reports `0.0.0`
  ```bash
  FUSEFORKS_CLI_VERSION="$(git describe --tags --abbrev=0 | sed 's/^v//')+g$(git rev-parse --short HEAD)" docker compose build
  ```
- **Do not run two copies of the same village** in two containers. Pre-check approvals are tied to the village ID,
  so every copy claims to be the same village
- The container sends data to the same places as the GUI (LLMs, MCP servers, Jev, the price table). Running in a
  container sends nothing new
