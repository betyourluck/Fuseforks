//! プロンプトに載る文脈。広場ログ（Spec 03 / 22）と入退室の通知（Spec 06）。
//!
//! **可視述語は 1 実装**（room_log::is_visible_in_room_log）を抜粋と pull が
//! 共有する。片方だけに分岐を足すと、宛先付きの発話が広場ログのオプトアウトを
//! 構造的に迂回する（failures.md #89 で実際に踏んだ）。
//!
//! ここで組んだものは **Role::System では積まない** — adapter が配列のどこに
//! あっても先頭へ畳むので、可変なものを System に置くと前方一致がそこで
//! 切れる（#45）。最終 user 発話へ畳むのは build_prompt の責務。

use super::*;

/// 入退室の通知を組み立てる（Spec 06 P1）。
///
/// System 発の発話（`set_status` が記録する入退室）だけを抽出する。
/// [`compose_room_log`] と別の関数なのは gate が違うから — 広場ログは
/// `hearsRoomLog` でオプトアウトできるが、こちらは全員に届く。
///
/// # 可視範囲
///
/// **広場ログと同じ窓**（`room_log_window` 件の遡り）に従う。窓から押し出された
/// 通知は見えなくなるが、情報が消えるのではなく時間軸だけが落ちる —
/// 現在の状態は顔ぶれ（P1.5）が常に持っている（顔ぶれが権威、通知が語り）。
pub(super) async fn compose_presence_notices(
    shared: &Shared,
    config: &OrchestratorConfig,
    has_roster: bool,
) -> Option<String> {
    if config.room_log_window == 0 {
        return None;
    }

    let lines: Vec<String> = {
        let log = shared.log.read().await;
        // 生ログの直近 window 件の中から System 発を拾う。
        // 「System 発だけを window 件」にはしない — それだと古い通知が
        // 会話に押し出されずいつまでも残り、「窓に従う」という契約が嘘になる。
        log.iter()
            .rev()
            .take(config.room_log_window)
            // **from だけでなく to も見る。** System 発には入退室のほかに
            // 「予定の配送」（System → Agent）が居り、from だけで拾うと
            // **他人宛の依頼文が全員のプロンプトへ入る**（実機で観測。
            // 広場ログを切った個体が自分宛でない予定の本文を読めていた）。
            //
            // ここは `hears_room_log` でオプトアウトできない経路なので、
            // **宛先付きの発話が混ざるとオプトアウトを迂回する**。
            // 通知は「場に向けた告知」= User 宛だけ、が正しい射程。
            .filter(|message| message.from == Endpoint::System && message.to == Endpoint::User)
            .map(|message| format!("- {}", message.content))
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    };

    if lines.is_empty() {
        return None;
    }

    // 「顔ぶれが権威、通知が語り」の案内は、顔ぶれの節が実際に出ている
    // 相手にだけ書く。接続 0 体の個体に存在しない節を指させない。
    let authority_note = if has_roster {
        "\n\n現在の状態は「今の顔ぶれ」が正です。"
    } else {
        ""
    };
    Some(format!(
        "## 入退室（新しいものが下）\n{}{authority_note}",
        lines.join("\n")
    ))
}

/// 「居合わせた会話」を組み立てる（広場ログ）。
///
/// # なぜ「聞こえる」と「反応する」を分けるのか
///
/// 各エージェントの履歴は私的で、他人の発言は一切見えなかった。だが村の広場では、
/// 話は自分宛でなくても聞こえる。かといって**聞こえるたびに反応させると
/// 反響が起き、トークンが人数分燃える**（failures.md #20）。
/// そこで配送（＝ターンの発火）は宛先だけに保ち、**可視性だけを共有する**。
/// これがこの関数の役割で、ここに載る発話はターンを発火させない。
///
/// # 何を載せないか
///
/// **ユーザーが宛先を選んだ発話は載せない。** ユーザーは聴衆を選んで話しており、
/// 広場ログがその選択を迂回する裏口になってはいけない
/// （「宛先外のエージェントはメッセージがあったことすら知らないべき」）。
/// 自分が送り手・受け手である発話も載せない — それは既に自分の履歴にある。
/// Endpoint の表示名（UI と同じ語彙 — 二重化を作らない）。
///
/// 抜粋（`compose_room_log`）と全文読み（`read_room_log`）が名前の解決を
/// 共有する。削除済みエージェントは ID で表す。
fn endpoint_label(world: &crate::world::World, endpoint: &Endpoint) -> String {
    match endpoint {
        Endpoint::User => "ユーザー".to_owned(),
        Endpoint::System => "Fuseforks".to_owned(),
        Endpoint::Agent { id } => world
            .agent(id)
            .map(|record| record.spec.name.clone())
            .unwrap_or_else(|_| id.to_string()),
        // 窓口が外へ返した答えは他のサーヴァントの広場ログに載る
        // （`mcp_server_contract` 凍結 9 — User 宛の返答と同じ扱い）ので、
        // 宛先の名前もここで解決される。**封筒と同じ述語を通す。**
        Endpoint::External { client } => external_label(world, client).to_owned(),
    }
}

/// `room_log` ツールの定義（Spec 22）。
///
/// スキーマは提示される個体の毎ターンに乗る固定費なので最小に保つ。
pub(super) fn room_log_tool_spec() -> ToolSpec {
    ToolSpec {
        name: crate::room_log::ROOM_LOG_TOOL_NAME.into(),
        description: "「この場で交わされていた会話」の切れた抜粋を全文で読む。\
                      抜粋の行頭に表示されている ID をそのまま指定する。"
            .into(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "発話 ID（抜粋の行頭に [ ] で表示されているもの）"
                }
            },
            "required": ["id"]
        }),
    }
}

/// `room_log` ツールの本体（Spec 22 — `room_log_pull` 契約）。
/// 抜粋の行頭 ID から発話の全文を返す。
///
/// 失敗も本文で返す（同梱ツールと同じ作法 — RepeatGuard が数えられる形）。
/// 可視でない発話は「見つからない」と**同じ文面** — 存在をエラーメッセージ
/// 経由で教えると「宛先外はメッセージがあったことすら知らないべき」の凍結が
/// 破れる。区別は計器（人向けログ）にだけ出す。
pub(super) async fn read_room_log(
    shared: &Shared,
    agent_id: &AgentId,
    call: &crate::llm::ToolCall,
) -> String {
    use crate::room_log::{ROOM_LOG_READ_MAX_CHARS, RoomLogLookup};

    let requested = call
        .args
        .get("id")
        .and_then(|value| value.as_str())
        .unwrap_or("")
        .trim()
        .to_owned();

    // 解決の結果。`Found` の借用をロックの外へ持ち出さないため、要る欄だけ複製する。
    enum Resolved {
        Full(Endpoint, Endpoint, String),
        Ambiguous,
        NotFound { hidden: bool },
    }
    let resolved = {
        let log = shared.log.read().await;
        match crate::room_log::resolve_message(&log, agent_id, &requested) {
            RoomLogLookup::Found(message) => Resolved::Full(
                message.from.clone(),
                message.to.clone(),
                message.content.clone(),
            ),
            RoomLogLookup::Ambiguous { .. } => Resolved::Ambiguous,
            RoomLogLookup::NotFound { hidden_match } => Resolved::NotFound {
                hidden: hidden_match,
            },
        }
    };

    let (outcome, body) = match resolved {
        Resolved::Full(from, to, content) => {
            let header = {
                let world = shared.world.read().await;
                format!(
                    "【送り手: {} → {}】",
                    endpoint_label(&world, &from),
                    endpoint_label(&world, &to)
                )
            };
            // 「全 N 字」の数え方は抜粋（compose_room_log）と揃える —
            // 抜粋の母数とここの母数がずれると、切れていない発話を
            // 「まだ続きがある」と読ませる。
            let full_chars = content.trim().chars().count();
            if full_chars > ROOM_LOG_READ_MAX_CHARS {
                // 打ち切りの 3 点セット（何が起きたか + 母数 + 次の手）。
                // 次の手だけは `ask` へ戻る — 原文性が構造で成立するのは
                // 上限まで（`room_log_pull` 契約）。
                let shown = truncate_chars(&content, ROOM_LOG_READ_MAX_CHARS);
                (
                    "truncated",
                    format!(
                        "{header}\n{shown}\n\n（全 {full_chars} 字中、先頭 \
                         {ROOM_LOG_READ_MAX_CHARS} 字までを表示しました。続きが必要なら、\
                         発言した相手へ `ask` で尋ねてください）"
                    ),
                )
            } else {
                ("ok", format!("{header}\n{}", content.trim()))
            }
        }
        Resolved::Ambiguous => (
            "ambiguous",
            // 候補は列挙しない — 列挙は可視でない発話の ID が漏れる余地を作る。
            "その ID では発言を一意に特定できませんでした。\
             抜粋の行頭に表示されている ID をそのまま指定してください。"
                .to_owned(),
        ),
        Resolved::NotFound { hidden } => (
            // モデル向け文面は hidden の有無で変えない。変えると存在が漏れる。
            if hidden { "not_visible" } else { "not_found" },
            "指定された ID に一致する発言は見つかりませんでした。\
             抜粋の行頭に表示されている ID をそのまま指定してください。"
                .to_owned(),
        ),
    };

    // 計器（`room_log_pull` 契約の D5）。本文はログへ書かない（#71 —
    // モデル出力を運ぶ計器は秘密の転送経路になる）。not_visible / not_found の
    // 区別はこの行だけで、モデル向け文面は同一。
    note!(
        "room_log read: agent={agent_id} id={} outcome={outcome} chars={}",
        if requested.is_empty() { "-" } else { &requested },
        body.chars().count(),
    );
    body
}

pub(super) async fn compose_room_log(
    shared: &Shared,
    agent_id: &AgentId,
    config: &OrchestratorConfig,
) -> Option<String> {
    if config.room_log_window == 0 {
        return None;
    }

    // 取っ手（表示 ID）は**切れた行だけ**に付ける（Spec 22 の D2 — 全文が
    // 既に見えている行に取っ手は要らず、「ID がある = 続きがある」が行の形で
    // 読める）。一意性は**ツールが解決に使う集合と同じもの**（可視述語を通した
    // リング全体）で測る — 窓内だけで測ると、窓の外の可視発話と衝突した前置を
    // 表示してしまい「表示された通りに打てば一意」が破れる。ロックを持って
    // いる間に計算するのは、リング全 ID の複製を避けるため。
    let (overheard, handles): (Vec<AgentMessage>, Vec<Option<String>>) = {
        let log = shared.log.read().await;
        let overheard: Vec<AgentMessage> = log
            .iter()
            .rev()
            // 述語は room_log.rs の 1 実装（Spec 22 — 抜粋に載る条件と
            // `room_log` ツールで読める条件は同じ問いの裏表）。
            .filter(|message| crate::room_log::is_visible_in_room_log(agent_id, message))
            .take(config.room_log_window)
            .cloned()
            .collect();
        let universe: Vec<&str> = log
            .iter()
            .filter(|message| crate::room_log::is_visible_in_room_log(agent_id, message))
            .map(|message| message.id.as_str())
            .collect();
        let handles = overheard
            .iter()
            .map(|message| {
                (message.content.trim().chars().count() > config.room_log_excerpt_chars)
                    .then(|| crate::room_log::display_id(&message.id, &universe))
            })
            .collect();
        (overheard, handles)
    };

    if overheard.is_empty() {
        return None;
    }

    let world = shared.world.read().await;
    let label = |endpoint: &Endpoint| endpoint_label(&world, endpoint);

    // 収集は新しい順なので、表示は古い順へ戻す。
    //
    // 切った行には**元の長さ**を添える。`…` だけでは「省略された」のか
    // 「相手がそこで言い終えた」のかをモデルが区別できず、抜粋を発言の全体だと
    // 読む（実機で観測、2026-08-04）。#55 の一般化 1 —「黙って切らない」は
    // 「切ったと言う」ではなく「切る前の量を言う」。
    let mut clipped = 0usize;
    let lines: Vec<String> = overheard
        .iter()
        .zip(handles.iter())
        .rev()
        .map(|(message, handle)| {
            let full_chars = message.content.trim().chars().count();
            let excerpt = truncate_chars(&message.content, config.room_log_excerpt_chars);
            let (prefix, tail) = match handle {
                Some(id) => {
                    clipped += 1;
                    (format!("[{id}] "), format!("（全 {full_chars} 字）"))
                }
                None => (String::new(), String::new()),
            };
            format!(
                "- {prefix}{} → {}: {excerpt}{tail}",
                label(&message.from),
                label(&message.to),
            )
        })
        .collect();

    // 次の手は行頭の ID を `room_log` ツールへ渡すこと（Spec 22 — 旧文面
    // 「本人へ ask」は撤回。上限超・リング溢れのときだけツール側の返答が
    // ask へ誘導する）。書かないと同じ抜粋を眺め続けることになる（#44）。
    let notice = if clipped > 0 {
        format!(
            "うち {clipped} 件は途中で切れています（行末の「全 N 字」が元の長さ）。\
             **全文が要るなら、行頭の [ ] 内の ID をそのまま `room_log` ツールに\
             指定してください。**"
        )
    } else {
        String::new()
    };

    Some(format!(
        "## この場で交わされていた会話\n\
         あなた宛ではありませんが、同じ場に居たので聞こえていた発言です。\
         **返事をする義務はありません。** 文脈として使ってください。\n\
         直近 {} 件まで・各行は先頭 {} 字の抜粋です。{notice}\n{}",
        config.room_log_window,
        config.room_log_excerpt_chars,
        lines.join("\n")
    ))
}


/// 文字数で切り詰める。マルチバイト文字の途中で切らない。
fn truncate_chars(text: &str, limit: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= limit {
        return trimmed.to_owned();
    }
    let head: String = trimmed.chars().take(limit).collect();
    format!("{head}…")
}
