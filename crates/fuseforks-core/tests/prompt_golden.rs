//! システムプロンプトの golden（Spec 35 検収 1）。
//!
//! **`(prompt, stable_len)` の両方をリテラルで固定する。** 文字列だけ固定すると
//! 境界の計算が変わっても緑のままになる（P0 で読み口を数えた結果 — 値を固定する
//! 既存テストは無かった）。この golden が緑である限り、日本語の村では
//! プロンプト生成が 1 バイトも変わっていない — 「加算である」ことの機械の証明。
//!
//! **リテラルは 2026-08-12（Spec 35 P1 着手前）の実出力を焼いたもの。**
//! 手で書いていないので、転記の誤りがあればこのテスト自身が最初に落ちる。

use fuseforks_core::config_store::ConfigStore;
use fuseforks_core::model::{AgentId, AgentSpec, ConfigFileKind};
use fuseforks_core::world::Language;

async fn store_with_fixtures(dir: &std::path::Path) -> (ConfigStore, AgentSpec) {
    let store = ConfigStore::new(dir);
    let id = AgentId::from("agent_01");
    store.write_ordinance("条例X").await.unwrap();
    store.write_config(&id, ConfigFileKind::Construct, "制約A").await.unwrap();
    store.write_config(&id, ConfigFileKind::Skill, "能力B").await.unwrap();
    store.write_config(&id, ConfigFileKind::Memory, "記憶C").await.unwrap();
    let mut spec = AgentSpec::new(id, "ザリ", "tpl");
    spec.work_dir = Some("/work".to_string());
    (store, spec)
}

fn temp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("ff-golden-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 全部品あり（条例・作業フォルダ・接地・Construct・Skill・顔ぶれ・Memory）。
#[tokio::test]
async fn ja_full_prompt_is_byte_identical_to_the_pre_spec35_output() {
    let dir = temp("full");
    let (store, spec) = store_with_fixtures(&dir).await;
    let (prompt, stable_len) = store
        .compose_system_prompt(&spec, true, Some("R1 / R2"), Language::Ja)
        .await
        .unwrap();

    assert_eq!(stable_len, 696, "安定境界が動いた（キャッシュが全員ぶん割れる）");
    assert_eq!(
        prompt,
        "# 村の条例（この場の全員に適用される規則。個別設定より優先される）\n条例X\n\n# あなたについて\nあなたはこの場に参加している **ザリ** です。\n- **ザリ 自身の発言だけを書いてください。** 他のエージェントの発言を代筆・代弁してはいけません。彼らはそれぞれ自分で発言します。\n- 発言に自分の名前を書く必要はありません。誰の発言かは自動的に伝わります。\n- 相手の発言を装って会話を先に進めないでください。\n\n## 作業フォルダ\nあなたのファイル系ツール（grep / fd / diff / sd / yq）が読み書きできるのは `/work` の中だけです。ツールが返す相対パスは、このフォルダ直下からのパスです。ファイルの場所を説明するときは、このパスを基準にしてください（それ以外の場所を推測で語らないこと）。\n\n## グラウンディング（Google 検索）について\nあなたは Google 検索で裏を取ってから答えられます。ただし**参照したページの URL は、あなたの手元には渡ってきません。**\n- **URL を書かないでください。** 出典を求められたら「URL は取得できない」と正直に答えてください。\n- 代わりに、**実際に検索した語**と、**発表元の名前**（気象庁・内閣府・◯◯新聞 など）は答えられます。それを示してください。\n- もっともらしい URL を組み立ててはいけません。実在しない URL は、出典が無いことより有害です（受け取った相手が確認済みだと誤解します）。\n\n## Construct\n制約A\n\n## Skill\n能力B\n\n## 今の顔ぶれ\nR1 / R2\n\n## Memory\n記憶C\n"
    );
}

/// 英語村の枠組みに日本語が 1 文字も無い（検収 2。**新規生成・空村に限定** —
/// 言語を切り替えた村では D6 の古い System 行が日本語で残るので、
/// ゼロは新規生成にしか成立しない）。
///
/// 村由来の本文（条例・Construct 等）は利用者の資産なので固定物で埋め、
/// **コアが書いた枠組みだけ**を検査する形にしている（ASCII の固定物なら
/// 仮名漢字カウントに混ざらない）。
#[tokio::test]
async fn en_framework_contains_no_japanese_for_a_fresh_village() {
    fn ja_chars(s: &str) -> usize {
        s.chars()
            .filter(|c| matches!(*c as u32, 0x3040..=0x309F | 0x30A0..=0x30FF | 0x4E00..=0x9FFF))
            .count()
    }

    let dir = temp("en");
    let store = ConfigStore::new(&dir);
    let id = AgentId::from("agent_01");
    store.write_ordinance("Ordinance body").await.unwrap();
    store.write_config(&id, ConfigFileKind::Construct, "Constraint A").await.unwrap();
    store.write_config(&id, ConfigFileKind::Skill, "Skill B").await.unwrap();
    store.write_config(&id, ConfigFileKind::Memory, "Memory C").await.unwrap();
    let mut spec = AgentSpec::new(id, "Zari", "tpl");
    spec.work_dir = Some("/work".to_string());

    // 全部品あり（接地含む）で組んで、枠組みの全分岐を通す。
    let (prompt, stable_len) = store
        .compose_system_prompt(&spec, true, Some("R1 / R2"), Language::En)
        .await
        .unwrap();

    assert_eq!(ja_chars(&prompt), 0, "コアの枠組みに日本語が残っている:\n{prompt}");
    assert!(prompt.contains("# About you"), "{prompt}");
    assert!(prompt.contains("## Working folder"), "{prompt}");
    assert!(prompt.contains("## Grounding (Google Search)"), "{prompt}");
    assert!(prompt.contains("## Current roster"), "{prompt}");
    // 安定境界の意味は言語に依存しない（顔ぶれ・Memory は境界の外）。
    let stable: String = prompt.chars().take(stable_len).collect();
    assert!(!stable.contains("Current roster"), "顔ぶれが安定部分に入った");
}

/// 最小形（空村・接地なし・顔ぶれなし・作業フォルダなし）。
#[tokio::test]
async fn ja_minimal_prompt_is_byte_identical_to_the_pre_spec35_output() {
    let dir = temp("min");
    let store = ConfigStore::new(&dir);
    let spec = AgentSpec::new(AgentId::from("agent_01"), "ザリ", "tpl");
    let (prompt, stable_len) = store.compose_system_prompt(&spec, false, None, Language::Ja).await.unwrap();

    assert_eq!(stable_len, 172);
    assert_eq!(
        prompt,
        "# あなたについて\nあなたはこの場に参加している **ザリ** です。\n- **ザリ 自身の発言だけを書いてください。** 他のエージェントの発言を代筆・代弁してはいけません。彼らはそれぞれ自分で発言します。\n- 発言に自分の名前を書く必要はありません。誰の発言かは自動的に伝わります。\n- 相手の発言を装って会話を先に進めないでください。\n\n"
    );
}
