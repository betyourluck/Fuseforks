# Spec 67: リモート MCP を OAuth の認可で繋ぐ

- 状態: **Draft rev2**（2026-10-11 起票 → 同日、査読 3 系統 20 点（重なりを除いて 18 項目）+ 仕様の本文から拾った 1 項目を反映して rev2・未決ゼロ。
  表は Notes 3。P0 の実測のうち、TradingView への動的登録と戻り先の検査は起票前に済ませた — 「前提の実測」が正）
- 起点: 利用者（2026-10-11）—「Authorize で MCP を繋げられるようにしたい。TradingView の MCP を使いたいというのがある。
  あと時代は Authorize MCP になっている」。参照: TradingView の告知
  （https://www.tradingview.com/blog/ja/tradingview-mcp-server-public-beta-60864/）
- 前提の Spec: [Spec 47](47_remote-mcp.md)（リモート MCP。Notes 2 で OAuth を射程外にした）/
  [Spec 64](64_headless-host.md)・[Spec 65](65_container-deploy.md)（GUI なしの実行・`--secrets keyring|env`・起動前検査・`bake`）
- 一次資料: MCP 仕様 2025-11-25「Authorization」（https://modelcontextprotocol.io/specification/2025-11-25/basic/authorization）

## Goal

`mcp.json` の http エントリに `"auth": "oauth"` と書くと、**利用者がブラウザでログインして承認するだけで**、
そのサーバーへ繋がるようにする。トークンを人が取って貼る経路（Spec 47 の `headers`）が無いサーバー —
TradingView のように OAuth 2.1 だけを受けるもの — を村で使えるようにする。GUI で一度認可すれば、
同じ端末の `fuseforks-cli`（資格情報ストアを使う既定の構成）からも使える。

**覆す判断が 1 つある。** Spec 47 Notes 2 は「OAuth はブラウザ連携ごと要るので別の獣」として射程の外に置き、
`AuthRequired` を受けたら「headers の Authorization に API キーを設定してください」と案内している。この Spec は
OAuth を宣言したエントリに「認可する」の入口を作り、宣言していないエントリの案内にも `"auth": "oauth"` の選択肢を足す（D11）。

## 前提の実測（2026-10-11）

**TradingView の MCP**（認証なしで叩いた）:

- `POST https://mcp.tradingview.com/mcp`（initialize）→ **401**、
  `WWW-Authenticate: Bearer resource_metadata="https://mcp.tradingview.com/.well-known/oauth-protected-resource/mcp"`
  （**`scope` は付かない**）、本文 `{"detail":"This server requires OAuth authentication"}`
- 保護リソースのメタデータ（RFC 9728）: `resource` = `https://mcp.tradingview.com/mcp`、
  `authorization_servers` = `["https://www.tradingview.com"]`、`bearer_methods_supported` = `["header"]`
  （**`scopes_supported` は無い**）
- 認可サーバーのメタデータ（RFC 8414、`/.well-known/oauth-authorization-server`。`openid-configuration` は 404）:

| 欄 | 値 |
|---|---|
| `authorization_endpoint` / `token_endpoint` | `/mcp/oauth/authorize` / `/mcp/oauth/token` |
| `registration_endpoint` | `/mcp/oauth/register`（**動的登録あり**） |
| `revocation_endpoint` | `/mcp/oauth/revoke` |
| `grant_types_supported` | `authorization_code` / `refresh_token` |
| `token_endpoint_auth_methods_supported` | `none` / `client_secret_basic` / `client_secret_post` |
| `code_challenge_methods_supported` | `S256` |
| `scopes_supported` | `mcp:read` / `mcp:tools` |

  `client_id_metadata_document_supported` は無い（Client ID Metadata Document は使えない）
- **動的登録は公開クライアントでループバックの戻り先を受理する** — `redirect_uris: ["http://127.0.0.1:53682/callback"]`、
  `token_endpoint_auth_method: "none"`、`application_type: "native"` で **201**、`client_secret: null`。
  P0 の試験用に 1 件登録した（`client_name: "Fuseforks (P0 probe)"`）。登録の管理口（RFC 7592）はメタデータに無いので、
  この 1 件は消せずに残る（使われないだけ）
- **認可口の戻り先の検査**（ログインの手前の応答で判定。誤りは 400、通れば 302 でログイン画面へ）:

| `redirect_uri` | 応答 |
|---|---|
| `http://127.0.0.1:53682/callback`（登録どおり） | 302 |
| `http://127.0.0.1:53999/callback`（**ポートだけ違う**） | **302** |
| `http://localhost:53682/callback`（ホスト名が違う） | 400 |
| `http://127.0.0.1:53682/other`（パスが違う） | 400 |

  **ポートは問わず、ホスト名とパスは厳密に照合する**（RFC 8252 §7.3 どおり）
- 利用条件は TradingView 側: Essential 以上の有料プラン / ベータ期間中は 1 日の要求数に上限がかかる場合がある /
  市場データは遅延（告知の本文）

**MCP 仕様 2025-11-25（本文を確認）**:

- **スコープの選び方**: ① 401 の `WWW-Authenticate` の `scope` → ② 保護リソースのメタデータの `scopes_supported` を全部 →
  **② が無ければ `scope` パラメータを送らない**。認可サーバーのメタデータの `scopes_supported` は順位に入らない
- **公開クライアントには、認可サーバーはリフレッシュトークンを回さなければならない（MUST）** — 使うたびに新しいものが返り、
  古いものは無効になる、が標準の挙動
- **`code_challenge_methods_supported` が無ければ、クライアントは進んではならない（MUST）**。`S256` を使う（MUST）
- `resource`（RFC 8707）は認可要求とトークン要求の両方に必ず付ける。正準形は「scheme と host を小文字・fragment なし・
  末尾の `/` は意味が無ければ付けない」
- 登録の優先順: 事前登録 → Client ID Metadata Document → 動的登録 → 利用者に入力させる。事前登録の値を入れる口は SHOULD
- 戻り先は `localhost`（ループバック）か HTTPS。`state` を使い、不一致は捨てる（SHOULD）

**rmcp 2.2**（今使っている版）:

- `auth` feature に OAuth のクライアント側が一通りある（`transport/auth.rs` 6,093 行）—
  `AuthorizationManager::discover_metadata`（保護リソースのメタデータ → 認可サーバーのメタデータ）/
  `register_client`（動的登録）/ `get_authorization_url`（PKCE と `resource` を付ける）/
  `exchange_code_for_token_with_issuer` / `get_access_token`（期限の手前で自動リフレッシュし、失敗したら
  `AuthError::AuthorizationRequired` を返す。ブラウザは開かない）/ `refresh_token`
- **`AuthClient<C>` が `StreamableHttpClient` を実装している**（`transport/common/auth/streamable_http_client.rs`）—
  今の `serve_http` のクライアントを差し替えるだけで、トークンの付与が要求ごとに掛かる。`AuthClient` は `Clone` で、
  中身は `Arc<Mutex<AuthorizationManager>>`
- **`get_access_token` は毎回 `credential_store.load()` を読み、`token_response` が無ければ `AuthorizationRequired`**。
  リフレッシュも `load` した `token_response` の `refresh_token()` から始める（D4 の二層化が要る理由）
- **`select_scopes` の順は仕様と 1 段違う** — ① `WWW-Authenticate` → ② 保護リソース → **③ 認可サーバーの
  `scopes_supported` を全部** → ④ 既定値（SEP-835 と書いてあるが、③ は仕様の外）。この Spec は rmcp の選び方を使わない（D9）
- 資格情報の置き場は trait `CredentialStore`（`load` / `save` / `clear`）で差し替えられる。既定はメモリだけ
- **Client ID Metadata Document は持たない** — 動的登録だけ。TradingView も Client ID Metadata Document を名乗っていない
- **依存の増分は 10 crate**（`cargo tree -p fuseforks-core -e normal` の前後。`oauth2` 5.0 / `rand` 0.8 系 3 本 /
  `base64` 0.21 / `thiserror` 1 系 2 本 / `serde_path_to_error` / `zerocopy` 2 本。**すべて純 Rust**。旧い版の並存を含む）

**Fuseforks の今**:

- `serve_http`（`mcp.rs`）は `StreamableHttpClientTransport::from_config` で繋ぎ、`headers` の
  `Authorization: Bearer …` だけを rmcp の `auth_header` へ渡す
- `AuthRequired` は `classify_http_connect_error` が「API キー（Bearer）を設定してください」へ写す（alphaXiv で実測）
- MCP の道具一覧は**接続した時に 1 回だけ**取る（`connect_one`）。接続の寿命は個体の稼働に一致し、
  同じ接続先でも個体ごとに別の接続を持つ
- ブラウザを開く `tauri-plugin-opener` は既にある
- 秘密の置き場は `SecretStore`（`get` / `set` / `delete` / `contains`）。GUI は常に `KeyringSecretStore`、
  `fuseforks-cli` は `--secrets keyring|env`（既定 keyring。`env` は読み取り専用の `EnvSecretStore`）
- 村の排他ロック（Spec 64）は**同じ村を 2 つのプロセスが開くこと**しか止めない。別の村を開いた 2 つのプロセスは、
  同じ OS ユーザーの資格情報ストアを共有する

## Design

### D1. 明示のオプトイン — `"auth": "oauth"`

```json
"tradingview": { "type": "http", "url": "https://mcp.tradingview.com/mcp", "auth": "oauth" }
```

- 欄は http エントリだけに書ける。stdio に書いたら保存の時点で名指しで拒否する（Spec 47 の検査と同じ層）
- **`headers` の `Authorization` との併記は拒否する** — 2 つの認証の席があると、どちらが効いたかが読めない。
  `Authorization` 以外の `headers` は併記できる
- 値は閉じた列挙で、今は `"oauth"` だけ。未知の値は拒否
- **401 を受けたら勝手に OAuth へ切り替える形は採らない** — ブラウザが開く理由が人に見えず、どのサーバーが
  何を求めたかを人が確かめる前に外部のログイン画面が出る。宣言していないエントリへの案内は D11

### D2. 戻り先はループバック — 受け口の寿命

- 戻り先は `http://127.0.0.1:<port>/callback`。**`localhost` は使わない**（TradingView が 400 を返した。
  名前解決の結果にも依存しない）
- ポートは**登録したときのポートを先に試し**、埋まっていれば空いているものを使う（D3 の再登録を減らすため）
- 受け口は「認可する」を押したときに開き、**次のどれかで閉じる**: (a) パスが `/callback` で `state` が一致する要求を
  1 本受けた（認可コードでも認可サーバーのエラーでも）(b) 待ちの上限 **10 分**を超えた (c) 利用者が「キャンセル」を押した
  (d) 同じ接続先の認可をもう一度始めた（古い受け口を閉じてから新しく開く）
- **(a) に当たらない要求には 400 を返し、受け口は開いたまま待ち続ける**（パスが違う / `state` が無い・違う）。
  同じ端末の別のプロセスが先に叩いても、`state` を知らなければ通らない
- 閉じるときは必ず待ち受けのソケットを解放する（4 つの経路すべて。テストで留める）
- ブラウザに返すページは自前の最小の HTML（外部の資源を読まない）。文言は「認可しました。このタブを閉じて
  Fuseforks に戻ってください」/ 失敗時は理由の分類だけ（認可サーバーが返した `error_description` は出さない —
  相手の文をそのまま画面へ写さない。Spec 47 D7 と同じ線）
- 画面は待っている間「ブラウザで認可を完了してください（残り N 分）」と「キャンセル」を出す

### D3. クライアント登録は認可サーバーごとに 1 件 — トークンとは別の鍵

- 登録の記録は**認可サーバー（`issuer`）ごと**に資格情報ストアへ置く: 鍵 `mcp-oauth-client:<issuer>`、
  値 `{ clientId, redirectUri }`（`redirectUri` は登録したときのポートを含む 1 本）。トークンの鍵（D4）とは分ける —
  1 つの認可サーバーが複数の MCP サーバーを守ることがあり、そのとき登録は 1 件で足りる
- 動的登録の `client_name` は `Fuseforks`（端末名や利用者名を足さない — 認可サーバーへ識別子を送らない）、
  `application_type` は `native`、`token_endpoint_auth_method` は `none`
- 認可口が戻り先の不一致で拒んだら（RFC 8252 に従わず、ポートまで照合するサーバー）、**その認可の中で 1 回だけ**
  登録し直し、記録を新しいものへ置き換える。古い登録は認可サーバーに残る（管理口が無ければ消せない）—
  ポートを先に登録どおりに試すのは、この残骸を増やさないため
- 認可サーバーが `code_challenge_methods_supported` に `S256` を名乗らなければ進まない（仕様の MUST）
- 動的登録を持たないサーバーは、接続の状態に「この接続先は動的登録を持たないため認可できません」と出して終わる
  （事前登録の値を入れる口は作らない — 「採らなかった形」）

### D4. トークンの置き場は二層 — OS には長命のものだけ

**鍵** `mcp-oauth:<resource>`。`<resource>` は保護リソースのメタデータの `resource` を MCP 仕様の正準形へ揃えたもの
（scheme と host を小文字・既定のポートを落とす・fragment を落とす・末尾の `/` を落とす。query は残す）。
メタデータに `resource` が無ければ `url` を同じ規則で揃えたもの。**同じ接続先なら個体が何体いても 1 組**。

**OS の資格情報ストアに置くもの**（JSON 1 つ）: `{ issuer, refreshToken, scopes, receivedAt }`。
**アクセストークンは置かない** — 保存する秘密を減らすためと、Windows の資格情報マネージャーの値の上限
（`CRED_MAX_CREDENTIAL_BLOB_SIZE` = 2,560 バイト。**記憶に基づく値で、P0 で実測して確かめる**）に JWT が近づきうるため。

**rmcp の `CredentialStore` との繋ぎ方**（`KeyringCredentialStore`。接続先ごとに 1 つ、全接続で共有）:

| 操作 | 振る舞い |
|---|---|
| `load` | メモリに完全な `StoredCredentials` があればそれを返す。無ければ OS の記録から組み立てる — `token_response` は `refreshToken` を持ち、アクセストークンは空・`expires_in` 0、`token_received_at` 0。**rmcp の `get_access_token` はこれを「期限切れ」と読んで 1 回リフレッシュする**（起動直後の経路）。OS の記録も無ければ `None`（= `AuthorizationRequired`） |
| `save` | メモリを完全なもので置き換え、OS の記録はリフレッシュトークンかスコープが変わったときだけ書き直す。**世代（D7）が古ければ何もしない** |
| `clear` | メモリと OS の両方を消し、世代を進める |

- **rmcp の挙動への依存を P1 のテストで留める** —「OS にリフレッシュトークンだけがある状態で `get_access_token` を呼ぶと、
  トークン要求がちょうど 1 回飛び、得たアクセストークンが返る」。rmcp を上げてこの読み方が変わったら赤くなる
- **スコープの照合**: 要求するスコープ（D9）が記録の `scopes` に含まれなければ、状態は `required`（再認可）。
  記録のトークンを狭いスコープのまま使い回さない
- **村を配ってもトークンは付いていかない**（`mcp.json` には `"auth": "oauth"` しか無い）。`${secret:…}` と同じ線

### D5. 接続先ごとに認可の管理者を 1 つ — リフレッシュの取り合いを避ける

- 公開クライアントでは認可サーバーがリフレッシュトークンを回す（仕様の MUST）。個体ごとに `AuthorizationManager` を
  持つと、同じリフレッシュトークンで 2 体が同時にリフレッシュし、**片方が無効なトークンを掴んで `required` へ落ちる**
- 処方は、プロセスの中に接続先ごとの `AuthClient` を 1 つだけ置き（`Arc<Mutex<AuthorizationManager>>` を共有）、
  各個体の接続はその複製を使うこと。リフレッシュは `Mutex` で直列になり、2 体目は新しいトークンを読む
- 置き場はコアの `McpOAuthRegistry`（`resource` → `AuthClient` + `KeyringCredentialStore`）。個体の接続を作るときに引く
- **プロセスをまたぐ取り合い**（別の村を開いた 2 つのプロセス、GUI と `fuseforks-cli`）は `Mutex` では止まらない。
  処方は「リフレッシュが `invalid_grant` で拒まれたら、メモリを捨てて OS の記録を読み直し、リフレッシュトークンが
  変わっていれば 1 回だけやり直す」— もう一方のプロセスが回した新しいトークンを拾う。変わっていなければ `required`。
  ファイルロックは作らない（同時にリフレッシュする窓は秒単位で、読み直しで足りる）

### D6. 状態の遷移 — 自動ではブラウザを開かない

状態は**接続先ごと**に持ち（`McpOAuthRegistry`）、個体の接続の有無と独立に決まる。`McpServerStatus` に `auth` を足して写す:

| 状態 | 条件 | 接続 |
|---|---|---|
| `none` | `"auth": "oauth"` でないエントリ | 従来どおり |
| `authorized` | OS の記録があり、要求するスコープを含む（アクセストークンの有無・期限は問わない） | 試みる。アクセストークンは rmcp が要るときに得る |
| `required` | OS の記録が無い / スコープが足りない / リフレッシュが拒まれ D5 の読み直しでも回復しない | **試みない**（`serve_http` を呼ばない）。`error` に「認可が必要です（MCP の設定の「認可する」から）」 |

- 遷移: `required` →（認可が済む）→ `authorized` / `authorized` →（リフレッシュの拒否・取り消し）→ `required`。
  アクセストークンの期限切れは遷移ではない（rmcp が黙ってリフレッシュする）
- **個体の起動・再接続・ターンの途中のどれでもブラウザは開かない**。開くのは人が「認可する」を押したときだけ
- ターンの途中でリフレッシュが拒まれたら、そのツール呼び出しは「認可が切れました。利用者に再認可を頼んでください」を
  本文で返す（次の手を書く — #44）。状態は `required` へ落ちる
- 稼働していない個体の設定画面にも、接続先の状態（`authorized` / `required`）をそのまま出す — 接続していないから
  `required` に見える、を作らない

### D7. 入口 — 「認可する」「認可を取り消す」と、取り消しの完全性

- 共通 MCP のダイアログと、サーヴァントの編集ダイアログの MCP タブの状態の行に、OAuth のエントリだけボタンを出す
- 「認可する」: コアが受け口を開き（D2）、認可 URL を作り、ブラウザで開く（`opener`）→ 戻りを受けてコードを交換 →
  保存 → **その接続先を使っている稼働中の個体の MCP を、MCP の設定を保存したときと同じ経路で繋ぎ直す**
  （道具一覧は接続時にしか取らないため。ターンの途中の扱いはその既存の経路に従う — P2 で読んで記録する）
- 「認可を取り消す」は次を**この順で**行う:
  1. 接続先の世代を進める（`KeyringCredentialStore` が持つ番号。共有しているすべての `AuthClient` の複製が同じ番号を見る）—
     **飛行中のリフレッシュが後から `save` しても書き戻されない**
  2. メモリと OS の記録を消す（`clear`）
  3. その接続先を使っている個体の MCP 接続を切り、状態を `required` へ
  4. 認可サーバーに `revocation_endpoint` があればリフレッシュトークンを失効させる。失敗しても 1〜3 は戻さない
     （手元に残さないことを優先し、失敗は通知に出す）
  - レジストリから取り除くだけでは足りない — 個体の接続が `AuthClient` の複製を持ち続けるため。1 の世代と 3 の切断で塞ぐ
- 認可はブラウザ側のログインなので、Fuseforks は TradingView のパスワードを一度も見ない

### D8. GUI なしの実行 — 資格情報ストアなら使える

- `fuseforks-cli --secrets keyring`（既定）: **OS の記録があれば使う**（`authorized`）。リフレッシュで回ったトークンも
  書き戻せる。記録が無いときだけ、起動前検査（`headless_preflight`）の拒否 **`MCP_OAUTH_REQUIRED`** で名指しして止める
  （ブラウザが無いので、ここでは認可できない。「GUI で認可してから」と案内する）
- `fuseforks-cli --secrets env`（コンテナ）: **拒否 `MCP_OAUTH_HEADLESS`**。`EnvSecretStore` は読み取り専用で、
  リフレッシュトークンは回る（仕様の MUST）ので、1 回目のリフレッシュで渡したトークンが無効になり、次の起動で必ず切れる
- `bake`: OAuth のエントリが写しに入るときは、`bake` の出力に警告を 1 行出して名指しする（止めない）。写しの起動前検査が
  `MCP_OAUTH_HEADLESS` で止める、と DETAIL の「コンテナで回す」にも書く

### D9. スコープ — MCP 仕様の順、認可サーバー側は使わない

- ① 401 の `WWW-Authenticate` の `scope` → ② 保護リソースのメタデータの `scopes_supported` を全部 →
  **どちらも無ければ `scope` パラメータを送らない**（認可サーバーの既定に委ねる）
- **認可サーバーの `scopes_supported` は使わない** — 仕様の順位に入っておらず、管理者用や排他のスコープを含みうるので、
  要求すると同意画面の手前で `invalid_scope` になる、あるいは過剰な権限を求める。rmcp の `select_scopes` は ③ として
  これを使うので、**呼ばずに自前の 1 本で選ぶ**
- 認可サーバーが `offline_access` を名乗っていれば足す（OIDC 系はこれが無いとリフレッシュトークンを出さない。rmcp と同じ）
- TradingView は ① も ② も無いので `scope` を送らない。**それで道具が使えるトークンが出るかは P4 で見る**。
  実行時に `403 insufficient_scope` が来たら、`WWW-Authenticate` の `scope` で再認可が要る状態（`required`）へ落とす
  （自動の再認可はしない — D6）

### D10. 計器 — 値を 1 文字も書かない

- `mcp oauth: server=… resource=… outcome=…` を 1 行。`outcome` は閉じた列挙
  （`authorized` / `refreshed` / `refetched`（D5 の読み直しで回復）/ `required` / `denied` / `timeout` / `cancelled` /
  `reregistered` / `no_registration` / `no_pkce` / `revoked` / `error`）
- トークン・認可コード・`state`・PKCE の値・`client_id`・認可サーバーの応答本文は書かない（#71 の系譜）。
  `client_id` は秘密ではないが、利用者のアカウントへ紐づく識別子なので書かない
- `resource`（と表示しない `issuer`）は**サーバーの識別子で、利用者の個人情報ではない**ので書く — 既存の計器も
  接続先の名前を出している。利用者のアカウント名やメールは Fuseforks の手元に無い（ブラウザ側のログイン）

### D11. 宣言していないエントリへの案内

- `"auth": "oauth"` の無い http エントリが `AuthRequired`（`WWW-Authenticate` に `resource_metadata` を持つ 401）を
  受けたら、案内を「このサーバーは OAuth の認可を求めています。`mcp.json` に `"auth": "oauth"` を足して「認可する」を
  押すか、API キーを受けるサーバーなら headers の Authorization に設定してください」にする（両方の選択肢を出す）
- 自動では書き換えない（D1）

### D12. 外へ送るもの・保存するもの（PRIVACY）

- **新しい送り先**: 認可サーバー（メタデータの取得 / 動的登録 / コードの交換 / リフレッシュ / 失効）。
  送るのは クライアント名 `Fuseforks`・ループバックの戻り先・PKCE の値・`resource`・認可コード・リフレッシュトークン・
  `client_id`。保護リソースのメタデータは MCP サーバーへ取りに行く
- **OS の資格情報ストアに保存するもの**: 接続先ごとに `issuer`・リフレッシュトークン・スコープ・受け取った時刻 /
  認可サーバーごとに `client_id`・登録した戻り先
- **メモリだけに置くもの**: アクセストークン（プロセスが終われば消える）
- 開発者へは何も送らない（今までどおり）。PRIVACY 日英の 3「認証情報」と 4-3「MCP サーバー」に書く。
  **この節は P0 で契約に含める**（外部との送受信の定義なので）

## 採らなかった形

- **401 で自動的に OAuth へ切り替える** — D1。ブラウザが開く理由が人に見えない（案内は D11 で足す）
- **認可サーバーの `scopes_supported` を全部要求する** — rev1 の推奨だったが、MCP 仕様の順位に無い（rmcp の実装を
  写した誤り）。D9
- **事前登録の値（`client_id`）を人が書く欄** — 仕様は SHOULD だが、今の対象（TradingView）は動的登録を持つ。
  要る接続先が実際に出てから足す（頻度を見てから）
- **Client ID Metadata Document** — rmcp 2.2 に無く、TradingView も名乗っていない。自前で書くと `client_id` として
  公開 URL を用意する必要があり、配布物の外に住むものが 1 つ増える
- **カスタムスキーム（`fuseforks://callback`）の戻り先** — OS へのスキーム登録が要り、3 OS で作法が違う。
  ループバックで足りることを実測した
- **アクセストークンも保存する** — D4。保存する秘密が増え、値の上限に近づく
- **個体ごとの認可** — 同じ人のアカウントを個体ごとに何度も承認させる理由が無い。個体ごとに別のアカウントで
  繋ぎたい需要が出たら、鍵に個体を含める（頻度を見てから）
- **プロセスをまたぐファイルロック** — D5 の読み直しで足りる
- **コンテナへリフレッシュトークンを環境変数で渡す** — D8。回るので 1 回で切れる
- **GUI なしでのデバイスフロー（RFC 8628）** — TradingView のメタデータに無い
- **`client_name` に端末名を足す** — 認可サーバーの管理画面での区別より、識別子を送らないことを取る

## Tasks

### P0 — 契約と残りの実測

- `data_contract.yaml` に `mcp_oauth_contract`（D1〜D12 の凍結。D12 の送り先と保存先を含む）。Spec 47 の `remote_mcp` の節へ
  「Notes 2 を覆した」を追記
- Windows の資格情報マネージャーに 2,560 / 2,561 / 4,096 バイトの値を `KeyringSecretStore` で書いて、上限を実測する
  （D4 の根拠を記憶から実測へ）
- rmcp の `auth` feature を足して `cargo build` が通ること・依存の増分（10 crate）を確定
- rmcp の `get_authorization_url` に空のスコープを渡したとき、`scope` パラメータを送らないかを読む（D9。送るなら
  自前で URL を組む）

### P1 — コア

- `mcp.rs`: `auth` 欄の型と検査（D1）/ 鍵の正準化（D4）/ `KeyringCredentialStore`（二層・世代。D4・D7）/
  `McpOAuthRegistry`（D5・D6）/ `serve_http` の OAuth の枝（`AuthClient` を `StreamableHttpClientTransport` へ）/
  スコープの選び方（D9）/ `McpServerStatus.auth` / 案内（D11）/ 計器（D10）
- `headless.rs`: `MCP_OAUTH_REQUIRED` / `MCP_OAUTH_HEADLESS`（D8）
- テスト（ループバックの最小 HTTP スタブで認可サーバーと MCP サーバーを立てる。`attachment_fallback.rs` と同じ水準）:
  - OS にリフレッシュトークンだけ → トークン要求がちょうど 1 回 → 繋がる（**rmcp の読み方を留める**）
  - OS の記録が無い → `required`、MCP サーバーへの要求は 0 本
  - 2 体が同じ接続先へ繋いでもリフレッシュは 1 回（D5）
  - リフレッシュが `invalid_grant` → OS の記録が別のトークンに変わっていれば 1 回やり直して回復（`refetched`）/
    変わっていなければ `required`
  - 取り消しの後に、飛行中だったリフレッシュの `save` が書き戻されない（世代）
  - スコープが記録に含まれない → `required`
  - `headers` の `Authorization` との併記の拒否 / stdio への `auth` の拒否 / `S256` の無い認可サーバーで止まる
  - 鍵の正準化（末尾の `/`・大文字の host・fragment が同じ鍵になる）

### P2 — 認可の流れ（ホスト）と画面

- `fuseforks-host` に認可の流れ（受け口と 4 つの閉じ方・`state` の照合・コードの交換・登録し直し・失効）
- GUI: IPC `mcp_oauth_authorize` / `mcp_oauth_cancel` / `mcp_oauth_revoke` / 状態の行のボタン・残り時間・キャンセル /
  辞書 ja・en / 戻り先のページ
- 認可が済んだら、その接続先を使う稼働中の個体の MCP を繋ぎ直す（既存の経路を読み、ターンの途中の扱いを記録する）
- `bake` の警告（D8）
- テスト: 受け口が 4 つの経路のどれでもソケットを解放する / `state` 違いは 400 で待ち続ける

### P3 — 台帳

- DETAIL 3 言語（リモート MCP の節に OAuth・「コンテナで回す」に D8）/ README 3 言語に 1 行 / PRIVACY 日英（D12）/
  Spec 47 Notes 2 の覆し

### P4 — 実機

1. TradingView を共通 MCP に `"auth": "oauth"` で書く → 状態が `required`、ブラウザは開かない
2. 「認可する」→ ブラウザでログインして承認 → `authorized`、道具の一覧が出る（**`scope` を送らずに道具が使えるか** — D9）
3. 個体に TradingView の道具を 1 本呼ばせて結果が返る
4. アプリを再起動しても、ブラウザを開かずに繋がる（OS の記録から 1 回リフレッシュ）。TradingView がリフレッシュトークンを
   回すかを見る
5. 2 体が同じ接続先を使い、ログの `refreshed` が同時に 2 本出ない（D5）
6. 「キャンセル」と 10 分の時間切れで受け口が閉じる（`timeout` / `cancelled`）
7. 「認可を取り消す」→ `required` へ戻り、資格情報ストアから鍵が消える
8. GUI で認可した後、`fuseforks-cli check --secrets keyring` が通り、`--secrets env` が `MCP_OAUTH_HEADLESS` で止まる

## 未決

（なし。rev1 の未決 2 つは査読で閉じた — スコープは D9、待ちの上限は D2 の 10 分）

## Notes

### 1. 起点の運用

利用者は Claude（claude.ai のコネクタ）で既に TradingView の MCP を使っている。村のサーヴァントにも使わせたい、
が直接の動機。TradingView の告知は対応クライアントに Claude Code を挙げ、接続の手順を「URL をコネクタに足す →
ログインして承認する」の 2 段で書いている — この Spec が作るのは後半の段。

### 2. 誰が何を見るか

認可はブラウザ側で完結するので、Fuseforks はアカウントのパスワードもアカウント名も見ない。Fuseforks が持つのは
承認の結果として渡されたトークンだけで、置き場は D12 のとおり — OS の資格情報ストアに長命のもの（リフレッシュトークン・
`client_id` など）、メモリに短命のもの（アクセストークン）。

### 3. rev1 の査読の反映（2026-10-11。3 系統 20 点・重なりを除いて 18 項目 + 仕様の本文から 1 項目）

| # | 指摘 | 系統 | 判定 | 反映 |
|---|---|---|---|---|
| 1 | スコープを「無ければ認可サーバー側を全部」は過剰権限・`invalid_scope` の危険 | A | **採用**（仕様の本文で確認。rmcp の実装を写した誤り） | D9 |
| 2 | 待ちの上限 5 分は二段階認証で足りない。キャンセルとソケットの解放 | A・C | 採用 | D2（10 分・4 つの閉じ方） |
| 3 | D2「1 本で閉じる」と「400 を返して待つ」が矛盾 | B | 採用 | D2（閉じる条件を (a)〜(d) に） |
| 4 | D3 の認可サーバー単位の登録が、D4 の接続先単位の保存だけでは使い回せない | B | 採用（「必ず重複登録」とまでは言わない — 指摘どおり） | D3（`mcp-oauth-client:<issuer>`） |
| 5 | 取り消しが OS の鍵だけで、メモリ・複製・飛行中のリフレッシュが未定義 | B | 採用 | D7（世代・順序） |
| 6 | Notes 2「持つのはリフレッシュトークンだけ」が D4 と矛盾。D12 の一覧が足りない | B | 採用 | Notes 2・D12 |
| 7 | OS にリフレッシュトークンだけでは rmcp の `load` → `get_access_token` が動かない | B・C | **採用**（rmcp を読んで確認） | D4（二層・起動の経路・P1 のテスト） |
| 8 | GUI なしを一律に拒否すると、資格情報ストアにあるトークンも使えず目的を潰す | C | 採用（CLI の既定は keyring と確認） | D8（keyring は使える・env だけ拒否） |
| 9 | 毎回空きポートだと、ポートまで照合するサーバーで毎回再登録になり残骸が増える | C | 採用 | D2（登録どおりのポートを先に）・D3 |
| 10 | 鍵の正準化（末尾 `/`・大文字・fragment）とスコープ変更が未定義 | C | 採用（正準形は仕様の本文に従う） | D4 |
| 11 | 宣言していない OAuth 専用サーバーへ「API キー」と案内してしまう | C | 採用 | D11 |
| 12 | プロセスをまたぐリフレッシュの取り合い | C | 採用（形を変えて — ファイルロックではなく読み直し） | D5 |
| 13 | 状態の遷移が未定義（記録あり・アクセストークン期限切れ、接続を試みるか） | C | 採用 | D6（表） |
| 14 | `bake` の写しが必ず止まる | C | 採用 | D8（警告と DETAIL） |
| 15 | 認可後の繋ぎ直しとターンの競合 / 稼働していない個体の状態 | C | 採用（繋ぎ直しは既存の経路に乗せ、P2 で読む） | D6・D7 |
| 16 | `client_name` に識別子が無いと管理画面で区別できない | C | **不採用**（識別子を送らないことを取る） | D3・採らなかった形 |
| 17 | ログの `resource` / `issuer` も個人情報になりうる | C | **前提を訂正して採用**（サーバーの識別子で利用者の個人情報ではない、と理由を書いた） | D10 |
| 18 | D12（PRIVACY）も契約に含めるべき | C | 採用 | D12・P0 |
| 19 | （仕様の本文から拾った）PKCE を名乗らない認可サーバーでは進まない / 公開クライアントのリフレッシュトークンは回る | — | 採用 | D3・D5・D8 |

系統 A の冒頭の「ツール実行環境の不具合」（`googlecloudtools.datacloud_telemetry` の PreToolUse フック）は、査読した側の
環境のもので、この Spec にもリポジトリにも関係しない。
