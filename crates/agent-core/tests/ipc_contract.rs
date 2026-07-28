//! TypeScript 側が送出する JSON が、Rust 側の型でそのまま受かることを固定するテスト。
//!
//! `apps/gui-tauri/src/types.ts` と `crates/agent-core/src/model.rs` は手で同期させる
//! 契約になっている（`data_contract.yaml` 参照）。この二言語の境界は型検査が届かないため、
//! **フロントが実際に組み立てるリテラルをそのまま貼って**デシリアライズを試す。
//!
//! ここに貼る JSON は推測で書かず、TS 側の実装から機械的に写すこと。

use agent_core::llm::{Effort, Provider};
use agent_core::model::{AgentSpec, CredentialSource, ModelTemplate};

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
        "order": 0
    }"#;

    let spec: AgentSpec = serde_json::from_str(payload).expect("GUI のエージェント定義が受かること");
    assert_eq!(spec.name, "PlannerAgent");
}

/// `types.ts` の `Provider` が取りうる全値を Rust 側が受け取れること。
#[test]
fn provider_values_match_typescript_union() {
    // types.ts: export type Provider = "open_ai_compat" | "anthropic";
    for (json, expected) in [
        (r#""open_ai_compat""#, Provider::OpenAiCompat),
        (r#""anthropic""#, Provider::Anthropic),
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
