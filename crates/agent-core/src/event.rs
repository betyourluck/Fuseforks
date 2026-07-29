//! コア層が外へ押し出すイベント。
//!
//! GUI 層はこの型を `broadcast` チャネルで購読し、そのまま Tauri イベントとして
//! ウィンドウへ中継する。**コア層は Tauri を知らない**ため、この境界が唯一の接点になる。
//!
//! `broadcast` を選んだ理由は、購読者が 0 でも 2 でも送信側が変わらないこと。
//! GUI が閉じていてもオーケストレーターは動き続けられる。

use serde::{Deserialize, Serialize};

use crate::error::ErrorPayload;
use crate::model::{AgentId, AgentMessage, AgentStatus};

/// UI へ通知する状態変化。
///
/// `type` タグつきの判別共用体としてシリアライズされるので、
/// TypeScript 側では discriminated union としてそのまま扱える。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum CoreEvent {
    /// エージェントのライフサイクル状態が変わった。
    #[serde(rename_all = "camelCase")]
    AgentStatusChanged {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 新しい状態。
        status: AgentStatus,
    },

    /// 稼働統計が更新された。稼働中のエージェントについて定期的に流れる。
    #[serde(rename_all = "camelCase")]
    AgentStatsUpdated {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 累積稼働秒数。
        uptime_secs: u64,
        /// 累積トークン数。
        total_tokens: u64,
    },

    /// 発話が 1 件確定した。中央ペインのログに追記される。
    ///
    /// 接地の来歴（`AgentMessage.grounding`）もこの経路で運ぶ。**専用の
    /// イベントを立てない**のは、来歴が発話に添う情報だからで、別便にすると
    /// (1) フロントが発話 ID との対応を自前で持つ必要があり、(2) 起動時の
    /// `list_messages` による再投影で来歴だけが消える。発話に載せておけば
    /// どちらも起きない。
    #[serde(rename_all = "camelCase")]
    MessageSent {
        /// 確定した発話。
        message: AgentMessage,
    },

    /// 接続関係が変わった。グラフの再描画が必要。
    #[serde(rename_all = "camelCase")]
    TopologyChanged,

    /// エージェントの実行が失敗した。
    ///
    /// 失敗はコマンドの戻り値としてではなく、この経路でも流す。
    /// エージェントの実行はコマンド呼び出しと非同期に進むため、
    /// 起動コマンドの `Result` だけでは 3 分後の失敗を UI へ届けられない。
    #[serde(rename_all = "camelCase")]
    AgentFailed {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 失敗内容。
        error: ErrorPayload,
    },

    /// 設定不備のため、要求されたモデルではなく代替バックエンドへ退避した。
    ///
    /// 退避しても応答自体は返るため、通知しないと「動いているのに設定が効いていない」
    /// 状態が延々と続く。実際に、環境変数がプロセスへ渡っていないだけの状態で
    /// エコー応答が返り続け、原因に辿り着けなくなった。
    #[serde(rename_all = "camelCase")]
    BackendDegraded {
        /// 退避したモデルテンプレート。
        model_template_id: crate::model::ModelTemplateId,
        /// 退避の理由（欠けている環境変数名など）。
        reason: String,
    },

    /// 会話がリセットされた（新規チャット。Spec 03）。
    ///
    /// 消えるのは会話ログと各エージェントの履歴だけで、稼働状態・統計・
    /// Memory.md・個別 MCP 接続は残る。フロントはこれを受けて表示中の
    /// メッセージ（`Shared.log` の投影）を空にする。
    #[serde(rename_all = "camelCase")]
    ConversationCleared,

    /// エージェントが受信した発話の処理を始めた / 終えた（入力中表示用）。
    ///
    /// 応答の生成には LLM 呼び出しとツール実行が含まれ、数十秒かかりうる。
    /// この間 UI に何も出ないと「届いていないのか、考えているのか」を
    /// 区別できない。処理の開始と終了を対で流し、UI は「入力中…」を出す。
    /// `active: false` は成功・失敗を問わず必ず流れる（出しっぱなしにしない）。
    #[serde(rename_all = "camelCase")]
    AgentTyping {
        /// 対象エージェント。
        agent_id: AgentId,
        /// true = 処理開始、false = 処理終了。
        active: bool,
    },

    /// ツールを実行した。
    ///
    /// エージェントが何をしたかは会話ログに現れない（結果はプロンプトの中で消える）。
    /// **黙って副作用だけ起きる状態を作らない**ために、実行そのものを通知する。
    #[serde(rename_all = "camelCase")]
    ToolInvoked {
        /// 実行したエージェント。
        agent_id: AgentId,
        /// ツール名。
        tool: String,
        /// 成功したか。
        ok: bool,
    },

    /// ツール実行の上限に達して打ち切った。
    #[serde(rename_all = "camelCase")]
    ToolLimitReached {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 適用された上限値。
        max_iterations: u8,
    },

    /// 転送上限に達して発話の連鎖を打ち切った。
    ///
    /// 相互接続されたエージェントは放置すると無限に往復する。打ち切りを
    /// 黙って行うと「なぜ会話が止まったか」が UI から見えなくなるため明示的に通知する。
    #[serde(rename_all = "camelCase")]
    HopLimitReached {
        /// 打ち切られた時点の発話元。
        agent_id: AgentId,
        /// 適用された上限値。
        max_hops: u8,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_as_tagged_union() {
        let event = CoreEvent::AgentStatusChanged {
            agent_id: AgentId::from("agent_01"),
            status: AgentStatus::Running,
        };

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "type": "agentStatusChanged",
                "agentId": "agent_01",
                "status": "running"
            })
        );
    }

    #[test]
    fn typing_event_serializes_with_camel_case_tag() {
        let event = CoreEvent::AgentTyping {
            agent_id: AgentId::from("agent_01"),
            active: true,
        };

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "type": "agentTyping",
                "agentId": "agent_01",
                "active": true
            })
        );
    }

    #[test]
    fn hop_limit_event_carries_the_applied_limit() {
        let event = CoreEvent::HopLimitReached {
            agent_id: AgentId::from("agent_02"),
            max_hops: 8,
        };
        let json = serde_json::to_value(&event).unwrap();

        assert_eq!(json["type"], "hopLimitReached");
        assert_eq!(json["maxHops"], 8);
    }
}
