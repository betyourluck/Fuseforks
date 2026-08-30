# Spec: リモート MCP — `mcp.json` の `type: "http"` サーバーへ繋ぐ

- 起票: 2026-08-30
- 状態: **Done（2026-08-30。rev2.1 承認 → P0〜P4 を同日）**
- 起点: 実機（2026-08-30）— 利用者が `mcp.json` へ `type: "http"` + `url` +
  `headers` のエントリ（elyth = `https://elythworld.com/api/mcp/remote`）を書いたら
  `[SERDE_FAILED] missing field 'command'` で保存ごと拒否された。
  利用者「繋げないとまずいかな。今はリモート MCP が主流だし」

## Goal

`mcp.json` に **HTTP のリモート MCP サーバー**を書けるようにする
（Streamable HTTP。Claude Desktop / Claude Code の設定と同じ書き方）。
stdio（コマンド起動）の既存エントリは **1 バイトも変えずに**そのまま動く。

**目立つ欠落を埋める側の機能**で、Spec 23（画像）と同じ Goal の形 —
「使わない村の挙動はゼロ変更、使う人には受け皿がある」。

## 起票前の実測（2026-08-30）

- `McpServerConfig`（`mcp.rs:59`）は `command` 必須の 1 形しかなく、
  `type` / `url` / `headers` の欄が無い。elyth のエントリはオブジェクトが
  閉じる位置で `missing field 'command'` になる
- **`"type": "stdio"` は今も黙って無視されている**（unknown field）。
  Claude Desktop の設定からの写しで普通に入ってくる行なので、正式に受理する側へ
- ~~アプリは `mcp.json` を再シリアライズしない~~ → **半分誤りだった**（P1 で
  再訂正）。生テキスト保存なのは**エージェント別**（`write_config`）だけで、
  **共通の `{workspace}/mcp.json` は `write_mcp_config(McpConfig)` が構造体から
  `to_string_pretty` で書き戻す**（McpDialog の保存経路）。rev2.1 の
  「http は `type` を必ず出力」は golden の都合ではなく**本番のファイル破壊を
  防ぐ側** — rev2 のままなら共通ダイアログで保存した瞬間に http エントリが
  stdio へ化けていた。査読の致命指摘は記録以上に正しかった
- **rmcp 2.2 は client 側の Streamable HTTP を持つ**（feature
  `transport-streamable-http-client` / `-reqwest`）。**reqwest 0.12 は
  LLM クライアントとして既にツリーに居る**（workspace 依存）ので、
  足すのは feature フラグで、新しい HTTP スタックは入らない
- 扉（Spec 25）は同じ crate の server 側 feature を既に使っている —
  クライアント側は「同じ crate の別の顔」で、依存の出自は既知

## Design

### D1: 書き方は Claude Desktop / Claude Code の Streamable HTTP と stdio に互換。`sse` は**意図的な非互換**

「キーが `mcpServers` なのは Claude Desktop の設定と互換にするため」という
既存の凍結（`mcp.rs`）の延長。**互換を主張するのは stdio と Streamable HTTP の
2 形だけ**で、旧 SSE transport（2024-11-05 仕様の `/sse` + `sessionId` の
2 エンドポイント構成）は互換に含めない — Claude Desktop は後方互換で受けるが、
この村はレガシーの受け皿を最初から持たない（rev2 — 査読 A の矛盾指摘を受けて
「互換」の射程を明示）。エントリは 2 形:

```jsonc
{
  "mcpServers": {
    "memoria": {                    // 従来形（type 無し = stdio）。不変
      "command": "D:\\memoria\\MemoriaAeterna.exe",
      "args": [], "env": {}, "enabled": true
    },
    "elyth": {                      // 新形
      "type": "http",
      "url": "https://elythworld.com/api/mcp/remote",
      "headers": { "Authorization": "Bearer …" },   // 任意
      "enabled": true
    }
  }
}
```

- `"type": "stdio"` の明示も**読みは正式に受理**する（今は黙って無視 — 受理へ
  格上げ）。**Serialize は形で分ける**（rev2.1 — 査読の致命 1 件を修正。
  rev2 の「type を常に省略」は http にも掛かる書き方で、再シリアライズの
  往復で http エントリが stdio と誤判別される自己破壊だった）:
  **stdio は `type` を常に省略**（出力は従来形に一致）、
  **http は `type: "http"` を常に出力**（省略すると次の読みで壊れる）。
  ファイルは生テキスト保存なので人が書いた `"type": "stdio"` が
  ファイルから消えることは無い（上の実測）
- `"type": "sse"` は**名指しで拒否**する。文言に前提条件を書く —
  「sse は旧形式です。接続先が Streamable HTTP に対応している必要があります
  （多くのサーバーは `/sse` ではなく `/mcp` などの単一エンドポイントです）」

### D2: 型は untagged enum にしない — 名指しの拒否と、検証の順序

serde の untagged は「どの variant にも一致しない」としか言えない
（xAI の 422 で観測した形。Spec 34/36 の実測）。**全欄 optional の raw 構造体で
受けて `TryFrom` で 2 形へ割る**。

**検証の順序を固定する**（rev2 — 査読 A の矛盾 D。順序が揺れると検収 3 の
文言が不安定になる）。1 エントリにつき**最初に当たった違反 1 つ**を、
エントリ名と欄を名指しして返す:

1. **形の判別**: `type` が `http` / `stdio` / 無し のどれか（`sse` と未知値は
   ここで拒否）
2. **相互排他（両向き — rev2.1）**: `type: "http"` に `command` / `args` /
   `env` があれば拒否（「http のエントリに command は書けません」）。
   **逆も同じ段で落とす**: `type: "stdio"` / `type` 無しに `url` / `headers` が
   あれば拒否（「stdio のエントリに url は書けません」— 黙ってどちらかを
   選ばない、は両向きで初めて成立する）
3. **必須欄**: http は `url`、stdio（type 無し含む）は `command`。
   後者の文言は「`command` がありません（リモートサーバーなら
   `"type": "http"` と `url` を指定してください）」
4. **値の形式**: `url` が空・相対は拒否。`headers` は文字列 → 文字列のみ。
   `enabled` の既定は真（既存と同じ）。**検証は `enabled` の値に関わらず
   全エントリへ掛かる — スキップされるのは接続だけ**（rev2.1。無効のまま
   不正な URL や平文 http を残せる形にしない）
5. **スキーム**: D4 の制約（https / loopback http）

なお**起点の実機入力（`type: "http"` + `url` + `headers`）は新設計では正常系**
になる（rev2 — 査読 B 1.1。rev1 の「今回のエラーはこの 2 つ目に落ちる」は
対応関係が誤り — 2 つ目に落ちるのは `type` を書き忘れた別の入力）。

### D3: `headers` は平文で受ける。ただし `env` より 1 段重いことを明記する

- keyring 参照化は本 Spec では作らない。理由は 2 つ:
  (a) `mcp.json` は設定ファイルタブの**生テキスト編集**が入口で、keyring 参照を
  作るには別の UI（値の入力欄・保存経路）が要る — 本 Spec の射程を超える
  (b) Claude Desktop の互換形がそもそも平文 headers で、写して動くことが
  この欄の存在理由
- **`env` と「同じ棚」ではない**（rev2 — 査読 A の矛盾 C）。`env` はローカルの
  子プロセスにしか渡らないが、`headers` の Authorization は**外部へ送信される**。
  リスクが 1 段重いことを doc と README に明記し、**村（workspace）を配ると
  headers のトークンごと配られる**ことを利用者が負う条件として書く
- **PRIVACY 日英は追記が必須**（rev2 — 「確認」から断定へ）。既存文言
  「利用者が自分で設定した接続先」は接続の事実は覆うが、**Authorization を
  含むヘッダーが送信されること**までは覆っていない
- `ModelTemplate::credential`（Unset / NotRequired / Keyring）と同じ形の
  参照化は別 Spec 候補として Notes に残す

### D4: URL は「https、または loopback の http」。判定は文字列照合ではなく host で

- 判定は `Url::host()` に対して行う: **IP なら `is_loopback()`（`127.0.0.0/8` と
  IPv6 `::1` を含む）、ホスト名なら `localhost` の完全一致**（rev2 — 査読 A/B の
  境界指摘。`[::1]` を含む）
- **`0.0.0.0`・プライベート帯（`192.168.` / `10.` / `172.16-31.`）・
  `host.docker.internal` は許可しない** — 平文 http で Authorization が LAN へ
  飛ぶ形を既定で塞ぐ。LAN の MCP サーバーへ繋ぐ実需が出たら、そのとき
  別途裁定する（要望が無いうちに開けない）

### D5: 接続層は transport の分岐 1 点。ただし**エラーの検知タイミングは stdio と違う**

`connect` の `TokioChildProcess::new(command)` の隣に
`StreamableHttpClientTransport`（reqwest 版・headers 注入）の腕が生える。
`().serve(transport)` 以降 — ツール列挙・`qualified_tool_name` の修飾・
`McpServerStatus` の表示・`enabled` の一時停止 — は **1 行も変えない**。

- **stdio は spawn の失敗で即エラーが出るが、HTTP は最初の `initialize` が
  返って初めて成否が確定する**（rev2 — 査読 B 2.3）。DNS 失敗 / タイムアウト /
  401・403 / 404・500 は HTTP 固有のエラーとして `McpServerStatus.error` へ
  分類文字列で写す（P2 で表を確定）
- **401 / 403 は「設定の確認が要る」として即確定し、自動で再試行しない** —
  期限切れの Bearer で叩き続ける形を作らない（接続は今も明示操作 /
  起動時の 1 回で、再接続ループは元から無い — その性質を凍結に格上げする）

### D6: 後方互換は「読み側の凍結」で機械に留める

- **golden は本 Spec で新設**する（既存の golden は carries / prompt /
  tool_spec の 3 本で、mcp のものは無い — rev2 で査読 A の「contract を変えた
  瞬間に golden が赤」は前提誤りとして反証。順序依存は存在しない）
- 凍結する主張は 2 つ: (a) **従来形（type 無し stdio）の `mcp.json` が
  そのまま読める**（fixture の読み込みが受理され続ける） (b) **Serialize は stdio で `type` を
  省略して従来形を出し、http で `type: "http"` を必ず出す**（rev2.1 —
  Serialize → Deserialize の往復で形が保存されることを golden の主張に含める）

### D7: 秘匿 — `headers` の値はエラーにもログにも出さない（rev2 新設）

`McpServerStatus.error`・`fuseforks.log`・D2 の名指しエラーの**どこにも
headers の値を載せない**。エラーへ載せるのは自分で組んだ分類文字列
（状態コード + 種別）だけで、**相手の応答本文とリクエストヘッダは載せない** —
応答本文に受信ヘッダをエコーするサーバーが実在するため、本文の転記は
ヘッダの転記になりうる。`failures.md` #71（計器は秘密の転送経路になる）の系譜。

## Tasks（Phase 分割）

- **P0**: probe — 実物のリモート MCP サーバー（elyth）へ rmcp クライアントで
  initialize → tools/list が通ることを使い捨てで確認。**トークンは環境変数から
  注入し、スクリプトにもリポジトリにも残さない**（rev2 — 査読 A）。
  **`Mcp-Session-Id` の往復が rmcp の transport 内で完結するかを確認項目に
  含める**（rev2 — 査読 B 2.2）。`cargo tree` の依存差分を実測。
  `data_contract` の `mcp_contract` 改訂
- **P1**: 型（raw 構造体 + TryFrom + D2 の順序で名指しの拒否）と読み込み。
  D6 の golden 新設 + 単体。ミューテーションで赤確認
- **P2**: 接続層（transport 分岐 + headers 注入 + D5 のエラー分類表 + D7 の
  秘匿）。結合（ループバックの最小 Streamable HTTP サーバー — Spec 25 の
  server 側 feature が既に居るのでテスト内に立てられる見込み）
- **P3**: 台帳 — README 3 言語 / DETAIL 日英の MCP の節（stdio / http の 2 形・
  **headers は平文で村と一緒に配られる**警告・**旧 sse サーバーは `/sse` から
  単一エンドポイントへのパス変更が要ることが多い**の一言）/
  **PRIVACY 日英へ追記**（リモート MCP に設定したヘッダー = Authorization を
  含む、がその接続先へ送信される）
- **P4**: 実機 — elyth を繋いでツールが生えるまで

## P0 実装記録（2026-08-30）

probe は `tests/mcp_http_probe.rs`（`#[ignore]`。トークンは `ELYTH_TOKEN`
環境変数で注入 — 値はスクリプトにもログにも残さない）。

- **依存差分は版込みで +1**（336 → 337 = reqwest **0.13.4**）。rmcp は自前の
  reqwest 0.13 を持ち込み、ワークスペースの 0.12（LLM クライアント）とは
  **別の実体**になる。起票時の「新しい HTTP スタックは入らない」は名前だけで
  数えた誤り — 版込みで数え直して訂正（unique 名の集合は 303 のまま動かず、
  重複版はその数え方の網の外）
- **TLS は rmcp の feature `"reqwest"` が鍵**（`reqwest?/rustls` を立てる）。
  `transport-streamable-http-client-reqwest` だけでは既定 Client に HTTPS
  コネクタが無く、https へ **`scheme is not http`** で落ちる（実測。
  エラー文がスキームの検査に見えるが実体はコネクタの欠如）
- **こちらから reqwest の型を名指ししない** — `StreamableHttpClientTransport::
  from_config(config)` が rmcp 側の 0.13 Client を内部で作る。`with_client` に
  ワークスペースの 0.12 Client を渡すと trait 不一致でコンパイルが落ちる
  （2 版の共存の帰結。実測）
- **認証は `auth_header(素のトークン)`** — reqwest の `bearer_auth` が
  `Bearer ` を付ける（実装読み）。`headers: {"Authorization": "Bearer x"}` を
  写すときは接頭辞を剥がして渡すか `custom_headers` を使う（P2 の判断点）
- **認証なしで実物の elyth へ到達し 401 を観測** — transport・TLS・URL は
  生きている。401 は `UnexpectedServerResponse("HTTP 401 Unauthorized:
  {応答本文}")` の形で**本文を逐語で運んでくる** = D7（応答本文を
  `McpServerStatus.error` へ転記しない）が要ることの実物
- **Bearer 付きの本走行が完走**（2026-08-30・利用者の端末から）:
  `elyth-remote v2.0.0`・protocol `2025-06-18`・**tools 26 本**が返った。
  initialize の後の tools/list が通った = **`Mcp-Session-Id` の往復は
  transport 内で完結**（査読 B 2.2 の確認項目はこれで閉じた）。
  `auth_header(素のトークン)` の読みも正しかった（401 → 200 の対）
- `data_contract` の `mcp_contract` を改訂（2 形・検証 5 段・URL 制約・
  D7 の秘匿・headers は env より 1 段重い）

## P1〜P3 実装記録（2026-08-30）

- **P1（型と検証）**: `McpServerConfig` を enum（`Stdio` / `Http`）へ。
  Deserialize は `RawMcpConfig`（全欄 optional）→ `TryFrom` で **`McpConfig` の
  層**に検証を置いた — エントリ名は map の鍵なので、**エントリ単位の try_from
  では名指しができない**。Serialize は手書きで形ごとに分け（stdio = type 省略 /
  http = type 必須）、**stdio の golden は旧 derive の実出力から捕獲した
  バイト列**（Spec 35 の golden 先行手順。捕獲時に heredoc の `\` 潰れを
  また踏んだ — Spec 46 P4 の同じ罠）。単体 11 本 + 変異 2 回で赤確認
  （loopback 判定を開く → plain_http が赤 / type 出力を消す → 往復が赤）。
  `url` を直接依存へ（reqwest が既に連れているので crate は増えない）
- **P2（接続層）**: `serve_http`（`from_config` — reqwest の型を名指ししない）+
  headers の写し（Authorization の Bearer は接頭辞を剥がして `auth_header` へ、
  他は `custom_headers`。名前・値の検査はここで初めて掛かり、**値はエラーに
  載せない**）+ `classify_http_connect_error`（純関数 — `HTTP <code>` を見つけ
  たら状態コードだけの自前の文へ差し替え、DNS / 接続拒否はそのまま通す。
  **全部を丸めると「動かないが理由が分からない」へ戻る**ので二分にした）。
  `http` crate を直接依存へ（ツリーは 1.4.2 の 1 版 — reqwest 0.12 / 0.13 で
  共有されており crate は増えない）。結合 `mcp_http_secrecy.rs` =
  **受け取った Authorization を応答本文へエコーする 401 サーバー**（D7 の
  脅威の実物）をループバックに立て、`McpServerStatus.error` にトークンが
  1 文字も出ないことを凍結。変異（分類を素通し）で赤確認。
  TS の `McpServerConfig` を 2 形 union へ（利用箇所ゼロ — 型宣言の写しのみ）。
  検収の網羅: core 876 / gui 23 / vitest 457 / clippy 0
- **P3（台帳）**: README 3 言語の MCP 行へ「stdio とリモート（Streamable
  HTTP）の両対応」/ DETAIL 日英へ「リモート MCP」の節 / **PRIVACY 日英へ
  「設定したヘッダーが接続先へ送信される・平文保存で村と一緒に配られる」を
  追記**（D3 の断定どおり）/ `data_contract` は P0 で改訂済み /
  CLAUDE.md へ続報

## 検収項目（実機）

1. **elyth が繋がる**: `type: "http"` + Bearer で保存が通り、接続状態にツール名が
   並び、サーヴァントがそのツールを 1 回呼べる
2. **既存の村は不変**: stdio だけの `mcp.json` の村で、設定ファイルタブから
   保存し直してもファイルがバイト等価（生テキスト保存の追認）で、接続も従来どおり
3. **名指しの拒否**: `type: "http"` で `url` 無しを保存すると、エントリ名と
   欠けた欄を名指しするエラーが出る（`SERDE_FAILED` の汎用文言ではなく。
   判定順は D2 の表 — 複数の違反を同時に書いた入力では 1 番の違反が出る）
4. **enabled: false が効く**: リモートエントリの一時停止で接続もツールも消える
5. **切れたリモートの報告**: URL を落として起動すると `McpServerStatus.error` に
   分類（DNS / タイムアウト等）が出て、他のサーバーは生きている
6. **秘匿**: 401 を返す接続先で、エラー表示と `fuseforks.log` のどこにも
   `Authorization` の値が現れない（D7。grep で確かめる）

## P4 実機記録（2026-08-30）

- **検収 1: 合格 — しかも 2 台のリモートで**。
  (a) **elyth**（Bearer 直指定）: 保存が通り 26 ツールが生え、ミュゼが
  `elyth__create_post` / `get_thread` / `create_reply` まで実走（19:03〜19:08 の
  `tool: … ok=true` 行）。(b) **alphaXiv**（`https://api.alphaxiv.org/mcp/v1`・
  API キーの Bearer）: ザリが `alphaxiv__discover_papers` ×2 +
  `answer_pdf_queries` ×2（body 37,725 字）で**論文調査タスクを丸ごと完走**
  （19:22〜19:23）。`reason=unsupported` も正しい — MCP ツールは理由欄の
  対象外（Spec 27 D2）がリモート経由でもそのまま効いている
- **alphaXiv で「stream だった」の正体を確定** — 旧 SSE ではなく
  **Streamable HTTP**（docs + 認証なし probe の実測）。応答の半分が
  `text/event-stream` で流れるのは Streamable HTTP の仕様の側。認証なしでは
  **HTTP 401 の行を持たない `AuthRequired`（OAuth の challenge）**が返ることを
  実測し、分類の枝を 1 本足した（「認証が必要です。headers の Authorization に
  API キー（Bearer）を設定してください」— OAuth の対話フローは Notes 2 の
  とおり射程外で、案内は API キーの側へ倒す）
- **検収 2: 合格** — `agents/agent/mcp.json` に stdio（`type: "stdio"`
  明示の memoria / type 無しの MCP_DOCKER）と http（elyth / alphaxiv）が
  同居して保存され、**両方が同時に稼働**（画面: memoria 16 ツール・
  MCP_DOCKER 27 ツールが接続済み + ザリが `memoria__store_memory` を実走 =
  19:28 の `tool:` 行。http 側は検収 1 で実走済み）
- **検収 3: 合格** — `url` 行を消した保存が
  「`elyth`: type が http ですが url がありません」で拒否された（エントリ名と
  欄の名指し。旧 `missing field 'command'` の汎用文言からの置き換わりが
  実機で確認できた）。画面の `[SERDE_FAILED] 直列化に失敗しました:` の
  前置きは既存のエラー枠で、名指しの本文はその中に入る
- **検収 4〜6 は機械の網へ降ろした**（利用者裁定 2026-08-30 — Spec 44
  検収 4b と同じ形）: 4 の接続スキップは stdio と共有の既存経路
  （`enabled: false` の表示「無効化されています」は実機の画面にも写った）/
  5 の分類は単体（`classify_http_connect_error` の 4 本）/ 6 の秘匿は結合
  `mcp_http_secrecy.rs`（**Authorization を応答本文へエコーする 401
  サーバー**で、トークンが 1 文字も出ないことを変異込みで凍結）

**検収 6 件（実機 3 + 機械 3）で P4 完了・Spec 47 は Done**（2026-08-30）。

## 査読記録（rev2.1・2026-08-30）

rev2 承認査読が**致命 1 件**を出した — 「Serialize は type を常に省略」が
http にも掛かり、typed 出力の往復で http エントリが stdio と誤判別されて
`command がありません` に落ちる自己破壊。**stdio = 省略 / http = 必須出力**へ
分けて修正（D1 / D6）。軽微 2 件（相互排他の逆向き / enabled: false でも
検証は掛かる）も同時に本文へ畳んだ。

## 査読記録（rev2・2026-08-30）

査読 2 系統（A = 矛盾 4 + 穴 4 + 齟齬 3 / B = 論理 3 + 仕様すり合わせ 3 +
運用 2）。**採用 13 / 前提を訂正して採用 2 / 反証 1**:

- **採用**: A-矛盾A（互換の射程を明示）/ A-矛盾C + B-3.1（headers は env より
  重い・PRIVACY 追記断定・D7 新設）/ A-矛盾D（検証順序の固定）/ A-穴1〜4
  （相互排他・値の形式・loopback 定義・sse の案内）/ A-検収1（probe の
  トークン運用）/ B-1.1（D2 の対応関係の誤りを訂正）/ B-1.3（`::1` 含む
  host 判定）/ B-2.1（sse 拒否文言の前提条件）/ B-2.2（Session-Id を P0 へ）/
  B-2.3 + B-3.2（HTTP 固有エラーの分類と 401 の即確定）
- **前提を訂正して採用**: A-矛盾B / B-1.2 — 「保存で type が消える/生える」は
  **ファイル層では起きない**（アプリは mcp.json を再シリアライズしない —
  生テキスト保存を実装で確認）。ただし Serialize の形は未定義だったので
  `skip_serializing_if` の明記という処方は採用（D1 / D6）
- **反証**: A-齟齬1「contract を変えた瞬間に golden が赤」— mcp の golden は
  存在しない（golden は 3 本とも別物）。golden は P1 で新設する側

## Notes

1. **headers の keyring 参照化は別 Spec 候補**（D3）。形は
   `CredentialSource::Keyring` の前例（キーはエントリ名で引く・村を配っても
   鍵は配られない）。設定ファイルタブの生テキスト編集と両立する UI が論点
2. OAuth（rmcp の `auth` feature・Dynamic Client Registration）は射程外。
   Bearer を人が取って貼る形が今の主流の運用で、OAuth フローはブラウザ連携ごと
   要るので別の獣
3. 旧 SSE transport の受け皿を後から足す判断が来たら、D1 の拒否文言が
   その要望の頻度計になる（名指しで拒否している限り、踏んだ人は必ず気づく）
