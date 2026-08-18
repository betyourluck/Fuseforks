[English](README.md) | **日本語**

# <img src="images/logo.webp" alt="Outcasts Fuseforks Logo" width="28" /> Outcasts Fuseforks

  [![Tauri](https://img.shields.io/badge/Tauri-2.0-orange?style=for-the-badge&logo=tauri&logoColor=white)](https://v2.tauri.app/ja/)
  [![Vue](https://img.shields.io/badge/Vue.js-3.0-4FC08D?style=for-the-badge&logo=vue.js&logoColor=white)](https://vuejs.org)
  [![TypeScript](https://img.shields.io/badge/TypeScript-Strict-3178C6?style=for-the-badge&logo=typescript&logoColor=white)](https://www.typescriptlang.org)
  [![Rust](https://img.shields.io/badge/Rust-Backend-000000?style=for-the-badge&logo=rust&logoColor=white)](https://www.rust-lang.org)

**AI エージェントの村を、手元で飼う。**

Outcasts Fuseforks は、複数の AI エージェントが相互に連携・会話する
マルチエージェント・オーケストレーションのデスクトップアプリです。
エージェントを作り、繋ぎ、話しかけると、村が動き出す —
委譲し、手分けし、束ね、時刻が来れば勝手に働く。
その全部が 3 ペインの 1 画面に見えています。

![Outcasts Fuseforks Japanese Light](images/fuseforks.webp)

![Outcasts Fuseforks English Dark](images/fuseforks_en.webp)


Rust（`fuseforks-core`）+ Tauri v2 + Vue 3 + Bun。アプリ内の表示名は「Fuseforks」。

## 何ができるか

| | |
|---|---|
| 🏘️ **村を組む** | エージェントを作って絆で結ぶ。**サーヴァントの絆**がそのまま制御盤 |
| 🤝 **委譲と合流** | 進行役が `ask` で訊き、`plan` でワーカーへ並列に手分けして束ねる |
| ⏰ **予定** | 「毎週 木曜 17:00」「10 分ごと」で依頼が時刻発火する。cron 式は書かせない |
| 🔎 **前判定** | 発火時にまずコマンドを走らせ、**出力が合図と一致したときだけ**依頼する。一致しない回はトークンを 1 つも使わない。配られた村のコマンドは承認するまで走らない |
| 🔌 **MCP** | Claude Desktop の `mcp.json` を**そのまま貼れる**。共通 + エージェント別 |
| 🔍 **グラウンディング** | Gemini の Google 検索、Grok の Live Search（web / X）、OpenAI と Meta の web 検索。**検索した事実と、出典が返らない事実を区別して見せる** |
| 🧠 **思考の要約** | モデルが考えたことの要約を、答えとは**別の枠**に畳んで置く。出典は検証できる指し先、要約は検証できない申告なので混ぜない |
| 🛠️ **同梱ツール** | `remember` / `grep` / `fd` / `diff` / `sd` / `yq` / `file` / `rag` / `run`。ファイル系は作業フォルダの外を構造的に読めない（例外は宣言したフォルダを読む `rag` と、囲いが許可リストである `run`） |
| 🪧 **ツールの理由** | 道具を使うとき、**何のために使うのか**が 1 行で会話に出る。**モデルの自己申告**であって監査の記録ではない |
| 🎚️ **転送の可否** | 進行役には「会話を引き渡す」道具を持たせない設定。**委譲（訊いて答えを受け取る）と手分けは残る**ので、答えが利用者へ逸れずに戻る |
| 🗣️ **広場ログ** | 他人の会話が聞こえる村。聞かない自由もある（コスト設定として） |
| 📎 **パス補完** | 入力欄で `@` を打つと作業フォルダのファイルが候補に出る。**入るのはパスだけ**で、サーヴァントが探す周回が消える |
| 🖼️ **添付（画像・音声・動画・PDF）** | 入力欄へ貼り付けるか選ぶと、宛先のサーヴァントが見て・聞いて答える。**渡るのはそのターンだけ**（滑る窓での再送を避ける）。**運べない接続先へは貼った時点で警告し、送信も断る** |
| 🏛️ **村の条例** | 全員のプロンプト最上段に入る共通規則。モデル間の憲法差を揃える正規化層 |
| 🎭 **役職** | サーヴァントの雛形。選んで作れば設定が入り、一覧と地図に色付きバッジが出る |
| 📁 **作業フォルダの一括切り替え** | 村ごと別のプロジェクトへ向け直すとき、チェックした全員の作業フォルダを 1 回で変える。**稼働中でも次の発話から効く** |
| 💾 **会話の保存** | 閉じて開き直すと前回の続きから。複数の会話を持ち替え、途中から分岐できる |
| 📊 **統計** | この村がいくら払ったかを、会話 × **サーヴァント × モデル** × ターンの終わり方で読む（モデルを切り替えた個体は行が割れる）。単位はトークン（予算と同じ重みの実効トークン）。**モデルごとの単価を登録すると、おおよその金額（`≈ $`）も出ます**（[Spec 41](specs/41_model-pricing.md)。単価は「取得」ボタンか手入力で入れる。**単価が無いモデルは合計から外れ、外れたことが画面に出ます**）。失敗したターンの払いも入る。**記録はこの版から**（それより前の会話は数えられない） |
| ⚙️ **システム設定** | 自分の呼び名とアイコン・言語（画面と、サーヴァントへの指示の両方が切り替わる）・トークン制限・確認ダイアログ。**左メニューが設定できるものの目録** |

接続先は OpenAI 互換 / Anthropic / Gemini / xAI / OpenAI / Meta のネイティブ。**base URL は自由**なので、
Ollama や LM Studio などローカル LLM の口にもそのまま繋がる。

## 思想 — おもちゃの形をした本物

このアプリの想定ユーザーは**エンジニアのホビー**である。業務のオーケストレーション
基盤は狙わない — 業務には人件費という計算があり「人に確認させるより AI で回す」が
成立するが、個人では自分の時間より API 費のほうが目につく。その非対称は
アプリ側からは動かせない。

ただし、**ホビー向けだからこそ中身は本物にする**。単純なグループ会話ツールなら
エンジニアのホビーにもならない。初期の Linux が Solaris 使いからおもちゃ扱い
されながら、中身が本物の Unix だったから家に持ち帰る価値があった — それと同じ形を
狙う。

だから設計は 2 層で規律が違う:

- **核（`fuseforks-core` / `data_contract.yaml` / 発火規則）は業務品質。**
  契約を凍結してから実装し、テストは赤を見てから緑にする。
  GUI への依存はゼロ（機械的に保証。このクレートだけで headless に動く）
- **殻（村・キャラクター・3 ペイン）はホビーの体験。**
  「設定が少なくて分かりやすい」が差別化軸で、cron 式や YAML の壁を利用者に
  登らせない

両方を中途半端にやるのが唯一の失敗形である。かわいさのために契約を緩めない。
業務の顔をするために設定を増やさない。

## ビルド

必要なもの: **Rust 1.85 以上**（edition 2024）、**[Bun](https://bun.sh)**、
各 OS の Tauri v2 前提（Windows は WebView2、Linux は WebKitGTK、macOS は Xcode CLT）。

```bash
cd apps/gui-tauri && bun install
```

開発用に起動する（HMR あり）:

```bash
cd apps/gui-tauri && bun run tauri dev
```

配布物を作る。インストーラは `target/release/bundle/` に出る:

```bash
cd apps/gui-tauri && bun run tauri build
```

テストと lint:

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd apps/gui-tauri && bun run test
```

> **アプリを起動したまま `cargo test --workspace` を回すと落ちる** —
> 実行ファイルを置き換えられないため。コアだけなら `cargo test -p fuseforks-core`
> がアプリ稼働中でも通る。

`v*.*` のタグを push すると、3 OS 分のリリースビルドが GitHub Actions で走る
（[`.github/workflows/build.yml`](.github/workflows/build.yml)）。
通常のコミットでは走らない。**macOS のビルドは Apple Silicon 専用**で、
Intel Mac は対象にしない。

## 技術スタック

**核（`crates/fuseforks-core`）**

- **Rust** 2024 edition — オーケストレーション、発火規則、ツール、LLM ワイヤ層
- **Tokio**（I/O と並行ターン）+ **Rayon**（CPU 側）
- **redb** — 会話の永続化。純 Rust・C 依存なし
- **keyring** — API キーは OS の資格情報ストアへ。設定ファイルには保存しない
- **rmcp** — MCP のクライアント（外部ツールを繋ぐ）とサーバー（外から依頼を受ける）
- **GUI への依存はゼロ。** このクレートだけで headless に動くことを機械的に保証している

**殻（`apps/gui-tauri`）**

- **Tauri v2** + **Vue 3** + **TypeScript** + **Vite**
- **Tailwind CSS v4** — 配色は 1 箇所に集約。ライト / ダークの両対応
- **v-network-graph** — サーヴァントの絆（中央上段の地図。SVG なのでノードは Vue のスロット）
- **CodeMirror 6** — 条例・役職・設定の編集面
- **vue-i18n** — 日本語 / 英語
- テストは **vitest**、パッケージマネージャは **Bun**

## もっと詳しく

| | |
|---|---|
| [DETAIL.md](DETAIL.md) | ディレクトリ構造・並行モデル・画面の構成・同梱ツールの安全境界・LLM ワイヤ層・運用 |
| [data_contract.yaml](data_contract.yaml) | ドメイン契約。**実装よりここが正** |
| [specs/](specs) | 仕様。起票 → 査読 → Phase 分割で実装 |
| [failures.md](failures.md) | 踏んだ罠（症状 → 真因 → 処方 → 一般化） |
| [PRIVACY.md](PRIVACY.md) | プライバシーポリシー（**開発者は何も受け取らない**） |

## ライセンス

**MPL-2.0**（[LICENSE](LICENSE)）。選定の意図（2026-08-05）:

- **改良は還流してほしい** — この配布物に含まれるファイルを**書き換えて**
  配る場合は、**そのファイルの**ソース公開が必要です。良くなった Fuseforks は、
  元の村にも還ってくる形にしています
- **義務はファイル単位で止まる** — Fuseforks を部品として組み込んだより大きな
  成果物（MPL の言う Larger Work）は、**あなたの条件で配れます**（§3.3）。
  新しく書き足したファイルは最初から対象外です
- **私的な改変は私的なまま** — 自分のマシンで自分用に改変して使う分には
  公開義務は一切ありません。義務が発火するのは配布のときだけです
- コミュニティでの共同開発を歓迎します。プルリクエストは MPL-2.0 の下で
  受け入れます
