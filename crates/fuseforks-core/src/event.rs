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
use crate::plan::PlanTaskState;

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
    ///
    /// キャッシュ率の材料（入力・キャッシュ読み取り）も**この便で**運ぶ。
    /// 以前は合計だけを載せており、率の分母と分子は `refreshAll`（起動時と
    /// 設定変更時）でしか届かなかった — 再起動後に会話だけしていると、
    /// 合計は生で増えるのに率は 0 除算で欄ごと消える。表示は実装していても、
    /// データの通り道が偶然にしか通らないなら出ないのと同じ（failures.md #33）。
    #[serde(rename_all = "camelCase")]
    AgentStatsUpdated {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 累積稼働秒数。
        uptime_secs: u64,
        /// 累積トークン数。
        total_tokens: u64,
        /// うち入力トークン数（キャッシュ率の分母）。
        prompt_tokens: u64,
        /// うちキャッシュから読まれた入力トークン数（キャッシュ率の分子）。
        cached_tokens: u64,
        /// 直近の LLM 呼び出し 1 回ぶんの入力トークン（Spec 49。輪の分子）。
        /// 乗せないと輪の更新が `refreshAll` 頼みになる（上の 2 欄と同じ理由）。
        last_prompt_tokens: Option<u64>,
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

    /// 開いているセッションが変わった（Spec 12 — 会話の永続化）。
    ///
    /// **加算的変更。** [`CoreEvent::ConversationCleared`] の意味は変えない
    /// （「会話ペインを空にせよ」という表示指示のまま）。意味を変えると既存 UI が
    /// 誤動作するので、開いたセッションの告知は別のイベントにしてある。
    ///
    /// 新規チャット・`resume_session`・`fork_session`・`continue_latest` の
    /// **すべて**がこれを出す（「今どのセッションを見ているか」の唯一の通知路）。
    /// 順序は `ConversationCleared` → `SessionSwitched` で固定。
    #[serde(rename_all = "camelCase")]
    SessionSwitched {
        /// 開いたセッションの ID。
        session_id: String,
    },

    /// ターンの使用量が `Record::Turn` として保存された（Spec 39。4 出口すべて）。
    ///
    /// **id だけを運び、数字を運ばない** — 数字は `session_stats` が集計から出す
    /// 1 経路に留める（イベントで運ぶと 2 経路目になる）。受け手は統計画面だけで、
    /// 開いていない間は読み捨てる。コストは `AgentStatsUpdated`（稼働中の個体ごとに
    /// 毎秒）の 1/数十以下。**加算的変更** — 既存 variant の意味は変えない。
    #[serde(rename_all = "camelCase")]
    TurnRecorded {
        /// ターンの主。
        agent_id: AgentId,
        /// 保存先の会話。
        session_id: String,
    },

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
    ///
    /// **Spec 27 で `reason` を足した。** それまでこの欄が運んでいたのは
    /// 名前と成否だけで、**置いた目的に対して運んでいる情報が足りていなかった** —
    /// `grep` が 5 回走ったことは分かっても、何を探したのかも、
    /// なぜ探したのかも読めない。**埋めたのは「なぜ」で、「何を」（引数）は
    /// いまも運んでいない**（引数には利用者の秘密が入りうる。`failures.md` #71）。
    #[serde(rename_all = "camelCase")]
    ToolInvoked {
        /// 実行したエージェント。
        agent_id: AgentId,
        /// ツール名。
        tool: String,
        /// **返り値が `Ok` だったか。副作用が成功したかではない。**
        ///
        /// 同梱ツールは失敗を `Err` ではなく `Ok(<エラー文>)` で返すので
        /// （`failures.md` #41）、**`ok=true` のまま失敗している行が常態**。
        /// **これは監査証跡ではない** — UI のラベルも「成功 / 失敗」ではなく
        /// 「返った / エラーで返った」にしてある（Spec 27 D11）。
        ok: bool,
        /// モデルが書いた 1 行の意図（Spec 27）。
        ///
        /// **自己申告であって監査証跡ではない。** 4 値の意味は
        /// [`crate::tool_reason::ReasonState`] を見る。
        reason: crate::tool_reason::ReasonState,
    },

    /// ツール実行の上限に達して打ち切った。
    #[serde(rename_all = "camelCase")]
    ToolLimitReached {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 適用された上限値。
        max_iterations: u8,
    },

    /// 同じツール呼び出しの繰り返しを検出し、実行せずに打ち切った
    /// （failures.md #41 の処方 1）。
    ///
    /// ツール名 + 引数 + 結果本文が完全一致で続いた回数が上限に達した状態。
    /// 上限到達（[`CoreEvent::ToolLimitReached`]）とは**別の打ち切り**で、
    /// 直し方も違う（上限は設定で上げられるが、こちらは頼み方の側の問題）。
    #[serde(rename_all = "camelCase")]
    ToolRepeatBlocked {
        /// 対象エージェント。
        agent_id: AgentId,
        /// 繰り返されたツール名。
        tool: String,
        /// 同じ結果が続いた回数（この回数の次の呼び出しを実行しなかった）。
        repeats: u32,
    },

    /// 飛行中のターンが人の指示で打ち切られた（Spec 10 — 割り込み停止）。
    ///
    /// **飛行中の中断でだけ**流れる — 未着手封筒の畳み（出口 2b、Phase 2）では
    /// 流さない（ターンは中断されていない。始まらなかっただけ）。発行者は
    /// 切られたターン自身（検知時に 1 回）なので、二重割り込み・interrupt_all・
    /// 親トークン経由が重なっても 1 本になる。打ち切りは失敗ではない —
    /// [`CoreEvent::AgentFailed`] は流れず、ステータスも Running のまま。
    #[serde(rename_all = "camelCase")]
    TurnInterrupted {
        /// 打ち切られたターンの主。
        agent_id: AgentId,
        /// ターンの通し番号（プロセス内で単調増加）。割り込みの有効範囲は
        /// この seq に束縛される（`turn_interrupt` の不変条件 6）。
        turn_seq: u64,
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

    /// plan の 1 波が配送された（Spec 08 — 波ペイン）。
    ///
    /// 配送ゼロの plan（静的差し戻し・hop 上限）では流れない — 波は
    /// 「配送が起きた単位」。順序保証は **per planId のみ**
    /// （Started → Resolved* → Finished）。再投影との突き合わせは
    /// `list_plan_waves` + planId upsert（data_contract の projection_rule）。
    #[serde(rename_all = "camelCase")]
    PlanWaveStarted {
        /// 波の同定子（プロセス内で単調増加・1 始まり）。
        plan_id: u64,
        /// 進行役。
        agent_id: AgentId,
        /// ターン内連番（stderr の `wave=` と同じ値）。
        wave: u32,
        /// 撒いたタスク（入力順）。
        tasks: Vec<crate::plan::PlanTaskAnnounced>,
        /// 波の開始時刻（epoch ms）。
        started_at_ms: u64,
    },

    /// plan の 1 タスクが解決した（Spec 08）。波内の相互順序は解決順で、保証しない。
    #[serde(rename_all = "camelCase")]
    PlanTaskResolved {
        /// 波の同定子。
        plan_id: u64,
        /// 宛先。タスクの同一性は `(planId, to)`（同一宛先の重複は静的な不正）。
        to: AgentId,
        /// 解決分類。
        state: PlanTaskState,
        /// 配送からこのタスクの解決まで（相手のキュー待ちを含む）。
        elapsed_ms: u64,
    },

    /// plan の 1 波が完了し、束ねが依頼主へ返った（Spec 08）。
    #[serde(rename_all = "camelCase")]
    PlanWaveFinished {
        /// 波の同定子。
        plan_id: u64,
        /// 束ねの文字数。
        bundle_chars: u64,
        /// 波全体の所要（= キュー待ち込みの最遅 1 体分）。
        elapsed_ms: u64,
    },

    /// plan の提案が記録された（Spec 43 — 編集窓）。配送は起きていない。
    /// 人の承認（`dispatch_plan_wave` → `PlanWaveStarted`）か破棄
    /// （`discard_plan_wave` → `PlanWaveDiscarded`）がこの後に続く。
    #[serde(rename_all = "camelCase")]
    PlanWaveProposed {
        /// 波の同定子。
        plan_id: u64,
        /// 進行役。
        agent_id: AgentId,
        /// ターン内連番。
        wave: u32,
        /// 提案されたタスク（入力順・本文つき — 編集 UI が読む提案の真実）。
        tasks: Vec<crate::plan::PlanTaskInput>,
        /// 提示時刻（epoch ms）。
        started_at_ms: u64,
    },

    /// plan の提案が破棄された（Spec 43）。配送は一度も起きていない。
    #[serde(rename_all = "camelCase")]
    PlanWaveDiscarded {
        /// 波の同定子。
        plan_id: u64,
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
    fn plan_events_serialize_with_camel_case_and_snake_case_state() {
        let event = CoreEvent::PlanTaskResolved {
            plan_id: 7,
            to: AgentId::from("agent_02"),
            state: PlanTaskState::HandedOff,
            elapsed_ms: 5210,
        };

        assert_eq!(
            serde_json::to_value(&event).unwrap(),
            serde_json::json!({
                "type": "planTaskResolved",
                "planId": 7,
                "to": "agent_02",
                // キーは camelCase、分類の値は snake_case (data_contract の enums の流儀)。
                "state": "handed_off",
                "elapsedMs": 5210
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
