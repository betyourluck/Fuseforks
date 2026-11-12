//! 会話への参照（Spec 75 D1・`session_binding_contract` 凍結 1・11）。
//!
//! 依頼の根は、根を作った時点の会話を [`SessionRef`] として封筒に載せて運ぶ。
//! その因果の書き込み（発話・1 往復・ターンの記録）はこの会話へ行き、
//! **書き込んだ瞬間に開いている会話へは行かない**。
//!
//! 参照は `Arc` の複製で数える。誰かが複製を持っている間、その会話には処理が残っている —
//! 受信箱で待っている封筒も、飛行中のターンも、承認待ちの波も、予定の後処理の登録も、
//! 走っている要約も、同じ数え方に乗る。会話を消す操作はこの数が 0 のときだけ通す
//! （件数の登録簿を処理の種類ごとに増やすと、どれかを数え忘れる）。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, Weak};

/// 参照の中身。会話 ID だけを持つ。生存数は `Arc` の強い参照の数。
#[derive(Debug)]
struct Ticket {
    id: String,
}

/// 会話への参照。
///
/// **`None` は保存先の無い村**（凍結 13）— 会話の単位が無いので「開いているか」の分岐をせず、
/// 常に開いている扱いになる。空の会話 ID を「全会話に共通」と読む経路は作らない。
#[derive(Debug, Clone, Default)]
pub(super) struct SessionRef(Option<Arc<Ticket>>);

impl SessionRef {
    /// 保存先の無い村の参照。
    pub(super) const fn none() -> Self {
        Self(None)
    }

    /// 会話 ID。保存先の無い村では `None`。
    pub(super) fn id(&self) -> Option<&str> {
        self.0.as_deref().map(|ticket| ticket.id.as_str())
    }

    /// ログとイベントに出す会話 ID（保存先の無い村では `-`）。
    pub(super) fn label(&self) -> &str {
        self.id().unwrap_or("-")
    }
}

/// 会話 ID ごとの参照の表。**弱い参照だけを持つ** — 表が参照を生かし続けると、
/// 処理が終わっても会話が永久に「処理中」になる。
#[derive(Debug, Default)]
pub(super) struct SessionRefs {
    tickets: Mutex<HashMap<String, Weak<Ticket>>>,
}

impl SessionRefs {
    /// 会話 `id` への参照を発行する。生きている参照があればその複製を返す。
    pub(super) fn issue(&self, id: &str) -> SessionRef {
        let mut tickets = self.lock();
        if let Some(ticket) = tickets.get(id).and_then(Weak::upgrade) {
            return SessionRef(Some(ticket));
        }
        // 死んだ弱い参照を掃除しながら入れ直す（表が会話の数だけ伸び続けないように）。
        tickets.retain(|_, weak| weak.strong_count() > 0);
        let ticket = Arc::new(Ticket { id: id.to_owned() });
        tickets.insert(id.to_owned(), Arc::downgrade(&ticket));
        SessionRef(Some(ticket))
    }

    /// 会話 `id` に処理が残っていなければ `act` を走らせる。残っていれば生きている参照の数を返す。
    ///
    /// **判定と `act` は表の lock を握ったまま行う** — 判定の後、`act`（保存先からの削除）の前に
    /// その会話への参照が新しく発行される窓を作らない（凍結 11）。
    pub(super) fn when_unreferenced<R>(&self, id: &str, act: impl FnOnce() -> R) -> Result<R, usize> {
        let mut tickets = self.lock();
        let live = tickets.get(id).map_or(0, Weak::strong_count);
        if live > 0 {
            return Err(live);
        }
        tickets.remove(id);
        Ok(act())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Weak<Ticket>>> {
        // 毒されても使う — 表は弱い参照の写しで、壊れて困る不変条件を持たない。
        self.tickets.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_count_as_live_work() {
        let refs = SessionRefs::default();
        let first = refs.issue("s1");
        let second = refs.issue("s1");
        assert_eq!(first.id(), Some("s1"));
        assert_eq!(refs.when_unreferenced("s1", || ()), Err(2));
        drop(first);
        assert_eq!(refs.when_unreferenced("s1", || ()), Err(1));
        drop(second);
        assert_eq!(refs.when_unreferenced("s1", || "deleted"), Ok("deleted"));
    }

    #[test]
    fn other_sessions_do_not_block() {
        let refs = SessionRefs::default();
        let _busy = refs.issue("busy");
        assert_eq!(refs.when_unreferenced("idle", || 1), Ok(1));
        assert_eq!(refs.when_unreferenced("busy", || 1), Err(1));
    }

    #[test]
    fn the_none_ref_has_no_id() {
        assert_eq!(SessionRef::none().id(), None);
        assert_eq!(SessionRef::none().label(), "-");
    }
}
