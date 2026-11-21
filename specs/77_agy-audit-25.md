# Spec 77: agy が挙げた 25 件の指摘の検証と、修正の TODO

- 状態: **A 群 7 件・B 群 17 件・C 群 9 件と #8b が着地 — 修正の TODO はすべて閉じた**（2026-11-19 起票 → 同日に 36 主張の判定を書いた。成立 16 / 一部成立 12 /
  不成立 6 / 設計どおり 2。ほかに指摘に無い問題 14 件（N1〜N14）。**N8 = OpenAI の課金切れの 429 が再試行されている、は `fuseforks.log` の
  実測**。→ 同日、利用者「A 群から着手」で A 群の 7 件（N8 / #3 / #5 + N2 + N3 / #15 / #11 + N11 / #14 / #1 + N9）を、どれも赤いテストを
  先に書いてから直した。全体 93 本・1,606 本が緑・clippy 0。記録は末尾の「A 群の実装記録」（`9a195f1`）。→ 2026-11-20、利用者「B 群から着手」で
  B 群の 17 件を同じ規律で直した（画面の 4 件はサブエージェント）。記録は末尾の「B 群の実装記録」（`a8ee8b9`）。→ 同日、利用者「C 群から着手」で
  C 群の 9 件も同じ規律で直した。記録は末尾の「C 群の実装記録」（`01e053b`）。→ 2026-11-21、利用者「#8b を進めよう」で、文書に答えが無いので
  実機で測り（HKLM の承認の「無効」は HKCU の `Run` に効かない）、判定を同じハイブの組へ直した。記録は末尾の「#8b の実装記録」。
  **実機は A〜C 群とも未確認**）
- 起点: 利用者（2026-11-19）—「agy で発見した 25 件のバグをこちらに書きます。TODO のリストを spec に作り、
  バグであるかを調査してください」。agy の報告は `fuseforks-core` / `fuseforks-host` / `apps/cli` / `apps/gui-tauri` の
  全体を対象にした 25 項目（枝を数えると 36 主張）
- 範囲: **判定まで**。コードは 1 行も変えない。修正は判定の後に、項目ごとに赤いテストを先に書いてから行う

## 判定の語彙

| 判定 | 意味 |
|---|---|
| 成立 | コードは指摘のとおりで、実運用の操作や入力で到達する |
| 一部成立 | 指摘の一部だけが当たる、または到達する条件が指摘より狭い |
| 不成立 | コードが指摘と違う、または上流で止まって到達しない |
| 設計どおり | コードは指摘のとおりだが、契約か Spec がその挙動を決めている |
| 未確定 | 読むだけでは決まらない（実測か PoC が要る） |

## TODO（調査）

- [x] #1 Windows で拡張子付きコマンド（`python.exe`）が `run` で実行できない — **成立（Windows）**
- [x] #2 エラー応答を 2,000 バイトで切った後に `error_signal` で JSON を parse している — **一部成立**（実データは最大 1,375 バイト）
- [x] #3 `parse_proto_duration` が巨大な有限値で panic する — **成立**（配布版は `panic = "abort"` でプロセスごと落ちる）
- [x] #4a `ApprovalStore::is_approved` が毒されたロックで承認を通す — **一部成立**（コメントと実装は逆。毒される経路が無い）
- [x] #4b `ApprovalStore::mutate` が書き込み失敗でメモリを戻さない・ロックの外で書く — **一部成立**
- [x] #5 ホストの設定ストア 5 箇所が `NotFound` 以外の読み取りエラーを「未作成」として扱う — **成立**（契約に反する）
- [x] #6 `env_file::plan` の `--update` で未解決のプレースホルダー行が `missing` に入らない・合鍵を作らない — **一部成立**（合鍵の側は不成立）
- [x] #7a `pricing_source::fetch_table` が https → http のリダイレクトに追従する — **成立（影響は小）**
- [x] #7b `fetch_table` と `UserAgentOAuthHttpClient::execute` が全量を読んでからサイズを検査する — **成立**
- [x] #8a Linux の AppImage で自動起動の `registeredElsewhere` が常に立つ — **成立**
- [x] #8b Windows で HKLM の `StartupApproved` に残った値のせいで有効にできない — ~~**設計どおり**（前提は未確認）~~ **成立**（2026-11-21 に実機で測って直した）
- [x] #9 `dispatch_plan_wave` / `discard_plan_wave` の TOCTOU（二重配送・打ち切り不能）— **成立**（画面からは困難）
- [x] #10 予定の配送と `ScheduleRuntime` への登録の順序 — **一部成立**（前判定つきと検収の再依頼だけ）
- [x] #11 `summarize_agents` が要約中に終わったターンの履歴を消す — **成立**（メモリの履歴だけ）
- [x] #12 `send_user_message_full` が配送の前に記録する（送信失敗で発話と添付が残る）— **一部成立**
- [x] #13 `delete_session` と `open_session` の競合 — **成立**（窓は数 ms）
- [x] #14 `PlanWaveStore` のリングの押し出しで `plan_sessions` が残る — **成立**（寿命の漏れ）
- [x] #15 まとめ呼び出しの前に打ち切りを確認していない — **成立**（Spec 10 に反する）
- [x] #16 長い ID の退避名（`transfer_to_agent_{index}` / `judge_{index}`）が既存の ID と衝突する — **一部成立**（転送・委譲の側）
- [x] #17a Responses 系 3 adapter がツールを送らないのに `tool_choice` を送る — **不成立**（防御が無いだけ）
- [x] #17b `openai_compat` の `use_tools == false` フォールバックが常に `tools[0]` を使う — **設計どおり**
- [x] #17c Gemini が空の assistant 発話を送る — **一部成立**（AI 下書きの判断役の再試行だけ）
- [x] #18 `sessionSwitched` の読み込みに世代の守りが無い（`useEditorSettings.ts` にも同じ形）— **不成立**
- [x] #19 `prepareFile` がサイズの検査の前にファイル全体を読む — **成立**
- [x] #20a `ChatPanel.vue` の `attachmentUrls` が `revokeObjectURL` を呼ばない — **成立**
- [x] #20b `ChatInput.vue` の `refPreviews` が掃除されない — **成立（影響は小）**
- [x] #20c `imageConvert.ts` の `bitmap.close()` が `try/finally` に入っていない — **一部成立**（実害はほぼ無い）
- [x] #21 `App.vue` の `onCloseRequested` で終了確認が二重に積まれる・リスナーを解除しない — **一部成立**（✕ の連打では起きない）
- [x] #22 `@` のパス補完の競合と、宛先を切り替えたときの取り違え — **成立**（Spec 24 D3 に反する）
- [x] #23 `expand_args` でパスの中の `{line}` が行番号に置き換わる — **一部成立**
- [x] #24 `markdownMath.ts` の `` $`…`$ `` の走査が `max` の外を読みうる — **不成立**
- [x] #25a `mcp_server::start` で `port = 0` のとき `allowed_origins` が合わない — **不成立**
- [x] #25b `set_startup_settings` のトレイの出し入れの TOCTOU — **不成立**
- [x] #25c `resolve_message_path` / `open_in_editor` の同期 I/O — **成立（Windows）**
- [x] #25d `ImageGenSettingsPage.vue` で保存に失敗したとき `<select>` の表示が戻らない — **不成立**
- [x] #25e `OrchestratorConfig::log_capacity == 0` で panic — **一部成立**（到達しない）

## 判定の詳細

読み手 6 本（Read / Grep だけ・ビルドなし・反証を先に試す）が実コードに当て、要の行は Neo が原文で確かめ直した。
行番号は 2026-11-19 の `727e058` のもの。

### #1 Windows で拡張子付きコマンド（`python.exe`）が `run` で実行できない — **成立（Windows）**

- `crates/fuseforks-core/src/process.rs:257-266` は Windows の拡張子の候補を `PATHEXT` だけで組み、空の拡張子を入れない。
  `resolve_in`（`:274`）は `{name}{ext}` を探すので、`python.exe` は `python.exe.EXE` としてしか探されない
- 拡張子を剥がすのは許可判定の側（`command.rs` の `normalize_command` → `strip_path_ext`）だけで、`tools/run.rs:325` は
  `policy.decide(command, …)`、`:394` は剥がしていない `resolve_program(command)`。**許可は通るのに「PATH に見つかりません」で終わる**
- 同じ原因の呼び出し元: 予定の前判定・後判定（`schedules.rs:606`）/ 外部エディタ（`editor.rs:145`。`code.cmd` と書くと失敗）/
  起動前検査（`preflight.rs:162`。下の N9）。MCP の stdio は rmcp の `which_command` なので対象外。絶対パスは `:253-254` で通る
- **設計に反する**: `specs/15_command-execution.md:234-235` と `data_contract.yaml:5954-5955`「`python` と `python.exe` を別物にしない」。
  Spec 15 の実機確認 6（`python` / `python.exe` / `Python` が同じ扱い）は未確認のまま
- ログの `run decision:` に拡張子付きの呼び出しは 0 件で、まだ踏んでいない
- 修正案: 名前が PATHEXT のどれかで終わる（大文字小文字を問わない）ときは空の拡張子を候補の先頭に入れる

### #2 エラー応答を 2,000 バイトで切った後に `error_signal` で JSON を parse している — **一部成立**

- `llm/client.rs:485-495` は `truncate_on_char_boundary(&mut body, MAX_ERROR_BODY)` の後に `error_signal(&body)` を呼ぶ。3 つの実装は
  本文全体を `serde_json::from_str` で読み、失敗すると空の `ErrorSignal` を返して status の既定の分類へ落ちる
- **今の実データでは届いていない**: `fuseforks.log` のエラー本文 31 件の最大は Gemini の 429 の 1,375 バイト。OpenAI の課金切れは 262 バイト
- ただし Gemini の 429 は `RetryInfo` が `details[]` の**最後**に来るので、2,000 バイトを超えると最初に失われるのが待ち時間（約 56 秒）。
  そのときは 200 ms / 400 ms で再送して外れる（Spec 52 以前の挙動に戻る）
- 設計: `client.rs:24-26`「ログ・UI に載せる際の最大バイト数」— 切り詰めは表示のためで、判定に使う本文を短くする意図は無い
- 指摘の「`insufficient_quota` が RateLimit に落ちる」は**症状としては実在する**が、原因は切り詰めではない（下の N8）
- 修正案: `error_signal` は切る前の本文で呼び、切り詰めは `LlmError::Api` に入れる本文にだけ掛ける

### #3 `parse_proto_duration` が巨大な有限値で panic する — **成立**

- `llm/retry.rs:242-249` は `!secs.is_finite() || secs < 0.0` だけを見て `Duration::from_secs_f64(secs)` を呼ぶ。`"1e20s"` は検査を通り、
  `from_secs_f64` は `Duration::MAX`（約 1.845e19 秒）を超えると panic する（std の仕様）
- 呼び出し元は `gemini.rs` の `error_signal` だけ。配布ビルドは `panic = "abort"` なので、**接続先の 1 応答で GUI がプロセスごと落ちる**
  （飛行中のターンが全部失われ、`turn:` 行も残らない）
- 到達は Gemini プロトコルの接続先（`baseUrl` で差し替えたプロキシや互換サーバを含む）が異常な `retryDelay` を返したときだけ。
  Google の実測値は 0〜58 秒
- 同じ形はほかに無い（`Retry-After` は `u64` の parse と `from_secs`）
- 修正案: `Duration::try_from_secs_f64(secs).ok()`

### #4a `ApprovalStore::is_approved` が毒されたロックで承認を通す — **一部成立**

- `crates/fuseforks-host/src/probe_approvals.rs:168-173`。コメントは「毒されたロックで承認を通さない。読めないなら走らせない側へ
  倒す」、実装は `Err(poisoned) => poisoned.into_inner().contains(key)`。**コメントと実装が逆**（`mutate` の `:134-137` も同じく中身を
  取り戻して続ける）
- **毒される経路が無い**: 書き込みロックの中で動くのは `insert` / `retain` / `iter().cloned().collect()` / `sort_unstable` だけで
  panic しない。配布ビルドはワークスペースの `Cargo.toml:191` の `panic = "abort"` で、panic した時点でプロセスごと終わる
- 影響は今は無い。ロックの中の処理を将来増やしたときに、毒されたロックで承認が通る形が黙って有効になる
- 修正案: `Err(_) => false`（コメントのとおり）

### #4b `ApprovalStore::mutate` が書き込み失敗でメモリを戻さない・ロックの外で書く — **一部成立**

- `probe_approvals.rs:133-151`。ロックを外してから `std::fs::write`、失敗してもメモリは編集済みのまま。どちらも事実
- **競合**: 呼び出し元の IPC 4 本（`create_schedule` / `update_schedule` / `approve_schedule_probe` / `delete_schedule`）を呼ぶ画面は
  `ScheduleDialog.vue` だけで、全操作が同じ `busy` で 1 本ずつ。画面からは届かない
- **書き込み失敗**: `retain_for` が失敗すると、メモリからは消えたのにファイルに残る。次に書けるまでに再起動すると、消した承認が
  戻る（`failures.md` #88 で塞いだ形が一時的に開く）。次に書けた時点で直る
- 修正案: ファイルを書き終えるまで書き込みロックを持ち、書けなければメモリを編集前へ戻す

### #5 ホストの設定ストア 5 箇所が `NotFound` 以外の読み取りエラーを「未作成」として扱う — **成立**

- 5 箇所とも `match std::fs::read_to_string(&path) { Ok(raw) => …parse 失敗なら blocked…, Err(_) => (既定, blocked: None) }` の形
  （`mcp_server.rs:107-135` / `jev_settings.rs:102-133` / `image_gen_settings.rs:107-133` / `pricing_source.rs:71-96` /
  `probe_approvals.rs:61-77`）。保存の側は `blocked` を見るので、`blocked: None` なら上書きが通る
- 到達: 中身が UTF-8 でない（`read_to_string` が `InvalidData`。メモ帳で UTF-16 で保存した・ディスクの破損）/ 権限が無い /
  Windows で他のプロセスが共有なしで開いている。判定は起動時の 1 回だけ
- **設計に反する**: `data_contract.yaml:3668-3670`「読めない probe_approvals.json は…既定を書き戻さない」/
  `specs/25_mcp-server.md:334-335`「mcp_server.json が読めないときは書き込みを拒む（schedules.json と同じ規律）」/
  Spec 59 D9「黙って OFF にしない」/ `failures.md` #70。コアの `config_store.rs` の `load_schedules`・`read_config`・`load_world` は
  `NotFound` を分けている
- 影響（棚ごと）: 承認 = 次に 1 件承認すると、それまでの承認が全部消える / 扉 = 次に ON にすると合鍵とポートが作り直され、
  外のクライアントの合鍵が黙って効かなくなる / Jev = 黙って OFF、保存で Account ID と閾値が消える / 画像生成 = 理由が
  `no_provider` と出て、保存で接続先ごとの枠が消える / 単価表 = URL を空にして外へ出ない設定でも既定の配信元へ戻る
- 既存のテストは JSON として壊れたファイルだけ。`pricing_source` には `blocked` のテスト自体が無い
- 修正案: 5 箇所とも `Err(e) if e.kind() == ErrorKind::NotFound => 既定`、それ以外は `blocked: Some(..)`

### #6 `env_file::plan` の `--update` で未解決の印つきコメントが `missing` に入らない — **一部成立**

- `crates/fuseforks-host/src/env_file.rs:300-312` の `Line::Placeholder` の腕で、値が `None` のとき `_ => out.push(line.to_owned())`。
  `missing` へ積むのは `render`（`:214-216`）の中だけなので、残した印つきコメントは報告に入らない
- **秘密の側は成立**: 1 回目の `--env-out` で印つきコメントになった鍵を登録しないまま `--update --env-out` すると、
  `ENV_SECRET_MISSING` が出ず CLI は「ストアに無い 0」と出す。ただしコンテナを起動すると `check` が `SECRET_MISSING` で止まる
- **扉の合鍵の側は不成立**: `render` は合鍵の値が無ければ作る（`:210-213`）ので、`bake` は合鍵の印つきコメントを書かない。
  `.env.example:17` の `# FUSEFORKS_SECRET_DOOR_TOKEN=` は印が無いので `CommentedOut` に分類される
- 設計: Spec 66 `:119`「印つきコメントは、ストアに値があれば代入の行へ置き換え、無ければそのまま」— 書き換えないのは設計どおり。
  残した印つきコメントを報告するかは明記なし
- 修正案: `_` の腕で `by_name` が `Some` かつ値が `None` なら `report.missing.push(name)`

### #7a `pricing_source::fetch_table` が https → http のリダイレクトに追従する — **成立（影響は小）**

- `pricing_source.rs:155-160` は `validate_url` の後 `Client::builder().timeout(TIMEOUT).build()` だけで、`.redirect` も
  `.https_only` も無い。reqwest 0.12.28 の既定は `Policy::limited(10)` で http への移動も許す
- 到達は配信元の側が http へ飛ばしたときだけ（https の途中に第三者は差し込めない）。$1,000/MTok 超・負・非有限の値は落とし、
  取得した値は保存するまで残らない
- 設計: `data_contract.yaml:3231`「取得は https のみ」。リダイレクトの後には触れていない
- 修正案: `.https_only(true)`

### #7b `fetch_table` と `UserAgentOAuthHttpClient::execute` が全量を読んでからサイズを見る — **成立**

- `pricing_source.rs:164` と `crates/fuseforks-core/src/mcp_oauth.rs:108-116` がどちらも `response.bytes().await` の後に `body.len()` を
  見る。`Content-Length` も見ていない
- `pricing_source.rs:32-33` のコメントは「桁外れの応答でメモリを食わないための衛生」と書く。OAuth の側は doc が「rmcp の
  `ReqwestOAuthHttpClient` の形を写した」と書くが、**写し元の rmcp 2.2.0 `transport/auth.rs:152-162` は `bytes_stream()` で 1 塊ずつ読み、
  溜める前に上限を見ている** — 写したときに守りが落ちた
- 到達: 単価表は利用者が設定した配信元（待ち 15 秒）、OAuth は MCP サーバーのメタデータが名指した認可サーバー（待ち 30 秒）。
  どちらも GUI のプロセスの中で読む
- 修正案: `chunk()` で回し、溜める前に上限を見る（`process.rs` の `read_capped` と同じ考え方）

### #8a Linux の AppImage で自動起動の `registeredElsewhere` が常に立つ — **成立**

- 登録は `APPIMAGE` のパス（`apps/gui-tauri/src-tauri/src/autostart.rs:264-271` の `app_path()`）、読み戻して比べる側は
  `current_exe_string()`（`:314`）。AppImage の `current_exe` はマウント先の `/tmp/.mount_…`（tauri-utils 2.9.3 `lib.rs:294-301`）
- `apps/gui-tauri/src/lib/startupSettings.ts:16-19` の `!registeredCommand.includes(currentExe)` は必ず真になり、注記
  「チェックを入れ直すと、今動いている Fuseforks へ書き換わります」が出続ける。入れ直しても同じ AppImage のパスを書くので消えない
- deb / rpm / macOS では当たらない。Spec 69 の実機は Windows だけ
- 影響: 誤った警告が出続けるだけで、登録と起動は正しく動く
- 修正案: 非 Windows の `read()` の `current_exe` を `app_path()` と同じ規則（`APPIMAGE` を優先・引用符なし）で組む

### #8b Windows で HKLM の `StartupApproved` に残った値のせいで有効にできない — ~~**設計どおり（前提は未確認）**~~ **成立（2026-11-21 に実機で確認）**

- `autostart.rs:121-123` は `registered && !user_disabled && !machine_disabled`、`machine_disabled` は HKLM の `Run` の有無に関係なく読む。
  `enable()` は HKCU の `Run` と `StartupApproved\Run` しか書かない。コードは指摘のとおり
- **契約とテストがこの挙動を決めている**: `data_contract.yaml:4590-4591`（両方の `StartupApproved\Run` で無効でなければ有効）/
  Spec 69 `:312-313`（`auto-launch` 0.6 の `is_enabled` と同じ判定）/ 単体 `the_windows_view_reads_both_hives_and_the_task_manager`
- 到達: HKLM に Fuseforks が書く経路は無い（Spec 69 `:320`）。HKLM の値をタスクマネージャーで無効にした後に `Run` の値だけ消えた
  ときだけ（管理者が手で消した / 管理者権限で起動した Fuseforks の `disable()` は `StartupApproved` を消さない）
- ~~**未確認**: Windows が HKLM の `StartupApproved\Run` を HKCU の `Run` にも適用するのか。修正案（`machine_disabled` は HKLM の `Run` に
  値があるときだけ数える）の正しさはここで決まるので、一次資料に当てるまで直さない~~
- **確認（2026-11-21）**: 適用しない。Microsoft の文書にこの対応の記述が無く、実機で測った（末尾の「#8b の実装記録」）。
  契約・Spec 69・単体テストが決めていた判定は `auto-launch` 0.6 の `is_enabled` の写しで、Windows の仕様の証拠ではなかった

### #9 `dispatch_plan_wave` / `discard_plan_wave` の TOCTOU — **成立（画面からは困難）**

- `crates/fuseforks-core/src/orchestrator/mod.rs:1472` で `proposal(plan_id)` を読み取りロックで確かめて外し、`mailboxes` と `world` の
  await をまたいで `:1551-1555` で `dispatch_wave` を呼ぶ。`plan.rs:282-288` は Pending でなければ `return` するだけで、戻り値は `()`。
  呼び出し側は遷移できたかを見ずに配送・`wave_runs.insert`（同じ plan_id で上書き）・`spawn` へ進む
- 筋書き: 承認 2 本 → 二重の配送・新品の天井が 2 つ・束ねが 2 回。`wave_runs` の token が上書きされ、先に終わった側が plan_id ごと
  外すので、残った実行者は「全ターン停止」で切れない。2 本目の `plan_sessions.remove` は `None` で開いている会話へ落ちる
  （Spec 75 凍結 6 に反する）。承認と破棄が重なると、破棄した計画が走る
- 到達: IPC は素通しで、守っているのは `PlanWavePane.vue` の `busy` だけ（タブを切り替えると `v-if` で初期値に戻る）
- 修正案: `dispatch_wave` / `discard_wave` が遷移できたかを返し、偽なら `PlanWaveNotPending` で戻る（判定と遷移を 1 回の書き込みロックで）

### #10 予定の配送と `ScheduleRuntime` への登録の順序 — **一部成立**

- `schedules.rs` の `deliver_scheduled`（`:386-437`）と `redeliver_for_acceptance`（`:825-848`）は、`deliver`（`try_send`）→
  `shared.record().await` → `in_flight` / `pending_acceptances` / `pending_summaries` へ insert の順
- **前判定なしの経路は守られている**: `deliver_batch` はティッカーの `select!` の `tick()` の腕で await されており、その間
  `events.recv()` は poll されないので、`AgentTyping { active: false }` は登録の後に処理される
- **前判定つき（`probe_and_deliver` は spawn）と検収の再依頼（`acceptance_step` は spawn）は並行に走る**ので、ターンが `record`
  （fsync を含む）と lock 2 つより速く終わると逆転する。早道はプロンプトの組み立ての失敗と、再依頼で受け継いだ財布の予約不足だけで、
  頻度は低い
- 起きた場合: `in_flight` が残り、その個体宛ての予定が次のターンまで黙って飛ばされる。残った `pending_*` は無関係な後続のターンで
  後判定や要約を発火させ、それまで `SessionRef` を持つので会話を削除できない
- 修正案: 配送の前に登録し、配送が `Err` なら取り消す

### #13 `delete_session` と `open_session` の競合 — **成立（窓は数 ms）**

- `orchestrator/sessions.rs:145` の `was_open` を `switch_gate` を取らずに判定し、`:156-165` の `when_unreferenced` でも `switch_gate` を
  取らない。開く側（`mod.rs:1143-1163`）は `switch_gate.write()` の下で読み込み → lock 3 つ → `set_session`。開く操作は `SessionRef` を
  発行しないので、削除側の参照の数は 0 のまま
- 筋書き: 開く側が会話 X を読み終える → 削除側が X を消す → 開く側が `set_session(X)`。開いている会話が削除済みの X を指す
- **規律に反する**: `mod.rs:858-859`「`switch_gate` の read を持って呼ぶ（持たずに呼ぶと、判定と書き込みの間に切り替わりうる）」
- 影響: 保存は毎回 `SessionNotFound` の WARN で捨てられ、再起動で発話が失われる（Spec 75 `:98` の網が拾う）。別の会話を開けば戻る
- 到達: 会話一覧のダイアログは同じ `busy` で直列。知らせの「開く」（`openNoticedSession`）はその外
- 修正案: `delete_session` が判定から削除まで `switch_gate` を握る

### #14 `PlanWaveStore` のリングの押し出しで `plan_sessions` が残る — **成立（寿命の漏れ）**

- `plan.rs:186-190`（`begin_wave`）と `:228-230`（`begin_pending_wave`）は、状態を問わず古い方から `pop_front`。Pending の提案も捨てる
- `plan_sessions` を入れるのは `delegation.rs:398` の 1 か所、外すのは `mod.rs:1610` と `:1644` の 2 か所だけで、どちらも
  `proposal(plan_id)` が `Some` の後ろにある。押し出された提案は `PlanWaveNotPending` で戻るので、`SessionRef` は再起動まで残る。
  個体の停止・会話の切り替え・新規チャットで外す経路は無い
- 到達: 計画の確認を ON にした進行役の提案を放置し、その後に波が 50 件積まれる（5 分ごとに自動承認の予定が走る村なら数時間）。
  画面も同じ規則で切るので、提案は一覧から**何の知らせもなく**消える
- 影響: 提示した会話が `SESSION_BUSY` でプロセスの寿命のあいだ削除できない
- 設計: `plan.rs:187-188` の「状態を問わず」は Spec 08 の時代の判断で、Pending（Spec 43）を想定していない
- 修正案: 押し出した波の plan_id を `begin_*` が返し、呼び出し側が `plan_sessions` から外して破棄の行を残す（または Pending を押し出さない）

### #11 `summarize_agents` が要約中に終わったターンの履歴を消す — **成立（消えるのはメモリの履歴だけ）**

- `orchestrator/mod.rs:888` で境界 `covers_up_to_seq` を取り、`:901-942` で履歴の写しを取って gate を外し、`:991` でロックを持たずに
  LLM を待ち、`:1057-1058` で無条件に `record.history.clear()`。その間に終わったターンが `push_exchange` で積んだ往復は、写しにも
  要約にも入らずに消える
- 到達: 要約ボタンが止めるのはボタン自身だけ（`ChatPanel.vue` の `:disabled="… || summarizing"`）で、入力欄は止まらない。要約の最中に
  その個体へ送る・別の個体の `ask` や予定がその個体で答え終わる、で踏む。個体は順に要約するので、後の個体ほど窓が長い
- 設計: `sessions.rs:209-212`（`data_contract.yaml:508-509`・Spec 12 `:362-364` に同文）「飛行中のターンがあっても拒まない…畳んだ履歴の
  上に 1 往復が乗るだけ」— ターンが `clear` の**後**に終わる場合だけを書いており、LLM を待つ間に終わる場合を書いていない
- 影響: その個体は直前のやり取りを忘れる。保存先には残り、`restore_histories` は `seq >= covers` を残すので、**開き直すか再起動すると
  戻る = 実行中と再開後で履歴が違う**（`push_exchange` の doc `:1210-1211` が避けるべきと書いている形）
- 修正案: 写しを取ったときの往復の数を覚え、`clear()` の代わりにその分だけ先頭から落とす

### #12 `send_user_message_full` が配送の前に記録する — **一部成立**

- `orchestrator/runtime.rs:473-481` は `self.shared.record(message.clone(), &session).await` の後に `deliver(…)`。配送の失敗は
  受信箱が無い・閉じた（`NotRunning`）と満杯（`MailboxFull`）の 2 つで、この関数は受信箱を先に見ていない（同じファイルの
  `ask_external` は見ている）
- **リポジトリ自身の規則に反する**: `settings.rs:668-669`「配送してから記録する（逆にすると、受信箱が飽和していたときに『配られていない
  発話』が会話ペインへ残る）」。同じ規則が `schedules.rs:324-325`・Spec 07 `:435`・Spec 56 `:335` にもある
- 到達は狭い: 画面は宛先が `running` のときしか送らせない。`NotRunning` は停止と送信の競合、`MailboxFull` は受信箱に 64 通積もったとき
- 影響: 届いていない発話が会話ペイン・`sessions.redb`・リングに残る。`ChatInput.vue` は失敗で文面・添付・参照を入力欄へ戻すので、
  送り直すと発話が 2 通になり、添付も別の ID でもう一度保存される
- 既存のテスト（`tests/orchestrator.rs` の停止中・満杯の 2 本）はエラーコードだけを見て、何も記録されないことは見ていない
- 修正案: `try_reserve` で受信箱の枠を先に取り、取れなければ何も書かずに返す

### #15 まとめ呼び出しの前に打ち切りを確認していない — **成立**

- 打ち切りを見るのは周の頭（`turn.rs:1736`）と承認待ちの腕（`:2078-2092`）だけ。繰り返しの打ち切り（`:2116-2125`）と上限到達
  （`:2128-2134`）は検査せずにループを抜け、まとめの分岐（`:2152-`）は予算の予約だけを見る。`chat_cancellable` の token が切るのは
  再試行の待ちだけで、最初の 1 回は送られる
- 到達: 最終周か、全呼び出しが RepeatGuard で止まった周の最中（LLM 呼び出し・ツール実行・長い委譲の待ち）に「■ 停止」/
  「全ターン停止」/ 依頼元の打ち切りの連鎖が来る
- **設計に反する**: `specs/10_turn-interrupt.md:172`「まとめの LLM 呼び出しはしない（打ち切りの直後にもう 1 回課金しない）」/
  `data_contract.yaml:4253-4255` の優先順位（cancel > budget_exhausted > approval_pending > RepeatGuard > 上限）
- 影響: 止めた後にまとめの 1 回が送られて課金され、ターンは打ち切りではなく `Repeat` / `ToolLimit` で閉じる（打ち切りの System 行も
  `TurnInterrupted` も出ない）
- 修正案: まとめの分岐の先頭（予算の予約より前）で `turn.token.is_cancelled()` を見て `finish_interrupted` へ

### #16 長い ID の退避名が既存の ID と衝突する — **一部成立**

- `orchestrator/delegation.rs:1319-1327` は自然な名前が 64 文字を超えるか既に使われていれば `transfer_to_agent_{index}` へ退避するが、
  **退避名が既に使われているかは見ない**。`index` は `enumerate` の 0 始まり
- 到達（転送・委譲）: 接続先 `[agent_2, agent_3, 53 文字以上の ID]` → 3 つ目が `transfer_to_agent_2` になり 1 つ目と重なる。ID の上限は
  64 バイト（`model.rs:10`）で、画面の `deriveId` は英語の名前を長さを切らずに ID にするので、画面から作れる。`ask_*` も同じ名前を
  引き継ぐ。逆引きは先頭一致の `find` なので、**長い ID の相手を呼ぶつもりの呼び出しが `agent_2` へ配送される**
- 判断役（`judging.rs:107-112` の `judge_{index}`）は数字だけの ID が要るので現実的ではない
- 設計: `delegation.rs:1305-1307`「切り詰めで衝突する場合は連番へ退避する…『たぶん大丈夫』で通さない」— 意図は一意で、実装が満たしていない
- 修正案: 退避名も既存の名前と照合し、空くまで接尾辞を足す

### #17a Responses 系 3 adapter がツールを送らないのに `tool_choice` を送る — **不成立（防御が無いだけ）**

- `openai_responses.rs:48-70`・`xai_responses.rs`・`perplexity_responses.rs` は `tool_choice` を `offer_tools` と無関係に組む。コードは指摘のとおり
- `Required` を作る呼び出しは 0 件。`Specific` は `orchestrator/assist.rs:126` だけで、同じ関数が `use_tools` の偽を断り、`tools` は
  常に `submit_draft` の 1 本。ほかは `Auto` か `None`
- 修正案（任意）: Anthropic（`anthropic.rs:129`）と同じく `tools` が空なら `tool_choice` を送らない

### #17b `openai_compat` の `use_tools == false` フォールバックが常に `tools[0]` を使う — **不成立（設計どおり）**

- `openai_compat.rs:27-61` の分岐は `tool_choice` を見ずに `tools[0]` のスキーマで指示文を足す。ただし「`tools` が空でない」かつ
  「バックエンドの `use_tools` が偽」を作る呼び出し元が無い（`turn.rs:1501` は `use_tools` が偽なら `tools` を空にする / `assist` は断る）
- 設計: `:21-23`「構造化出力を得るためのフォールバック経路」、テスト `encode_falls_back_to_prompt_instruction_without_tool_support` が
  意図して留めている（Kataribe 由来）

### #17c Gemini が空の assistant 発話を送る — **一部成立**

- `gemini.rs:352-376` は本文も呼び出しも無い assistant を `{"role":"model","parts":[]}` として送る。Anthropic（`:69-80`）と
  Responses 系（`responses_input.rs:79-80`）は落とす
- 村のターンは守られている（`world.rs` の `exchange_pair` が空を「（発言なし）」にする / 要約は「承知しました。」）
- **抜けているのは AI 下書きの判断役の再試行だけ**: `assist.rs:159-179` で 1 回目が検証に落ち、2 回目が空の応答（Gemini の止められた
  応答は `Ok` で `text: None`）だと、`assistant_tool_calls("", [])` を `appended` に入れて返す。`AssistPanel.vue` はそれを履歴に積んで
  次の要求で送り返し、`check_history` は空の assistant を拒まない
- 未確認: Gemini が空の `parts` を 400 で拒むか（撃っていない）
- 修正案: Gemini の encode で空の assistant を落とし、`assist` も空の assistant を `appended` に入れない

### #18 `sessionSwitched` の読み込みに世代の守りが無い — **不成立（守りが無いのは事実）**

- `apps/gui-tauri/src/composables/useOrchestrator.ts:763-773` は `await ipc.listMessages(…)` の後に、いま開いている会話の ID と照らしていない
- ただし `listMessages` は会話の ID を取らず、コアの `message_log`（`orchestrator/mod.rs:1424-1429`）は**呼ばれた時点で開いている会話**の
  リングを返す。`open_session` はリングを差し替えてから `SessionSwitched` を出すので、A の呼び出しが遅れれば読むのは B のリング。
  A のリングで上書きされるのは「A の読み取りが B への切り替えより前に済み、その応答が B の応答より後に届く」ときだけ
- 画面から 2 回続けて切り替える経路は、`SessionDialog.vue` の `busy` と「開けたら閉じる」で人の操作より速くならない。
  `useEditorSettings.ts` も同じ形で、読み込みは起動時と設定ページを開いたとき、保存は人が押したとき
- 修正案（任意）: 待った後に `state.currentSessionId !== event.sessionId` なら捨てる

### #19 `prepareFile` がサイズの検査の前にファイル全体を読む — **成立**

- `apps/gui-tauri/src/lib/attachment.ts:234` の `await file.arrayBuffer()` が先で、無変換の種別の上限は `:240`、画像の元ファイルの上限は
  `convertImageFile` の `:261`。呼び出し元の `ChatInput.vue` の `attach` は件数で切るだけで、サイズは見ない
- 種別の判定 `detectKind` が読むのは先頭 12 バイトまで。全体が要るのは画像の変換だけ
- 設計: `attachment.ts:9-11`「元ファイル 10MB — デコードの前に見る」。デコードの前には見ているが、読み込みの前には見ていない
- 影響: 数百 MB の動画を落とすと、その大きさのメモリを確保してから「大きすぎます」になる。`arrayBuffer()` 自体が失敗すると
  `AttachmentError` ではないので「変換に失敗」と誤った理由が出る
- 修正案: 先に `file.size` を種別の上限の最大（12MB）と元画像の上限（10MB）の大きい方と比べ、種別は `file.slice(0, 16)` で判定する

### #20a `ChatPanel.vue` の `attachmentUrls` が `revokeObjectURL` を呼ばない — **成立**

- `components/ChatPanel.vue:913-931` は添付 ID ごとに 1 回 `URL.createObjectURL` を作ってキャッシュし、解放しない。`revokeObjectURL` は
  リポジトリ全体で `useOrchestrator.ts` のアイコンの 7 か所だけ
- `ChatPanel` は `App.vue` の `v-show` の中でアプリの寿命まで unmount されず、キャッシュは会話の切り替えでも消えない
- 設計: Spec 23 `:345-347`「アイコンと同じ `Vec<u8>` + object URL の形」— そのアイコンの側は `useOrchestrator.ts:544-545` で
  「放置すると Blob が溜まり続ける」と書いて解放している。添付だけ同じ扱いになっていない
- 量を大きくしている要因: 音声・動画・PDF でもファイル名を出すために `attachmentUrl(att)` を呼び、最大 12MB を読む（下の N12）
- 修正案: `conversationCleared` で（拡大表示中の URL を避けて）まとめて解放する

### #20b `ChatInput.vue` の `refPreviews` が掃除されない — **成立（影響は小）**

- 入れるのは `ChatInput.vue:735-741` の 1 か所だけで、`removeAttachment`・送信・会話の切り替えのどれも消さない
- 鍵は `ref:<添付ID>` なので、増えるのは渡し直した重複しない画像の数 × 最大約 2.7MB の base64
- 修正案: 削除の後と送信の成功の後に、`attachments` に残っている鍵だけへ絞る（送信の前に消すと `restoreDraft` で戻したチップの
  サムネイルが消える）

### #20c `imageConvert.ts` の `bitmap.close()` が `try/finally` に入っていない — **一部成立（実害はほぼ無い）**

- `workers/imageConvert.ts:29-39` で `close()` の前に例外が出ると解放されない。指摘の前提と違い、worker は使い回す
  （`attachment.ts:156-158` の `ensureWorker`）
- 例外が出るのは `OffscreenCanvas` の 2D を持たない WebView だけで、`ImageBitmap` は GC で回収される
- 修正案（任意）: `try { … } finally { bitmap.close(); }`

### #21 `App.vue` の `onCloseRequested` で終了確認が二重に積まれる — **一部成立**

- `App.vue:213-230` の `onCloseRequested` には、`onTrayQuit`（`:240-255`）にある `exitConfirmInFlight` の守りが無く、戻り値の解除関数も捨てる。
  `useConfirm.ts` は同じ確認を畳まずに積む
- **✕ の連打では起きない**: 確認は `ConfirmHost.vue` の `fixed inset-0` の覆いで窓全体を塞ぎ、タイトルバーの ✕ は覆いの
  `@click.self` に当たって取り消しになる
- 起きるのは、確認が出ている間に OS 側から閉じる要求が届いたとき（Alt+F4・タスクバーの「ウィンドウを閉じる」）。1 つ目を
  キャンセルしても同じ確認がもう一度出る
- 設計: `App.vue:236-238`「終了の確認を出している間は 2 回目を積まない」— トレイの経路にしか適用されていない
- 解除しないことの実害は無い（`App.vue` は根で unmount されない。開発時の HMR で二重登録になりうるだけ）
- 修正案: `exitConfirmInFlight` を `confirmExit` を包む 1 つの関数へ移し、両方の経路で共有する
- 未確認: `decorations: false` の窓で Alt+F4 が `CloseRequested` として届くか

### #22 `@` のパス補完の競合と、宛先を切り替えたときの取り違え — **成立（条件は狭い）**

- `ChatInput.vue:399-431` の watch は補完の種別だけを見て、`props.agentId` を見ない。`listWorkDirFiles(props.agentId)` が返った後に、
  いまの宛先とトリガーを確かめ直さない
- 到達: `@` の補完を開いたまま `Alt+↑↓` で、稼働中で作業フォルダを持つ別のサーヴァントへ移る。フォーカスが入力欄に残るので補完は
  閉じず、前のサーヴァントの一覧が残る。入力欄の `@keydown.up`（`.exact` なし）が候補の選択も同時に動かす（下の N14）。
  マウスで宛先を変えると `@blur` で閉じるので安全
- **設計に反する**: `specs/24_path-completion.md:121`「宛先を切り替えたら候補も切り替わる」
- 影響: 前のサーヴァントの作業フォルダからの相対パスが本文に入る。作業フォルダを共有する個体どうし（利用者の村は 10 体中 9 体）なら害は無い
- 修正案: `watch(() => props.agentId, closeCompletion)` を足し、結果を入れる前に宛先とトリガーを確かめる

### #23 `expand_args` でパスの中の `{line}` が行番号に置き換わる — **一部成立**

- `apps/gui-tauri/src-tauri/src/editor.rs:125` の `arg.replace(PATH_PLACEHOLDER, path).replace(LINE_PLACEHOLDER, &line)`。コードは指摘のとおり
- 到達は指摘より狭い: 発話の文字列は `messagePath.ts:32` の `NOT_IN_PATH` が `{` `}` を除くので、届くのは作業フォルダかリンク先の
  フォルダ名に `{line}` を含むときだけ。さらにひな形に `{path}` が要る
- 影響: エディタが存在しないパスで起動する（VS Code の `-g` なら新しいファイルとして開く）
- 修正案: ひな形を 1 回だけ走査して置き換える。テストに `{line}` を含むパスを 1 本

### #24 `markdownMath.ts` の `` $`…`$ `` の走査が `max` の外を読みうる — **不成立**

- `lib/markdownMath.ts:130-142` は `after === max` のとき `src[max]` を読む。コードは指摘のとおり
- markdown-it 14.3.0 で `posMax < src.length` になるのはリンクのラベルの中だけ（`link.mjs:119-120` / `image.mjs:136`）で、そのとき
  `src[posMax]` は常に `]`。表のセルは `rules_core/inline.mjs` がセルごとに別の `src` で `inline.parse` を呼ぶので `posMax === src.length`
- 修正案（任意）: `after < max` を条件に足す（markdown-it の版上げへの備え）

### #25a `mcp_server::start` で `port = 0` のとき `allowed_origins` が合わない — **不成立**

- `mcp_server.rs:376-379` が bind の前に引数の `port` で Origin を組むのは事実
- 入口はどれも 0 を弾く: 画面 `SettingsDialog.vue:524-526`（1024〜65535）/ CLI `apps/cli/src/args.rs:363-369`（`port != 0`）。
  届くのは手で書いた `mcp_server.json` の `"port": 0` だけで、そのとき画面の `McpHostStatus.port` も 0 を出すので繋ぐ先がそもそも分からない
- 修正案（任意）: `set_mcp_host` と `load` でも 0（と 1024 未満）を拒む

### #25b `set_startup_settings` のトレイの出し入れの TOCTOU — **不成立**

- `commands.rs:892-911` で保存とトレイの出し入れは 1 つの操作になっていないが、呼び出し元は `SettingsDialog.vue` の `applyStartup` だけで、
  `startupBusy` で弾き、チェックボックスも `:disabled="startupBusy"`。1 本目が返るまで 2 本目は送られない
- ✕ で隠すかは `App.vue:217-221` の `hidesOnClose` が設定の値で決め、トレイの有無は見ない

### #25c `resolve_message_path` / `open_in_editor` の同期 I/O — **成立（Windows）**

- どちらも同期の `pub fn`（`commands.rs:1925` / `:1979`）。中で `std::fs::canonicalize` / `is_dir` / `exists` / `resolve_program`
  （PATH を探す）/ `spawn` を行う
- tauri-macros 2.6.3 は `async` でないコマンドを `ExecutionContext::Blocking` にし、受け取った場所で呼ぶ。その場所は wry 0.55.1 の
  WebView2 の `WebResourceRequested` のイベントの中（同期）なので、**Windows では UI スレッドで走る**
- 到達: `parsePathRef` は UNC（`\\server\share\…`）を受けるので、応答しない共有・切断したネットワークドライブで SMB のタイムアウトまで
  窓が「応答なし」になる。`usePathMenu.ts:21` は解決中の状態を持っており、非同期の前提で書かれている
- 修正案: `#[tauri::command(async)]`（`pub fn` のままなので `pathMenuWiring.test.ts` の字面の検査も通る）
- 未確認: macOS / Linux で同じスレッドか

### #25d `ImageGenSettingsPage.vue` で保存に失敗したとき `<select>` の表示が戻らない — **不成立**

- `components/ImageGenSettingsPage.vue:292-296` は `:value` + `@change`。`save` は失敗しても `view` を変えないが、`busy` の出し入れで
  再描画が起き、Vue は `value` を毎回当て直す（`runtime-core` の `key === "value"` の分岐 → `runtime-dom` が DOM の `el.value` と比べて戻す）。
  DOM の選択は元の値へ戻る

### #25e `OrchestratorConfig::log_capacity == 0` で panic — **一部成立（到達しない）**

- `orchestrator/mod.rs:774-780` は **push の前に** `drain(..log.len() + 1 - capacity)` を呼ぶので、容量 0 だと終端が長さを超えて panic する
- `OrchestratorConfig` は設定ファイルから読めず、組み立てる箇所は全部 `..OrchestratorConfig::default()`（5,000）。画面にも欄が無い。
  踏めるのは公開 API で `log_capacity: 0` を渡す Rust の呼び手だけ
- 修正案（任意）: 組み立て時に `max(1)`

## 指摘に無い問題（検証の途中で見つかったもの）

（N1〜N7 は起票の時点で末尾の「直さないと決めるもの」の後ろに置いていた。2026-11-20 にこの節へ移した — 中身は変えていない）

- **N1 実行中に「閉じるとトレイに隠れる」を ON にしてトレイの作成に失敗すると、✕ で隠れて戻せない** — `set_startup_settings` は
  `tray::install` の失敗を `note!` で握って設定を ON のまま返す。`hidesOnClose` は設定の値しか見ない。起動時は `after_boot` の
  `hidden && !tray_ready`（`lib.rs:328`）で防いでいるが、実行中の切り替えでは防いでいない（Spec 69 D5 が起動時にしか効いていない）
- **N2 単価表の取得が `blocked` を見ない** — `fetch_model_prices`（`commands.rs:1684-1694`）は `pricing.json` が壊れているとき既定の URL を
  取りに行く。URL を空にして外へ出ない設定の人でも、壊れた後に「取得」を押すと開発者の配信元へ出る（#5 と組で直す）
- **N3 `mcp_server.json` 以外の 4 つの棚は保存が一時ファイル + rename でない** — 承認・Jev・画像生成・単価表は素の `std::fs::write`。
  書く途中で落ちると壊れた JSON が残り、承認の台帳なら人が直すまで前判定と後判定が全部止まる
- **N4 `.env.example` を写してから `--update` すると、どの秘密も埋まらない** — 秘密の行は印の無い `# NAME=` なので `CommentedOut` に
  分類され、`ENV_COMMENTED_OUT` が出るだけになる
- **N5 #7b と同じ形（上限なしの全量読み）** — `llm/client.rs:485, 517`、`jev.rs:398, 401`、`image_gen.rs:870` の `response.text()`。
  相手は利用者が設定した API
- **N6 `schedules.rs:484` の `discard_empty_session` も `switch_gate` を取らずに削除する**（#13 と同じ形。空の会話なので害は小さい）
- **N7 Linux の `app_path()` は `APPIMAGE` を検査せずに使う** — 環境変数 1 つで自動起動の登録先を任意のファイルに向けられる。
  tauri-utils はマウント先にいるかを確かめている
- **N8 OpenAI の課金切れの 429 が `rate_limit` に分類され、2 回再試行している（実測）** — 実物の本文は `"type": "insufficient_quota"` と
  `"code": "credit_balance_exhausted"` を両方持ち、`openai_compat::error_signal`（`:457-461`）は `code` を優先するので、表に無い
  `credit_balance_exhausted` が分類へ渡り、`classify` は status の既定の `RateLimit` を返す。`fuseforks.log` 25278-25279 / 25303-25304 行
  （2026-09-24・`gpt-6-sol`）に `class=rate_limit code=credit_balance_exhausted` の `llm retry:` が 2 回ずつ出ている。Spec 52 の
  「`insufficient_quota` は 429 でも止める」がこの本文では効いていない。修正案: `code` と `type` の両方を表と突き合わせる
- **N9 起動前検査が Windows で誤った警告を出す** — `preflight.rs:162` の `command_on_path` が `resolve_program` を使うので、`mcp.json` の
  `npx.cmd` や `run.json` の `python.exe *` が `MCP_COMMAND_NOT_FOUND` / `RUN_COMMAND_NOT_FOUND` になる（#1 と同じ原因で、#1 を直せば消える）
- **N10 `deliver_and_wait` にも #12 と同じ形がある** — `delegation.rs:197` が委譲の発話を記録してから `:217` で配送する。サーヴァント同士の
  発話は広場ログに出るので、配られなかった依頼が他の個体のプロンプトに載る。同じ関数の輪の拒否（`:166-169`）は「記録すると広場ログが
  起きなかった配送を語る」と書いて記録を避けている
- **N11 要約の境界と写しの取り方が別のタイミング（#11 の隣）** — `covers_up_to_seq` は gate の外（`mod.rs:888`）、写しは gate の中
  （`:901`）。その間に積まれた往復は要約の本文に入り、`seq > covers` なので再起動後には生の往復としても戻る（二重になる）。窓は短い
- **N12 会話ペインが音声・動画・PDF でも添付の本体を読む** — `ChatPanel.vue:1586` / `:1595` / `:1605` の条件がファイル名を出すだけの種別でも
  `attachmentUrl(att)` を呼び、最大 12MB を `readAttachment`（`number[]` の JSON）で読んで object URL を持ち続ける（#20a の量を大きくしている）
- **N13 `sessionSwitched` の読み直しが、取得中に届いた発話を上書きしうる** — `useOrchestrator.ts:771` / `:973` の
  `state.messages = await ipc.listMessages(…)` は、読み取りの後に記録されて `messageSent` で先に積まれた発話を消しうる（未確認）
- **N14 補完を開いたまま `Alt+↑↓` を押すと、候補の選択とサーヴァントの選択が同時に動く** — 入力欄の `@keydown.up` / `.down` に `.exact` が
  無く、`App.vue` の `onNavKey` は `defaultPrevented` を見ない（#22 と組で直す）

## TODO（修正）

判定が成立・一部成立のものだけ。**どれも赤いテストを先に書いてから直す**（PoC が無い修正は推測）。順は「実害が出ているか・1 回で
アプリが落ちるかデータが消えるか」で並べた。並びは推しで、着手する範囲は利用者の裁定を待つ。

### A. 実害が出ている、または 1 回でアプリが落ちる・設定が消える

- [x] **N8** 課金切れの 429 を `code` と `type` の両方で表と突き合わせる（実測で 2 回ずつ再試行している）— `llm/openai_compat.rs` /
  `llm/retry.rs`。赤 = 実物の本文（`type: insufficient_quota` + `code: credit_balance_exhausted`）で `Billing` にならない単体
- [x] **#3** `parse_proto_duration` を `try_from_secs_f64` へ — `llm/retry.rs`。赤 = `"1e20s"` で panic する単体
- [x] **#5 + N2 + N3** ホストの棚 5 つで `NotFound` だけを「未作成」にする / 単価表の取得は `blocked` を見る / 4 つの棚の保存を一時ファイル
  + rename へ — `fuseforks-host` の 5 ファイル。赤 = UTF-8 でない中身のファイルを置いて `blocked` が立たない結合（棚ごと）
- [x] **#15** まとめ呼び出しの前で打ち切りを見る — `orchestrator/turn.rs`。赤 = 最終周の最中に打ち切って、まとめの呼び出しが走る結合
- [x] **#11 + N11** 要約の後は写しの分だけ履歴を落とす / 境界と写しを同じ lock の下で取る — `orchestrator/mod.rs`。赤 = 要約の LLM を
  止めている間にターンを 1 本終わらせ、その往復が消える結合（N11 は赤を取れていない — 下の実装記録）
- [x] **#14** 押し出した Pending の波の `plan_sessions` を外し、破棄の行を残す — `plan.rs` / `orchestrator/delegation.rs`。赤 = 提案の後に
  波を 50 件積むと会話が `SessionBusy` で消せない結合
- [x] **#1 + N9** Windows の `resolve_program` で拡張子付きの名前は空の拡張子を先に試す — `process.rs`。赤 = `resolve_in` に `tool.exe` を
  渡して見つからない単体（拡張子の列を引数で渡すので OS を問わず回る）

### B. 競合・条件の狭いもの

- [x] **#9** `dispatch_wave` / `discard_wave` が遷移できたかを返し、偽なら `PlanWaveNotPending` — `plan.rs` / `orchestrator/mod.rs`
- [x] **#13 + N6** `delete_session` と `discard_empty_session` が判定から削除まで `switch_gate` を握る — `orchestrator/sessions.rs` / `schedules.rs`
- [x] **#10** 予定の配送の前に `in_flight` / `pending_*` へ登録し、配送が失敗したら取り消す — `orchestrator/schedules.rs`
- [x] **#12 + N10** 利用者の発話と委譲の発話を、受信箱の枠を先に取ってから記録する — `orchestrator/runtime.rs` / `delegation.rs`
- [x] **#16** 退避名も既存の名前と照合する — `orchestrator/delegation.rs` / `judging.rs`
- [x] **#2** `error_signal` は切る前の本文で呼ぶ — `llm/client.rs`
- [x] **#7a + #7b + N5** 単価表は `https_only(true)` / 単価表と OAuth は塊ごとに上限を見て読む（LLM・Jev・画像生成の `text()` も同じ形）
- [x] **#25c** `resolve_message_path` / `open_in_editor` を `#[tauri::command(async)]` へ — `commands.rs`
- [x] **#19** `prepareFile` はサイズと先頭のバイトを見てから全体を読む — `lib/attachment.ts`
- [x] **#20a + N12** 会話を切り替えたら添付の object URL を解放する / 画像以外は本体を読まない — `ChatPanel.vue`
- [x] **#22 + N14** 宛先が変わったら補完を閉じ、結果を入れる前に宛先を確かめる / 補完の `↑↓` に `.exact` — `ChatInput.vue`
- [x] **#21** 終了確認の守りを ✕ と OS の閉じる要求とトレイで共有する — `App.vue`
- [x] **#8a** 非 Windows の自動起動の読みで `current_exe` を `APPIMAGE` で組む — `autostart.rs`
- [x] **N1** 実行中に「閉じるとトレイに隠れる」を ON にしてトレイの作成に失敗したら、設定を OFF に戻すか ✕ で隠さない — `commands.rs`
- [x] **#6** `--update` で残した印つきコメントも `missing` に数える — `env_file.rs`
- [x] **#4a + #4b** 毒されたロックは承認しない / 書き終えるまでロックを持ち、失敗したらメモリを戻す — `probe_approvals.rs`
- [x] **#17c** Gemini の encode で空の assistant を落とす / `assist` は空の assistant を `appended` に入れない

### C. 任意（防御だけ・害が小さい）

- [x] **#23** `expand_args` を 1 回の走査で置き換える
- [x] **#20b** `refPreviews` を `attachments` に残っている鍵へ絞る
- [x] **#20c** `bitmap.close()` を `finally` へ
- [x] **#17a** Responses 系 3 adapter で `tools` が空なら `tool_choice` を送らない
- [x] **#25a** `set_mcp_host` と `load` でポート 0 と 1024 未満を拒む
- [x] **#25e** `log_capacity` を組み立て時に `max(1)`（受信箱とイベントの容量も同じ形なので一緒に）
- [x] **#18 + N13** `sessionSwitched` の読み直しを、いま開いている会話の ID で照らす
- [x] **N4** `.env.example` の秘密の行に `# bake:` の印を付ける（扉の合鍵の空きも埋まるようにした）
- [x] **N7** `APPIMAGE` がマウント先にいるかを確かめてから使う

### A 群の実装記録（2026-11-19）

7 件とも**赤いテストを先に書いて、狙った主張で赤になることを確かめてから**直した。

| 項目 | 赤のテスト | 赤の出方 | 処方 |
|---|---|---|---|
| N8 | `openai_compat.rs` の単体 `billing_is_found_in_type_when_the_code_is_not_in_the_table`（本文は 2026-09-24 のログの実物） | 分類が `Billing` にならない | `error_signal` は `code` と `type` のうち分類の表にある語を先に渡す（`retry::is_classified_code`）。`failures.md` #158 |
| #3 | `retry.rs` の単体 `proto_duration_beyond_duration_max_is_none_not_a_panic` | std の `time.rs` で panic | `Duration::try_from_secs_f64(secs).ok()` |
| #5 + N2 + N3 | 結合 `crates/fuseforks-host/tests/settings_store_unreadable.rs`（棚 5 つ + 単価表の取得の URL + 対照）/ 走査 `settings_file.rs` の単体 | 5 つとも `blocked` が立たない・保存が通る | 新しいモジュール `settings_file`（`load` = NotFound だけを未作成 / `write_atomic` = 一時ファイル + rename + 権限 / `restrict_permissions`）へ 5 つの棚を寄せた。`PricingSourceStore::fetch_url` は読めないあいだ URL を渡さず、GUI の `fetch_model_prices` はそれを呼ぶ |
| #15 | 結合 `tests/interrupt_before_summary.rs` | まとめの呼び出しが走って 2 回 | まとめの分岐の先頭（予算の予約より前）で `turn.token.is_cancelled()` を見て `finish_interrupted` |
| #11 + N11 | 結合 `session_persistence.rs` の `an_exchange_that_lands_while_the_summary_is_pending_is_kept` / 単体 `world.rs` の `appended_since_counts_only_what_landed_after_the_snapshot` | 待つ間に終わった往復が消える | 畳むときは写しの後に末尾へ積まれた分を残す（`world::appended_since`）/ 境界と写しを `world` の読み取りロックの下で一緒に取る |
| #14 | 結合 `plan_edit_window.rs` の `an_evicted_pending_plan_releases_its_conversation` / 単体 `plan.rs` の `an_evicted_proposal_is_handed_back_once` | `SessionBusy { refs: 1 }`・破棄の行が 50（押し出しの 1 行が無い） | `PlanWaveStore::make_room` が押し出した提案を控え、`take_evicted_proposals` で取り出した呼び出し側が `release_evicted_proposals` で控えを外し、破棄の行を残す（計器 `plan evicted:`）。押し出しの規則は変えない |
| #1 + N9 | `process.rs` の単体 `a_name_that_already_carries_an_extension_is_found` | `None` | `resolve_in` は名前が既に拡張子の候補で終わっていれば、そのままの名前を先に試す（大文字小文字を問わない） |

- **N3 の走査テストは書いた後に一発で緑だった**ので、`jev_settings.rs` の保存を `std::fs::write` へ戻す変異を入れて赤を確かめ、戻した
- **N11 は赤のテストを作れていない。** 境界を取ってから写しを取るまでの間に往復が積まれる窓は、await をまたぐが外から決定的に
  割り込めない。処方は構造（同じ `world` の読み取りロックの下で取る — 往復は `push_exchange` が `world` の書き込みロックの下で積む）で、
  テストは留めていない
- **#15 は要約の分岐の先頭だけを塞いだ。** まとめの呼び出しの最中に打ち切られて `Err` が返った腕（再試行の待ちを切られた形）は、
  今もターンを上限到達として閉じる。到達する条件（まとめの呼び出しが再試行に入り、その待ちの間に打ち切られる）が狭いので残した
- **#5 で権限の扱いが 1 つ変わった** — Unix では、それまで権限を絞っていなかった `jev.json` / `image_gen.json` / `pricing.json` も
  0600 で書く（`write_atomic` が一時ファイルに掛ける）。Windows は従来どおり何もしない
- 台帳: `data_contract.yaml` の 6 か所（承認の台帳の「読めない」/ 波の押し出し / 打ち切りの検査点 / 計画の計器 / 再試行の code / Spec 15 の
  解決）・Spec 12 の (d)・`failures.md` #158

### B 群の実装記録（2026-11-20）

17 件とも**赤いテストを先に書いて、狙った主張で赤になることを確かめてから**直した。画面の 4 件（#19 / #20a + N12 / #22 + N14 / #21）は
サブエージェントが同じ規律で直し、Rust 側はメインが直した。**どの項目も実機では確かめていない。**

| 項目 | 赤のテスト | 赤の出方 | 処方 |
|---|---|---|---|
| #9 | 単体 `orchestrator/mod.rs` の `race_tests::two_approvals_of_one_proposal_dispatch_it_once` / `a_proposal_discarded_mid_approval_is_not_dispatched`（`world` の書き込みロックを握って窓を作る） | 2 本とも `Ok` / 破棄した計画の配送が `Ok` | `PlanWaveStore::dispatch_wave` / `discard_wave` が遷移できたかを `bool`（`#[must_use]`）で返し、偽なら `PlanWaveNotPending` |
| #13 + N6 | 単体 `race_tests::a_conversation_being_opened_is_not_deleted_under_it` | 開いている会話が消えた会話を指す | `delete_session` は判定から削除まで `switch_gate` の read を握る（開いていれば read を放して移ってから、もう一度握って確かめる）。`discard_empty_session` も read を握り、開かれていれば消さない（N6 は赤を取れていない — 下） |
| #10 | 走査 `schedules.rs` の `registration_order_tests`（2 本） | 登録が配送より後 | `deliver_scheduled` / `redeliver_for_acceptance` は配送の前に `in_flight` / `pending_*` へ登録し、配送が `Err` なら置き換えた前の登録へ戻す（`Registered`） |
| #12 + N10 | 結合 `tests/orchestrator.rs` の既存 2 本へ「記録しない」の主張を足した（`sending_to_a_stopped_agent_is_refused` / `asking_a_stopped_agent_fails_without_hanging`） | 配られていない発話・依頼が会話に残る | `reserve_mailbox`（受信箱の枠を `try_reserve_owned` で先に取る。配送の失敗の判定の 1 実装）を記録の前に呼ぶ。`deliver_envelope` も同じ関数を通る |
| #16 | 単体 `delegation.rs` の `a_fallback_name_never_takes_an_existing_name` / `unique_tool_name_adds_a_suffix_until_free` | `["transfer_to_agent_2", "transfer_to_agent_3", "transfer_to_agent_2"]` | `unique_tool_name` = 退避名も既に決めた名前と照合し、空くまで `_2`, `_3`, … を足す。転送・委譲と判断役（`judge_{index}`）が共有 |
| #2 | 結合 `tests/error_body_truncation.rs` の `the_signal_is_read_before_the_body_is_cut` / `a_long_gemini_429_keeps_its_retry_delay` | `ClientError`（`ContextLength` のはず）/ `retry_after: None` | `error_signal` は切る前の本文で呼び、`MAX_ERROR_BODY` の切り詰めは表示へ運ぶ本文にだけ掛ける |
| #7a + #7b + N5 | 結合 `tests/response_body_cap.rs`（送り終えないスタブ — LLM と OAuth の探索）/ 走査 `pricing_source.rs` の `the_fetch_stays_on_https_and_reads_with_a_cap` | 2 本とも待ちの上限まで返らない（`Elapsed`）/ `.https_only(true)` が無い | 新しいモジュール `http_body`（`ChunkSource` を reqwest 0.12 と 0.13 に実装・`read_capped` / `read_prefix`）を LLM（成功 32 MiB・エラー 64 KiB）・Jev（4 MiB）・画像生成（64 MiB）・OAuth（1 MiB）・単価表（8 MiB）が通る。単価表は `.https_only(true)`。字面の網 `every_outbound_response_is_read_through_the_cap`（変異 = Jev のエラー本文を `.text().await` へ戻す → 赤） |
| #25c | 走査 `editor.rs` の `the_path_commands_run_off_the_ui_thread` | `#[tauri::command]` | `#[tauri::command(async)]` |
| #8a | 単体と走査 `autostart.rs` の `an_appimage_registers_and_compares_the_same_path` | 読み戻す側が `current_exe_string()` | `launch_target`（純関数）+ 非 Windows の `app_target` を登録（`app_path`）と読み戻し（`read`）が共有。**非 Windows の分岐はこの端末ではコンパイルされていない** |
| N1 | 単体と走査 `startup.rs` の `a_tray_that_could_not_be_made_turns_hiding_off` | 保存する値をトレイの結果で決めていない | トレイを先に作り、作れなければ `with_tray_outcome` が「トレイに隠れる」を OFF にして保存する。保存に失敗したら、前の値が OFF なら作ったトレイを外す |
| #6 | 単体 `env_file.rs` の `a_placeholder_that_stays_is_still_reported_missing` | `missing` が空 | 値の無い印つきコメントは行をそのまま残し、`missing` へ積む（扉の合鍵は `bake` が作るので数えない） |
| #4a + #4b | 単体 `probe_approvals.rs` の `a_poisoned_lock_approves_nothing` / `a_failed_write_leaves_memory_as_it_was` | 毒されたロックで承認が通る / 書けなかった承認が効く | 毒されたロックは承認せず書かない。`mutate` は書き込みロックを持ったまま書き、失敗したら編集前へ戻す |
| #17c | 単体 `gemini.rs` の `an_empty_assistant_turn_is_not_sent` / 結合 `assist_draft.rs` の `an_empty_reply_in_the_judge_loop_leaves_no_empty_assistant` | `{"role":"model","parts":[]}` を送る / 空の assistant が appended に残る | Gemini の encode は本文も呼び出しも無い assistant を落とす。`assist` は空の応答を積まない |
| #19 | `attachment.test.ts` の `prepareFile（#19）` 6 本（全体を読んだ回数を数える偽のファイル） | 13MB の PDF などで全体を 1 回読んでいた / `NotReadableError` | 大きさ → 先頭 16 バイト（`HEAD_BYTES`）で種別 → 種別の上限 → 全体、の順。読み込みの失敗は `readFailed`（辞書 3 言語 `chatInput.attachReadFailed`） |
| #20a + N12 | `attachmentUrls.test.ts`（10 本）/ `attachmentUrlsWiring.test.ts`（6 本） | モジュールが無い / 画像以外でも本体を読む / `revokeObjectURL` が無い | `lib/attachmentUrls.ts` の `createAttachmentUrlCache`（画像だけ読む・`releaseAll(keep)` で解放・世代で古い読み込みを捨てる）。会話の一覧が空に差し替わったとき解放する |
| #22 + N14 | `pathComplete.test.ts` の `isCurrentListing` 4 本 / `pathCompleteWiring.test.ts` 5 本 | 宛先の監視が無い / 世代が無い / `.exact` が無い | 宛先が変わったら閉じる・一覧が届いたら世代と宛先と種別を確かめてから入れる・補完の `↑↓` に `.exact` |
| #21 | `closeConfirm.test.ts` 3 本 / `launchExitWiring.test.ts` を 2 本に割った | ✕ / OS の経路が守りを通らない | `singleExitConfirm` を ✕・OS の閉じる要求・トレイの「終了」の 3 経路で共有（`exitConfirmInFlight` は撤去）。`onCloseRequested` の解除関数を取って解除する |

- **赤を取れていないもの 1 つ（N6）** — 予定のために作った空の会話を、作ってから消すまでの間に人が開く窓は、外から決定的に作れない。
  処方は構造（`delete_session` と同じく `switch_gate` の read を握り、開かれていれば消さない）で、テストは留めていない
- **#10 と #8a / N1 / #25c は字面で留めた。** #10 は切り離したタスクの中の順序で外から割り込めない。#8a は非 Windows の分岐がこの端末で
  コンパイルされず、N1 / #25c は `AppHandle` と Tauri の実行環境が要る。字面の網は**赤を確かめた後に**直した（Spec 57 P1 と同じ扱い）
- **N1 の副作用**: 保存の前にトレイを作るようになった。保存に失敗したときは、前の値が OFF ならトレイを外す（Spec 69 D7 の「書けなかったときは
  手元の値も変えない」を保つ）
- **N12 で 1 つ後退した**（サブエージェントの報告）— 音声・動画・PDF は本体を読まなくなったので、保持期間で消えたかどうかが会話ペインでは
  分からない。消えていてもファイル名の行が出て、「削除されました」の枠は出ない（Spec 23 D9 が画像以外では効かない）。消えた添付の
  「入力欄へ」は出たままで、押すと入力欄が読んで「削除されました」と断る。本体を読まずに有無だけ確かめる経路は作っていない
- **#20a の残り**: 大きく見ている画像の URL は次に会話を切り替えるまで解放されない（多くて 1 つ）。リングの上限から押し出された発話の添付は
  解放していない
- **未確認**: #22 の「Alt+↑↓ でキーを離すと新しい宛先の一覧で補完が開き直る」はコードの読み / #21 の `decorations: false` の窓で Alt+F4 が
  `CloseRequested` として届くか / #7b の成功の本文の上限（LLM 32 MiB）は実データで測っていない（画像の写しと web 検索の結果を含んでも
  数 MB という見込み）
- **作業で踏んだもの**: `schedules.rs` の作業コピーが CRLF のまま、ヒアドキュメントで LF の行を足して混在した（LF にそろえた。走査テストは
  `\r\n` を `\n` へ直してから読む）/ clippy の `large_enum_variant` が `Registered::Acceptance` に掛かった（箱に入れた）
- **検証**: Rust 全体 94 本・1,631 本が緑・clippy 0 / vitest 137 ファイル・1,190 本・vue-tsc 0・build 緑

### C 群の実装記録（2026-11-20）

9 件とも**赤いテストを先に書いて、狙った主張で赤になることを確かめてから**直した（今回は画面の 3 件もメインが直した）。
**どの項目も実機では確かめていない。**

| 項目 | 赤のテスト | 赤の出方 | 処方 |
|---|---|---|---|
| #23 | 単体 `editor.rs` の `a_placeholder_inside_the_path_is_left_as_is` | `C:\work\7\{path}\a.rs:7`（パスの中の `{line}` が置き換わった） | `expand_one` = ひな形を左から 1 回だけ走査し、差し込んだ値はもう走査しない。知らない `{…}` と閉じていない `{` はそのまま |
| #17a | 単体 `openai_responses.rs` / `xai_responses.rs` / `perplexity_responses.rs` の `no_tools_means_no_tool_choice` | 3 本とも `Some("required")` | ツールを 1 本も送らない要求には `tool_choice` も送らない（Anthropic と同じ形） |
| #25a | 単体 `mcp_server.rs` の `a_port_outside_the_range_is_refused_on_save` / `a_port_outside_the_range_in_the_file_blocks_the_store` | 0 / 80 / 1023 を保存できた / ポート 0 のファイルを読めるものとして扱った | `MIN_PORT` = 1024 と `check_port`。`save` は範囲の外を断り、`load` は範囲の外のポートを読めないファイルと同じ扱い（扉を開かず保存も断る） |
| #25e | 単体 `race_tests::zero_capacities_are_raised_to_one` | イベントの channel の構築で panic（`broadcast channel capacity cannot be zero`） | `OrchestratorConfig::with_capacity_floors` を `bootstrap` の入口で掛け、会話ログ・受信箱・イベントの容量を 1 以上へ。**受信箱と会話ログの下限は 1 つずつ外す変異で赤を確かめた**（`runtime.rs:91` の `mpsc::channel(0)` / `drain` の範囲外） |
| N7 | 単体 `autostart.rs` の `appimage_is_used_only_when_running_from_its_mount` | `/home/u/evil` を返した | `launch_target` は今の実行ファイルが `{temp}/.mount_` の下にいるときだけ `APPIMAGE` を使う（tauri-utils の `Env::default` と同じ確かめ方）。**非 Windows の分岐はこの端末ではコンパイルされていない** |
| N4 | 単体 `env_file.rs` の `the_example_file_is_filled_by_update`（`deploy/.env.example` を `include_str!` で読む） | 例の秘密の行がすべて `commented_out` | 例の 5 行へ `# bake: 例` の印と、先頭に手順の 2 行。**扉の合鍵の印つきの空きを `bake` が作って埋めるようにした** — B 群の #6 で「扉の合鍵は `bake` が値を作る」と書いたが、空きの枝では作っていなかった（例のファイルに扉の行の空きが入って初めて届く経路。`failures.md` #160） |
| #18 + N13 | `messageMerge.test.ts` の配線 2 本（関数を先に置いてから走らせた） | 読み直しが `state.messages = await ipc.listMessages` / 待った後に開いている会話を確かめていない | `lib/messageMerge.ts` の `mergeFetchedMessages`（読んだ結果の後ろに、読んでいる間に届いた発話のうちまだ無いものを足す）を、切り替えの読み直しと起動時の 2 か所が使う。切り替えの読み直しは、待った後にいま開いている会話が同じときだけ当てる |
| #20b | `attachmentListWiring.test.ts` の 1 本 / `attachmentList.test.ts` の `keepPreviews` 2 本 | 外したとき・送れたときに絞っていない | `keepPreviews` を `removeAttachment` と送信の成功の枝で呼ぶ。送る前には絞らない（失敗して戻したチップのサムネイルが消える） |
| #20c | `imageConvertWiring.test.ts` の 1 本 | `finally` が無い | `try { … } finally { bitmap.close(); }` |

- **テストの側で 2 つ直した**: N4 の初版は名前の出現数で「末尾に足し直さない」を数えていて、`.env.example` の説明の行
  （`claude_sonnet → FUSEFORKS_SECRET_CLAUDE_SONNET`）を拾って 2 件になった（`NAME=` の形で数える）/ `quoteRefWiring.test.ts` が
  送信の成功の枝を `if (ok) return;` の字面で留めていた（#20b でブロックになった。成功の枝が抜けることと、中で戻さないことは留めたまま
  ブロックも通す形へ）
- **#25e で範囲を 1 つ広げた**: TODO は `log_capacity` だけだったが、受信箱（`mpsc::channel(0)`）とイベント（`broadcast::channel(0)`）も
  0 で構築が panic する同じ形なので一緒に寄せた。**間隔の 0（`stats_interval` / `schedule_interval`）は寄せていない** — `tokio::time::interval`
  は 0 で panic するはずだが、切り離したタスクの中なので落ちるのはティッカーだけで、確かめるテストを書いていない（コードの読み）
- **検証**: Rust 全体 94 本・1,640 本が緑・clippy 0 / vitest 139 ファイル・1,200 本・vue-tsc 0・build 緑

### 直さないと決めるもの

- ~~**#8b** 契約（`data_contract.yaml:4590-4591`）とテストが決めている。Windows が HKLM の `StartupApproved\Run` を HKCU の `Run` にも
  適用するかを一次資料で確かめてから、契約ごと変えるかを決める~~ → 2026-11-21 に直した（末尾の「#8b の実装記録」）
- **#17b** 設計どおり（構造化出力のフォールバック。テストが意図して留めている）
- **#24 / #25b / #25d** 不成立

## #8b の実装記録（2026-11-21）

利用者「#8b を進めよう」。**文書で決まらなかったので実機で測り、判定を直した。**

- **文書**: Microsoft Learn の「Startup apps」（Windows 8 cookbook）は Run キーの種類を挙げるだけで `StartupApproved` に触れていない。
  対応を書いている資料（windowsir 2022-07「Does 'Autostart' Really Mean 'Autostart'?」/ Nutanix「Windows OS Optimization Essentials,
  Part 4」2021-12）はどちらも同じハイブの組（HKLM の承認 ↔ HKLM の `Run`、HKCU ↔ HKCU、`Run32` ↔ WOW6432Node の `Run`）で書くが、
  説明として書いているだけで、ハイブをまたぐ場合を試していない。村（ルナ。ジェミーは別件の 400 で停止）の調べも「分からない」だった。
  ハイブをまたいで判定しているのは `auto-launch` の現行（`zzzgydi/auto-launch` `8408d6f`。両方のハイブの承認を AND）だけで、
  0.5.0 は HKCU しか見ていない — どちらも作者の組み方
- **試験**（使い捨てのスクリプト。リポジトリには入れていない）: HKCU の `Run` に `ff8b-probe` と `ff8b-control`（どちらも
  `cmd.exe /d /c echo %DATE% %TIME%> "%TEMP%\<名前>.txt"`）、HKLM の `StartupApproved\Run` に `ff8b-probe` の「無効」
  （`03 00 00 00` + 置いた時刻の FILETIME = タスクマネージャーが書く形）。HKCU の承認と HKLM の `Run` には何も置かない。
  HKLM へは UAC の昇格で書き、昇格していないシェルから同じ HKCU に値が見えることを確かめてから、利用者がサインインし直した
- **結果**（Windows 11 Pro 26H2・10.0.26320・UBR 10003）: サインイン 11:11:50 → Explorer 11:11:51 → OneDrive 11:13:04 →
  **`ff8b-probe` 11:14:30.86 / `ff8b-control` 11:14:31.41 の両方が起動**。HKLM の承認の「無効」は HKCU の `Run` の同名の値に効かない。
  試験の値は UAC で片付け、6 つとも残っていないことを昇格していないシェルから確かめた
- **測っていない向き**: HKCU の承認の「無効」が HKLM の `Run` の同名の値に効くか（HKLM の `Run` へ値を置く試験が別に要る）。
  同じ規則が対称に効くと読んで、判定は対称に直した
- **直したこと**: `autostart.rs` の `windows_view` を「HKCU の `Run` に値があり HKCU の承認で無効でない、**または** HKLM の `Run` に値があり
  HKLM の承認で無効でない」へ。赤いテスト `startup_approved_applies_only_to_its_own_hive` を先に書き、旧い判定で 1 つ目の主張
  （HKCU の `Run` だけ + HKLM の承認が無効 → 有効）が赤になるのを確かめた。既存テストの「HKCU の登録 + HKLM の承認が無効 → 無効」
  （旧 452 行目）は実測と逆なので新しいテストへ移した。**変異 1 回**: 2 つのハイブの承認を入れ替えると、新しいテストと既存の
  `the_windows_view_reads_both_hives_and_the_task_manager` の 2 本が赤（予測どおり。sha1 で復元を確認）
- **画面の帰結**: HKLM に「無効」だけが残った端末でも、チェックを入れれば HKCU へ書いて ON と表示され、サインイン時に起動する
  （旧い判定では OFF のまま戻らなかった）
- 台帳: `data_contract.yaml` の `launch_exit_contract` 8 / Spec 69 の「Windows の読み」に覆しを書いた。`failures.md` #161
- **別件で見つけたもの**: ジェミー（`gemini-4-hydrogen`）が Google 検索と関数ツールを併用するターンで
  `Function call is missing a thought_signature`（400）を返して停止した（2026-11-17 にも 1 回）。#8b とは別のセッション
  「Gemini 4 の thought_signature 欠落で 400 になる」で扱う
