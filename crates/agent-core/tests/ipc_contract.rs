//! TypeScript 側が送出する JSON が、Rust 側の型でそのまま受かることを固定するテスト。
//!
//! `apps/gui-tauri/src/types.ts` と `crates/agent-core/src/model.rs` は手で同期させる
//! 契約になっている（`data_contract.yaml` 参照）。この二言語の境界は型検査が届かないため、
//! **フロントが実際に組み立てるリテラルをそのまま貼って**デシリアライズを試す。
//!
//! ここに貼る JSON は推測で書かず、TS 側の実装から機械的に写すこと。

use agent_core::llm::{Effort, Provider};
use agent_core::model::{
    AgentId, AgentMessage, AgentSnapshot, AgentSpec, AgentStatus, CredentialSource, Endpoint,
    ModelTemplate,
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
            "contextLength",
            "credential",
            "effort",
            "googleSearch",
            "id",
            "maxOutputTokens",
            "maxRetries",
            "model",
            "name",
            "provider",
            "requestTimeoutSecs",
            "temperature",
            "useTools",
        ],
        "ModelTemplate のフィールドが変わった"
    );

    assert_eq!(
        wire_keys(&AgentSpec::new("agent_01", "PlannerAgent", "tpl")),
        vec![
            "connectedAgents",
            "enabledTools",
            "hearsRoomLog",
            "id",
            "maxToolIterations",
            "modelTemplateId",
            "name",
            "order",
            "ragSources",
            "workDir",
        ],
        "AgentSpec のフィールドが変わった"
    );

    let snapshot = AgentSnapshot {
        id: AgentId::from("agent_01"),
        name: "PlannerAgent".into(),
        model: "gpt-4o".into(),
        model_template_id: "tpl".into(),
        status: AgentStatus::Idle,
        uptime_secs: 0,
        total_tokens: 0,
        cached_tokens: 0,
        rag_sources: Vec::new(),
        connected_agents: Vec::new(),
        order: 0,
        work_dir: None,
        max_tool_iterations: None,
        enabled_tools: None,
        hears_room_log: true,
        last_error: None,
    };
    assert_eq!(
        wire_keys(&snapshot),
        vec![
            "cachedTokens",
            "connectedAgents",
            "enabledTools",
            "hearsRoomLog",
            "id",
            "lastError",
            "maxToolIterations",
            "model",
            "modelTemplateId",
            "name",
            "order",
            "ragSources",
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
}

/// `types.ts` の `Provider` が取りうる全値を Rust 側が受け取れること。
#[test]
fn provider_values_match_typescript_union() {
    // types.ts: export type Provider = "open_ai_compat" | "anthropic" | "gemini";
    for (json, expected) in [
        (r#""open_ai_compat""#, Provider::OpenAiCompat),
        (r#""anthropic""#, Provider::Anthropic),
        (r#""gemini""#, Provider::Gemini),
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

    for provider in [Provider::OpenAiCompat, Provider::Anthropic] {
        let json = serde_json::to_string(&provider).unwrap();
        let back: Provider = serde_json::from_str(&json).unwrap();
        assert_eq!(back, provider, "往復で壊れる: {json}");
    }
}
