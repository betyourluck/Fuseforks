# Spec 67: リモート MCP を OAuth の認可で繋ぐ

- 状態: **rev2 → P0〜P4 完了 = Done**（2026-10-11。**起票から Done まで同日**。P4 = 実機の確認 8 件すべて観測 —
  「P4 実機記録」。P3 = 台帳 —「P3 台帳記録」。P2 = 認可の流れ（ホスト）・GUI の IPC 3 本と状態の行のボタン・`bake` の警告 —「P2 実装記録」。P1 = コアの実装と結合テスト 7 本 —「P1 実装記録」。P0 = 契約 `mcp_oauth_contract` の凍結 / Windows の資格情報マネージャーの上限は
  **UTF-16 で 1,280 単位**と実測（D4 を訂正）/ rmcp `auth` でビルドが通る / 空のスコープは `scope` を付けない —
  「P0 実測記録」）（2026-10-11 起票 → 同日、査読 3 系統 20 点（重なりを除いて 18 項目）+ 仕様の本文から拾った 1 項目を反映して rev2・未決ゼロ。
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
**アクセストークンは置かない** — 保存する秘密を減らすためと、Windows の資格情報マネージャーの値の上限に
JWT が収まらないことがあるため。**上限は P0 で実測した: UTF-16 で 1,280 単位**（`keyring` 3 は Windows で値を
UTF-16 で保存し、上限 2,560 バイトがその単位で掛かる。ASCII でも日本語でも 1,280 字は通り 1,281 字で失敗 —
「P0 実測記録」）。**OS の記録がこれを超えるなら、黙って落とさず名前の付いたエラー（`token_too_large`）で止める**
（リフレッシュトークンが長いサーバーでは JSON で包んだ記録が超えうる）。

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
- **rmcp は `resource` に、manager が受け取った URL の `Url::to_string()` を送る**（P0 で読んだ）。パスの無い URL
  （`https://mcp.example.com`）は末尾に `/` が付き、D4 の正準形とずれる。manager へは正準形の URL を渡し、
  それでも付くならトークン要求の `resource` を自前で揃える（P1 で確かめる）

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

### P0 — 契約と残りの実測（**完了**・2026-10-11。「P0 実測記録」）

- [x] `data_contract.yaml` に `mcp_oauth_contract`（D1〜D12 を不変条件 12 本で凍結。D12 の送り先と保存先を含む）。
  `mcp_contract` の末尾へ「Spec 67 で OAuth を足した・401/403 即確定は OAuth でないエントリの規律」を追記
- [x] Windows の資格情報マネージャーの上限を `KeyringSecretStore` で実測 — **UTF-16 で 1,280 単位**（D4 を訂正）
- [x] rmcp の `auth` feature を足して `cargo check -p fuseforks-core -p fuseforks-host` が通る（47 秒）
- [x] rmcp の `get_authorization_url` に空のスコープを渡すと `scope` パラメータは付かない（`oauth2` 5.0 の実装を読んだ）

### P1 — コア（**完了**・2026-10-11。「P1 実装記録」）

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
  - OS の記録が UTF-16 で 1,280 単位を超えるとき `token_too_large` で止まり、メモリだけで動き続けない（D4）
  - ~~manager へ渡す URL は正準形 — パスの無い URL でも `resource` に末尾の `/` が付かない（D9）~~ → 付くことは避けられない
    （`url` の `Url` はパスの無い URL を常に `/` で表す）。「P1 実装記録」の 4

### P2 — 認可の流れ（ホスト）と画面（**完了**・2026-10-11。「P2 実装記録」）

- `fuseforks-host` に認可の流れ（受け口と 4 つの閉じ方・`state` の照合・コードの交換・登録し直し・失効）
- GUI: IPC `mcp_oauth_authorize` / `mcp_oauth_cancel` / `mcp_oauth_revoke` / 状態の行のボタン・残り時間・キャンセル /
  辞書 ja・en / 戻り先のページ
- 認可が済んだら、その接続先を使う稼働中の個体の MCP を繋ぎ直す（既存の経路を読み、ターンの途中の扱いを記録する）
- `bake` の警告（D8）
- テスト: 受け口が 4 つの経路のどれでもソケットを解放する / `state` 違いは 400 で待ち続ける

### P3 — 台帳（**完了**・2026-10-11。「P3 台帳記録」）

- DETAIL 3 言語（リモート MCP の節に OAuth・「コンテナで回す」に D8）/ README 3 言語に 1 行 / PRIVACY 日英（D12）/
  Spec 47 Notes 2 の覆し

### P4 — 実機（**完了**・8 件すべて観測。「P4 実機記録」）

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

## P4 実機記録（2026-10-11）

**穴が 2 つ出て、同日に直した。**

1. **`www.tradingview.com` は User-Agent の無い要求を 403 で断る。** rmcp の既定のクライアントは UA を付けないので
   認可サーバーのメタデータが取れず、rmcp は黙って `{MCP の origin}/authorize` の推測の値へ落ちた（PKCE の欄が空）。
   画面には「認可サーバーが PKCE（S256）に対応していない」と出た。P0 の実測は curl（UA を自動で付ける）だったので
   見えなかった。処方は `mcp_oauth::USER_AGENT`（`Fuseforks`）を付けたクライアント — rmcp には `OAuthHttpClient` を
   自前で実装して `new_with_oauth_http_client` で渡す。`failures.md` #145
2. **D11 の案内が出なかった。** 判定は rmcp の内部表記 `AuthRequired` だけを探していたが、実機で届いたのは表示用の
   文面 `… error: Auth required, when send initialize request`。両方を見る `is_auth_required_text` へ

**観測（`fuseforks.log`）**:

| # | 結果 |
|---|---|
| 1 | `"auth": "oauth"` を書く前は認可を試さずエラー。書いた後は状態 `required`・「認可する」が出てブラウザは開かない |
| 2 | 認可が完了（`outcome=authorized`）。**`scope` を送らずに道具が使えるトークンが出た** — D9 の未知が閉じた |
| 3 | `tradingview__mcp-tv-get-screener-columns` ×7・`mcp-tv-run-screener` ×6 が全部 `ok=true`。ザリが日本株の週間騰落を表で答えた |
| 4 | 17:12:35 に起動し直し、17:13:20 に `refreshed` → `authorized`。ブラウザは開いていない（認可の流れの行が無い）。OS の記録のリフレッシュトークンから 1 回リフレッシュして繋がった |
| 5 | ザリ（`agent`）が 17:13:20、2 体目の `agent_9` が 17:13:40 に `authorized`。**`refreshed` は 1 本だけ** — 2 体目は接続先ごとに 1 つの `AuthClient`（D5）を共有し、得たトークンを使い回した。両方の個体が道具を呼んで `ok=true` |
| 6 | `outcome=cancelled`（15:30・16:54）と `outcome=timeout`（15:40 = 10 分）の両方が出た |
| 7 | 18:06:06 に `outcome=revoked remote=yes`（TradingView は `revocation_endpoint` を持ち、失効の要求が通った）。続く 18:06:32・18:06:43 に `outcome=required` が 2 本で、**その間に `refreshed` は 1 本も無い** = 取り消しの後に書き戻された記録は無い。画面のバッジが `required` へ戻り、資格情報ストアから `mcp-oauth:https://mcp.tradingview.com/mcp` が消えたことは利用者が確認した。`mcp-oauth-client:https://www.tradingview.com`（動的登録のクライアント ID）は残る — 認可サーバーごとに使い回す設計どおり |
| 8 | GUI を閉じて `fuseforks-cli check --for ask --start agent,agent_9`。keyring では `MCP_OAUTH_*` の指摘が 0 件（`--bypass-plan-review` を付けて拒否 0・終了コード 0）。`--secrets env` では `agent:tradingview` と `agent_9:tradingview` の 2 件が `MCP_OAUTH_HEADLESS`（終了コード 3） |

- **認可の直後に `outcome=refreshed` が 2 本出ている**（16:53:13 / 16:53:41）が、**2 本は別のもの**（確認 7 の後の
  認可し直しで同じ並びを見て訂正した。初版はどちらも繋ぎ直しのリフレッシュと読んでいた）。`refreshed` は `save` で
  OS の記録が変わったときに出る（P1 実装記録 6）ので、**コードの交換で記録を初めて書いた時にも出る**。見分けは直後の
  `authorized` までの間隔で、**40 ms なら認可の流れ**（交換 → 保存 → `mcp_oauth_flow` の `authorized`）、**0.8 秒前後なら
  繋ぎ直し**（リフレッシュ → 保存 → MCP の接続の `authorized`）。16:53:13 / 18:07:48 が前者、16:53:41 / 17:13:20 /
  18:08:01 が後者。**TradingView がリフレッシュのたびにリフレッシュトークンを回している**という読みは、記録が既に
  あった後者の行だけから言える（確認 4 の半分）。計器の名前は変えていない — 交換の初回保存を別の語で出すなら
  `data_contract` の列挙も動く（頻度を見てから）
- **`mcp-tv-run-screener` の出力は 71,901 字・90,282 字**で、圧縮は `outcome=structured`（入れ子の JSON で配列型に
  当たらない）として素通り。1 回の呼び出しでこの量がターンに乗る。機構は足さない（頻度を見てから）

- 2 体目の個体では Jev の圧縮が配列型で効いた（`mcp-tv-get-economic-calendar` 75,682 → 48,088 字 / `get-news-story`
  6,744 → 3,268 字）。`get-news` は `structured` で素通り

- **確認 7 の後、18:07:48 に認可し直した並びがある**（`refreshed` → 40 ms 後に `authorized` = 認可の流れの並び。ログからの読み）。
  18:08:01〜18:08:14 の `refreshed` 1 本と `authorized` 2 本は 2 体の繋ぎ直し。だから記録の時点で資格情報ストアには
  `mcp-oauth:` の鍵が**再び在る** — 取り消しが効かなかったのではない

## P3 台帳記録（2026-10-11）

- **DETAIL 3 言語** — 「リモート MCP」の後ろに「リモート MCP を OAuth の認可で繋ぐ」の節（明示のオプトイン・併記の拒否・
  3 つの状態・ブラウザは押したときだけ・二層の置き場・接続先ごとに 1 つ・取り消し・スコープの順・ログ）/ GUI なしの
  拒否の表に `MCP_OAUTH_REQUIRED` と `MCP_OAUTH_HEADLESS` / 「コンテナで回す」に D8（`MCP_OAUTH_IN_COPY`・`--env-out` は
  認可の記録を書き出さない）/ ディレクトリ木に `mcp_oauth.rs` / `mcp_oauth_flow.rs` / `McpOAuthControls.vue`。
  中国語版は抄訳の形に合わせて短く書いた
- **README 3 言語** — 既存の MCP の行を延ばした（行数は 171 / 171 / 167 のまま。入口で読めるべき理由は、ブラウザが開く
  条件とトークンの置き場が利用者の判断に関わるため）
- **PRIVACY 日英** — 要旨（送り先に「MCP サーバーが指定する認可サーバー」を括弧で足した — 要旨の「設定した接続先と
  単価表の 2 つだけ」は、認可サーバーを利用者が直接書かないので、そのままでは言い切れなくなる）/ 3（保存するもの・
  アクセストークンはメモリだけ・`--secrets env` では止まる・`--env-out` は書き出さない）/ 4-3（ブラウザ・認可サーバーへ
  送るもの 4 つ・待ち受けの寿命・取り消し・配布しても付いていかない）/ 6（ログは接続先の識別子と結果の分類だけ）/
  8（「認可を取り消す」）。最終更新を 2026-10-11 へ
- **Spec 47 Notes 2** に覆しを書いた
- **追従の grep で実装の穴を 1 件** — `bake` の警告の文面の途中に 25 個の空白（`failures.md` #130 の 4 例目）。
  直して、テストに「空白の連なりが無い」を 1 行足し、修正を戻す変異でその 1 本だけ赤を確認した
- **ランディングページは未更新（利用者の判断待ち）** — FAQ の Q3 に「LLM 以外の外向き通信はモデル単価表の取得だけ」
  （日英）がある。この文は OAuth の前から MCP サーバーへの通信を数えておらず、OAuth で認可サーバーが加わって
  さらに離れた。公開ページで別リポジトリなので、この P3 では書き換えていない

## P2 実装記録（2026-10-11）

**ホスト `crates/fuseforks-host/src/mcp_oauth_flow.rs`** — `McpOAuthFlows`（接続先ごとに進行中の認可を 1 つ。
もう一度始めると古い受け口をキャンセル）・`authorize`・`cancel`・`revoke`。`Host` が `mcp_oauth_flows` を持つ。
**ブラウザを開く関数は呼び出し側が渡す**（GUI は `tauri-plugin-opener` の `OpenerExt::open_url`）ので、この層は
Tauri を知らない。順は D9 のスコープ（401 の `scope` を認証なしの initialize で拾う → 保護リソースの
`scopes_supported` → 無ければ送らない）→ 受け口（登録したときのポートを先に試す）→ 登録（認可サーバーごとに 1 件。
無ければ動的登録して `mcp-oauth-client:<issuer>` へ）→ 認可 URL → ブラウザ → 戻り → コードの交換（保存は
コアの `KeyringCredentialStore`）。成功したら `McpOAuthRegistry::authorized` で拒否の印を外す。

**GUI** — IPC `mcp_oauth_authorize` / `mcp_oauth_cancel` / `mcp_oauth_revoke`（認可・取り消しの後は
`Orchestrator::reconnect_mcp_for` で、その接続先を使う共通と稼働中の個体の MCP を繋ぎ直す）/
`McpOAuthControls.vue`（共通 MCP とサーヴァントの MCP の状態の行に置く。状態のバッジ・「認可する」・待っている間の
残り時間と「キャンセル」・「認可を取り消す」の確認）/ 辞書 `mcp.oauth.*` 17 鍵 ja・en / 走査テスト
`lib/mcpOAuthWiring.test.ts`（2 か所の入口・IPC の登録・**開いただけでは認可を始めない**・OAuth でないエントリには
描かない）。コアの `McpServerStatus` に `url`（OAuth のエントリだけ。ボタンが接続先を名指す）を足した。

**`bake`** — 写しに有効な OAuth のエントリが入ると警告 `MCP_OAUTH_IN_COPY` で名指しする（止めない。無効な
エントリは数えない）。

**実装で決めた・見つけたこと**:

1. **rmcp の `configure_client_id` は戻り先に MCP の URL を入れる** — 接続の時点（P1。リフレッシュだけ）なら
   害が無いが、認可の流れでは戻り先がループバックでなければならない。流れでは `configure_client(OAuthClientConfig::new(
   client_id, redirect_uri))` を使う
2. **戻り先の照合がポートまで厳密なサーバーは、戻りが来ない形で失敗する**（認可口がブラウザに 400 を出し、こちらへは
   何も返らない）。だから「戻りを見て登録し直す」ことはできず、**ブラウザを開く前に認可 URL を 1 回 GET し、
   400 なら登録し直す**（P0 で手で確かめた形をそのまま使った）。登録したときのポートで受けられたときは確かめない
3. **認可サーバーがリフレッシュトークンを返さなければ保存しない** — 再起動のたびに認可し直すことになるため、
   `Failed` で名指しして終わる
4. **認可の直後のアクセストークンは使わない** — 交換で得たものは流れの中の置き場のメモリにだけあり、繋ぎ直しは
   OS の記録から組み直すので 1 回リフレッシュが走る（接続の経路を 1 つに保つための代償）
5. **取り消しの順** — リフレッシュトークンを読む → `registry.revoke_local`（世代を進め、メモリと OS を消す。既存の
   接続の次の要求は `load` が `None` を返して認可エラーになる）→ 失効の要求 → GUI が繋ぎ直す。D7 の「接続を切ってから
   失効」と順が 1 つ入れ替わるが、手元の記録を先に消すので、失効の要求の間に古いトークンで要求が通ることは無い
6. **サーヴァントの編集ダイアログでは、稼働中の個体の状態の行にだけボタンが出る**（停止中は接続の状態そのものが
   無い既存の作り）。共通 MCP には常に出る。D6 の「稼働していない個体の画面にも状態を出す」は、共通 MCP の側で満たす

**テスト**: host の結合 `tests/mcp_oauth_flow.rs` 8 本（登録 1 回・PKCE の verifier と `resource`・2 回目は登録を
使い回して同じポート / `state` の違う戻りは 400 で待ち続ける / キャンセルでポートが解放される / 時間切れ / 拒否 /
ポートまで照合するサーバーで 1 回だけ登録し直す / PKCE の無いサーバーではブラウザを開かない / 取り消し）。
スタブの認可口は戻り先へ `code` と `state` を付けて 302 し、「ブラウザ」は認可 URL を辿って戻り先を叩く関数。
**変異 7 本のうち 6 本は 1 回目で狙った 1 本が赤、F3（登録したポートを先に試さない）は緑**だった — テストが
登録の記録の戻り先を比べており、記録は登録し直さない限り変わらない。スタブが認可口で受けた戻り先を記録して比べる形へ
直し、赤を確認した。**もう 1 つ、テストの穴を変異の前に見つけた** — `state` の違う戻りの 400 を spawn したタスクの
中の `assert` で見ていた（panic は本体へ伝わらない）うえ、スタブが認可コードを照合していなかったので、`state` の
照合を外しても緑になりえた。状態を本体で確かめ、スタブがコードを照合する形へ直した。`bake` の警告 1 本・走査テスト
4 本も、それぞれ変異で赤を確認した。

## P1 実装記録（2026-10-11）

**新しいファイル `crates/fuseforks-core/src/mcp_oauth.rs`**（`mcp.rs` は既に 1,400 行を超えていたので分けた）—
純関数（`canonical_resource` / `select_scopes` / `scopes_cover` / `fits_credential_limit`）、記録の型と読み書き
（`TokenRecord` / `ClientRecord`。`TokenRecord` の Debug はリフレッシュトークンを伏せる）、二層の置き場
`KeyringCredentialStore`、接続先ごとの `McpOAuthRegistry`。`mcp.rs` は `auth` 欄（型・検査・書き戻し）、
`McpServerStatus.auth`、`connect_all` の第 3 引数、`serve_http_oauth`、D11 の案内。`Shared` に
`mcp_oauth: Arc<McpOAuthRegistry>`（プロセスに 1 つ）。`headless.rs` に 2 つの識別子と `OAuthServer`、host の
`preflight.rs` が OAuth のエントリと置き場の種類（`secrets_writable`）を渡す。

**依存**: rmcp の `auth` に加え、`reqwest_rmcp`（reqwest 0.13 の別名）と `oauth2` 5 を直接の依存へ。どちらも木に
既にある版に揃うので crate は増えない（`cargo tree` 200 → 210 = P0 の +10 のまま）。Spec 47 の「reqwest の型を
名指ししない」は版の食い違いを避ける判断で、0.13 に固定した別名なら rmcp が版を上げたときにコンパイルエラーとして
表に出る。

**実装で決めた・訂正したこと**:

1. **鍵と `resource` は設定の `url` の正準形**（D4 の「保護リソースのメタデータの `resource`」から変えた）。
   鍵を引くのにメタデータを取りに行かずに済み（記録が無ければネットワークに出ずに `required` にできる）、
   MCP 仕様の「MCP サーバーを特定する最も具体的な URI」にも当たる。TradingView は 2 つが同じ値
2. **プロセスをまたぐ取り合い（D5）は `load` の照合で吸収する**（rev2 の「`invalid_grant` で読み直して 1 回やり直す」を
   前へ倒した）。rmcp の manager は要求ごとに `load` を呼ぶので、そのたびに OS の記録と照合し、別のプロセスが回した
   リフレッシュトークンを**自分がリフレッシュする前に**拾う。アクセストークンが生きていればそのまま使い、次の
   リフレッシュだけを新しいトークンで行う — 双方が回し合わないため。代償は要求ごとに資格情報ストアを 1 回読むこと。
   接続が拒否で失敗し、OS のトークンが使ったものと違うときの 1 回のやり直しも残した（`refetched`）
3. **この実行で拒まれた接続先は、人が認可し直すまで接続を試みない**（`McpOAuthRegistry` の `required` の集合）。
   rmcp はリフレッシュの失敗を通信の失敗も含めて `TokenRefreshFailed` に畳むので、**OS の記録は消さない**
   （一時的な障害で認可を失わせない）。次の起動では 1 回だけ試す
4. **パスの無い URL では `resource` に末尾の `/` が付く** — rmcp は manager の `base_url`（`Url`）を `to_string()` で
   送り、`url` の `Url` はパスの無い URL を常に `/` で表す。正準形の文字列を渡しても避けられない。MCP 仕様は
   「付けない」を SHOULD としている。TradingView は `/mcp` のパスがあるので当たらない。自前で揃えるには rmcp の
   トークン要求を組み直すことになるので、当たる接続先が出るまで据え置く
5. **期限切れの形は 2 つの条件で作っている**（`expires_in` 0 と `token_received_at` 0）。どちらか片方だけでも
   rmcp はリフレッシュする（変異で確かめた — 片方だけの変異は緑、両方で 4 本が赤）
6. **計器の `outcome` に `token_too_large` を足した**（P0 の契約の列挙に無かった）。`refreshed` は `save` で記録が
   書き換わったときに出す（サーバー名はこの層に無いので `server=-`）
7. **`McpServerStatus.auth` は `#[serde(default)]`** — 旧いフロントが読んでも壊れない。画面は P2

**テスト**: 単体（`mcp_oauth.rs` 10 本・`mcp.rs` に 2 本・`headless.rs` に 1 本）と結合 `tests/mcp_oauth.rs` 7 本。
結合はループバックに保護リソースのメタデータ・認可サーバー・MCP サーバー（Bearer を検査する initialize / tools/list）を
1 つで受けるスタブを立て、rmcp の実際の経路を通す。**変異 6 本のうち 5 本は予測どおり狙った 1 本だけが赤**、
M1（期限だけを 3600 にする）は 5 の理由で緑、両方を壊す変異で赤を確認した。

## P0 実測記録（2026-10-11）

**Windows の資格情報マネージャーの上限** — 製品と同じ `KeyringSecretStore` を試験用のサービス名
（`fuseforks-p0-probe`）で作り、値を書いて読み戻して消した（残骸 0 を `cmdkey /list` で確認）:

| 値 | 結果 |
|---|---|
| ASCII 1,280 字 | 保存・読み戻しとも一致 |
| ASCII 1,281 / 2,560 / 2,561 / 4,096 字 | 失敗 — `Attribute 'password encoded as UTF-16' is longer than platform limit of 2560 chars` |
| 「あ」853 / 854 / 1,280 字 | 保存できる |
| 「あ」1,281 字 | 失敗（同じ文面） |

rev2 の「2,560 バイト」は記憶の値で、バイト数としては合っていたが、`keyring` 3 が UTF-16 で保存するので
**実際に入るのは 1,280 字**。UTF-8 のバイト数とは関係が無い。D4 を訂正し、超えたときの `token_too_large` を足した。
既存の API キーや `${secret:…}` の値も同じ上限を受けるが、短いので今まで当たっていない。

**rmcp の `auth` feature** — `Cargo.toml` に足して `cargo check -p fuseforks-core -p fuseforks-host` が 47 秒で通った。
依存は起票時の数え（`cargo tree -p fuseforks-core -e normal` で 200 → 222 行・一意の crate +10）のまま。

**空のスコープ** — rmcp の `get_authorization_url(scopes)` は `scopes` の各要素を `add_scope` するだけで、
`oauth2` 5.0 の `code.rs` は `if !scopes.is_empty()` のときだけ `scope` を付ける（トークン要求の `endpoint.rs` も同じ）。
D9 の「送らない」は `get_authorization_url(&[])` で実現できる。あわせて、`resource` が manager の受け取った URL の
`Url::to_string()` であること（パスの無い URL で末尾に `/` が付く）を読んだ — D9 と P1 に足した。

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
