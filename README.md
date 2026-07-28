# Concordia

複数の AI エージェントが相互に連携・会話するマルチエージェント・オーケストレーション
デスクトップ GUI。Bun + Tauri v2 + Vue 3 + Rust。

エージェントの稼働状態・トポロジー（接続関係）・参照 RAG・設定ファイル（`SKILL.md` 等）を
1 画面で視覚化し、制御する。

---

## ディレクトリ構造

```text
ConcordiaOrcehstrator/
├── Cargo.toml                       Cargo ワークスペース（resolver 3 / edition 2024）
├── data_contract.yaml               ドメイン名詞の台帳（型を変えたら先にここ）
│
├── crates/
│   └── agent-core/                  ★ 中核。GUI 層に一切依存しない
│       ├── src/
│       │   ├── lib.rs               公開 API と依存方向の宣言
│       │   ├── model.rs             ドメインの名詞（AgentId / AgentSpec / ModelTemplate …）
│       │   ├── error.rs             CoreError と、UI へ渡す ErrorPayload
│       │   ├── event.rs             CoreEvent（broadcast で押し出す状態変化）
│       │   ├── world.rs             登録簿。同期的な純データ構造（ロックを持たない）
│       │   ├── config_store.rs      SKILL.md / Memory.md / Construct.md と world.json の入出力
│       │   ├── orchestrator.rs      ★ ライフサイクルとメッセージ配送（Tokio）
│       │   ├── compute.rs           ★ CPU バウンド処理と Tokio↔Rayon の橋渡し
│       │   ├── rag.rs               RAG 索引（検索は Rayon 側で走る）
│       │   ├── secret.rs            秘密の保管（OS 資格情報ストア / テスト用の in-memory）
│       │   └── llm/
│       │       ├── mod.rs           LlmBackend / BackendFactory / EchoBackend
│       │       ├── canonical.rs     プロバイダ中立の型
│       │       ├── wire.rs          プロバイダの生 JSON 形（唯一の真実）
│       │       ├── openai_compat.rs OpenAI 互換 adapter（encode/decode 純関数）
│       │       ├── anthropic.rs     Anthropic Messages API adapter
│       │       ├── client.rs        HTTP 核（URL・ヘッダ・再試行）
│       │       └── error.rs         LlmError（再試行可否の判断軸）
│       └── tests/orchestrator.rs    結合テスト（ネットワーク不要）
│
└── apps/
    └── gui-tauri/                   ★ 外殻。agent-core に依存する
        ├── src-tauri/src/
        │   ├── lib.rs               ウィンドウ起動と IPC コマンド登録
        │   ├── state.rs             オーケストレーター組み立て + イベント中継
        │   └── commands.rs          IPC コマンド（薄い転送層）
        └── src/
            ├── types.ts             Rust 型のミラー（手で同期させる契約）
            ├── lib/ipc.ts           型付き invoke ラッパ
            ├── composables/useOrchestrator.ts   単一ストア
            ├── App.vue              3 ペインのグリッド
            └── components/
                ├── AgentList.vue / AgentCard.vue      左ペイン
                ├── TopologyMap.vue / MessageLog.vue   中央ペイン
                ├── InspectorPanel.vue / MarkdownEditor.vue   右ペイン
                └── ModelTemplateDialog.vue / ToastHost.vue
```

## クレート分離の保証

依存は一方向だけ。

```text
apps/gui-tauri  ──依存──▶  crates/agent-core
```

`crates/agent-core/Cargo.toml` に `tauri` が現れないことが、この分離の機械的な保証になっている。
GUI への通知は `CoreEvent` を `broadcast` チャネルへ流すだけで、受け手が Tauri か
テストコードかをコア層は知らない。結果として、**GUI を起動せずに全経路を検証できる**。

---

## 並行モデル — Rayon と Tokio の担当

| 担当 | ランタイム | 理由 |
|---|---|---|
| エージェント実行・LLM 呼び出し・配送 | **Tokio** | I/O バウンド。待機中にスレッドを占有しない |
| RAG 類似検索・ログ集計 | **Rayon** | CPU バウンド。コア数ぶんの並列で焼き切る |

橋渡しは `compute::spawn_rayon` が `oneshot` チャネルで行い、双方をブロックしない。

> **エージェント実行を Rayon に載せないこと。** Rayon のスレッドプールは物理コア数に
> 固定されるため、そこでネットワークを待つと 1 エージェント = 1 スレッド占有となり、
> 同時稼働数がコア数で頭打ちになる（8 コア機で 9 体目が刺さる）。
> 要求である「UI をブロックしない」は、この割り方でも完全に満たされる。

---

## UI の同期規則

真実はコア側にあり、`useOrchestrator` の `state` はその投影でしかない。
投影がずれないよう、規則を 2 つだけ置いている。

1. **変更系の IPC は `mutate()` で包み、成否によらず必ずコアから読み直す。**
   呼び出しごとに「ここは再同期が要るか」を判断する方式にしていたところ、
   判断を落とした経路（接続の更新・並び替え）だけが古い表示のまま残った。
   判断の対象にしないほうが正しい。参照系は `guard()` を使う。

2. **編集中の下書きを持つ画面は、保存後にコア側の値で作り直す。**
   保存してフォームを閉じると、保存した結果が画面から消えて
   「反映されていない」ようにしか見えない。保存後もその項目に留まり、
   コアが受け取った値をそのまま表示する（`ModelTemplateDialog` の `reseedDraft`）。

手元で「こうなったはず」と代入しないこと。コアが別の判断をしたときに食い違う
（例: キー削除後の取得元は `NotRequired` ではなく `Unset`）。

---

## 無限往復の抑止

相互接続されたエージェントは、放っておくと際限なく往復して課金を焼き続ける。
各発話は `hop` を持ち、`OrchestratorConfig::max_hops`（既定 8）に達した時点で連鎖を打ち切る。
打ち切りは `CoreEvent::HopLimitReached` で通知される — 黙って止めると
「なぜ会話が終わったのか」が UI から永久に分からなくなる。

トポロジー上の循環そのものは**許可**している。エージェント同士が往復するのは
このシステムの目的であり、止めるのは hop の層が正しい。

---

## LLM ワイヤ層

canonical ⇄ wire の adapter 分離を採る。オーケストレーターは canonical 型だけを組み、
方言の差分は adapter が全部持つ。新しいプロバイダを足す作業が adapter 1 ファイルに閉じる。

実運用で踏んだ罠は `data_contract.yaml` の `llm_wire.invariants` に列挙してある。
特に効くもの:

- `temperature` は `Option`。**未設定ならキーごと省略する** — 新しめのモデルは非対応で、送ると 400
- OpenAI 系の `tool_calls[].function.arguments` は **JSON 文字列**。decode 境界で 1 回だけ parse する
- 応答側の全フィールドに `#[serde(default)]`。互換を名乗るサーバは実際には形がまちまち
- 本文空 + tool_calls 空 + `finish == length` のときだけ「推論の空応答」として再試行に乗せる
- パース失敗は **raw を保持**する（却下理由と一緒に差し戻して再生成させる燃料）

API キーは **OS の資格情報ストア**に保存する（Windows 資格情報マネージャー /
macOS キーチェーン / freedesktop Secret Service）。`ModelTemplate` が持つのは
`credential`（取得元）だけで、**秘密を保持できるフィールドが存在しない**。
平文の `world.json` へ秘密が入る経路は型の段階で無い。

```rust
pub enum CredentialSource {
    Unset,        // 未設定（既定）。送信前に弾く
    NotRequired,  // 認証不要だとユーザーが明示した（ローカル推論サーバ）
    Keyring,      // OS の資格情報ストア。キーはテンプレート ID
}
```

`Unset` と `NotRequired` を分けているのは、**「まだ入れていない」と「要らない」が
別の状態**だから。まとめると、キー未登録のテンプレートが「認証不要」と解釈され、
認証ヘッダ無しのリクエストが外部へ出て、ローカルで捕まえられるはずの設定不備が
サーバー側の 401 になる。同じ理由で、キーを削除したときは `NotRequired` ではなく
`Unset` へ戻す。

秘密がプロセス内を通る区間は `LlmConfig::from_template` から HTTP ヘッダまでで、
設定ファイル・イベント・エラーメッセージ・IPC 応答のいずれにも現れない。
UI へ返るのは「登録済みかどうか」だけで、**値を読み出す API は存在しない**。

> ここは 2 度作り直している。当初は `String` の `apiKeyEnv` で、防御は UI のラベルと
> 注意書きだけだった。運用初日に実キーが貼られて平文で保存された。
> 次に環境変数名を要求する方式にしたが、これはデスクトップ GUI に不適合だった
> （端末操作と再起動を要求し、Windows では設定済みの変数が起動済みプロセスへ
> 伝播しない）。**注意書きは制御ではない。そして「書けなくする」だけでは足りず、
> 利用者が正しく置ける場所を用意するところまでが設計。**

---

## 開発

```bash
cd apps/gui-tauri && bun install
```

```bash
cd apps/gui-tauri && bun run tauri dev
```

```bash
cargo test --workspace
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

### 初回起動

API キーが未設定でもアプリは動く。`HttpBackendFactory::echo_on_failure` が
エコー応答へ退避する。ここで沈黙させると、設定不備なのか実装不具合なのかを
切り分ける手段が無くなるため。

**ただし退避は必ず名乗る。** 退避したときは `BackendDegraded` イベントで警告が出て、
応答本文にも理由（キーが未登録である等）が入る。退避したバックエンドは
キャッシュしないので、原因を直せばそのまま復帰する。

> 「エコー応答」とだけ名乗る実装だった頃、設定が届いていないだけの状態で
> 偽の応答が返り続け、原因に辿り着けなかった。**退避は許すが、黙って退避しない。**

実際に LLM へ繋ぐには、⚙ からモデルテンプレートを作り、`API キー` 欄にキーを貼って
`登録` を押す。**アプリ内で完結する。** 端末操作も再起動も要らない。

`プロトコル` を切り替えると `base URL` が既定値（OpenAI 互換なら
`https://api.openai.com/v1`、Anthropic なら `https://api.anthropic.com/v1`）に
追随する。手で入れた URL は上書きされない。

### ワークスペース

エージェント設定は OS のアプリデータ領域に置かれる。

```text
{app_data_dir}/workspace/
  world.json                  エージェント定義とモデルテンプレート
  agents/{agent_id}/
    SKILL.md
    Memory.md
    Construct.md
```

右ペインの 📁 ボタンから、選択中エージェントの設定フォルダを直接開ける。

---

## 意図的に未実装の部分

骨組みとして接続点だけ用意し、中身を入れていない箇所。**動くふりをさせていない**ので、
そのまま次の作業単位として着手できる。

| 箇所 | 現状 | 次の一手 |
|---|---|---|
| RAG の取り込み UI | `index_rag_chunk` コマンドは通っているが、GUI からの投入口が無い。右ペインの参照 RAG 欄は索引が空だとその旨を表示する | ファイル取り込み・チャンク分割・ソース管理の画面 |
| 埋め込みモデル | `rag::HashEmbedder` は語をハッシュで次元へ振り分けるだけで、意味的な近さを捉えない。名前でそれを明示している | `Embedder` trait を実装した実モデルへ差し替え |
| Gemini adapter | `Provider` は OpenAI 互換と Anthropic の 2 つ。canonical の接続点は開いている | `llm/gemini.rs` を足して `Provider` に 1 バリアント追加 |
| ツール呼び出しの実行 | ワイヤ層は `ToolSpec` / `ToolCall` を往復できるが、オーケストレーターは現状 `ToolChoice::None` で呼んでいる | ツール登録簿と実行ループ |
| ノード座標の永続化 | 手で動かした配置はセッション内のみ。既定は円環自動配置 | 座標を持つなら `world.json` の拡張が要る |
