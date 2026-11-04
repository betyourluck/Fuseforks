# Spec: 画面の簡体字中国語（コアは英語のまま）

**ID**: 74
**Date**: 2026-11-04
**Status**: rev2 → **P0〜P1 完了**（2026-11-04。P0 = `settings_contract` の凍結 / P1 = Rust の `DisplayLanguage`・テスト 1,510 本全緑。記録は末尾）
**Branch**: main 直（Phase ごとにコミット）

---

## Goal

画面の表示言語に **简体中文** を足す。目的は中国語圏の利用者と開発者を呼び込むことと、
その先で利用者が自分では使わない中国製 LLM の固有機能（DeepSeek の `reasoning_content`、
Qwen の `enable_thinking`、Kimi の `$web_search` など）を足してもらう入口を作ること。

**利用者裁定（2026-11-04）: コアがモデルへ渡す文は英語のまま。** 中文の村では、画面は中国語になり、
システムプロンプト・ツールの説明・System 行・封筒の語は英語になる。理由は 3 つある。
(1) 中国製の LLM はどれも英語のプロンプトで十分に動く。
(2) モデルへ渡す文の誤りは振る舞いを黙って変えるが、利用者は中国語を読まないので誰も気づけない
（画面の誤訳なら見た目の問題で済む）。
(3) コアに 3 本目の腕（約 200 か所）を書かずに済む。

**「英語のまま」の範囲はモデルへ渡す文だけ**で、コアの文全体ではない。コアのエラー文（`CoreError` の
`#[error]`）は Spec 13 以来日本語固定で、言語を知らない層が書く。これは画面に原文として出るだけで、
モデルへは渡らない（D5）。

---

## 接地（起票前の実測。2026-11-04）

| 層 | 現状 | 数 |
|---|---|---|
| 型 | `world::Language { Ja, En }`。`from_os_locale` は「`ja` 前方一致なら Ja、それ以外は En」 | 1 |
| コアの文 | `Language::pick(ja, en)` の呼び出し 100 か所（headless 41・fs 11・blackboard 10・run 9…）+ `Language` の `match` 約 90 か所（delegation 19・tool_image 18・assist 16・turn 12…） | 約 200 |
| 言語を読む口 | `Orchestrator::language()` の呼び出し元 5 か所 — **コアの声が要る** 2（`orchestrator/assist.rs`・`orchestrator/template_probe.rs`）/ **表示の言語が要る** 3（`commands::get_language`・`commands::set_startup_settings` のトレイ・`lib.rs` の `after_boot` のトレイ） | 5 |
| 言語を書く口 | `set_language(` の呼び出し元 — 本番は `bootstrap.rs`（OS から初回確定）と IPC の 2 つ。**テストは約 50 ファイル**（コアの結合・headless の単体・`apps/cli/tests/cli.rs`・`launch_exit_wiring.rs`）が `set_language(Language::Ja / En)` | 約 52 |
| GUI の Rust | `tray.rs` の `labels(Language)`（トレイのメニュー 2 項目）/ IPC `get_language` / `set_language` | 3 |
| UI の辞書 | `locales/ja.json` 1,420 鍵・値の合計 28,448 字 / `en.json` 同じ鍵 | 2 ファイル |
| エディタ | `lib/editorPhrases.ts` が CodeMirror の検索パネルの文言を ja だけ上書き（「三言語目を足すときは腕が 1 つ増える」と doc が予告している） | 1 |
| フォント | `body` の `font-family` が `"Inter", "Noto Sans JP", system-ui, …`。**Noto Sans JP は同梱していない**（`@font-face` は DotGothic16 だけ）ので、入っている端末でだけ効く | 1 |
| `<html lang>` | `setLocale` が `document.documentElement.lang = locale`。`index.html` の初期値は `lang="ja"` で、書き換わるのは起動の後 | 2 |
| フロントの型 | `types.ts` の `Language = "ja" \| "en"` を読むのは 4 ファイル（SettingsDialog・i18n・editorPhrases・ipc） | 5 |
| 中文の台帳 | `README_zh.md`（166 行）/ `DETAIL_zh.md`（737 行）は既にある。用語は 仆从 38 / 村庄 43 / 羁绊 9 / 条例 10 / 黑板 7 / 职务 10 | — |
| OS のロケール | sys-locale 0.3.2 は BCP 47（ハイフン区切り）で返す。Unix は `zh_CN.UTF-8` → `zh-CN`、Windows は `zh-CN` 形、macOS は優先言語（`zh-Hans-CN` 形） | — |

**既に繋がっている中国製 LLM**: Qwen3-VL（さくら・OpenAI 互換。Spec 23 P0 で画像まで通した）/
`perplexity/deepseek-v4-flash-0731`（Perplexity の Responses）。**ワイヤが無くても繋がる**。
欠けているのは固有機能だけで、たとえば `reasoning_content` はコアで 1 度も読んでいない
（grep 0 件）。

**起票時に見つけた既存の腐り**: 設定ページの注記 `settings.language.promptNote`（ja / en）は
Spec 13 当時の「サーヴァントへの指示は翻訳されません」のままで、**Spec 35 以降、日英では指示も切り替わる
実装と食い違っている**。`world.rs` の `Language` の doc「コアはこの値で分岐しない」と、
`bootstrap.rs` のコメント「コアはこの値で分岐しない」も同じく Spec 35 以降は嘘（P0 / P2 で回収）。

---

## 決定（D）

### D1: 型を 2 つに割る — 表示の言語と、コアの声

- **`DisplayLanguage { Ja, En, ZhHans }`**（新設）を `world.json` に保存し、画面・IPC・トレイが読む
- **`Language { Ja, En }`** は**コアの声**として据え置く。`pick` と約 90 か所の `match` は 1 行も変えない
- 写し方は `DisplayLanguage::voice(self) -> Language` の 1 実装（`ZhHans → En`）
- **読む口は 2 つに分ける**:
  - `World::language()` / `Orchestrator::language()` は今までどおり `Option<Language>` / `Language`
    （`voice()` を通したコアの声）を返す。呼び出し元のうち `assist.rs`・`template_probe.rs` はそのまま使う
  - `World::display_language()` / `Orchestrator::display_language()` を新設する。
    `commands::get_language`・`set_startup_settings` のトレイ・`after_boot` のトレイはこちらへ移す
- **書く口は `impl Into<DisplayLanguage>` で受ける**（`World::set_language` / `Orchestrator::set_language`）。
  `From<Language> for DisplayLanguage` を置くので、約 50 ファイルのテストの
  `set_language(Language::Ja)` は書き換えずにコンパイルが通る

**保証の範囲を正確に書く**: 保証するのは「**約 200 か所の分岐（`pick` と `match`）が `ZhHans` を
受け取らない**」ことで、手段は**型の分離**（分岐が受け取る型 `Language` に 3 値目が無い）。
`DisplayLanguage` はコアの crate の中にも在る（保存と初回確定はコアの仕事）。その字面が現れる場所を
**走査テストで 3 ファイルに留める** — `world.rs` / `orchestrator/settings.rs` / `orchestrator/bootstrap.rs`
（`crates/fuseforks-core/src` が対象。`tests/` は `Into` 経由なので字面が出ない）。

`Language` に 3 値目を足して各所で `En | Zh` と書く形は採らない。約 90 か所をコンパイラが指すが、
どれも同じ腕を足すだけになる。しかも将来コアに中国語を入れる日に、「意図して英語にした腕」と
「写し忘れた腕」を区別できなくなる。

### D2: 簡体字だけ。ワイヤ値は `"zh-Hans"`

- ワイヤ値（`world.json` の `language`・IPC・i18n の locale 名・`<html lang>`）は **`"zh-Hans"`**
- `"zh"` にしない。繁体字を足す日に `"zh"` がどちらを指すか読めなくなる
- `<html lang="zh-Hans">` は正しい BCP 47 で、WebView は簡体字のフォントを選ぶ材料にする
- **`DisplayLanguage::parse` は自分が書く 3 値だけを受け付ける**（閉じた集合）。`zh-Hant` のような
  知らない値は従来どおり「未確定」へ倒す（D7）。保存値に D3 の正規化を掛けることはしない —
  D3 は OS の文字列を解釈する規則で、保存値はこの版が書いた値か壊れた値のどちらかしかない

### D3: OS のロケールからの判定（初回起動の確定）

`DisplayLanguage::from_os_locale` は BCP 47 の文字列を `-` で割り、**上から順に評価する**。
大文字と小文字は区別しない。

| 順 | 条件 | 結果 |
|---|---|---|
| 1 | 主言語が `ja` | Ja |
| 2 | 主言語が `zh` で、**文字体系サブタグ**が `Hans` | **ZhHans** |
| 3 | 主言語が `zh` で、文字体系サブタグが `Hant` | **En** |
| 4 | 主言語が `zh` で文字体系サブタグが無く、地域が `TW` / `HK` / `MO` | **En** |
| 5 | 主言語が `zh`（`zh` / `zh-CN` / `zh-SG` …） | **ZhHans** |
| 6 | それ以外・取得失敗 | En |

**文字体系を地域より先に見る**。地域を先に見ると、香港・マカオ・台湾で簡体字を優先言語にした
macOS の `zh-Hans-HK` / `zh-Hans-MO` / `zh-Hans-TW` が英語へ誤って倒れる。
主言語は**完全一致**で見る（前方一致だと `zha`（壮語）が中文に当たる）。

繁体字圏を英語へ倒すのは、繁体字の利用者へ簡体字を出すより英語のほうが無難なため。
**確定済みの村は再判定しない**（既存の規律。今ある村の言語は変わらない）。

### D4: フォント — 簡体字のときだけ SC のフォントを先に立てる

```css
html:lang(zh-Hans) body {
  font-family: "Inter", "Noto Sans SC", "Microsoft YaHei UI", "Microsoft YaHei",
    "PingFang SC", system-ui, -apple-system, "Segoe UI", sans-serif;
}
```

これが無いと、Noto Sans JP の入った端末では漢字が日本の字形で描かれ、簡体字にしか無い字
（们・这・认 など）だけが別のフォントへ落ちて字形が混ざる。
**フォントは同梱しない**（OS の既定に任せる。Windows は YaHei、macOS は PingFang、
Linux は Noto Sans CJK SC が普通は入っている）。

**`index.html` の起動画面は対象外。** 起動画面の間 `<html lang>` は常に `"ja"`
（言語は world.json にあり IPC が要るので、`setLocale` は起動の後）で、`:lang(zh-Hans)` は当たらない。
しかも起動画面の字は等幅の `starting up...` だけで漢字を含まない。

### D5: 中文の村で中国語にならないもの（利用者が負う条件）

**英語で出るもの**
- **System 行**（入退室・承認待ちで止まった・予算切れなど）。記録した時の声で書く規律
  （Spec 35 D6）なので英語になる
- **ヘッドレス実行の起動前検査の指摘文**（`fuseforks-cli check / ask / serve`）。`world.language()` = コアの声を読む

**日本語で出るもの**
- **エラーの原文**。画面は訳語の後ろに原文を添える（英語と同じ経路）。コアのエラー文は
  日本語固定（Spec 13）なので、添えられる原文は日本語
- **予定の再現規則のラベル**（予定ダイアログの一覧・削除の確認に出る「毎週 木曜 17:00」「10 分ごと」）。
  IPC が `Recurrence::label_ja()` を固定で返す（Spec 35 P3 で英語の村でも据え置いた）
- **`fuseforks-cli` の外枠**（`エラー` / `警告` / `情報` / `直し方:`。`apps/cli/src/output.rs`）
- **日時の書式** — `"ja-JP"` 固定の 6 か所（SessionDialog・ScheduleDialog ×2・ChatPanel・quoteRef・
  AgentCard）。英語の村でも同じで、既存の宿題。もう 1 か所の `"ja-JP"`（`lib/format.ts`）は数値の
  桁区切りで、中文でも同じ形になるので害は無い

どれも本 Spec の外。中文の村だけの問題ではなく、英語の村でも同じ形で残っている。

### D6: 用語は `README_zh.md` に揃える

仆从（サーヴァント）/ 村庄（村）/ 羁绊（絆。英語の `Kizuna` に当たる固有名）/ 条例 / 黑板 /
职务（役職）/ 委派 / 汇总。訳は Neo が書く。**利用者は検証できない**ので、README_zh の冒頭に
「訳の誤りは Issue / PR で歓迎」の 1 行を足す（誤訳を直してもらうことも呼び込みの一部）。

### D7: 旧版で中文の村を開くと、言語は書き戻される

v0.6.5 以前は `"zh-Hans"` を未知の値として「未確定」へ倒し、次の起動で OS から ja / en を確定して
**保存する**。world.json の未知の欄の保護（#112）は**知らない欄**を守る仕組みで、**知っている欄に入った
知らない値**は守らない。害は言語の 1 欄だけ（画面が日本語か英語に戻る）なので、機構は足さず
リリースノートの「利用者が負う条件」に書く。

**旧い画面と新しいコアの組み合わせは起きない。** 画面（`frontendDist`）とコアは 1 つの配布物に
同梱されるので、版の食い違いは world.json を挟んだ組み合わせ（この D7）だけ。

### D8: 設定ページの注記を 3 言語とも実態に合わせる

`settings.language.promptNote` を書き直す（接地の「既存の腐り」）。文意は 3 言語で同じ:
「日本語と English では、サーヴァントへの指示（システムプロンプト・ツール説明）も切り替わります。
简体中文では画面だけが切り替わり、指示と会話ログの System 行は英語になります。System 行は書かれた
当時の言語のまま残ります。」 README 3 言語の「システム設定」の行（`语言（画面与发给仆从的指示都会切换）`
ほか）も同じ形へ直す。

---

## 範囲の外

- コアの中国語（D1。入れる日は `Language` に腕を足す別 Spec）
- 繁体字
- D5 に並べたもの（日時の書式・再現規則のラベル・エラーの原文・CLI の外枠）
- **`CONTRIBUTING`（ワイヤ・固有スキルの足し方の手引き）** — 呼び込みの効き目は画面の言語より
  こちらが大きいという見立て。`Provider` に 1 値足すと carries の表・`providerSkills.ts`・
  辞書・stranded の行まで触る必要があり、外から来た人はここで詰まる。別の作業として起こす
- ランディングページ

---

## Phases

### P0: 契約

- `data_contract.yaml` の `settings_contract`: **`world.json` の `language` を検証する型は
  `DisplayLanguage`**（3 値 `"ja" | "en" | "zh-Hans"`）、コアの分岐が受け取るのは `Language`（2 値）と
  明記・選択肢 3 つ・判定の表（D3。評価順つき）・`voice()`（D1）・フォント（D4）
- `world.rs` の `Language` の doc と `bootstrap.rs` のコメント「コアはこの値で分岐しない」を直す
- Spec 13 / Spec 35 へ行き先を 1 行

### P1: Rust

- `DisplayLanguage` + `voice()` + `parse` / `as_str`（ワイヤ値は D2）+ `from_os_locale`（D3）+
  `From<Language>`
- `World` の保存を `DisplayLanguage` へ。`language()` は `voice()` を通した値、`display_language()` を新設。
  `set_language(impl Into<DisplayLanguage>)`
- `Orchestrator::display_language()` を新設し、`commands::get_language`・`set_startup_settings`・
  `after_boot` をそちらへ。IPC `set_language` は `DisplayLanguage` を受ける。トレイの文言に 3 本目
- 単体: 判定の表の全行（`zh-Hans-HK` / `zh-Hans-TW` / `zh-Hant-CN` / `zh-TW` / `zh-SG` / `zh` /
  `zha` / `ja-JP` / 取得失敗）/ ワイヤ値の往復 / 未知の値（`zh-Hant` を含む）は未確定 /
  `voice(ZhHans) == En`
- 結合: 中文の村で組んだシステムプロンプトとツールの提示が、英語の村とバイト等価（既存の golden の
  英語側と一致）。**ZhHans の村で `failed_turn_note` が英語**（Spec 35 の失敗の注記の経路）
- 走査: `crates/fuseforks-core/src` で `DisplayLanguage` の字面が現れるのは 3 ファイルだけ（D1）
- コードと一緒に直す doc（今のコードを正しく説明しているので P0 では触らなかった）:
  `world.rs` の `from_os_locale` と単体の「選択肢は 2 つだけ」/ `bootstrap.rs` の「ja / en のどちらかへ確定」

### P2: 画面

- `types.ts` の `Language` を **`DisplayLanguage`** へ改名し `"zh-Hans"` を足す（読む 4 ファイルは
  型検査が指す）。Rust と名前を揃え、将来コアに中国語を入れる日に「言語」が 2 つある状態を作らない
- `locales/zh-Hans.json`（1,420 鍵）と i18n の messages。**ファイル名と import の綴りは大文字小文字まで
  一致させる** — Linux は区別するので、CI の Ubuntu の `bun run test` が食い違いを落とす
- 鍵集合一致のテストと vue-i18n のコンパイル検査を 3 言語へ
- システム設定の言語の選択肢に「简体中文」と D8 の注記
- `editorPhrases.ts` に ZH の表（ライブラリの原文を走査するテストがそのまま効く）
- `style.css` のフォント（D4。`index.html` は触らない）
- `types.ts` の型の doc「選択肢は 2 つだけ」を型と一緒に直す

### P3: 台帳

- DETAIL 3 言語・README 3 言語（「システム設定」の行を D8 の形へ）・README_zh の冒頭に誤訳歓迎の 1 行
- CLAUDE.md の Spec の状態

### P4: 実機

1. OS が日本語の端末で、設定から简体中文へ切り替えると画面が中国語になり、再起動後も残る
2. 中文で、漢字が日本の字形で描かれていない（「们」「这」「骨」「直」で見る。Noto Sans JP の
   入った端末で確かめる）
3. 中文の村で依頼すると、`fuseforks.log` の `cache:` 行の `prompt` が、同じ依頼を英語の村で
   投げたときと同じ値になる
4. トレイのメニューが中国語
5. Ctrl+F の検索パネルが中国語

（OS のロケールが中国語の端末での初回確定は手元で踏めないので、単体の表に預ける）

---

## Notes

1. **目論見の前提の確認**: 中国製の LLM は今でも OpenAI 互換で繋がるので、「ワイヤを作ってもらう」の
   実体は固有機能を足してもらうことになる。画面の中文化は利用者を、手引きは貢献者を呼ぶ手。
2. **訳の量**: ja の値の合計は 28,448 字。英語版（`en.json`）を元に訳すと、日本語特有の言い回しを
   引きずらない。用語だけ README_zh に揃える。

3. **査読（2 系統 12 点 = A は 5 + 軽微 2、B は 5）の処理** — 採用 9 / 前提を訂正して採用 1 /
   反証 1（一部は採用）/ 不採用 1（評価順の明記は採用）

| # | 指摘 | 判定 | 処理 |
|---|---|---|---|
| A1 | Goal「コアは英語」とエラー原文が日本語なのは矛盾 | 前提を訂正して採用 | エラー文はモデルへ渡らないので、P1 のバイト等価のテストは落ちない（失敗の注記は `voice()` を通るので英語）。Goal に範囲を書いた |
| A2 | 「一度も見ない」と「world.rs で読む」の矛盾 | 採用 | D1 の保証を「分岐が受け取らない」へ直し、走査を 3 ファイルへ |
| A3 | `parse` にも D3 の正規化を掛けよ | 不採用 | 保存値はこの版が書いた値か壊れた値だけ。知らない値は未確定へ倒す既存の規律を保つ（D2）。将来 `zh-Hant` が保存されても、どちらの扱いでも次の保存で書き戻される。評価順を明記せよ、は B3 と合わせて採用 |
| A4 | 旧 GUI と新コアの組み合わせ | 反証 | 同じ配布物に同梱されるので起きない（D7）。ファイル名の大文字小文字は採用（P2） |
| A5 | `:lang(zh-Hans) body` の書き方 / `index.html` | 採用 | `html:lang(zh-Hans) body` へ。`index.html` は B2 で対象外 |
| A6 | 契約が検証するのは `DisplayLanguage` と明記 | 採用 | P0 |
| A7 | フロントの型も `DisplayLanguage` へ | 採用 | P2 |
| B1 | `bootstrap.rs`・`Orchestrator::language` の 5 呼び出し元・`set_language` のテスト約 50 本 | 採用 | 読む口を 2 つに分け、書く口を `Into` にした（D1）。接地の表に行を足した |
| B2 | `index.html` の `:lang()` は起動画面の間は当たらない | 採用 | D4 で対象外に |
| B3 | 地域を先に見ると `zh-Hans-HK` が英語に倒れる | 採用 | D3 を文字体系 → 地域の順の表へ。主言語は完全一致 |
| B4 | `promptNote` と README_zh の言語の行が実態と食い違う | 採用 | D8（既存の腐りも同時に回収） |
| B5 | 「7 か所」の内訳 / 再現規則のラベル / CLI | 採用 | D5 を英語で出るもの・日本語で出るものに分けた |

査読の外で足したもの: 中文の村で失敗の注記が英語になることの結合テスト（P1）。走査の範囲を
`tests/` まで広げなかったのは、テストが `Into` 経由で `Language` を渡すだけで字面が出ないため。

---

## P0 契約記録（2026-11-04）

- `data_contract.yaml` の `settings_contract` — 村に保存する欄の `language` を 3 値（検証する型は
  `DisplayLanguage`）/ 言語の項を 3 つの選択肢・判定の表（評価順つき）・ワイヤ値 `"zh-Hans"`・閉じた
  `parse` へ / **表示の言語とコアの声の分離**の項を新設 / 简体中文のフォントの項を新設 / 多言語化の
  層 (1) の辞書と層 (3) に「简体中文の村は英語を送る」。`launch_exit_contract` のトレイの文言を
  「表示の言語」へ
- `world.rs` の `Language` の doc を「コアの声」へ書き直した（旧文「コアはこの値で分岐しない」は
  Spec 35 以降嘘）。`bootstrap.rs` の同じ文も直した
- Spec 13 と Spec 35 のヘッダに行き先
- 追従の grep で残したもの: 「コアは言語を知らない」（`errorText.ts`・Spec 13・契約の層 (2)）は
  **エラー文の層についての記述**で今も正しい。「選択肢は 2 つだけ」「ja / en のどちらかへ確定」
  （`world.rs` 2 か所・`bootstrap.rs`・`types.ts`）は今のコードの説明なので、コードと一緒に
  P1 / P2 で直す（Tasks に書いた）

---

## P1 実装記録（2026-11-04）

- `world.rs` に `DisplayLanguage { Ja, En, ZhHans }`（serde は `"ja"` / `"en"` / `"zh-Hans"`）+ `voice()` +
  `from_os_locale`（判定表の 6 行）+ 閉じた `parse` / `as_str` + `From<Language>`。`Language::from_os_locale` は
  撤去した（呼び出し元は bootstrap だけで、2 値の判定を残すと表が 2 つになる）
- `World` の保存を `Option<DisplayLanguage>` へ。`language()` は `voice()` を通した値、`display_language()` を新設、
  `set_language(impl Into<DisplayLanguage>)`。**約 50 ファイルのテストの `set_language(Language::Ja)` は 1 行も
  書き換えずに通った**
- `Orchestrator::display_language()` を新設し、`commands::get_language`・`set_startup_settings` のトレイ・
  `after_boot` のトレイをこちらへ。IPC `set_language` は `DisplayLanguage` を受ける。トレイの 3 本目は
  「显示 Fuseforks」/「退出」
- テスト: 単体 3 本（判定表 24 行 / `voice` と保存の往復 / ワイヤ値の閉じた集合）+ 結合 2 本
  （`display_language_voice.rs` = 中文の村の `ChatRequest` が英語の村と丸ごと一致。負の対照として日本語の村は
  違うことを先に確かめる / `failed_turn_note_language.rs` に中文の村の注記が英語の 1 本）+ 走査 1 本
  （`display_language_scope.rs` = コアの src で字面が出るのは 3 ファイルだけ）
- **変異 4 回とも予測どおり**（`voice` で ZhHans を Ja へ → 3 本 / 地域を文字体系より先に → 1 本 / turn.rs に字面 →
  1 本 / `parse` が `"zh"` を受ける → 1 本）。**回し方で 1 回外した** — `cargo test --lib world:: --test …` の
  名前フィルタ `world::` が結合テストにも掛かり、M1 は単体の 1 本しか数えず、M3 は 0 本だった
  （`failures.md` #133 の再演）。結合テストを分けて回し直して予測と一致した
- 全テストの 1 回目は `LNK1104`（`user32.lib` を開けない）で 1 本も走らなかった（集計が空 = #132 の形）。
  clippy と並走させていた。`-j 4` で回し直して **ワークスペース 1,510 本全緑**（85 バイナリ）・clippy 0
