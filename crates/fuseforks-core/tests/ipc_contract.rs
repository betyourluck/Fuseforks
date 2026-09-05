//! TypeScript 側が送出する JSON が、Rust 側の型でそのまま受かることを固定するテスト。
//!
//! `apps/gui-tauri/src/types.ts` と `crates/fuseforks-core/src/model.rs` は手で同期させる
//! 契約になっている（`data_contract.yaml` 参照）。この二言語の境界は型検査が届かないため、
//! **フロントが実際に組み立てるリテラルをそのまま貼って**デシリアライズを試す。
//!
//! ここに貼る JSON は推測で書かず、TS 側の実装から機械的に写すこと。

use fuseforks_core::llm::{Effort, Provider};
use fuseforks_core::model::{
    AgentId, AgentMessage, AgentRole, AgentRoleDefaults, AgentSnapshot, AgentSpec, AgentStatus,
    CredentialSource, Endpoint, ModelTemplate,
};

/// 直列化されたキー集合を並べ替えて返す。
fn wire_keys<T: serde::Serialize>(value: &T) -> Vec<String> {
    let json = serde_json::to_value(value).expect("直列化できること");
    let mut keys: Vec<String> = json
        .as_object()
        .expect("オブジェクトであること")
        .keys()
        .cloned()
        .collect();
    keys.sort();
    keys
}

/// ワイヤに出るフィールド集合を固定する。
///
/// **このテストが落ちたら、まず `apps/gui-tauri/src/types.ts` を直すこと。**
/// 型検査は二言語の境界に届かないので、フィールドを増減しても TS 側は黙って古いままになる。
/// ここで落として「TS も見ろ」と強制するのが目的で、期待値の更新だけして通すのは
/// このテストの意味を消す行為（failures.md #9 と同じ形）。
#[test]
fn wire_field_sets_are_frozen() {
    assert_eq!(
        wire_keys(&ModelTemplate::new("tpl", "既定", "gpt-4o")),
        vec![
            "baseUrl",
            // 単価（Spec 41 P1）。Option + serde(default) で既存の world.json も読める。
            "cacheReadPerMtok",
            "cacheWrite1hPerMtok",
            "cacheWritePerMtok",
            "contextLength",
            "credential",
            "effort",
            // Gemini の URL context（Spec 48 D3）。types.ts も同時に足すこと（P2）。
            "geminiUrlContext",
            "googleSearch",
            "id",
            "inputPerMtok",
            "maxOutputTokens",
            "maxRetries",
            "metaWebSearch",
            "model",
            "name",
            "openaiReasoningPro",
            "openaiWebSearch",
            "outputPerMtok",
            "perplexityFetchUrl",
            "perplexityFinanceSearch",
            "perplexityPeopleSearch",
            "perplexityWebSearch",
            "pricingAsOf",
            "provider",
            "requestTimeoutSecs",
            "temperature",
            "useTools",
            "xaiWebSearch",
            "xaiXSearch",
        ],
        "ModelTemplate のフィールドが変わった"
    );

    assert_eq!(
        wire_keys(&AgentSpec::new("agent_01", "PlannerAgent", "tpl")),
        vec![
            "allowHandoff",
            "batchStart",
            "connectedAgents",
            "enabledTools",
            // Spec 51。所属（参照）。types.ts と snapshotToSpec も同時に足すこと。
            "groupId",
            "hearsRoomLog",
            "id",
            "maxToolIterations",
            "modelTemplateId",
            "name",
            "order",
            "planReview",
            "ragSources",
            "roleId",
            "workDir",
        ],
        "AgentSpec のフィールドが変わった"
    );

    // 役職（Spec 14）。**雛形とラベルの 2 役**を分けて凍結する — defaults に
    // 欄が増えると「役職を選んだら何が入るか」が変わり、入れてはいけない欄
    // （connectedAgents / workDir …）が紛れ込む経路にもなる。
    assert_eq!(
        wire_keys(&AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: String::new(),
            color: None,
            defaults: AgentRoleDefaults::default(),
        }),
        // color は skip_serializing_if なので、None では現れない。
        vec!["defaults", "description", "id", "name"],
        "Role のフィールドが変わった"
    );

    // 値が入った状態でも固定する。**空だけを固定していると、任意フィールドを
    // 足しても凍結が反応しない**（AgentMessage の coRecipients / grounding が
    // 実際にこの穴を通り抜けた）。
    assert_eq!(
        wire_keys(&AgentRole {
            id: "researcher".into(),
            name: "調査役".into(),
            description: "説明".into(),
            color: Some(fuseforks_core::RoleColor::Teal),
            defaults: AgentRoleDefaults::default(),
        }),
        vec!["color", "defaults", "description", "id", "name"],
        "Role の省略フィールドが変わった"
    );
    // 色の値は lowercase（TS の union リテラルと一致させる）。
    assert_eq!(
        serde_json::to_value(fuseforks_core::RoleColor::Violet).unwrap(),
        serde_json::json!("violet")
    );

    assert_eq!(
        wire_keys(&AgentRoleDefaults::default()),
        vec![
            "construct",
            "enabledTools",
            "maxToolIterations",
            "modelTemplateId",
        ],
        concat!(
            "RoleDefaults のフィールドが変わった。",
            "**入れてはいけない 6 欄が紛れていないか確認すること** — ",
            "connectedAgents（線は人が引く）/ workDir（端末ごとに違う絶対パス）/ ",
            "ragSources（Spec 18 D10 で workDir と同じ性質になった）/ ",
            "order / batchStart / hearsRoomLog（役職でなく運用の選択）"
        )
    );

    let snapshot = AgentSnapshot {
        id: AgentId::from("agent_01"),
        name: "PlannerAgent".into(),
        model: "gpt-4o".into(),
        model_template_id: "tpl".into(),
        status: AgentStatus::Idle,
        uptime_secs: 0,
        total_tokens: 0,
        prompt_tokens: 0,
        cached_tokens: 0,
        last_prompt_tokens: None,
        rag_sources: Vec::new(),
        connected_agents: Vec::new(),
        order: 0,
        work_dir: None,
        max_tool_iterations: None,
        enabled_tools: None,
        hears_room_log: true,
        allow_handoff: true,
        plan_review: false,
        batch_start: true,
        role_id: None,
        group_id: None,
        last_error: None,
    };
    assert_eq!(
        wire_keys(&snapshot),
        vec![
            "allowHandoff",
            "batchStart",
            "cachedTokens",
            "connectedAgents",
            "enabledTools",
            // Spec 51。所属（参照）。types.ts と snapshotToSpec も同時に足すこと。
            "groupId",
            "hearsRoomLog",
            "id",
            "lastError",
            // Spec 49。直近の呼び出しの入力（累計ではない）。types.ts も同時に足すこと。
            "lastPromptTokens",
            "maxToolIterations",
            "model",
            "modelTemplateId",
            "name",
            "order",
            "planReview",
            "promptTokens",
            "ragSources",
            "roleId",
            "status",
            "totalTokens",
            "uptimeSecs",
            "workDir",
        ],
        "AgentSnapshot のフィールドが変わった"
    );

    let message = AgentMessage::new(Endpoint::User, Endpoint::System, "本文", 0);
    assert_eq!(
        wire_keys(&message),
        vec!["content", "from", "hop", "id", "to", "tokens", "tsMs"],
        "AgentMessage のフィールドが変わった"
    );

    // 省略されるフィールド（`skip_serializing_if`）は、空の値では現れない。
    // 空だけを固定していると、**任意フィールドを足しても凍結が反応しない** —
    // 実際、`coRecipients` と `grounding` はこの穴を通り抜けられる形で入った。
    // 値を入れた状態でもう一度固定して、TS 側への追従を強制する。
    let mut filled = AgentMessage::new(Endpoint::User, Endpoint::System, "本文", 0);
    filled.co_recipients = vec![AgentId::from("agent_a")];
    filled.grounding = fuseforks_core::llm::Grounding {
        queries: vec!["検索語".into()],
        sources: vec![fuseforks_core::llm::GroundingSource {
            uri: "https://example.test/a".into(),
            title: "表題".into(),
        }],
        engine: fuseforks_core::llm::GroundingEngine::default(),
    };
    assert_eq!(
        wire_keys(&filled),
        vec![
            "coRecipients",
            "content",
            "from",
            "grounding",
            "hop",
            "id",
            "to",
            "tokens",
            "tsMs",
        ],
        "AgentMessage の省略フィールドが変わった"
    );
    assert_eq!(
        wire_keys(&filled.grounding),
        vec!["engine", "queries", "sources"],
        "Grounding のフィールドが変わった"
    );
    assert_eq!(
        wire_keys(&filled.grounding.sources[0]),
        vec!["title", "uri"],
        "GroundingSource のフィールドが変わった"
    );

    // `Endpoint` のワイヤ形（Spec 25 で 4 つ目の variant が入った）。
    // **タグは `kind` で、内容は同じ階層に平坦化される** — TS 側の判別共用体が
    // これに一致していないと、外部依頼の発話だけ描画が落ちる。
    assert_eq!(
        serde_json::to_value(Endpoint::User).unwrap(),
        serde_json::json!({ "kind": "user" })
    );
    assert_eq!(
        serde_json::to_value(Endpoint::System).unwrap(),
        serde_json::json!({ "kind": "system" })
    );
    assert_eq!(
        serde_json::to_value(Endpoint::Agent {
            id: AgentId::from("agent_a")
        })
        .unwrap(),
        serde_json::json!({ "kind": "agent", "id": "agent_a" })
    );
    assert_eq!(
        serde_json::to_value(Endpoint::External {
            client: "Claude Code".to_owned()
        })
        .unwrap(),
        serde_json::json!({ "kind": "external", "client": "Claude Code" }),
        "Endpoint::External のワイヤ形が変わった（types.ts の Endpoint を直すこと）"
    );
}

/// 波の記録のワイヤ形を固定する（Spec 08 — 波ペイン）。
///
/// `list_plan_waves` と `CoreEvent` の plan 3 種がこの型で境界を渡る。
/// 落ちたら types.ts の `PlanWaveRecord` / `PlanTaskRecord` / `PlanTaskState` を直すこと。
#[test]
fn plan_wave_wire_fields_are_frozen() {
    use fuseforks_core::plan::{PlanTaskState, PlanWaveStore};

    let mut store = PlanWaveStore::default();
    let id = store.begin_wave(AgentId::from("agent_1"), 1, &[(AgentId::from("agent_2"), 12)], 55);
    store.resolve_task(id, &AgentId::from("agent_2"), PlanTaskState::Answered, 3);
    store.finish_wave(id, 40, 9);

    let wave = &store.list()[0];
    assert_eq!(
        wire_keys(wave),
        vec![
            "agentId",
            "bundleChars",
            "elapsedMs",
            "planId",
            "startedAtMs",
            "state",
            "tasks",
            "wave",
        ],
        "PlanWaveRecord のフィールドが変わった"
    );
    assert_eq!(
        wire_keys(&wave.tasks[0]),
        vec!["elapsedMs", "msgChars", "state", "to"],
        "PlanTaskRecord のフィールドが変わった（配送済みは message を持たない）"
    );
    // 分類の値は snake_case（TS の union リテラルと一致させる）。
    assert_eq!(
        serde_json::to_value(wave.tasks[0].state).unwrap(),
        serde_json::json!("answered")
    );
    // 波レベル状態も snake_case（Spec 43）。
    assert_eq!(
        serde_json::to_value(wave.state).unwrap(),
        serde_json::json!("dispatched")
    );

    // 提案（pending）は本文を持つ — `message` は skip_serializing_if なので、
    // **値の入った状態でも固定する**（上の AgentMessage の教訓と同じ穴を
    // 通り抜けさせない）。
    let pending_id = store.begin_pending_wave(
        AgentId::from("agent_1"),
        2,
        &[(AgentId::from("agent_2"), "調べて".to_owned())],
        56,
    );
    let pending = store
        .list()
        .into_iter()
        .find(|w| w.plan_id == pending_id)
        .unwrap();
    assert_eq!(
        wire_keys(&pending.tasks[0]),
        vec!["elapsedMs", "message", "msgChars", "state", "to"],
        "pending の PlanTaskRecord は本文を持つ（編集 UI が読む提案の真実）"
    );
    assert_eq!(
        serde_json::to_value(pending.state).unwrap(),
        serde_json::json!("pending")
    );
}

/// `ModelTemplateDialog.vue` の `blank()` が組み立てる新規テンプレート。
#[test]
fn new_model_template_payload_deserializes() {
    let payload = r#"{
        "id": "template",
        "name": "新しいテンプレート",
        "baseUrl": "https://api.openai.com/v1",
        "model": "gpt-4o",
        "contextLength": 128000,
        "temperature": null,
        "maxOutputTokens": 4096,
        "credential": "unset",
        "provider": null,
        "useTools": true,
        "effort": null,
        "googleSearch": false,
        "requestTimeoutSecs": 120,
        "maxRetries": 3
    }"#;

    let template: ModelTemplate =
        serde_json::from_str(payload).expect("GUI の新規テンプレートが受かること");

    assert_eq!(template.base_url, "https://api.openai.com/v1");
    assert_eq!(template.temperature, None);
    assert_eq!(template.provider, None);
    assert_eq!(template.credential, CredentialSource::Unset);
    assert!(template.use_tools);
    assert!(!template.google_search);
}

/// `googleSearch` の追加前に保存された `world.json` がそのまま読めること。
///
/// 既存の村（ザリ・ジェミー・ロボットくん）の設定はこの形で保存されている。
/// 既定は「接地なし」— 設定を触っていない利用者のリクエストの形を変えない。
#[test]
fn model_template_saved_before_google_search_still_loads() {
    let payload = r#"{
        "id": "template",
        "name": "gemini-3.6-flash",
        "baseUrl": "https://generativelanguage.googleapis.com/v1beta",
        "model": "gemini-3.6-flash",
        "contextLength": 128000,
        "maxOutputTokens": 4096,
        "credential": "keyring",
        "provider": null,
        "useTools": true
    }"#;

    let template: ModelTemplate = serde_json::from_str(payload).expect("旧テンプレートが受かること");

    assert!(!template.google_search, "既定は接地なし");
    assert_eq!(template.provider, None, "自動判定のまま = OpenAI 互換経路");
}

/// `types.ts` の `CredentialSource` が取りうる全値を Rust 側が受け取れること。
#[test]
fn credential_source_values_match_typescript_union() {
    // types.ts: export type CredentialSource = "unset" | "not_required" | "keyring";
    for (json, expected) in [
        (r#""unset""#, CredentialSource::Unset),
        (r#""not_required""#, CredentialSource::NotRequired),
        (r#""keyring""#, CredentialSource::Keyring),
    ] {
        let parsed: CredentialSource = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("CredentialSource {json} が受からない: {e}"));
        assert_eq!(parsed, expected);
    }
}

/// 旧版の `"none"` は「認証不要」ではなく「未設定」として読む。
///
/// 「認証不要」と読み替えると、キーを入れていないだけのテンプレートが
/// 認証ヘッダ無しで外部へ送られ、サーバー側の 401 になる。
#[test]
fn the_legacy_none_value_maps_to_unset_not_to_not_required() {
    let parsed: CredentialSource = serde_json::from_str(r#""none""#).unwrap();
    assert_eq!(parsed, CredentialSource::Unset);
    assert!(parsed.is_unresolved());
}

/// `AgentList.vue` の `submitNew()` が組み立てるエージェント定義。
#[test]
fn new_agent_spec_payload_deserializes() {
    let payload = r#"{
        "id": "planneragent",
        "name": "PlannerAgent",
        "modelTemplateId": "template",
        "ragSources": [],
        "connectedAgents": [],
        "order": 0,
        "workDir": null,
        "maxToolIterations": null,
        "enabledTools": null,
        "hearsRoomLog": true
    }"#;

    let spec: AgentSpec = serde_json::from_str(payload).expect("GUI のエージェント定義が受かること");
    assert_eq!(spec.name, "PlannerAgent");
    assert_eq!(spec.work_dir, None);
    assert_eq!(spec.max_tool_iterations, None);
    assert_eq!(spec.enabled_tools, None, "新規作成の保存値は null（既定に従う）");
}

/// `workDir` 導入前に保存された `world.json` も開けること。
#[test]
fn agent_spec_saved_before_work_dir_still_loads() {
    let legacy = r#"{
        "id": "old_agent",
        "name": "旧エージェント",
        "modelTemplateId": "template",
        "ragSources": [],
        "connectedAgents": [],
        "order": 0
    }"#;

    let spec: AgentSpec = serde_json::from_str(legacy).expect("旧形式も開けること");
    assert_eq!(spec.work_dir, None);
    assert!(spec.hears_room_log, "フィールド不在は true（現状互換）");
    // 一括起動の既定も真。偽にすると、更新した利用者の ▶ が初回に
    // 何もしないボタンになる（既存の村が全員「対象外」で復元される）。
    assert!(spec.batch_start, "フィールド不在は true（既存の村は全員が対象）");
}

/// `types.ts` の `Provider` が取りうる全値を Rust 側が受け取れること。
///
/// **ワイヤを 1 本足したらここへ 1 行足す。** 型検査は二言語の境界に届かないので、
/// 抜けても Rust も TS も黙って通り、**そのテンプレートを保存した村だけが
/// 読み込みで落ちる**。実際 4〜6 本目（xai / openai / meta）の 3 値は
/// この一覧から漏れたまま P4 まで来ていた。
#[test]
fn provider_values_match_typescript_union() {
    // types.ts: export type Provider =
    //   "open_ai_compat" | "anthropic" | "gemini"
    //   | "xai_responses" | "open_ai_responses" | "meta_responses";
    for (json, expected) in [
        (r#""open_ai_compat""#, Provider::OpenAiCompat),
        (r#""anthropic""#, Provider::Anthropic),
        (r#""gemini""#, Provider::Gemini),
        (r#""xai_responses""#, Provider::XaiResponses),
        (r#""open_ai_responses""#, Provider::OpenAiResponses),
        (r#""meta_responses""#, Provider::MetaResponses),
    ] {
        let parsed: Provider = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("Provider {json} が受からない: {e}"));
        assert_eq!(parsed, expected);
    }
}

/// `types.ts` の `Effort` が取りうる全値を Rust 側が受け取れること。
#[test]
fn effort_values_match_typescript_union() {
    // types.ts: export type Effort = "low" | "medium" | "high" | "xhigh" | "max";
    for (json, expected) in [
        (r#""low""#, Effort::Low),
        (r#""medium""#, Effort::Medium),
        (r#""high""#, Effort::High),
        (r#""xhigh""#, Effort::XHigh),
        (r#""max""#, Effort::Max),
    ] {
        let parsed: Effort = serde_json::from_str(json)
            .unwrap_or_else(|e| panic!("Effort {json} が受からない: {e}"));
        assert_eq!(parsed, expected);
    }
}

/// 列挙は往復すること。片道だけ通っても、保存した設定を読み戻せない。
#[test]
fn enums_round_trip_through_json() {
    for effort in [
        Effort::Low,
        Effort::Medium,
        Effort::High,
        Effort::XHigh,
        Effort::Max,
    ] {
        let json = serde_json::to_string(&effort).unwrap();
        let back: Effort = serde_json::from_str(&json).unwrap();
        assert_eq!(back, effort, "往復で壊れる: {json}");
    }

    // **全 variant を並べる。** 片道（受け取り）は上のテストが見ているが、
    // 往復が壊れると「保存した設定を読み戻せない」形で出る。
    for provider in [
        Provider::OpenAiCompat,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::XaiResponses,
        Provider::OpenAiResponses,
        Provider::MetaResponses,
    ] {
        let json = serde_json::to_string(&provider).unwrap();
        let back: Provider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, provider, "往復で壊れる: {json}");
    }
}

/// 会話（セッション）のワイヤ形を固定する（Spec 12 P3）。
///
/// `SessionSummary` / `SessionMeta` / `ForkPoint` は `types.ts` と手で同期させる
/// 契約にある。`parentId` / `forkedAtSeq` は**分岐で生まれたときだけ出る**
/// （TS 側では省略可能）ので、両方の形を固定する。
#[test]
fn session_wire_fields_are_frozen() {
    use fuseforks_core::{ForkPoint, SessionMeta, SessionSummary};

    let plain = SessionSummary {
        id: "1785600000000-abcd1234".into(),
        meta: SessionMeta {
            title: "黒板の運用を決めたい".into(),
            created_at: 1_785_600_000_000,
            updated_at: 1_785_600_060_000,
            parent_id: None,
            forked_at_seq: None,
            record_count: 12,
        },
    };
    assert_eq!(wire_keys(&plain), vec!["id", "meta"]);
    assert_eq!(
        wire_keys(&plain.meta),
        vec!["createdAt", "recordCount", "title", "updatedAt"],
        "分岐していない会話は parentId / forkedAtSeq を出さない"
    );

    let forked = SessionMeta {
        parent_id: Some("1785600000000-abcd1234".into()),
        forked_at_seq: Some(7),
        ..plain.meta.clone()
    };
    assert_eq!(
        wire_keys(&forked),
        vec![
            "createdAt",
            "forkedAtSeq",
            "parentId",
            "recordCount",
            "title",
            "updatedAt",
        ],
        "SessionMeta のフィールドが変わった"
    );

    let point = ForkPoint {
        at_seq: 6,
        preview: "最初の依頼です".into(),
        text: "最初の依頼\nです".into(),
        to: Some(AgentId::from("agent_01")),
        ts_ms: 1_785_600_000_000,
    };
    assert_eq!(
        wire_keys(&point),
        vec!["atSeq", "preview", "text", "to", "tsMs"],
        "ForkPoint のフィールドが変わった"
    );
    assert_eq!(
        wire_keys(&ForkPoint { to: None, ..point }),
        vec!["atSeq", "preview", "text", "tsMs"],
        "宛先がエージェントでなければ to は出さない"
    );
}
