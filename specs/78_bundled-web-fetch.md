# Spec 78: 同梱の web 取得ツール `fetch` — Docker の MCP なしで web ページを読む

- 状態: **rev5**（2026-11-21 起票 → 同日、査読 2 系統を反映して rev2。重なりを除いて 43 項目 → 採用 39 /
  前提を訂正して採用 2 / 反証 2。表は Notes 6 → 同日、**利用者が未決 5 つを推しのとおりに裁定して rev3** →
  同日、**P0a の測定を終えて rev4**。D3 を実測で直した（ISO-2022-JP）→ 同日、**利用者が P0a の後の裁定 4 つを推しのとおりに
  決めて rev5**（D1 に `links` を足し、D2 を書き直した）→ 同日 **P0b 完了**（`data_contract.yaml` の `web_fetch_contract`
  ほか 6 箇所。記録は「P0b 凍結記録」）→ 同日 **P1 / P2 完了**（ブランチ `claude/web-fetch`。コア・ホスト・画面。
  記録は「P1 実装記録」「P2 実装記録」）。次は P3（台帳）
- 起点: 利用者（2026-11-21）—「webfetch を同梱ツールとして入れるべきかなと思っている。今は Docker の fetch を
  使っているが、けっこう読み込みが重い。できれば車輪の再発明ではなく Rust でよい crate があればそれを使いたい」
- 調査だけのターンを 1 回取った（`fuseforks.log` の `MCP_DOCKER__fetch` の行 / crates.io の候補 12 本 / 候補を
  コアへ足したときの `Cargo.lock` の差 / 使い捨てのプログラムで実際のページ 8 本を取得・抽出・変換）。rev2 では
  公式の fetch サーバーの原典 2 版（Docker が使っている版と上流の main）・reqwest 0.12.28 / hyper-util 0.1.21 の
  原典・URL の解析の実測を足した。下の「現況」がその結果
- **rev2 で変わった骨格**: 推しの 1 つを反転（未決 3 = 既定集合に**入れない**）/ 返す文字列を「枠」と「本文」に分け、
  枠を Jev の圧縮の外に置く（`ToolOutput` に 2 欄）/ 取りに行ける先の判定を上流の main の表に揃え、名前・IP の直書き・
  リダイレクトの 3 経路それぞれの仕掛けを名指し / 文字コードの順を WHATWG に合わせる（BOM が先）/
  P0 を「測定 → 裁定 → 凍結」に割る

## Goal

1. Docker Desktop と MCP Toolkit を入れていない村でも、**サーバーが HTML（または Markdown・テキスト）で返すページ**を
   サーヴァントが Markdown として読める。JavaScript で中身を組み立てるページは対象外（Notes 4）
2. 同梱の `fetch` の 1 回の取得が、Docker の fetch（中央値 2.3 秒）より速い
3. 文字コードを宣言しているページ（BOM・応答ヘッダ・HTML の `<meta>`・XML の宣言）は、Shift_JIS / EUC-JP でも
   文字化けさせずに読む。**宣言の無いページをどうするかは P0a の数で決める**（D3）
4. **同梱の `fetch` には**、この端末と同じネットワークの中（ループバック・プライベート・リンクローカルほか、公開でない
   住所）を読ませない。リダイレクトの先と robots.txt の取得も同じ。**Docker の fetch を併用している個体には効かない**
   （Docker の版は住所を判定しない。Notes 3）
5. `enabledTools` に `fetch` を持たない個体の挙動は 1 バイトも変わらない（ツールの提示・プロンプト・ログ）。
   **`enabledTools: null`（既定に従う）の個体も含む** — だから既定集合には入れない（未決 3。2026-11-21 裁定）
6. アプリ自身が、**モデルが選んだ URL へ**取りに行くようになることを PRIVACY に書く（既存の文の 3 か所が嘘になる — P3）

## 現況（実測 2026-11-21。数字は同日夜の再集計）

### Docker の fetch の使われ方（`fuseforks.log` 2026-08-09〜11-21）

- `MCP_DOCKER__fetch` の `tool:` 行は **1,063 本**。`file`（2,681）・`blackboard`（1,503）に次いで 3 番目で、
  `fd`（906）・`grep`（729）より多い。個体別はジェミー（agent_3）564 / ザリ（agent）184 / ルナ（agent_8）181 /
  ミュゼ（agent_9）116 / agent_11 13 / イクス（agent_10）5（合計 1,063）
- **`body_chars` は Jev の圧縮の後の字数**（`turn.rs` の `prune_body` の後で数えている）。2026-09-22 以降の
  149 本は `tool prune:` の `outcome=ok` で縮んでいるので、その 149 本は直前の `tool prune:` 行の `raw_chars` へ
  戻して数えた。**圧縮前の字数は中央値 4,874 字 / 上位 25% 8,195 / 上位 10% 15,176 / 上位 5% 20,235 / 最長 66,278**
- **既定の 5,000 字で切られている呼び出しは少ない。** 5,000 字で切ると本文は 5,000 字 + 前置き
  （`Contents of {url}:`）+ 末尾の案内（約 110 字）になるが、5,100〜5,200 字に居るのは **50 本（4.7%）**。
  一方で **5,400 字を超える呼び出しが 428 本（40%）** あり、モデルが自分で大きい `max_length` を渡していると読める
  （引数はログに出ないので確定はしない）。rev1 の「中央値 5,163 字が 5,000 の打ち切りを示す」は誤り（圧縮後の字数を
  そのまま数えていた。rev1 の数字は再現できなかった）
- 所要時間（`elapsed_ms=`）は **257 回で中央値 2,309 ms / 上位 10% 4,240 ms / 最長 89,369 ms**。`elapsed_ms=` は
  2026-11-11 に足した欄で、fetch の最初の行は 11-13。**この 257 回は 11-13 以降の全数**で、大きさで選んだ部分集合ではない

### 公式の fetch サーバー（原典を読んだ）

**村が今使っているのは上流の古い版で、上流の main は大きく育っている。** 2 つを分けて書く。

| | Docker の `mcp/fetch`（村が使っている版） | 上流の main（2026-11-21） |
|---|---|---|
| 出所 | カタログの像の元は `4503e2d`（2026-04-17）。`server.py` 288 行 | `server.py` 1,178 行 |
| 引数 | `url` / `max_length`（既定 5,000・1〜999,999）/ `start_index`（既定 0）/ `raw` | 同じ 4 つ + `timeout_ms` |
| 住所の判定 | **無い** | 公開でない住所を各段で断る（IANA の特別用途の表。IPv4 射影・NAT64・IPv4 変換は中の IPv4 で判定。2000::/3 の外の IPv6 は断る）。繋ぐ時点の住所も固定する（`pinning.py`） |
| robots.txt | **既定で見る**（UA `ModelContextProtocol/1.0 (Autonomous; …)`）。401 / 403 → 断る / 他の 4xx → 通す / 繋がらない → 断る。最初の URL だけ | 同じ規則 + 5xx も断る + **リダイレクトの各段**でその先の origin を見る。robots.txt の取得も住所の判定と上限を通る |
| 本文の上限 | 無い（全部読む） | 5 MiB（展開後）。`Content-Length` が超えれば読まずに断り、読んで超えれば断る（途中までを返さない） |
| 圧縮 | httpx 任せ | `gzip, deflate` だけを頼み、上限つきで展開 |
| 種類 | `<html` を含むか `text/html` か型が無ければ HTML | `Accept: text/markdown, text/html;q=0.9, */*;q=0.8`。Markdown が返ればそのまま。型が無ければ本文が `<!doctype html` / `<html` で始まるときだけ HTML |
| 文字コード | 応答ヘッダだけ | BOM かヘッダ → 先頭 1,024 バイトの `<meta>`。ASCII 互換でない宣言は無視、UTF-16/32 の宣言は UTF-8 として読む |
| 状態 4xx / 5xx | 本文を読まず「status code N」 | 同じ |
| 時間 | 30 秒 | 引数 `timeout_ms` |
| 返す形 | `Contents of {url}:` + 本文。切ったら末尾に `<error>Content truncated. Call the fetch tool with a start_index of N to get more content.</error>` | 同じ（題名を本文の頭に足す） |
| 位置の単位 | Python の `str` の添字（コードポイント）。前置きの `Contents of …` は数えない | 同じ（題名は数える） |

- **村のモデルが慣れているのは左の列**（引数 4 つ・5,000 字・続きの案内の文）
- **左の列は記事の抽出をしていない**（P0a で判明）。像に Node.js が無く、`readabilipy` が純 Python の簡略化へ黙って切り替わる。
  返るのは「`<nav>` 以外のページ全体の文字・リンクの URL なし・画像なし」で、GitHub の README と docs.rs の説明文は落ちる
- rev1 の「モデルが自分で取りに行くときは robots.txt を確かめる」は記憶で書いていたが、原典で正しかった。
  **村は今、robots.txt を見る fetch を使っている**

### Rust の候補（crates.io 2026-11-21）

| crate | 版 | 更新 | ライセンス | 中身 |
|---|---|---|---|---|
| **dom_smoothie** | 0.19.0 | 2026-10-27 | MIT | Mozilla Readability の移植。`TextMode::Markdown` で本文を Markdown で返す。`dom_query` の上に作られている |
| htmd | 0.6.0 | 2026-11-18 | Apache-2.0 | turndown.js の移植。HTML を Markdown へ変換するだけ（本文の抽出は無い） |
| html2text | 0.17.4 | 2026-10-18 | MIT | HTML をテキストへ描く |
| html-to-markdown-rs | 3.22.2 | 2026-11-15 | MIT | 依存が多い（regex / serde_json / tracing ほか） |
| fast_html2md | 0.0.66 | 2026-11-16 | MIT | spider の変換器。版が 0.0.x |
| html2md / article_scraper | — | — | **GPL** | MPL-2.0 の村には入れられない |
| readability | 0.3.0 | **2023-12-20** | MIT | 更新が止まっている |
| encoding_rs | 0.8.44 | 2026-10-30 | (Apache-2.0 OR MIT) AND BSD-3-Clause | Firefox の文字コード変換。Shift_JIS / EUC-JP を UTF-8 へ |

- **`dom_query` 0.29 自身に Markdown 変換（`md()`）がある。** feature `markdown` は `dom_query` の既定で、
  dom_smoothie が有効にしている（`dom_query-0.29.0/Cargo.toml` の `default = ["markdown"]`）。ページ全体への戻り先に
  htmd を足さずに済む根拠はこれ（**変換の質は測っていない** — 下の比較表の右列は htmd の数字で、`md()` の数字ではない）
- **Tauri が既に `dom_query` 0.27（html5ever 0.38）を連れている**（`tauri-utils` 経由）。dom_smoothie 0.19 は
  `dom_query` 0.29 を要求するので、版は共有できない
- **コアへ足したときに `Cargo.lock` に増える項目**（実測。版違いの再登場も 1 と数える）: dom_smoothie 単独 **22** /
  htmd 単独 11 / 両方 24。encoding_rs は直接の依存が 5 本（`cfg-if` / `core_detect` / `multiversion_no_op` /
  `scopeguard` / `simdutf8`）で、`Cargo.lock` に増えるのは 3（P0a で実測）。robots.txt の crate は P0a で選ぶ。**すべて純 Rust**
- この村の reqwest は `default-features = false, features = ["json", "rustls-tls"]` で、`charset` も `gzip` も無い。
  `Response::text()` は UTF-8 としか読まず、`Accept-Encoding` は送らない

### reqwest と URL の解析（原典と実測）

- **`ClientBuilder::dns_resolver` は feature に縛られていない**（reqwest 0.12.28 の `async_impl/client.rs` 2290 行。
  `cfg(feature)` 無し）。`redirect::Policy::custom` も同じ（`redirect.rs` 102 行。各段の `Attempt::url()` が見られる）
- **IP をそのまま書いた URL は名前解決を通らない。** hyper-util 0.1.21 の接続器は `SocketAddrs::try_parse(host, port)` で
  IP の直書きを先に解釈し、解決器を呼ばない（`client/legacy/connect/http.rs` 559 行）
- **IPv4 の別の書き方は URL の解析が正規化する**（`reqwest::Url` = url crate の実測）:
  `0x7f000001` / `2130706433` / `127.1` / `017700000001` / `0177.0.0.1` → すべて `Host::Ipv4(127.0.0.1)`。
  `[::ffff:127.0.0.1]` と `[::ffff:7f00:1]` → `Host::Ipv6(::ffff:127.0.0.1)`（IPv6 のまま）。`169.254.169.254.` →
  `Ipv4(169.254.169.254)`。`localhost` → `Domain("localhost")`。`☃.com` → `Domain("xn--n3h.com")`

### 実ページでの比較（使い捨てのプログラム。reqwest 0.12 + dom_smoothie 0.19 + htmd 0.6）

| ページ | 取得 | dom_smoothie（本文 → Markdown） | htmd（ページ全体 → Markdown） |
|---|---|---|---|
| The Rust Programming Language（1.1 Installation） | 38 ms | 6,168 字 / 1 ms | 6,865 字 / 0 ms |
| arXiv の論文の要旨ページ | 30 ms | 3,460 字 / 2 ms | 9,777 字 / 1 ms |
| GitHub の README（dom_smoothie） | 475 ms | 16,654 字 / 13 ms | 29,391 字 / 9 ms |
| 日本語版 Wikipedia（Rust） | 678 ms | 96,280 字 / 29 ms | 189,035 字 / 16 ms |
| Hacker News の一覧 | 510 ms | 14,402 字 / 4 ms | 12,149 字 / 1 ms |
| ITmedia NEWS の一覧 | 45 ms | **1,110 字** / 7 ms | 14,716 字 / 2 ms |
| tokio のチュートリアル | 350 ms | 5,320 字 / 1 ms | 6,808 字 / 0 ms |
| 青空文庫（Shift_JIS） | 303 ms | 298,404 字のうち **208,572 字が U+FFFD** | 同じく文字化け |

- **dom_smoothie は本文から始まる。** Rust の本では htmd がキーボードのショートカットの説明やテーマの選択肢を拾い、
  dom_smoothie は本文の 1 行目から始まった
- **一覧ページでは本文の抽出が一部しか拾わないことがある**（ITmedia は最初の記事リストの 1,110 字だけ）
- **リンクの URL の `&` が `&amp;` のまま出る**（arXiv の著者リンク・Hacker News）
- 青空文庫は応答の `Content-Type` に `charset` が無く、文字コードは HTML の
  `<meta http-equiv="Content-Type" content="text/html;charset=Shift_JIS">` にだけある

### 村の側の仕組み

- 同梱ツールは `AgentTool`（`tool.rs`）を実装し、`fuseforks-host` の `build_host`（`boot.rs`）で登録する。名前の表は
  `BUNDLED_TOOL_NAMES`（9 本）/ `DEFAULT_ENABLED_TOOLS`（7 本・`enabledTools: null` の個体に出る）/
  `WORK_DIR_TOOL_NAMES`（6 本）。**`run` と `image` は既定集合の外**で、理由は `enabled_tools_invariant` が凍結している —
  「**更新しただけで実行能力が増える作りにしない**」（`run`）/「費用と送信先が増えるので、更新しただけで全個体に
  生やさない」（`image`）
- この村のサーヴァントは全員 `enabledTools` を明示の配列で持つが、**それはこの村の観測で、他の利用者の村には成り立たない**
- `AgentTool::prunable(&self) -> bool` は**引数を見られない**（ツールごとに 1 つの答え）。真を返すのは `McpTool` だけ
- Jev の圧縮（`prune::prepare`）は空行で段落に割り、**120 字未満の段落を隣へ寄せる**（末尾は前へ）。先頭の短い行や
  末尾の短い案内は、寄せた先の段落ごと落ちうる
- `ToolOutput`（`tool.rs`）は `text` / `images` / `dropped` / `approval_pending` の 4 欄で `Default` を持つ
- `sender_envelope::defuse` は寄せた件数を返す（`(String, usize)`。#72 — 握り潰した事実を数えられない防御を作らない）
- 応答の本文を上限つきで読む `http_body::read_capped`（Spec 77 N5）が既にある。`every_outbound_response_is_read_through_the_cap`
  が外向きの HTTP を持つファイルを**手で列挙して**字面で留めている
- 画面: `AgentSettingsDialog.vue` の `BUNDLED_TOOLS`（9 本。`run` / `image` を含む）/ `RoleDialog.vue` の `BUNDLED_TOOLS`
  （7 本 = 既定集合。役職の雛形には `run` / `image` を出さない）/ `toolLabel.ts` の `KNOWN_TOOLS`（`toolLabel.test.ts` が
  Rust の `BUNDLED_TOOL_NAMES` と突き合わせる）
- PRIVACY.md は「本アプリが外部へ通信するのは、利用者が自分で設定した接続先と、単価表の取得先の 2 つだけ」（要旨）/
  「利用者が設定した接続先に対してのみ通信します」（4 章の冒頭）/ 圧縮の節は「MCP のツールが返した長い本文から」
  「対象外のツール（`file`・`run`・`rag` など MCP 以外）」と書いている

## Design

### D1. 名前・引数・返す文字列の形

- 名前は **`fetch`**。MCP 由来は `MCP_DOCKER__fetch` のように接続先の名前が付くので衝突しない
- 引数は `url`（必須）/ `max_length`（既定 20,000。上限 999,999。未決 1 の裁定）/ `start_index`（既定 0）/ `raw`（既定 false）/
  **`links`（既定 false。裁定 2）** + 理由欄（Spec 27。`wants_reason` は既定の真のまま）。**`url` / `max_length` / `start_index` /
  `raw` の形と意味は Docker の版と同じ**。変わるのは名前（`MCP_DOCKER__fetch` → `fetch`）と `max_length` の既定と、`links` が
  1 つ増えること。`links: true` で本文のリンクの URL と画像を残し、`false` では外す（D2）。上流の main の `timeout_ms` は
  採らない（打ち切りは 30 秒の固定と「■ 停止」— D5）
- **返す文字列は「枠」と「本文」に分ける。** 枠は次の順の行で、文は村の言語（Spec 35）:

  ```text
  URL: <リダイレクトの後の最終 URL>
  題名: <題名>                       ← HTML から取れたときだけ
  取り出し: <本文（ページの文字の N%） | ページ全体 | Markdown | そのまま | 変換なし>
  （空行）
  <本文の切り出し>
  （空行）
  <続きの案内>                       ← 切ったときだけ。「続きは start_index を N にして呼ぶ」（#44 — 打ち切りは次の手を書く）
  ```

  本文を返さなかったとき（断った・失敗した）は、`URL:` の行と理由の文だけを返す（ツールの失敗で会話を止めない）。
  「本文（ページの文字の N%）」の N は D2 の比（整数の百分率・四捨五入）
- **`start_index` と `max_length` の単位は本文の文字**（Unicode のスカラ値 = Rust の `char`。Docker の版の Python の
  `str` の添字と同じ）。**数える範囲は本文だけ**で、枠（URL・題名・取り出し方・続きの案内）は含まない
- **本文 = 取り出した文字列に `defuse` を掛けた後の文字列。** `defuse` を掛けてから切るので、同じページを同じ引数で
  取り直せば位置が合う（`defuse` は決定的）
- `start_index` が本文の長さ以上なら、本文の代わりに「これ以上の本文はありません」を返す
- 同じターンの中で続きを読むときも**取り直す**（Docker の版と同じ）。現況の事前測定で取得は 30〜680 ms だったので、
  ターンの中に本文を持つ機構は作らない。ページが取得の間に変わると位置がずれるが、それは Docker の版でも同じ

### D2. 本文の取り出し（rev5 で裁定 1・2 を反映して書き直した）

- HTML は **dom_smoothie** で本文を取る（取り出し: 本文）。dom_smoothie が返す本文の HTML（`Article.content`）を `dom_query` で
  読み直し、`links: false` なら `<a>` を外して文字だけ残し（`strip_elements(&["a"])`）、`img` / `picture` / `source` / `video` /
  `audio` を消してから、`md()` で Markdown にする。**URL を落とすのは DOM の上で、Markdown の文字列を正規表現で削らない**。
  `links: true` も同じ経路（読み直して `md()`）を通る — 1 つの実装で両方を出す（dom_smoothie 自身の Markdown と 26 本中 25 本で
  同じ字数。P0a）
- **「取り出し:」の行に比を出す。** 比 = 本文の HTML のテキスト ÷ `<body>` のテキスト（`script` / `style` / `noscript` /
  `template` / `svg` を除く）。どちらも空白を除いた字数。**比で何かを切り替えることはしない** — P0a で、比では取りこぼしと
  成功を分けられないと分かった（取りこぼし 0.103 と最も比の低い成功 0.147 の差が 0.044。ナビの多いニュースサイトでは記事を
  取れても比が 0.15〜0.38）。比はモデルが一覧の取りこぼしに気づく手がかりとしてだけ出す（裁定 1）。rev2〜rev4 の
  「比が閾値未満ならページ全体へ戻る」は取り消した
- **dom_smoothie が失敗したか、本文が空（空白だけ）のときだけページ全体へ戻る**（取り出し: ページ全体）。`<body>` を
  `md()` に渡し、`script` / `style` / `meta` / `head` / `nav` / `header` / `footer` / `aside` / `form` / `noscript` /
  `template` / `svg` / `iframe` を中身ごと飛ばす（`md()` の `skip_tags`）。`links` の扱いは上と同じ。ページ全体のときは比を出さない
- それも空なら、本文の代わりに「本文を取り出せませんでした。`raw: true` で HTML を読めます」を返す（`<frameset>` のページ —
  P0a の阿部寛のホームページ。outcome=empty）
- `text/markdown` で返ったものはそのまま（取り出し: Markdown。**`links` は効かない** — Markdown を解析しない）。`raw: true` は
  変換せずに、文字コードを直した後の HTML を返す（取り出し: 変換なし。`links` は効かない。**圧縮はしない、封筒の無害化は
  掛ける** — D6）
- `&amp;` の後処理（`links: true` のときだけ行き先がある）は **Markdown のリンクと画像の行き先（`](` の直後から対応する `)` まで。
  `<…>` で囲まれた形も）だけ**で、`&amp;` → `&`・`&quot;` → `"`・`&nbsp;` → NBSP を**左から 1 回だけ**戻す。地の文は触らない。
  `dom_query` は行き先にだけ `html_escape` を掛け、`&` を例外なく書き換えるので、この 1 回で元の URL に正確に戻る（P0a — 原典と
  27 本の実測）
- 画像は、`links: true` なら `![alt](url)` のまま残す（画像そのものは取りに行かない）。`false` なら代替テキストも含めて消す
  （Docker の版と同じ）
- 依存は dom_smoothie（22）+ encoding_rs（3）+ robotxt（2）= **27**（P0a で実測）。htmd を足さないぶん 2 つ少ない

### D3. 文字コード

順に見て、最初に決まったものを使う（WHATWG の「エンコーディングの判定」と同じ順）:

1. **BOM**（UTF-8 / UTF-16LE / UTF-16BE）— `encoding_rs::Encoding::for_bom`
2. 応答の `Content-Type` の `charset`
3. 本文の先頭 1,024 バイトの宣言:
   - HTML（`text/html` / `application/xhtml+xml` / 型が無い）— `<meta charset>` と
     `<meta http-equiv="Content-Type" content="…charset=…">`。走査は WHATWG の prescan に沿う（コメントを飛ばし、
     `<meta` の属性だけを見る。`data-charset` のような別の属性は宣言と読まない）
   - XML（`application/xml` / `text/xml` / `*+xml`）— 先頭の `<?xml … encoding="…"?>`
4. どれも無ければ UTF-8

- ラベルの解釈（`Shift_JIS` / `x-sjis` / `windows-31j` を同じものとして扱う）は `Encoding::for_label` に任せる
- 3 の宣言が UTF-16 なら UTF-8 として（ASCII のバイトで書かれた UTF-16 の宣言は真であり得ない）、`x-user-defined` なら
  windows-1252 として読む（WHATWG の prescan と同じ置き換え）。**それ以外の宣言は、ASCII 互換でなくても宣言どおりに読む** —
  ISO-2022-JP の宣言を持つページが実在し（P0a で 2 本）、rev2 の「ASCII 互換でない宣言は無視」（上流の main と同じ）だと
  UTF-8 に落ちてエスケープ列が本文に残る。宣言どおりに読むと 2 本とも U+FFFD 0 で読めた
- 3 の走査は自前で書く（構文解析の前に要るので、html5ever には頼めない）
- **自動推定（chardetng）は P0a の数で決める。** ~~20 本以上のうち「宣言の無い非 UTF-8」が 0 本なら入れない。1 本でも
  あれば依存の増分を数えて利用者に出す~~ → **P0a で 0 本**（27 本 + 古い日本語のページ 24 本）。**裁定 3: 入れない**（依存は +1 で
  安いが、効く実例が無い）。宣言の無いページは UTF-8 で読む

### D4. 取りに行ける先 — この端末のネットワークの中を読ませない

- スキームは `http` / `https` だけ（それ以外は `bad_scheme`）
- **判定は上流の main の表に揃える**（`_is_public_address`）。Rust の `is_global` は安定化していないので、表を純関数で持つ:
  - IPv4 は IANA の特別用途の登録簿のうち公開でないものを断る — 0.0.0.0/8・10/8・100.64/10・127/8・169.254/16・
    172.16/12・192.0.0/24・192.0.2/24・192.88.99/24・192.168/16・198.18/15・198.51.100/24・203.0.113/24・
    240/4（255.255.255.255 を含む）・マルチキャスト（224/4）。例外として公開の 192.0.0.9 / 192.0.0.10 は通す
  - **IPv4 射影（`::ffff:0:0/96`）・IPv4 変換（`::ffff:0:0:0/96`）・NAT64（`64:ff9b::/96`）は、先に中の IPv4 を
    取り出して IPv4 の表で判定する**（`Ipv6Addr::to_ipv4_mapped` と下位 32 ビット）。IPv4 互換（`::/96`。`::` と `::1` を
    含む）は中身に関わらず断る
  - 残りの IPv6 は **2000::/3 の外を全部断る**（許容の側から引く。64:ff9b:1::/48・fc00::/7・fe80::/10・fec0::/10・
    マルチキャストはここで落ちる）。2000::/3 の中でも 2001::/23・2001:db8::/32・2002::/16・3fff::/20 は断る
    （上流の例外 2001:1::1 / 2001:1::2 / 2001:3::/32 / 2001:4:112::/48 / 2001:20::/28 / 2001:30::/28 は通す）
- **3 つの経路それぞれに仕掛けを置く**:
  1. **名前** — `ClientBuilder::dns_resolver` に自前の解決を渡す。解決した住所を全部判定し、1 つでも当たれば断る
     （一部だけ外して繋ぐことはしない）。**reqwest はこの解決が返した住所にしか繋がないので、判定した住所と繋ぐ住所が
     同じになる**（上流が `pinning.py` で後から閉じた「判定と接続の間の差し替え」が、構造で最初から閉じる）。
     `localhost` も名前なので、ここで 127.0.0.1 に解けて断る。解けなければ繋がらない（プロキシを使わないので、上流の
     「解けなければプロキシに任せて通す」は要らない）
  2. **IP の直書き** — 接続器は名前解決を呼ばない（現況）。送る前に **`Url::host()` の型（`Host::Ipv4` / `Host::Ipv6`）**で
     判定する。**文字列を自分で IP として読まない** — `0x7f000001` / `2130706433` のような別の書き方は url crate が
     `Ipv4(127.0.0.1)` へ正規化している（現況の実測）
  3. **リダイレクト** — ~~`redirect::Policy::custom` で~~ **reqwest には追わせず（`Policy::none`）、自前のループで 1 段ずつ追い**、
     各段の URL にスキームと 2 の判定と robots.txt を掛け直し、5 段を超えたら止める（`too_many_redirects`）。名前の段は 1 が拾う。
     **P1 で変えた** — robots.txt を各段で見るには非同期が要り、`Policy::custom` の閉包は同期
- **プロキシは使わない**（`no_proxy()`）。プロキシ越しだと名前を引くのはプロキシで、上の判定が効かない。代償は Notes 1
- 断ったときは、住所の分類（「ループバック」「プライベートアドレス」「リンクローカル」…）と「この端末のネットワークの中の
  住所なので読めません」を本文で返す

### D5. 受け取る中身と時間

- **時間は 1 回の `fetch` 呼び出しの全体で 30 秒**（robots.txt の取得・各段のリダイレクト・本文の読み取りを含む）。
  取得全体の future を `ToolContext.cancel` と `select!` で競わせるので、**ネットワークを待っている最中でも「■ 停止」で
  止まる**（future を捨てると要求も切れる）。cancel を見るのはこのツールだけで、ターンループの検査点は増やさない
  （`run` と同じ）
- 送る見出し: `Accept: text/markdown, text/html;q=0.9, */*;q=0.8`（上流の main と同じ。`raw` のときは `*/*`）。
  **`Accept-Encoding` は送らない**（展開しない）。`Content-Encoding` が `identity` 以外で届いたら読まずに断る
  （`unsupported_encoding`）。workspace の reqwest に `gzip` を足さないのは、feature は crate に 1 つなので **LLM の
  要求の形まで変わる**から（代償は転送量 — 圧縮されない）
- **本文の上限は 5 MiB**（上流の main と同じ）。`Content-Length` が超えていれば読まずに断り、読んで超えたら断る
  （`too_large`。`http_body::read_capped`）。**途中までを返さない** — 5 MiB を超えたページは `start_index` でも読めない。
  本文の抽出は 5 MiB に収まった HTML に対して掛かるので、抽出後の本文が長いページは今までどおり続きで読める
- 状態が 4xx / 5xx なら**本文を読まずに**「状態 N で返りました」と返す（`http_error`。Docker の版と同じ）
- 中身の種類（`Content-Type` の型）:
  - `text/html` / `application/xhtml+xml`、または**型が無く本文が `<!doctype html` か `<html` で始まる** → D2
  - `text/markdown` → そのまま
  - `text/*` / `application/json` / `*+json` / `application/xml` / `*+xml`、または**型が無く HTML でもない** →
    文字コードを直してそのまま（取り出し: そのまま）
  - それ以外（PDF・画像・動画・zip ほか）→ 読まずに「この種類（`application/pdf`）は読めません」（`unsupported_type`）
- `raw` と JSON・XML を字数で切ると、2 回目以降はタグや JSON の途中から始まる。**受け入れる**（Docker の版も同じ。
  続きの案内に位置が書いてある）

### D6. Jev の圧縮と、封筒の無害化

- **`prunable()` を真にする**（Spec 59 の対象に入れる）。`tool_prune_contract` の「対象は MCP 由来」を
  「MCP 由来と `fetch`」へ書き換える。Docker の fetch から乗り換えると圧縮されなくなる、を防ぐ
- **`ToolOutput` に 2 欄を足す**（加算。既定は今と同じで、既存のツールは 1 バイトも変わらない）:
  - **圧縮の外に置く枠**（先頭と末尾の文字列）— `turn.rs` は「先頭 + 圧縮した `text` + 末尾」を返す。`fetch` は D1 の
    枠をここへ置く。**枠が無いと、URL の行や続きの案内（どちらも 120 字未満）が隣の段落へ寄せられ、段落ごと落ちうる**
  - **この結果は圧縮しない**の旗 — `prunable()` は引数を見られないので、`raw: true` の結果は結果の側で止める
    （空行で割った HTML から段落を落とすと構造が壊れる）
  - RepeatGuard が数える本文は**枠を含めた全体**（P1 で `ToolOutput::whole()`）。~~`omitted` が戻す生の本文も枠を含めた全体~~ →
    **P1 で訂正**: 圧縮に回すのは `text` だけなので、`omitted` が戻す段落も `text` の段落。枠は落ちないので戻す必要が無い
  - `turn.rs` の分解（`let (raw, images, dropped, approval_pending) = match result`）に欄を足し忘れると黙って既定へ
    落ちる（Spec 72 D2 の注記と同じ形）。P1 のテストで留める
- **本文に `defuse` を掛ける**（取ってきた本文は外の誰かが書いた文。`【送り手: …】` が書かれていても送り手として
  読まれないように）。**`raw` にも掛ける** — 文字を足すだけで HTML の構造は変えない。寄せた件数は計器へ（D9）
- **`defuse_quote_tags`（Spec 58）は掛けない。** `<quoted_message>` の外枠は利用者の発話の中にだけあり、ツールの結果は
  別のメッセージとして届くので、結果の中に閉じタグがあっても外枠を閉じられない
- MCP 由来の結果（Docker の fetch を含む）の封筒は範囲外（Notes 2）

### D7. 名乗りと robots.txt

- User-Agent は `Fuseforks/{版} (+https://github.com/betyourluck/Fuseforks)`。**ブラウザを名乗らない**（取りに行って
  いるのが自動の道具だと相手が分かるように）。代償は、ブラウザ以外を断るサイトでは読めないこと
- robots.txt は**見る**（未決 2 の裁定）。crate は **robotxt 0.7**（`default-features = false, features = ["parser"]`。裁定 4。
  robotstxt は長い行 + 多バイト文字で panic する — P0a）。**使うのは `Robots::parse` だけ**で、状態コードの振り分けは下の規則で
  自前に書く（robotxt の `AccessResult` は RFC 9309 どおり 4xx を全部許すので、401 / 403 を断る規則と違う）。
  robots.txt の本文は**先頭 `robotxt::BYTE_LIMIT`（500 KiB）+ 1 バイトまで読んで止める**（超えた分は読まずに捨て、断らない。
  RFC 9309 2.5 節は 500 KiB までの解析を求める）。`http_body` に「先頭だけ読む」関数を足して使う — 5 MiB の `read_capped` は
  超えたら本文を返さないので robots.txt には合わない（**P1: `read_head`**。既存の `read_prefix` は読み取りの失敗で黙って途中までを
  返すので、途中で切れた規則で判定しうる — 失敗を返す版を足した）。規則は**上流の main に揃える**:
  - robots.txt の取得も D4（住所の判定・リダイレクト）と D5（30 秒の内側。本文は上の 500 KiB + 1 バイトまで）を通る。robots.txt 自身には
    robots.txt を当てない
  - 401 / 403 / 5xx → 断る（`robots_unavailable`）/ それ以外の 4xx → 規則が無いとして通す / 繋がらない → 断る
    （`robots_unavailable`）。**robots.txt の取得が住所の判定で断られたら `blocked_address`**（P1 — 名前が公開でない住所に
    解ける先は最初の要求の robots.txt で必ず当たり、`robots_unavailable` と書くと理由が嘘になる。結合テストが先に見つけた）
  - `Disallow` に当たったら理由を返す（`robots_disallowed`）。照合する UA の名前は `Fuseforks`
  - **リダイレクトの各段**でも、その先の origin の robots.txt を見る（Docker の版は最初の URL だけ）
  - origin（スキーム・ホスト・ポート）ごとに 1 時間覚える（プロセスの寿命。保存しない）。**覚えるのは規則が決まったときだけ**
    （2xx の解析と規則なしの 4xx。P1 — 一時的な失敗で 1 時間そのサイトを読めなくしない）

### D8. 提示 — 同梱ツールの 10 本目

- `BUNDLED_TOOL_NAMES` へ足して 10 本。`WORK_DIR_TOOL_NAMES` には入れない（作業フォルダを使わない）
- `DEFAULT_ENABLED_TOOLS` へは**入れない**（未決 3 の裁定）
- 画面: `AgentSettingsDialog.vue` の `BUNDLED_TOOLS` に行を足す / `toolLabel.ts` の `KNOWN_TOOLS` と辞書 3 言語 /
  `RoleDialog.vue` の `BUNDLED_TOOLS`（役職の雛形）は**既定集合と同じ扱い** — 既定集合の外なら入れない（`run` / `image` と同じ）
- Docker の fetch と両方ある個体では、モデルがどちらでも選べる。**自動で片方を外す仕組みは作らない**（Notes 3）

### D9. 計器

`fetch:` の 1 行 — `agent=` / `host=` / `status=` / `bytes=` / `chars=` / `mode=` / `links=` / `share=` / `encoding=` /
`defused=` / `redirects=` / `elapsed_ms=` / `outcome=`。

- `host=` は**ホスト名だけ**（パスとクエリは書かない — URL のクエリに鍵や個人の情報が入ることがある。#71）。断った先が
  IP の直書きならその IP を書く（ログは端末の中にあり、どの住所を読もうとしたかは診断に要る。#71 が守るのは秘密で、
  住所の分類はそれに当たらない）
- **測っていない欄は `-`**（0 を書かない。#72 / Spec 59 の `ratio=-` と同じ）— 断った呼び出しの `status=` / `bytes=` /
  `chars=` / `encoding=` など
- `mode=` は本文を返したときだけ `readability | body | markdown | text | raw`、返さなかったら `-`（`outcome=` と重ねない）
- `links=` は引数の値（`yes` / `no`）。`share=` は `mode=readability` のときの比の百分率（整数）、他は `-`
- `outcome=` は `ok | empty | bad_url | bad_scheme | blocked_address | robots_disallowed | robots_unavailable |
  too_many_redirects | http_error | too_large | unsupported_type | unsupported_encoding | timeout | interrupted |
  network_error` の 15 値（`empty` は D2 の「本文を取り出せませんでした」。rev5 で足した）
- ツールの結果の本文は今までどおりログに書かない（`tool:` 行は字数だけ）。**結果の中の最終 URL がログへ出る経路は無い**

### D10. GUI なし・コンテナ

純 Rust なので `fuseforks-cli` でもそのまま動く。`bake` は何も写さない（設定を持たない）。コンテナの中から取りに行く先は
コンテナのネットワークの外（D4 の判定は同じ）

## 未決（2026-11-21 利用者裁定 — 5 つとも推しのとおり）

1. **`max_length` の既定。** Docker の版は 5,000 字。既定が効くのはモデルが `max_length` を省いた呼び出しだけで、
   今のログでは 4 割の呼び出しがモデル自身の指定で 5,400 字を超えている（現況）。小さいと続きを読むたびに 1 周増え、
   周ごとにプロンプト全体を送り直す。大きいと読まない部分まで払う（Jev の圧縮が後ろで削る）。
   **→ 裁定: 20,000 字**（圧縮前の上位 5% が 20,235 字。ただし返った字数は `max_length` で頭打ちになった後の分布なので、
   ページの本当の長さはこれ以上）
2. **robots.txt を見るか。** **→ 裁定: 見る**（規則は D7）。理由は 2 つ — 村が今使っている Docker の版が既定で見ているので、
   見ないと今より緩くなる / 予定の発火で無人で回る村があり、利用者が開いたブラウザとは扱いが違う。crate は P0a で選ぶ
3. **既定で提示するか**（`DEFAULT_ENABLED_TOOLS`）。**→ 裁定: 入れない**（rev1 の推しを反転）。入れると、
   `enabledTools: null` の個体は**アプリを更新しただけで**モデルが選んだ URL へ送れるようになり、Goal 5 と
   `enabled_tools_invariant` の「更新しただけで送信先が増える作りにしない」（`image` の凍結の理由）の両方を破る。
   代償は、新しく来た人がサーヴァント設定でチェックを付けるまで使えないこと（案内とサーヴァント設定の注記で受ける — P2）。
   入れるなら、Goal 5 を「明示の配列を持つ個体」に縮め、`enabled_tools_invariant` の理由を書き換える
4. **プロキシ。** D4 は `no_proxy()` で、社内のプロキシの内側の端末では使えない。プロキシを許す設定を作るか（作るなら
   プロキシ越しでは住所の判定が効かないことを画面に書く）。**→ 裁定: 作らない**（頻度を見てから）
5. **Docker の fetch との並び。** 両方ある個体で同梱のほうを使わせたいなら、説明文で優先を書くか、利用者に `mcp.json` の
   `--servers` から fetch を外してもらう（DETAIL に書く）。**→ 裁定: 後者**（説明文で他のツールより優先を書くと、他人の
   MCP の名前を説明文が知ることになる）

## Tasks

- **P0a（測定）**
  - 実ページ 20 本以上（日本語のニュース記事・短い記事・ブログ・技術文書・一覧ページ・Wikipedia・GitHub・Shift_JIS /
    EUC-JP のページ・リダイレクトするページ・`text/markdown` を返すサイト・`Content-Type` の無い応答）で、
    dom_smoothie の本文と `dom_query` の `md()` の `<body>` 全体の字数・`<body>` のテキストの字数を並べ、D2 の比の閾値を
    決める。`md()` の変換の質もここで見る。可能なら同じ URL を Docker の fetch でも取り、字数と中身を比べる
  - 文字コードの宣言が 1 つも無い非 UTF-8 のページが何本あるか（D3 の chardetng の判断）
  - `dom_query` の Markdown が `&amp;` を出す場所（リンクと画像の URL だけか）
  - robots.txt の crate の候補（RFC 9309 に沿うもの）と依存の増分
  - PDF の URL は `unsupported_type` の経路を確かめるためだけに使う（D2 の閾値の測定には入れない）
- **裁定** — 未決 1〜5 は P0a の前に閉じた（2026-11-21）。P0a の数を見て利用者が決めるのは 4 つ（「P0a 実測記録」の末尾）
- **P0b（凍結）** — **完了（2026-11-21）**。`data_contract.yaml` へ `web_fetch_contract`（D1〜D9）/ `tool_prune_contract` の対象の記述 /
  `enabled_tools_invariant`（`BUNDLED = DEFAULT ∪ {run, image, fetch}` = 10 = 7 + 3。理由に「モデルが選んだ URL へ送れる」を足す）/
  `ToolOutput` の 2 欄
- **P1（コア）** — **完了（2026-11-21。「P1 実装記録」）**。`tools/fetch.rs`（`FetchTool`）/ 住所の判定（純関数 + 単体。上流の表の境界の両側・IPv4 射影・NAT64・
  `::/96`）/ 解決器と `redirect::Policy::custom` / 文字コードの判定（純関数 + 単体。青空文庫の先頭のバイト列・BOM と
  ヘッダの食い違い・XML の宣言を fixture に）/ `ToolOutput` の 2 欄と `turn.rs` の組み立て（枠が圧縮の外に残る・`raw` が
  圧縮されない・RepeatGuard と `omitted` が枠込みの本文を見る）/ 本文の取り出し（`links` の 2 通りを 1 つの経路で・比・
  ページ全体への戻り・`empty`。P0a で見た形を写した**合成の** fixture — 一覧の取りこぼし・ナビの多い記事・`<frameset>`。他人のページを repo に置かない）/ `&amp;` の後処理
  （純関数 + 単体）/ robotxt と `http_body` の「先頭だけ読む」関数 / `BUNDLED_TOOL_NAMES` / `prunable` / `defuse` / 計器 /
  `boot.rs` の登録 / `http_body` の走査テストの列挙に `tools/fetch.rs` を足す。
  **ループバックのスタブへ繋ぐテストは `tools/fetch.rs` の中の単体だけ**に置き、ループバックを許す構築は
  `#[cfg(test)]` の中にだけ書く — **配布物にはその構築が存在しない**（結合テスト `tests/*.rs` からは見えない）。
  結合テストは「本番の構築では 127.0.0.1 のスタブに繋がらず `blocked_address` になる」の向きで書く
- **P2（画面）** — **完了（2026-11-21。「P2 実装記録」）**。`AgentSettingsDialog.vue` の行 / `toolLabel.ts` の `KNOWN_TOOLS` と辞書 3 言語 / `RoleDialog.vue`
  （既定集合の外なので入れない）/ `defaultEnabledTools.test.ts` / サーヴァント設定の行に「チェックすると
  モデルが選んだ URL を取りに行きます」の注記
- **P3（台帳）** — DETAIL 3 言語（同梱ツールの表・取得の節・Docker の fetch を外す手順）/ README 3 言語の同梱ツールの列挙 /
  PRIVACY 日英: **要旨の「2 つだけ」/ 4 章の冒頭「設定した接続先に対してのみ」/ 4-4 の「MCP のツールが返した」
  「MCP 以外」**を書き換え、取得の節を足す（送るもの: モデルが選んだ URL への GET・この端末の IP・User-Agent・
  robots.txt の GET）/ `failures.md` は出たら
- **P4（実機）** — 提示: チェックした個体にだけ `fetch` が出る（`enabledTools: null` の個体には出ないことも見る）/ 日本語の記事と Shift_JIS のページが読める / `http://127.0.0.1:…`・`http://192.168.…`・
  `http://[::ffff:127.0.0.1]/` を断る（理由が本文に出る）/ 一覧ページで `<body>` 全体へ戻る / 「■ 停止」で取得中に
  止まる / Jev の圧縮が掛かり、枠（URL・続きの案内）が残る（`tool prune:` の `name=fetch`）/ 所要時間を Docker の
  fetch と比べる

## P0a 実測記録（2026-11-21）

使い捨てのプログラム（reqwest 0.12 + dom_smoothie 0.19 + dom_query 0.29 + encoding_rs 0.8 + robotxt 0.7。UA は
`spec-probe/0.1`、`Accept` は D5 と同じ、プロキシなし、リダイレクトは 5 段まで）で 31 本を取った。内訳は HTML 27
（技術文書 5・論文の要旨 1・GitHub 1・Wikipedia 日英 2・一覧 3・日本語の記事 3・ブログ 2・短いページ 1・官公庁 1・
Shift_JIS 3・EUC-JP 1・ISO-2022-JP 2・`charset=` が空 1・http から https へのリダイレクト 2。重なりあり）/
`text/markdown` 1 / `text/plain` 1 / JSON 1 / PDF 1。プログラムと出力は scratchpad に置いて捨てる（Spec 34 と同じ扱い。
数字はここが正）。

### 取得の時間

- 31 本の取得は中央値 78 ms・最長 945 ms（MDN）。Docker の fetch は中央値 2,309 ms（現況。ページは違う）
- 本文の抽出（dom_smoothie）は最長 77 ms（青空文庫 618 KB）、`md()` は最長 3 ms

### 文字コード（D3）

- 決まった経路は、応答ヘッダ 18 本 / 本文の宣言 8 本 / どれも無い 1 本。27 本とも U+FFFD は 0
- **宣言の無い非 UTF-8 は 0 本。** 27 本に加え、古い日本語の個人サイト・大学・ISP などのページ 24 本の宣言だけを調べたが、
  非 UTF-8 のページはすべて宣言を持っていた
- ラベル `shift-jis`（vector.co.jp）と `iso-2022-jp`（2 本）は `Encoding::for_label` が解釈した。
  `Content-Type: text/html; charset=`（値が空・nurs.or.jp）は解釈できずに次へ進み、本文が ASCII だけなので UTF-8 で読めた
- **ISO-2022-JP の宣言を持つページが 2 本あった**（jp.freebsd.org・st.rim.or.jp）。ISO-2022-JP は
  `Encoding::is_ascii_compatible()` が偽なので、rev2 の D3（上流の main と同じ「ASCII 互換でない宣言は無視」）では UTF-8 に
  落ち、エスケープ列（`ESC $ B`）が本文に残る。宣言どおりに読むと 2 本とも U+FFFD 0 で読めた → **D3 を直した**
- 7 ビットだけの ISO-2022-JP は UTF-8 としても正しいバイト列なので、「UTF-8 として正しいか」では見分けられない

### 本文の抽出（D2）

比 = dom_smoothie の本文（`TextMode::Formatted`）÷ `<body>` のテキスト（`script` / `style` / `noscript` / `template` /
`svg` を除く）。どちらも空白を除いた字数。

| ページ | 比 | 本文を取れたか（出力を読んで確かめた） |
|---|---|---|
| ITmedia NEWS の一覧 | 0.103 | **取りこぼした**（先頭の「ピックアップ」5 件だけ） |
| PC Watch の記事 | 0.147 | 取れた（本文と画像を最後まで） |
| GIGAZINE の記事 | 0.225 | 取れた |
| MDN（日本語） | 0.282 | 取れた（見出し 11 本が `<body>` 全体と同じ） |
| vector.co.jp（入口のページ） | 0.348 | 取れた（中央の欄） |
| ITmedia NEWS の記事 | 0.375 | 取れた（末尾に関連記事の見出しまで含む） |
| simonwillison.net の短い記事 | 0.404 | 取れた |
| 残り 19 本 | 0.45〜1.00 | 取れた |
| 阿部寛のホームページ（`<frameset>`） | — | dom_smoothie が失敗、`<body>` も空 |

- **比では取りこぼしと成功を分けられない。** 取りこぼした 1 本（0.103）と最も比の低い成功（0.147）の差は 0.044。
  ナビの多いニュースサイトでは、記事を最後まで取れても比が 0.15〜0.38 に下がる。rev2 の「短い記事は比が高い」は外れた
- 抽出に成功した 26 本のうち、取りこぼしは 1 本（一覧ページ）
- `<frameset>` のページは本文も `<body>` も空。`raw: true` なら `<frame src>` が見える

### 村が今使っている Docker の fetch の実際

- **像に Node.js が無く、記事の抽出をしていない。** 像は `python:3.12-slim-bookworm`（`4503e2d` の `Dockerfile`）で、
  `readabilipy` は `use_readability=True` でも Node.js が無いと純 Python の簡略化へ黙って切り替える（`simple_json.py` の
  `have_node()`）。純 Python の簡略化は `nav`・フォーム・画像・`script` などを消し、`a`・`strong`・`code` などは**タグを外して
  文字だけ残す**。返るのは「`<nav>` 以外のページ全体の文字・リンクの URL なし・画像なし」
- 同じ HTML に `readabilipy`（純 Python）+ `markdownify` を当てた再現では、**GitHub の README がほぼ全部落ちた**
  （「Readability」の出現が 1 回 対 dom_smoothie 37 回）。docs.rs もクレートの説明文の大半が落ちた（`ClientBuilder` が 2 回
  対 8 回）
- 青空文庫（`<?xml … encoding="Shift_JIS"?>` で始まる）は、文字列で渡すと lxml が `ValueError`（エンコーディング宣言のある
  Unicode 文字列は扱えない）で落ちた。Docker の版は `response.text`（文字列）を渡すので同じ形で失敗するはず（像そのものでは
  確かめていない）

### リンクの URL が字数の 4 割

- Docker の版の再現が成功した 26 本の合計: dom_smoothie 367,499 字 / **リンクの URL と画像（`![alt](url)` まるごと）を除くと
  206,198 字（44% 減。うち画像は 4%）** / Docker の版の再現 219,885 字
- **URL を除いた dom_smoothie は、Docker の版の出力とほぼ同じ字数。** URL を残すと今の約 1.67 倍になる
- ページごとの URL の割合は 0.08（Rust の本）〜 0.84（GIGAZINE の一覧）。一覧とニュースサイトほど高い

### `&amp;`（D2）

- `dom_query` 0.29 の Markdown は、リンクと画像の行き先だけを `html_escape` に通す（`serializing/md/text_utils.rs` の
  `push_md_url`）。`&` → `&amp;`・`"` → `&quot;`・NBSP → `&nbsp;` の 3 つで、地の文には掛けない。dom_smoothie の Markdown も
  同じ関数を通る（`readability.rs` の `root_node.md(None)`）
- 27 本で `&amp;` はすべて行き先の中だった（dom_smoothie 105 回 / `<body>` 全体 360 回）。`&quot;` と `&nbsp;` は行き先の中に 0
- `html_escape` は `&` を例外なく書き換えるので、行き先の中の `&` は必ず 3 つのどれかの頭。左から 1 回だけ戻せば元の URL に
  正確に戻る

### robots.txt（D7）

| crate | 版・更新 | `Cargo.lock` の増分 | 長い行 + 多バイト文字 | ランダムな入力 5 万件 | Google の判定器との食い違い（43,635 件） |
|---|---|---|---|---|---|
| robotstxt | 0.3.0・2021-02 | +1 | **panic** | panic 23 | （基準） |
| robotxt | 0.7.0・2026-10 | +2（bstr 1） | 通る | panic 0 | **0** |
| texting_robots | 0.2.2・2023-03 | +6 | 通る | panic 0 | 86 |

- **robotstxt（Google の C++ 判定器の移植）は、1 行が 16,664 バイトを超えた先に多バイト文字があると panic する**
  （`parser.rs` 169 行。超えた分を文字数で数えてバイト位置に足すので、文字の途中で切る）。配布版は panic で落ちる設定なので、
  外のサイトが返す robots.txt 1 本でアプリが落ちる。依存 0 だが採れない
- robotxt は `#![forbid(unsafe_code)]`・バイト列で処理・先頭 500 KiB だけを読む（Google と同じ上限）・UA の照合は大文字小文字を
  区別しない（RFC 9309）。状態コードの扱い（`AccessResult`）は 4xx を全部許すので、D7 の規則とは違う → 使うのは `Robots::parse` だけ
- 27 本のうち robots.txt が 200 で返ったのは 19 本で、すべて `Fuseforks` に許していた。404 が 8 本（規則なし = 通す）

### 依存

- dom_smoothie 22 + encoding_rs 3 + robotxt 2 = **27**（実際の村のワークスペースへ `cargo add` して `Cargo.lock` の差を数えた。
  encoding_rs の直接の依存 5 本のうち 2 本は既にある）。chardetng を足すと +1

### その他

- PDF（arxiv.org/pdf）は `Content-Type: application/pdf` で本文を読まずに止まった（25 ms）
- `developers.cloudflare.com` は `Accept` に `text/markdown` を入れると `text/markdown` を返した（6,853 バイト）。Docker の版
  （`4503e2d`）はこの `Accept` を送らないので、今は HTML を受けている
- `Content-Type` の無い応答は、31 本にも追加の 24 本にも無かった。P1 のスタブで確かめる

### 裁定（2026-11-21 利用者 — 4 つとも推しのとおり。D1 / D2 / D3 / D7 へ反映済み）

1. **抽出が痩せたときの戻り方（D2）。** 比の閾値は使えない（上の表）。**→ 裁定: 閾値を持たない。** dom_smoothie が失敗したか
   空を返したときだけ、`<body>` から `nav` / `header` / `footer` / `aside` / `form` などを除いた `md()` へ戻る（PC Watch の記事で
   3,709 字 = dom_smoothie とほぼ同じ）。どちらも空なら「本文を取り出せませんでした。`raw: true` で HTML を読めます」。
   枠の「取り出し:」の行に比を出す（例「本文（ページの文字の 10%）」）— 一覧の取りこぼしに気づく手がかりをモデルに渡す
2. **リンクの URL を残すか（新）。** 残すと字数が今の約 1.67 倍になり、残さないとモデルはページの中のリンクをたどれない
   （今の Docker の版もたどれない）。**→ 裁定: 既定で落とし、引数 `links`（既定 false）で残す。** 既定の字数は今と同じになり、
   リンクが要るときだけ払う。代償は引数が Docker の版より 1 つ多いこと（D1 を直す）。`links: false` のとき画像は
   代替テキストも含めて落とす（Docker の版と同じ）
3. **chardetng（D3）。** 0 本。**→ 裁定: 入れない**（+1 で安いが、効く実例が無い）
4. **robots.txt の crate（D7）。** **→ 裁定: robotxt 0.7**（`default-features = false, features = ["parser"]`）

## P0b 凍結記録（2026-11-21）

`data_contract.yaml` に書いたもの:

- **`web_fetch_contract` を新設**（`image_gen_contract` の後ろ。不変条件 16 本 + 退けた形）。D1〜D10 と裁定 4 つを
  そのまま写した — 引数 5 つ / 枠と本文 / 位置の単位 / 本文の取り出し（`links` の 2 通りを 1 つの経路で・ページ全体への
  戻りは失敗か空のときだけ）/ 比は出すだけ / `&amp;` の後処理 / 文字コード（ASCII 互換でない宣言も宣言どおり）/
  取りに行ける先（表と 3 つの経路）/ 30 秒・5 MiB / 送る見出し / 中身の種類 / robots.txt（robotxt・先頭 500 KiB）/
  `ToolOutput` の 2 欄 / 計器（`outcome` 15 値）/ GUI なしと PRIVACY
- `enabled_tools_invariant`: 見出しに「Spec 78 で 10 本」、Spec 76 の行の下に Spec 78 の行（`BUNDLED = DEFAULT ∪
  {run, image, fetch}` = 10 = 7 + 3。既定集合に入れない理由 = 更新しただけで送信先が増える）
- `tool_prune_contract`: 見出しの「MCP 由来のツールと rag」が rev4（rag を外した）の後も残っていたのを直し、対象の行に
  「同梱の fetch も真を返す」
- `ToolOutput` を「3 つ」と書いていた行（Spec 70 の節）に、Spec 72 と Spec 78 で欄が増えたことを足した — **Spec 72 の時点で
  既に古かった**（数の記述は欄の名前で grep しても出ない。#62 の形）
- 同梱ツールの本数を書いた 2 行（`tool_extension_point` の「現在 9 本」/ `command_tool_contract` の等式）と、起動の順序の
  同梱ツールの列挙に fetch を足した
- 構文: このファイルは編集前から厳密なパーサを通らない（「TTL を分ける理由」の行で止まる既知の件）。編集の後も同じ行で
  止まり（挿入した 122 行ぶんずれただけ）、今回の編集はすべてその手前にある。`web_fetch_contract` を単独で解析すると
  通る（107 行）

## P1 実装記録（2026-11-21）

ブランチ `claude/web-fetch`（worktree）。コアとホストだけで、画面（P2）はまだ。

**置いたもの**

- `crates/fuseforks-core/src/tools/fetch/` — `mod.rs`（`FetchTool`・HTTP・リダイレクト・切り出し・枠・計器。**外へ出る通信はこの
  ファイルにだけ置く**）/ `address.rs`（住所の判定。上流の main の `_is_public_address` の表 2 つと順序をそのまま写した純関数）/
  `charset.rs`（BOM → ヘッダ → 先頭 1,024 バイトの宣言 → UTF-8。WHATWG の prescan を簡略に写した）/ `extract.rs`（dom_smoothie →
  本文の HTML を dom_query で読み直して `md()`。失敗か空なら `<body>` 全体。`&amp;` の後処理）/ `robots.rs`（状態コードの振り分けと
  1 時間の棚）。Spec は `tools/fetch.rs` の 1 ファイルと書いていたが、純関数の 4 つを分けた
- `ToolOutput` に `frame_head` / `frame_tail` / `no_prune` と `whole()`。`turn.rs` は `ToolOutput` を丸ごと受けてから欄を取る形に
  変えた（分解のタプルに欄を足し忘れて黙って既定へ落ちる形を、構造で無くした）。圧縮に回すのは `text` だけで、圧縮したら
  「先頭の枠 + 圧縮した本文 + 末尾の枠」を返す。枠が空のツール（`fetch` 以外の全部）は今までと 1 バイトも変わらない
- `http_body::read_head`（先頭だけ読み、読み取りの失敗を返す）と、外へ出る HTTP の走査テストの列挙に `tools/fetch/mod.rs`
- `BUNDLED_TOOL_NAMES` に `fetch`（10 本・名前順）。`build_host` で登録（User-Agent の版は `version:` の行と同じ `opts.app_version`）
- 依存: dom_smoothie 0.19 / dom_query 0.29 / encoding_rs 0.8 / robotxt 0.7（parser のみ）と、コアの tokio に `net`。
  `Cargo.lock` に増えたのは **27**（P0a の見積もりと同じ）

**仕様から変えた点**（Spec の D4 / D6 / D7 と契約へ書き戻した）

1. リダイレクトは `Policy::custom` ではなく `Policy::none` + 自前のループ（robots.txt を各段で見るには非同期が要る）
2. robots.txt の取得が住所の判定で断られたら `blocked_address`。**結合テストが先に見つけた** — localhost は最初の要求の
   robots.txt で解決器に当たり、`robots_unavailable`（「robots.txt を確かめられなかった」）と書いていた
3. robots.txt の棚に覚えるのは規則が決まったときだけ（一時的な失敗で 1 時間読めなくしない）
4. `omitted` が戻すのは `text` の段落（枠は圧縮しないので戻す必要が無い）。rev5 の「枠を含めた全体」を訂正
5. 数でない値・範囲の外の引数は既定か端へ寄せる（`bad_args` を足さない）

**テスト**

- 単体 44 本（住所 4・文字コード 9・取り出し 8・robots 4・本体 19）。本体の 19 本はループバックのスタブへ繋ぐ —
  ループバックを許す構築（`for_loopback_tests`）と方針の欄は `#[cfg(test)]` の中にだけあり、配布物には存在しない。スタブは受けた要求の
  道と見出しを記録するので、「robots.txt が断ったページは要求しない」「Accept-Encoding を送らない」まで見ている
- 結合 `tests/web_fetch.rs` 3 本 — **本番の構築は 127.0.0.1 にも localhost にも 1 本も繋がない**（スタブへの接続数が 0）・`fetch:` の行・
  提示の門（`enabledTools: null` の個体には出ない）
- 結合 `tests/tool_prune.rs` に 2 本 — 枠が圧縮の外に残る / `no_prune` の結果は採点器を呼ばずにそのまま
- 時間切れは `start_paused` の時計で 30 秒を進めた（実時間を待たない）
- **画面の 2 本が P1 の時点で赤だった**（`toolLabel.test.ts` / `defaultEnabledTools.test.ts`）— Rust の表に `fetch` が増え、画面の表に
  まだ無かった。表のずれを捕まえる網が働いた結果で、P2 で通った。**P1 だけを main へ入れると main が赤になる**ので、P2 までブランチに留めた
- core + host の全テスト 1,638 本全緑・clippy 0。**変異 12 回とも予測どおり**（解決器が判定しない → 本番の構築の結合 1 本 / IP の直書きを判定しない → 3 本 / 圧縮した本文に枠を付け直さない・枠ごと圧縮に回す → それぞれ枠の 1 本 / `no_prune` を見ない → 1 本 / ASCII 互換でない宣言を
  無視する → ISO-2022-JP の 1 本 / 401・403 の robots.txt を通す → 2 本 / `links: false` でもリンクを残す → 2 本 / 行き先の `&amp;` を
  戻さない → 5 本 / 続きの案内の位置をずらす → 1 本 / 封筒を無害化しない → 1 本 / `Disallow` を見ない → 1 本。復元はハッシュで確認）

## P2 実装記録（2026-11-21）

同じブランチ `claude/web-fetch`。画面だけ（コアも IPC も 0 行）。

- `AgentSettingsDialog.vue` の同梱ツールの一覧に `fetch` の行（`needsWorkDir: false` — 作業フォルダを使わない）。
  チェックしている間だけ、その行の直下に注記「web の取得を許しています。モデルが選んだ URL へこの端末から取りに行き、相手の
  サイトにはこの端末の IP アドレスと User-Agent（Fuseforks）が残ります。この端末と同じネットワークの中と、robots.txt が断る
  ページは読みません」（`data-tool-note="fetch"`。Spec 76 の `image` の注記と同じ置き場）
- `toolLabel.ts` の `KNOWN_TOOLS` に `fetch`、辞書 3 言語に `tools.fetch`（会話ペインの動作名「web ページを読む」）と
  `agentSettings.tools.fetch`。doc の「同梱 11 本 + 合成 3 種」は本数を書かない形へ（足すたびに腐る — #62）
- `RoleDialog.vue` は変えていない（既定集合の外なので役職の雛形のチェックに入れない、で Tasks のとおり）
- **既存の誤りを 1 つ回収した** — 設定の「既定」の札が 3 言語とも「既定（すべて。新ツールも自動追加）」のままだった。
  **Spec 15 で `run` が既定集合の外に出た時点から嘘**で、Spec 76 の `image` で外れが 2 つになっていた。「run・image・fetch 以外」へ
  直し、`defaultEnabledTools.test.ts` に「札は既定集合の外のツールを 3 言語とも名指しする」を足した（外の集合は Rust の
  `BUNDLED_TOOL_NAMES` − `DEFAULT_ENABLED_TOOLS` から引くので、外のツールを足して札を直し忘れると赤）
- 初回案内のサーヴァントの歩（`tour.steps.servant.body`）に「web の取得」と「コマンド実行と web の取得はチェックを付けるまで使えません」
- P1 で赤だった画面の 2 本（`toolLabel.test.ts` / `defaultEnabledTools.test.ts`）はこれで通った

**テスト**: 走査 `agentToolNotesWiring.test.ts` に `fetch` の注記の 1 本 / `defaultEnabledTools.test.ts` に札の 1 本。
vitest 1,209（140 ファイル）全緑・vue-tsc 0・build 緑。**変異 5 回とも予測どおり**（`KNOWN_TOOLS` から落とす → `toolLabel.test.ts` /
一覧の行を落とす → `defaultEnabledTools.test.ts` / ja の札から fetch を落とす → `defaultEnabledTools.test.ts` / 注記の条件を壊す →
`agentToolNotesWiring.test.ts` / ja の `tools.fetch` を落とす → `toolLabel.test.ts` と `index.test.ts`。復元はハッシュで確認）。
**実機は未確認**（P4）

## Notes

1. **プロキシを外す代償。** 名前解決を差し替えても、プロキシ越しだと名前を引くのはプロキシなので、こちらの判定は
   ホスト名にしか掛からない。LLM の呼び出しは今も環境変数のプロキシに従っている（reqwest の既定）ので、`fetch` だけが
   プロキシを使わない形になる。社内のネットワークで「LLM は使えるのに fetch だけ繋がらない」が起きうる
2. **MCP のツール結果の封筒。** D6 の `defuse` は同梱の `fetch` にだけ掛ける。MCP 由来の結果（Docker の fetch を含む）にも
   同じ穴があるが、他人の MCP の結果を書き換えることになるので本 Spec の範囲外
3. **Docker の fetch を自動で外さない理由と、併用の代償。** どの MCP サーバーのどのツールが同じ役かを、アプリは名前でしか
   知らない。名前で決め打ちすると、利用者が意図して残した fetch を黙って消すことになる。**併用している個体では Goal 4 が
   成り立たない** — Docker の版（`4503e2d`）は住所を判定しない。ただしコンテナの中の 127.0.0.1 はコンテナ自身で、
   端末のループバックではない（LAN とクラウドのメタデータの口に届くかは確かめていない）
4. **JavaScript で中身を組み立てるページは読めない**（Docker の fetch と同じ。ブラウザを持たない）。X（旧 Twitter）など
5. **依存の版が 2 つ並ぶ。** Tauri の `dom_query` 0.27 / html5ever 0.38 と、dom_smoothie の 0.29 / 0.40 が GUI の
   ビルドでは両方入る。Tauri か dom_smoothie が版を揃えたら 1 つに戻る
6. **rev2 の査読の表**（査読 2 系統。重なりを除いて 43 項目）

   | # | 指摘 | 判定 | 反映 |
   |---|---|---|---|
   | 1 | Goal 5 と未決 3 の推し（`null` の個体に生える）が衝突 | 採用 | 推しを「入れない」へ反転。Goal 5 に `null` の個体を含むと明記 |
   | 2 | Goal 4 と D8 の併存（Docker の fetch は判定を持たない） | 採用 | Goal 4 の主語を同梱の `fetch` へ。Docker の版に判定が無いことを原典で確認（Notes 3） |
   | 3 | Goal 3 と D3（宣言の無いページは化ける） | 採用 | Goal 3 を宣言のあるページに絞り、推定は P0a の数で決める |
   | 4 | Goal 1 と Notes 4（JavaScript のページ） | 採用 | Goal 1 をサーバーが HTML で返すページに絞る |
   | 5 | D1「呼び方を変えずに済む」は引数の形だけ | 採用 | 名前と `max_length` の既定が変わると明記 |
   | 6 | 先頭の行の順と題名の出所が未定義 | 採用 | 枠の行の順を定義。本文を返さないときの形も |
   | 7 | `start_index` / `max_length` の単位と数える範囲 | 採用 | 本文の `char`。枠は数えない |
   | 8 | `raw` と `prunable` / `defuse` | 採用 | 結果ごとの「圧縮しない」の旗。`raw` にも `defuse` |
   | 9 | 続きの案内が圧縮で落ちる | 採用 | 枠を圧縮の外に置く欄を `ToolOutput` へ |
   | 10 | `defuse` と `max_length` の順 | 採用 | `defuse` を掛けてから切る |
   | 11 | robots.txt の扱いが記憶ベース | 採用 | 原典で確認（Docker の版も上流の main も既定で見る） |
   | 12 | robots.txt の取得に D4 / D5 を掛けるか・30 秒の範囲・失敗時 | 採用 | D7（上流の main に揃える）と D5（呼び出し全体で 30 秒） |
   | 13 | `outcome` の欠け（robots・スキーム・リダイレクト上限） | 採用 | D9 を 14 値へ |
   | 14 | PRIVACY の既存の文 3 か所と矛盾 | 採用 | P3 で 3 か所を名指し |
   | 15 | IPv4 射影 IPv6・ブロードキャスト | 採用 | 上流の main の表に揃える（D4） |
   | 16 | `0x7f000001` / `2130706433` の別表記 | 前提を訂正して採用 | url crate が `Ipv4(127.0.0.1)` へ正規化することを実測。判定は文字列ではなく `Url::host()` の型で |
   | 17 | リダイレクトの各段の IP の直書きは名前解決を通らない | 採用 | `redirect::Policy::custom` と明記。接続器が IP の直書きで解決を飛ばすことを原典で確認 |
   | 18 | `dns_resolver` が今の feature で使えないかもしれない | 反証 | reqwest 0.12.28 の `dns_resolver` は feature に縛られていない |
   | 19 | 戻り先の根拠が htmd の数字・`dom_query` の Markdown の有無 | 前提を訂正して採用 | `md()` は既定の feature に在る（原典）。質は P0a で測る |
   | 20 | 「依存は 22 のまま」 | 採用 | 22 + 5 + robots の crate |
   | 21 | 痩せの閾値を絶対字数で決める | 採用 | 本文 ÷ `<body>` のテキストの比 |
   | 22 | `&amp;` の後処理の範囲 | 採用 | リンクと画像の URL の部分だけ |
   | 23 | D3 の BOM の順が WHATWG と逆 | 採用 | BOM を先頭へ |
   | 24 | XML の `encoding` 宣言 | 採用 | D3 の 3 に XML の宣言 |
   | 25 | `Content-Type` が無い応答 | 採用 | 上流の main と同じ（本文が `<html` で始まるときだけ HTML） |
   | 26 | 4xx / 5xx の本文を読むか | 採用 | 読まない |
   | 27 | gzip / brotli | 採用 | `Accept-Encoding` を送らない。届いたら断る。reqwest の feature を足さない理由も |
   | 28 | 5 MiB と続き | 採用 | 超えたら断る（途中までを返さない） |
   | 29 | 「即座に」と「1 箇所だけ」 | 採用 | 取得全体を `select!` で cancel と競わせる |
   | 30 | 計器に `defuse` の件数が無い | 採用 | `defused=` |
   | 31 | `defuse_quote_tags` を掛けない理由 | 採用 | D6 に書く |
   | 32 | 結果の中の URL のクエリがログへ漏れる | 反証 | ツールの結果は `fuseforks.log` に書かれない（字数だけ） |
   | 33 | 断った内部の住所をログに書くか・断った呼び出しの欄・`mode` と `outcome` の重なり | 採用 | 書く（理由つき）/ 測っていない欄は `-` / `mode` は本文を返したときだけ |
   | 34 | 個体別の数の合計が合わない | 採用 | `tool:` 行だけで数え直した（合計 1,063） |
   | 35 | 所要時間の母数 | 採用 | 欄を足した日以降の全数であって、大きさで選んだ部分集合ではないと明記 |
   | 36 | 5,000 の打ち切りの推論が弱い | 採用 | 数え直した。圧縮後の字数が混ざっていた。5,000 で切られているのは 4.7% |
   | 37 | 「P0 で 30〜680 ms」 | 採用 | 「現況の事前測定」へ |
   | 38 | P0 の凍結と未決の順 | 採用 | P0a 測定 → 裁定 → P0b 凍結 |
   | 39 | P0 の PDF | 採用 | `unsupported_type` の経路の確認だけに使う |
   | 40 | P1 のテスト用の抜け道 | 採用 | `#[cfg(test)]` の構築だけ。配布物に存在しない |
   | 41 | P4 が未決 3 次第 | 採用 | 未決 3 の結論ごとの見る項目を書く |
   | 42 | `http_body` の走査テストの列挙 | 採用 | P1 に入れる |
   | 43 | `toolLabel` の `KNOWN_TOOLS` と `RoleDialog` の `BUNDLED_TOOLS` | 採用 | D8 と P2 に入れる |
