//! 広場ログの pull 化（Spec 22）の純機構 — 可視述語・ID 解決・表示 ID の伸長。
//!
//! ここに置くのは判断だけで、リングの読み・ツールの合成・計器は
//! orchestrator 側。`schedule.rs` / `budget.rs` と同じ分業で、テストが
//! リングやターンを組まずに書ける形を保つ。

use crate::model::{AgentId, AgentMessage, Endpoint};

/// モデルへ提示するツール名。orchestrator 合成側（`ask_*` と同じ棚）が使う。
pub const ROOM_LOG_TOOL_NAME: &str = "room_log";

/// `room_log` ツールが 1 回で返す本文の上限（文字数）。
///
/// **原文性が構造で成立するのはここまで**（`room_log_pull` 契約）。超過は
/// 打ち切りの 3 点セット（何が起きたか + 母数 + 次の手）で返し、次の手だけは
/// 従来の「発言した相手へ `ask`」へ戻る。
pub const ROOM_LOG_READ_MAX_CHARS: usize = 20_000;

/// 広場ログの可視述語。**「抜粋に載る条件」と「読める条件」は同じ問いの裏表**
/// なので、`compose_room_log`（抜粋）と `room_log` ツール（全文読み）の両方が
/// この 1 実装を呼ぶ（`room_log_pull` 契約）。別の述語を置くと答えがずれる。
///
/// 共有するのは述語**だけ** — 窓切り（`room_log_window`）と切り詰め
/// （`room_log_excerpt_chars`）は抜粋側の責務で、ここには入れない
/// （入れると「窓の外でも読める」が実装で潰れる）。
///
/// ユーザー発の発話が偽になるのは「宛先外はメッセージがあったことすら
/// 知らないべき」の凍結 — 注意書きではなく述語で守る。
pub fn is_visible_in_room_log(agent_id: &AgentId, message: &AgentMessage) -> bool {
    let involves_me =
        |endpoint: &Endpoint| matches!(endpoint, Endpoint::Agent { id } if id == agent_id);
    // エージェント発の発話だけ（ユーザー発は聴衆が選ばれている）。
    // 自分が送り手・受け手の発話は履歴に既にある。
    matches!(message.from, Endpoint::Agent { .. })
        && !involves_me(&message.from)
        && !involves_me(&message.to)
}

/// 表示 ID の伸長段階。8 字で一意ならそのまま、衝突する行だけ伸ばす
/// （git short hash と同じ方式）。段階を使い切ったら全長（UUID 文字列 36 字）。
const DISPLAY_ID_STEPS: [usize; 3] = [8, 12, 16];

/// `target` の発話 ID を、`universe` の中で一意になる最短の前置で返す。
///
/// `universe` には**ツールが解決に使う集合と同じもの**（可視述語を通した
/// リング全体の ID）を渡すこと。表示行（窓内の切れた行）だけで一意性を
/// 測ると、窓の外の可視発話と衝突した前置を表示してしまい、「表示された
/// 通りに打てば一意に決まる」（`room_log_pull` 契約）が破れる。
pub fn display_id(target: &str, universe: &[&str]) -> String {
    for len in DISPLAY_ID_STEPS {
        let prefix = char_prefix(target, len);
        if universe.iter().filter(|id| id.starts_with(prefix)).count() <= 1 {
            return prefix.to_owned();
        }
    }
    target.to_owned()
}

/// ID 前方一致の解決結果。
///
/// `NotFound` の `hidden_match` は**計器（人向けログ）専用** — モデル向けの
/// 文面は hidden の有無で変えない（`room_log_pull` 契約の文面規則。存在を
/// エラーメッセージ経由で教えると凍結が破れる）。
#[derive(Debug, PartialEq)]
pub enum RoomLogLookup<'a> {
    /// 可視集合内でちょうど 1 件に決まった。
    Found(&'a AgentMessage),
    /// 可視集合内で複数件に一致した。表示 ID は合成時点で一意なので、
    /// 起きるのは「合成後に前置一致の新発話が到着した」場合だけ
    /// （1 発話あたり 2^-32。機構は足さず計器で観測する）。
    Ambiguous {
        /// 一致した可視発話の件数。計器に載せる。候補の列挙はしない。
        candidates: usize,
    },
    /// 可視集合内に一致が無い。
    NotFound {
        /// 可視**でない**発話への一致があったか。ログの
        /// `not_visible` / `not_found` の区別にだけ使う。
        hidden_match: bool,
    },
}

/// `id_prefix` をリングから解決する。
///
/// 照合は**可視述語を通した後**の集合に対する前方一致 — 可視でない発話は
/// 曖昧性の数えにも入らない（入れると Ambiguous の文面が不可視発話の存在を
/// 漏らす）。リングは古い順・新しい順のどちらで渡してもよい（一致件数しか
/// 見ないので順序に依存しない）。
pub fn resolve_message<'a>(
    log: &'a [AgentMessage],
    agent_id: &AgentId,
    id_prefix: &str,
) -> RoomLogLookup<'a> {
    let id_prefix = id_prefix.trim();
    if id_prefix.is_empty() {
        return RoomLogLookup::NotFound {
            hidden_match: false,
        };
    }

    let mut found: Option<&AgentMessage> = None;
    let mut candidates = 0usize;
    let mut hidden_match = false;
    for message in log {
        if !message.id.starts_with(id_prefix) {
            continue;
        }
        if is_visible_in_room_log(agent_id, message) {
            candidates += 1;
            found = Some(message);
        } else {
            hidden_match = true;
        }
    }

    match candidates {
        0 => RoomLogLookup::NotFound { hidden_match },
        1 => RoomLogLookup::Found(found.expect("candidates == 1 なら必ず保持している")),
        _ => RoomLogLookup::Ambiguous { candidates },
    }
}

/// 文字数で切る前置。UUID 文字列は ASCII だが、char 境界で切る作法を保つ
/// （バイト境界の slice は将来の入力変更で panic になる）。
fn char_prefix(s: &str, len: usize) -> &str {
    match s.char_indices().nth(len) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str) -> Endpoint {
        Endpoint::Agent {
            id: AgentId::new(id),
        }
    }

    fn message(id: &str, from: Endpoint, to: Endpoint) -> AgentMessage {
        AgentMessage {
            id: id.to_owned(),
            from,
            to,
            content: "本文".to_owned(),
            tokens: 0,
            ts_ms: 0,
            hop: 0,
            co_recipients: Vec::new(),
            grounding: crate::llm::Grounding::default(),
        }
    }

    fn me() -> AgentId {
        AgentId::new("agent_me")
    }

    // ---- 可視述語 ----

    #[test]
    fn user_messages_are_invisible_even_when_addressed_to_others() {
        // ユーザーは聴衆を選んで話している。宛先が他エージェントでも落ちる。
        let m = message("id-user", Endpoint::User, agent("agent_a"));
        assert!(!is_visible_in_room_log(&me(), &m));
    }

    #[test]
    fn own_messages_are_invisible_in_both_directions() {
        let sent = message("id-1", agent("agent_me"), agent("agent_a"));
        let received = message("id-2", agent("agent_a"), agent("agent_me"));
        assert!(!is_visible_in_room_log(&me(), &sent));
        assert!(!is_visible_in_room_log(&me(), &received));
    }

    #[test]
    fn peer_to_peer_messages_are_visible() {
        let m = message("id-3", agent("agent_a"), agent("agent_b"));
        assert!(is_visible_in_room_log(&me(), &m));
    }

    // ---- ID 解決 ----

    #[test]
    fn unique_prefix_resolves() {
        let log = vec![
            message("aaaa1111-x", agent("agent_a"), agent("agent_b")),
            message("bbbb2222-x", agent("agent_a"), agent("agent_b")),
        ];
        let got = resolve_message(&log, &me(), "aaaa1111");
        assert_eq!(got, RoomLogLookup::Found(&log[0]));
    }

    #[test]
    fn colliding_prefix_is_ambiguous_without_listing() {
        let log = vec![
            message("cccc3333-1", agent("agent_a"), agent("agent_b")),
            message("cccc3333-2", agent("agent_b"), agent("agent_a")),
        ];
        let got = resolve_message(&log, &me(), "cccc3333");
        assert_eq!(got, RoomLogLookup::Ambiguous { candidates: 2 });
    }

    #[test]
    fn unknown_prefix_is_not_found() {
        let log = vec![message("aaaa1111-x", agent("agent_a"), agent("agent_b"))];
        let got = resolve_message(&log, &me(), "ffff0000");
        assert_eq!(
            got,
            RoomLogLookup::NotFound {
                hidden_match: false
            }
        );
    }

    #[test]
    fn hidden_only_match_is_not_found_with_hidden_flag() {
        // S2 の凍結: ユーザー発話は ID を直接知っていても読めない。
        // モデル向けには not_found と同じ扱いで、hidden_match は計器専用。
        let log = vec![message("dddd4444-x", Endpoint::User, agent("agent_a"))];
        let got = resolve_message(&log, &me(), "dddd4444");
        assert_eq!(got, RoomLogLookup::NotFound { hidden_match: true });
    }

    #[test]
    fn hidden_match_does_not_join_ambiguity_count() {
        // 可視 1 件 + 不可視 1 件が同じ前置 → Found。不可視を数えに入れると
        // Ambiguous の文面が不可視発話の存在を漏らす。
        let log = vec![
            message("eeee5555-1", agent("agent_a"), agent("agent_b")),
            message("eeee5555-2", Endpoint::User, agent("agent_a")),
        ];
        let got = resolve_message(&log, &me(), "eeee5555");
        assert_eq!(got, RoomLogLookup::Found(&log[0]));
    }

    #[test]
    fn empty_prefix_is_not_found() {
        let log = vec![message("aaaa1111-x", agent("agent_a"), agent("agent_b"))];
        let got = resolve_message(&log, &me(), "  ");
        assert_eq!(
            got,
            RoomLogLookup::NotFound {
                hidden_match: false
            }
        );
    }

    // ---- 表示 ID の伸長 ----

    #[test]
    fn display_id_defaults_to_eight_chars() {
        let a = "aaaa1111-2222-3333-4444-555566667777";
        let b = "bbbb1111-2222-3333-4444-555566667777";
        let universe = [a, b];
        assert_eq!(display_id(a, &universe), "aaaa1111");
    }

    #[test]
    fn colliding_pair_both_lengthen() {
        // 8 字が衝突する 2 件は両方 12 字へ伸びる（片方だけ伸ばすと、
        // 短いほうの表示が依然として 2 件に一致する）。
        let a = "aaaa1111-2222-3333-4444-555566667777";
        let b = "aaaa1111-9999-3333-4444-555566667777";
        let universe = [a, b];
        assert_eq!(display_id(a, &universe), "aaaa1111-222");
        assert_eq!(display_id(b, &universe), "aaaa1111-999");
    }

    #[test]
    fn deep_collision_falls_back_to_full_id() {
        // 16 字でも衝突する対は全長で表示する。
        let a = "aaaa1111-2222-3333-4444-555566667777";
        let b = "aaaa1111-2222-33x3-4444-555566667777";
        let universe = [a, b];
        assert_eq!(display_id(a, &universe), a);
        assert_eq!(display_id(b, &universe), b);
    }

    #[test]
    fn display_id_shorter_than_step_returns_whole() {
        let a = "short";
        let universe = [a];
        assert_eq!(display_id(a, &universe), "short");
    }
}
