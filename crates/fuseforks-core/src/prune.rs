//! ツール結果の即時圧縮の純機構（Spec 59 / `data_contract` の `tool_prune_contract`）。
//!
//! **HTTP も Jev も知らない。** 本文を段落へ割り、採点の束ねを作り、返ってきた
//! スコアから残す段落を決めて組み立てるところまで。採点そのものは
//! [`ParagraphScorer`] の実装（`jev.rs`）が受け持つ。
//!
//! 使う側の並びは 2 段:
//!
//! ```text
//! prepare(raw, basis)  → Prepared（割った段落・包装型なら差し替えの位置）
//!   → scorer.score(basis, prepared.candidates())      … ここだけが外へ出る
//! apply(&prepared, &scores, threshold, id)  → Option<String>（None = 全文のまま）
//! ```
//!
//! **2 段に割ったのは、間に外部呼び出しが挟まるから**。片方だけを単体で試せる
//! （偽の採点器を使わずに割り方と組み立てを確かめられる）。

use std::borrow::Cow;

/// これ未満の本文は圧縮しない（`tool_prune_contract`）。設定に出さないコード定数。
pub const MIN_CHARS: usize = 4_000;
/// これを超える段落は採点せず残す（Jev の state は約 32k トークン）。
pub const MAX_PARA: usize = 6_000;
/// これ未満の段落は次へ寄せる。
pub const MIN_PARA: usize = 120;
/// 1 呼び出しの段落数。
pub const BATCH_PARAGRAPHS: usize = 16;
/// 1 呼び出しの字数。
pub const BATCH_CHARS: usize = 12_000;
/// 基準がこれ未満なら圧縮しない。
pub const MIN_BASIS: usize = 20;
/// 包装型と認める、最長の文字列値が本文全体に占める割合。
pub const WRAPPER_SHARE: f64 = 0.6;
/// 落とした段落へ戻るための合成ツールの名前（Spec 59 D7）。
///
/// **`AgentTool` ではない。** `room_log` と同じ orchestrator 合成で、
/// `is_runnable` と実行の分岐に名前で足す（足し忘れると呼び出しが素通りし、
/// モデルが呼んだのに何も起きず本文だけ返る）。
pub const OMITTED_TOOL_NAME: &str = "omitted";
/// [`OMITTED_TOOL_NAME`] が 1 回に返す上限（`room_log` と同じ）。
pub const OMITTED_MAX_CHARS: usize = 20_000;

/// 段落ごとの関連度を返す器（Spec 59 D3）。
///
/// **コアはこの trait しか知らない。** 実装（`jev.rs`）が束ねと並列と HTTP を
/// 受け持つ。結合テストは偽の実装で圧縮の経路を丸ごと試せる。
#[async_trait::async_trait]
pub trait ParagraphScorer: Send + Sync {
    /// `paragraphs` と**同じ長さ**の列を返す。`None` は「返らなかった」= 残す
    /// （D8 の「一部のバッチだけ失敗したら、その呼び出しの段落は残す」）。
    ///
    /// # Errors
    ///
    /// 全体が失敗したとき（[`ScoreError`]）。**そのときは全文を通す** —
    /// Jev は検証器ではないので fail-open でよい。
    async fn score(&self, basis: &str, paragraphs: &[&str]) -> Result<ScoreReport, ScoreError>;

    /// 落とす閾値（設定。既定 0.2）。
    ///
    /// **採点器と一緒に差し込まれる** — どちらも `jev.json` から来る 1 組の設定で、
    /// 別々に持ち回ると「採点器は在るが閾値は既定」の状態を作れてしまう。
    fn threshold(&self) -> f32;
}

/// 採点の結果と、計器に出す実測（`tool prune:` の `calls=` / `jev_tokens=`）。
#[derive(Debug, Clone, Default)]
pub struct ScoreReport {
    /// 段落ごとの関連度。入力と同じ長さ。`None` は返らなかった段落。
    pub scores: Vec<Option<f32>>,
    /// 実際に投げた呼び出しの数（**0 なら外部送信が起きていない**）。
    pub calls: usize,
    /// 入力トークン。`TurnSpend` にも予算にも載せない — **ログだけ**（D11）。
    pub tokens: u64,
}

/// 採点が丸ごと失敗した理由（`tool prune:` の `outcome=`）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScoreError {
    /// 全体の締め切り（20 秒）を超えた。**済んだバッチの判定も捨てる** —
    /// 部分適用すると落ちる分布が呼び出しの速さで変わり、同じ入力で結果が
    /// 変わる経路が 2 つ目になる（Jev の非決定性に加えて）。
    Timeout,
    /// ターンの打ち切り（Spec 10）で待ちが切れた。
    Cancelled,
    /// HTTP の失敗・402・解釈できない応答。文面は計器に出さない（#71）。
    Failed(String),
}

impl ScoreError {
    /// ログに出す名前。
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Cancelled => "cancelled",
            Self::Failed(_) => "failed",
        }
    }
}

/// 圧縮を試みなかった理由（`tool prune:` の `outcome=`）。
///
/// **どれも「今日の挙動へ戻る」だけ**で、本文は 1 バイトも変わらない。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skip {
    /// 本文が [`MIN_CHARS`] 未満。
    UnderMin,
    /// JSON だが包装型ではない（配列型ほか）。Spec 60 の範囲。
    Structured,
    /// 基準が [`MIN_BASIS`] 未満（「了解」のような相槌のターン）。
    NoBasis,
    /// 採点できる段落が 1 つも無い（全段落が [`MAX_PARA`] 超）。
    NoCandidates,
}

impl Skip {
    /// ログに出す名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::UnderMin => "under_min",
            Self::Structured => "structured",
            Self::NoBasis => "no_basis",
            Self::NoCandidates => "no_candidates",
        }
    }
}

/// 圧縮した本文の形（`tool prune:` の `shape=`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// 素のテキスト。
    Text,
    /// JSON の包装型 — 1 つの文字列値の中身だけを差し替える（D4b）。
    JsonWrapper,
}

impl Shape {
    /// ログに出す名前。
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::JsonWrapper => "json_wrapper",
        }
    }
}

/// 割り終えた素材。採点の前後で持ち回る。
#[derive(Debug, Clone)]
pub struct Prepared {
    /// 0 始まりの段落。**本文の印・`omitted` の範囲・ログの `paragraphs=` は
    /// すべてこの index で数える**（`tool_prune_contract`）。
    paragraphs: Vec<String>,
    /// 包装型のとき、元テキストの中で差し替える範囲（バイト位置）。
    /// `None` なら素のテキストで、本文全体が差し替えの対象。
    wrapper: Option<WrapperSlot>,
    /// 割る前の本文（`omitted` が逐語で返す元。差し替えにも使う）。
    raw: String,
}

/// 包装型の差し替え先。**元テキストの上で置き換える**ので位置で持つ。
#[derive(Debug, Clone)]
struct WrapperSlot {
    /// 文字列リテラル（引用符を含む）の開始バイト位置。
    start: usize,
    /// 同じく終端（排他）。
    end: usize,
}

impl Prepared {
    /// 段落の総数。
    #[must_use]
    pub fn len(&self) -> usize {
        self.paragraphs.len()
    }

    /// 段落が 1 つも無いか。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.paragraphs.is_empty()
    }

    /// 形（ログの `shape=`）。
    #[must_use]
    pub fn shape(&self) -> Shape {
        if self.wrapper.is_some() {
            Shape::JsonWrapper
        } else {
            Shape::Text
        }
    }

    /// 段落の列（読み取り）。
    #[must_use]
    pub fn paragraphs(&self) -> &[String] {
        &self.paragraphs
    }

    /// 採点する段落の index。**[`MAX_PARA`] を超える段落は入らない**（D4）。
    #[must_use]
    pub fn candidates(&self) -> Vec<usize> {
        self.paragraphs
            .iter()
            .enumerate()
            .filter(|(_, p)| p.chars().count() <= MAX_PARA)
            .map(|(i, _)| i)
            .collect()
    }

    /// 採点の束ね（D4 の 6）。返るのは段落の index の列。
    ///
    /// **先頭から詰める決定的な割り方**で、同じ本文・同じ依頼なら同じ束ねができる。
    /// 中間帯の段落は束ね方で 0.17〜0.29 動く（P0 の実測）ので、ここが決定的で
    /// ないと同じ入力に同じ結果が返らなくなる。
    #[must_use]
    pub fn batches(&self) -> Vec<Vec<usize>> {
        let mut out: Vec<Vec<usize>> = Vec::new();
        let mut batch: Vec<usize> = Vec::new();
        let mut size = 0usize;
        for i in self.candidates() {
            let n = self.paragraphs[i].chars().count();
            if !batch.is_empty() && (batch.len() >= BATCH_PARAGRAPHS || size + n > BATCH_CHARS) {
                out.push(std::mem::take(&mut batch));
                size = 0;
            }
            batch.push(i);
            size += n;
        }
        if !batch.is_empty() {
            out.push(batch);
        }
        out
    }

    /// 圧縮前の本文（`omitted` が逐語で返す元）。
    #[must_use]
    pub fn raw(&self) -> &str {
        &self.raw
    }

    /// 採点へ渡す本文（[`Self::candidates`] と同じ並び）。
    #[must_use]
    pub fn candidate_texts(&self) -> Vec<&str> {
        self.candidates()
            .into_iter()
            .map(|i| self.paragraphs[i].as_str())
            .collect()
    }

    /// 候補ごとの採点結果を、**全段落ぶんの列へ広げる**（[`apply`] の入力）。
    ///
    /// **この変換を呼び出し側に書かせない** — 候補の index と採点の並びを
    /// 突き合わせる規律が 2 箇所に生えると、片方がずれても型では落ちない
    /// （採点が 1 つ後ろの段落に付く形の事故になる）。
    #[must_use]
    pub fn spread(&self, scored: &[Option<f32>]) -> Vec<Option<f32>> {
        let mut out = vec![None; self.paragraphs.len()];
        for (slot, index) in scored.iter().zip(self.candidates()) {
            out[index] = *slot;
        }
        out
    }
}

/// 本文を段落へ割る（D4 の 1〜5）。
///
/// # 順序が固定されている理由
///
/// 順が無いと「短い段落を柵に寄せて柵が壊れる」「寄せ続けて上限を超える」が
/// 実装ごとに割れる（Spec 59 の査読 1-2）。並びは:
///
/// 1. ``` の柵を閉じた単位として確保（柵の中の空行では割らない）
/// 2. 空行で割る
/// 3. [`MAX_PARA`] 超を「採点しない・残す」へ確定
/// 4. [`MIN_PARA`] 未満を次へ寄せる。末尾は前へ。**寄せる先が 3 で確定した
///    ものか、寄せた結果 [`MAX_PARA`] を超えるなら寄せない**
/// 5. ここで並んだものが段落（0 始まり）
///
/// # Errors
///
/// 圧縮を試みない条件（[`Skip`]）に当たったとき。
pub fn prepare(raw: &str, basis: &str) -> Result<Prepared, Skip> {
    if raw.chars().count() < MIN_CHARS {
        return Err(Skip::UnderMin);
    }
    if basis.trim().chars().count() < MIN_BASIS {
        return Err(Skip::NoBasis);
    }

    // JSON なら包装型だけを通す（D4b）。判定はローカルで、ここで落ちた本文は
    // 外部へ 1 バイトも出ない。
    let (body, wrapper) = match wrapper_of(raw) {
        WrapperCheck::NotJson => (Cow::Borrowed(raw), None),
        WrapperCheck::Wrapper { text, start, end } => {
            (Cow::Owned(text), Some(WrapperSlot { start, end }))
        }
        WrapperCheck::Other => return Err(Skip::Structured),
    };

    let paragraphs = merge_short(split_blocks(&body));
    let prepared = Prepared {
        paragraphs,
        wrapper,
        raw: raw.to_owned(),
    };
    if prepared.candidates().is_empty() {
        return Err(Skip::NoCandidates);
    }
    Ok(prepared)
}

/// D4 の 1〜2: ``` の柵を閉じた単位として確保し、柵の外だけ空行で割る。
fn split_blocks(text: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut cur: Vec<&str> = Vec::new();
    let mut in_fence = false;

    for line in text.lines() {
        let stripped = line.trim_start();
        if !in_fence && stripped.starts_with("```") {
            flush(&mut cur, &mut blocks);
            cur.push(line);
            in_fence = true;
            continue;
        }
        if in_fence {
            cur.push(line);
            if stripped.starts_with("```") {
                let block = cur.join("\n");
                blocks.push(block.trim().to_owned());
                cur.clear();
                in_fence = false;
            }
            continue;
        }
        if stripped.is_empty() {
            flush(&mut cur, &mut blocks);
        } else {
            cur.push(line);
        }
    }
    flush(&mut cur, &mut blocks);
    blocks
}

fn flush(cur: &mut Vec<&str>, blocks: &mut Vec<String>) {
    if cur.iter().any(|l| !l.trim().is_empty()) {
        blocks.push(cur.join("\n").trim().to_owned());
    }
    cur.clear();
}

/// D4 の 4: [`MIN_PARA`] 未満を次へ寄せる。末尾は前へ。
fn merge_short(blocks: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut held: Option<String> = None;

    for block in blocks {
        let mut block = block;
        if let Some(prev) = held.take() {
            // 寄せる先が 6,000 字超で確定したものか、寄せた結果が超えるなら寄せない。
            if block.chars().count() > MAX_PARA
                || prev.chars().count() + 2 + block.chars().count() > MAX_PARA
            {
                out.push(prev);
            } else {
                block = format!("{prev}\n\n{block}");
            }
        }
        if block.chars().count() < MIN_PARA {
            held = Some(block);
        } else {
            out.push(block);
        }
    }

    if let Some(tail) = held {
        let fits = out.last().is_some_and(|last| {
            last.chars().count() <= MAX_PARA
                && last.chars().count() + 2 + tail.chars().count() <= MAX_PARA
        });
        if fits {
            let last = out.last_mut().expect("fits が真なら末尾が在る");
            last.push_str("\n\n");
            last.push_str(&tail);
        } else {
            out.push(tail);
        }
    }
    out
}

/// [`wrapper_of`] の結果。
enum WrapperCheck {
    /// JSON として読めない = 素のテキスト。
    NotJson,
    /// 包装型（D4b の 4 条件をすべて満たす）。
    Wrapper {
        /// 中身の文字列（エスケープを解いたもの）。
        text: String,
        /// 元テキストの中の文字列リテラル（引用符を含む）の範囲。
        start: usize,
        end: usize,
    },
    /// JSON だが包装型ではない（配列型ほか）。Spec 60 の範囲。
    Other,
}

/// D4b: JSON の包装型か。
///
/// 条件は 4 つで、**1 つでも外れたら丸ごと諦める**（部分的に解釈して構文を
/// 壊す経路を作らない。AionUi の `toon` と同じ規律）:
///
/// - トップレベルが JSON のオブジェクト
/// - その**直下**の文字列値のうち最長が、本文全体の [`WRAPPER_SHARE`] 以上
/// - その文字列が [`MIN_CHARS`] 以上
/// - その JSON 表現が元テキストに**ちょうど 1 回だけ**現れる
///
/// 入れ子の中は探さず、配列は見ない。
fn wrapper_of(raw: &str) -> WrapperCheck {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with('{') && !trimmed.starts_with('[') {
        return WrapperCheck::NotJson;
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return WrapperCheck::NotJson;
    };
    let serde_json::Value::Object(map) = &value else {
        return WrapperCheck::Other;
    };

    let total = raw.chars().count() as f64;
    let Some(longest) = map
        .values()
        .filter_map(|v| v.as_str())
        .max_by_key(|s| s.chars().count())
    else {
        return WrapperCheck::Other;
    };
    if longest.chars().count() < MIN_CHARS || (longest.chars().count() as f64) < total * WRAPPER_SHARE
    {
        return WrapperCheck::Other;
    }

    // 元テキストの上で位置を取る。**ちょうど 1 回だけ**現れるときに限る
    // （2 回以上あると、どちらを差し替えたかが読めない）。
    let literal = serde_json::Value::String(longest.to_owned()).to_string();
    let mut hits = raw.match_indices(&literal);
    let Some((start, _)) = hits.next() else {
        return WrapperCheck::Other;
    };
    if hits.next().is_some() {
        return WrapperCheck::Other;
    }
    WrapperCheck::Wrapper {
        text: longest.to_owned(),
        start,
        end: start + literal.len(),
    }
}

/// 採点の結果から本文を組み立てる。
///
/// `scores` は段落 index ごとの関連度。`None` は「返らなかった」= **残す**
/// （[`MAX_PARA`] 超で採点しなかった段落もここに入る）。
///
/// 返るのは `None`（全文のまま = 圧縮しない）か、圧縮後の本文。
/// **全部落ちたら `None`** — 依頼と結果が噛み合っていないか採点の失敗で、
/// どちらでも全文を返すほうが害が小さい（`all_dropped`）。
#[must_use]
pub fn apply(prepared: &Prepared, scores: &[Option<f32>], threshold: f32, id: &str) -> Option<Pruned> {
    let keep: Vec<bool> = (0..prepared.len())
        .map(|i| scores.get(i).copied().flatten().is_none_or(|s| s >= threshold))
        .collect();

    // 採点した段落が 1 つ以上あり、そのすべてが閾値未満なら圧縮しない。
    // **6,000 字超は採点対象ではないので `kept` 扱い**（D6）。
    let scored: Vec<usize> = (0..prepared.len())
        .filter(|i| scores.get(*i).copied().flatten().is_some())
        .collect();
    if !scored.is_empty() && scored.iter().all(|i| !keep[*i]) {
        return None;
    }
    if keep.iter().all(|k| *k) {
        return None; // 1 つも落ちなかった
    }

    let body = render(prepared, &keep, id);
    let dropped: usize = keep.iter().filter(|k| !**k).count();
    let kept_chars = body.chars().count();
    Some(Pruned {
        body,
        kept_chars,
        dropped,
        paragraphs: prepared.len(),
        shape: prepared.shape(),
    })
}

/// 圧縮の結果（ログの欄と、モデルへ返す本文）。
#[derive(Debug, Clone)]
pub struct Pruned {
    /// モデルへ返す本文。
    pub body: String,
    /// その字数。
    pub kept_chars: usize,
    /// 落とした段落の数。
    pub dropped: usize,
    /// 段落の総数。
    pub paragraphs: usize,
    /// 形（`shape=`）。
    pub shape: Shape,
}

/// 印を入れて組み立てる（D7）。包装型なら元テキストの上で差し替える。
fn render(prepared: &Prepared, keep: &[bool], id: &str) -> String {
    let dropped_paras: Vec<usize> = (0..prepared.len()).filter(|i| !keep[*i]).collect();
    let dropped_chars: usize = dropped_paras
        .iter()
        .map(|i| prepared.paragraphs[*i].chars().count())
        .sum();
    let total_chars: usize = prepared.paragraphs.iter().map(|p| p.chars().count()).sum();

    let mut out = String::new();
    out.push_str(&format!(
        "【関連度で段落を省略しています: 全 {} 段落・{} 字のうち {} 段落・{} 字を省略。\n \
         省略した段落は `omitted` に id と段落番号を渡すと逐語で読めます。id={id}】\n\n",
        prepared.len(),
        total_chars,
        dropped_paras.len(),
        dropped_chars,
    ));

    let mut i = 0usize;
    while i < prepared.len() {
        if keep[i] {
            out.push_str(&prepared.paragraphs[i]);
            out.push_str("\n\n");
            i += 1;
            continue;
        }
        // 連続して落ちた範囲を 1 つの印に畳む。
        let from = i;
        while i < prepared.len() && !keep[i] {
            i += 1;
        }
        let to = i - 1;
        let chars: usize = (from..=to)
            .map(|k| prepared.paragraphs[k].chars().count())
            .sum();
        let span = if from == to {
            format!("段落 {from}")
        } else {
            format!("段落 {from}〜{to}")
        };
        out.push_str(&format!(
            "［… {span}・{} 段落・{chars} 字を省略 …］\n\n",
            to - from + 1
        ));
    }
    let body = out.trim_end().to_owned();

    match &prepared.wrapper {
        // 包装型は**元テキストの上で文字列リテラルだけを差し替える**。
        // 再シリアライズすると整形・キーの順序・他の値の表現が変わりうるので、
        // これが「圧縮した文字列以外は 1 バイトも変わらない」を保つ唯一の方法。
        Some(slot) => {
            let literal = serde_json::Value::String(body).to_string();
            let mut replaced = String::with_capacity(prepared.raw.len());
            replaced.push_str(&prepared.raw[..slot.start]);
            replaced.push_str(&literal);
            replaced.push_str(&prepared.raw[slot.end..]);
            replaced
        }
        None => body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(n: usize) -> String {
        "あ".repeat(n)
    }

    /// 柵の中の空行では割らない（D4 の 1）。
    #[test]
    fn a_fence_stays_whole_even_with_blank_lines_inside() {
        let text = format!(
            "見出し\n\n```rust\nfn a() {{}}\n\nfn b() {{}}\n```\n\n{}",
            long(200)
        );
        let blocks = split_blocks(&text);
        assert_eq!(blocks.len(), 3, "見出し / 柵 / 本文: {blocks:?}");
        assert!(blocks[1].contains("fn a()") && blocks[1].contains("fn b()"));
    }

    /// 短い段落は次へ寄せる。**寄せた結果 6,000 字を超えるなら寄せない**。
    #[test]
    fn a_short_paragraph_is_held_unless_the_merge_would_overflow() {
        let merged = merge_short(vec!["短い".into(), long(200)]);
        assert_eq!(merged.len(), 1, "寄せられる組は 1 つになる");

        let kept = merge_short(vec!["短い".into(), long(MAX_PARA - 1)]);
        assert_eq!(kept.len(), 2, "寄せると 6,000 字を超えるので単独で残す");
        assert_eq!(kept[0], "短い");
    }

    /// 末尾の短い段落は**前へ**寄せる（次が無い）。
    #[test]
    fn a_short_tail_goes_backwards() {
        let merged = merge_short(vec![long(200), "締め".into()]);
        assert_eq!(merged.len(), 1);
        assert!(merged[0].ends_with("締め"));
    }

    /// 6,000 字超は採点対象に入らない（D4 の 3）。
    #[test]
    fn an_oversized_paragraph_is_never_scored() {
        let text = format!("{}\n\n{}", long(MAX_PARA + 10), long(200));
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p.candidates(), vec![1], "大きすぎる段落は候補から外れる");
    }

    /// 束ねは 16 段落・12,000 字で切る（D4 の 6）。
    #[test]
    fn batches_are_capped_by_count_and_chars() {
        let text = (0..20).map(|_| long(200)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let b = p.batches();
        assert_eq!(b.len(), 2, "20 段落は 16 + 4 に割れる: {b:?}");
        assert_eq!(b[0].len(), BATCH_PARAGRAPHS);
        assert_eq!(b[1].len(), 4);
    }

    /// 基準が 20 字未満なら圧縮しない（`no_basis`）。
    #[test]
    fn a_short_basis_skips_the_whole_thing() {
        let text = long(MIN_CHARS + 10);
        assert_eq!(prepare(&text, "了解").unwrap_err(), Skip::NoBasis);
        assert_eq!(prepare(&text, "   ").unwrap_err(), Skip::NoBasis);
    }

    /// 4,000 字未満は圧縮しない。
    #[test]
    fn a_small_body_is_left_alone() {
        assert_eq!(
            prepare(&long(100), "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::UnderMin
        );
    }

    /// 配列型の JSON は素通し（Spec 60 の範囲）。
    #[test]
    fn an_array_shaped_json_is_skipped() {
        let body = format!(r#"{{"posts":["{}"]}}"#, long(MIN_CHARS + 10));
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// 包装型は中身を割り、**それ以外は 1 バイトも変えない**（D4b）。
    #[test]
    fn a_wrapper_json_is_pruned_in_place() {
        let inner = (0..20)
            .map(|i| format!("段落 {i} です。{}", long(200)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let raw = serde_json::json!({
            "breadcrumb": "Root > a > b",
            "content": inner,
            "match_mode": "exact",
        })
        .to_string();

        let p = prepare(&raw, "この依頼は 20 字以上ありますので通ります").unwrap();
        assert_eq!(p.shape(), Shape::JsonWrapper);
        assert_eq!(p.len(), 20);

        // 先頭だけ残す。
        let scores: Vec<Option<f32>> = (0..20)
            .map(|i| Some(if i == 0 { 0.9 } else { 0.01 }))
            .collect();
        let out = apply(&p, &scores, 0.2, "P1").expect("圧縮されること");
        assert_eq!(out.shape, Shape::JsonWrapper);
        assert_eq!(out.dropped, 19);

        // **JSON として読めるまま**で、他の値は逐語。
        let back: serde_json::Value = serde_json::from_str(&out.body).expect("JSON のまま");
        assert_eq!(back["breadcrumb"], "Root > a > b");
        assert_eq!(back["match_mode"], "exact");
        let content = back["content"].as_str().unwrap();
        assert!(content.contains("段落 0 です"), "残した段落は逐語");
        assert!(content.contains("省略"), "印が入る");
        assert!(!content.contains("段落 5 です"), "落とした段落は消える");
    }

    /// 全部落ちたら圧縮しない（`all_dropped`）。
    #[test]
    fn dropping_everything_falls_back_to_the_full_text() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores = vec![Some(0.01); 5];
        assert!(apply(&p, &scores, 0.2, "P1").is_none());
    }

    /// 1 つも落ちなければ圧縮しない（印だけ足して本文を太らせない）。
    #[test]
    fn keeping_everything_also_falls_back() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores = vec![Some(0.9); 5];
        assert!(apply(&p, &scores, 0.2, "P1").is_none());
    }

    /// 連続して落ちた範囲は 1 つの印に畳み、**0 始まりの index** で書く。
    #[test]
    fn a_run_of_dropped_paragraphs_becomes_one_marker() {
        let text = (0..6)
            .map(|i| format!("段落 {i}。{}", long(1000)))
            .collect::<Vec<_>>()
            .join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        // 0 と 5 だけ残す。
        let scores: Vec<Option<f32>> = (0..6)
            .map(|i| Some(if i == 0 || i == 5 { 0.9 } else { 0.01 }))
            .collect();
        let out = apply(&p, &scores, 0.2, "P2").expect("圧縮されること");
        assert!(out.body.contains("段落 1〜4・4 段落"), "畳まれた印: {}", out.body);
        assert!(out.body.contains("id=P2"));
        assert_eq!(out.dropped, 4);
    }

    /// 採点が返らなかった段落は残す（`None` = 残す。D8 の「一部のバッチだけ失敗」）。
    #[test]
    fn an_unscored_paragraph_is_kept() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        // 0 は採点が返らず残る / 1 は高得点で残る / 2〜4 が落ちる。
        let scores = vec![None, Some(0.9), Some(0.01), Some(0.01), Some(0.01)];
        let out = apply(&p, &scores, 0.2, "P1").expect("圧縮されること");
        assert_eq!(out.dropped, 3, "未採点の 0 と高得点の 1 が残る");
        assert!(out.body.contains("段落 2〜4"), "落ちたのは 2〜4: {}", out.body);
    }

    /// **採点した段落が全滅したら、未採点の段落が残っていても全文へ倒す。**
    ///
    /// D6 の `all_dropped` の判定は「採点した段落が 1 つ以上あり、そのすべてが
    /// 閾値未満」。採点が全滅している時点で、依頼と結果が噛み合っていないか
    /// 採点の失敗なので、**残った段落の由来に関わらず**全文を返すほうが害が小さい。
    #[test]
    fn all_scored_dropped_falls_back_even_if_something_unscored_remains() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores = vec![None, Some(0.01), Some(0.01), Some(0.01), Some(0.01)];
        assert!(
            apply(&p, &scores, 0.2, "P1").is_none(),
            "採点した 4 段落が全滅したので圧縮しない"
        );
    }

    /// 採点対象が 1 つも無いときは [`prepare`] が弾く（`all_dropped` にしない）。
    ///
    /// 全段落が 6,000 字超だと Jev を 1 回も呼ばないので、外部送信も起きない
    /// （ログでは `calls=0`）。
    #[test]
    fn a_body_of_only_oversized_paragraphs_is_skipped_before_scoring() {
        let text = format!("{}\n\n{}", long(MAX_PARA + 10), long(MAX_PARA + 10));
        assert_eq!(
            prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::NoCandidates
        );
    }
}
