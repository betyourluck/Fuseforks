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
//! apply(&prepared, &scores, threshold, id)  → Applied（Pruned 以外は全文のまま）
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
/// **正味の削減がこれに満たなければ圧縮しない**（rev4。上流の `minReductionRatio`）。
///
/// 分母は元の本文、分子は**印を入れた後の差**（落とした字数ではない）。印は
/// 150 字前後あるので、少ししか落ちない本文では**足すほうが多くなる**。
///
/// 実測（2026-09-22 の P4）: `alphaxiv__get_paper_content` は 20,954 → 20,829 で
/// 正味 0.6%、`MCP_DOCKER__fetch` は 4,096 → 3,858 で 5.8%。しかも後者は**次の周で
/// モデルが `omitted` を呼び、落とした 380 字を丸ごと読み直した** — 印と往復のぶん
/// 差し引きで増えている。**この門があれば、どちらも圧縮せずに済んでいた。**
pub const MIN_REDUCTION: f64 = 0.25;
/// 採点全体の締め切り。超えたら**済んだ束ねの判定も捨てて**全文を返す。
pub const DEADLINE: std::time::Duration = std::time::Duration::from_secs(20);
/// 同時に投げる束ねの本数。
pub const PARALLEL: usize = 4;

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
    ///
    /// **割り方そのものは [`batch`] に 1 実装**。ここはその結果を候補の index へ
    /// 写すだけ — 採点器（`jev.rs`）は `candidate_texts` の平らな列しか見ないので、
    /// 束ね方を 2 箇所に書くと片方だけずれても型では落ちない。
    #[must_use]
    pub fn batches(&self) -> Vec<Vec<usize>> {
        let candidates = self.candidates();
        batch(&self.candidate_texts())
            .into_iter()
            .map(|g| g.into_iter().map(|i| candidates[i]).collect())
            .collect()
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

/// 採点の束ねを作る（D4 の 6）。返るのは `texts` の index の列。
///
/// **先頭から詰める決定的な割り方**で、同じ並びなら同じ束ねができる。中間帯の
/// 段落は束ね方で 0.17〜0.29 動く（P0 の実測）ので、ここが決定的でないと同じ
/// 入力に同じ結果が返らなくなる。
///
/// 採点器が受け取るのは [`Prepared::candidate_texts`] の平らな列なので、
/// **この関数が束ね方の唯一の実装**（[`Prepared::batches`] もここを通る）。
#[must_use]
pub fn batch(texts: &[&str]) -> Vec<Vec<usize>> {
    let mut out: Vec<Vec<usize>> = Vec::new();
    let mut group: Vec<usize> = Vec::new();
    let mut size = 0usize;
    for (i, text) in texts.iter().enumerate() {
        let n = text.chars().count();
        if !group.is_empty() && (group.len() >= BATCH_PARAGRAPHS || size + n > BATCH_CHARS) {
            out.push(std::mem::take(&mut group));
            size = 0;
        }
        group.push(i);
        size += n;
    }
    if !group.is_empty() {
        out.push(group);
    }
    out
}

/// 採点の待ちに**締め切りと打ち切り**を掛ける（D8）。
///
/// **採点器ではなく呼ぶ側に置く。** [`ParagraphScorer`] は打ち切りトークンを
/// 受け取らないので、ここで包まないと実装ごとに規律が割れる（偽の採点器を使う
/// 結合テストでも同じ網が掛かるのが要点）。
///
/// 締め切りを超えたら `fut` を**丸ごと落とす** — 済んだ束ねの判定も一緒に捨てる。
/// 部分適用すると落ちる分布が呼び出しの速さで変わり、同じ入力で結果が変わる
/// 経路が 2 つ目になる（Jev の非決定性に加えて）。
///
/// # Errors
///
/// 締め切り超過なら [`ScoreError::Timeout`]、打ち切りなら [`ScoreError::Cancelled`]。
/// `fut` 自身の失敗はそのまま通す。
pub async fn with_deadline<F>(
    fut: F,
    deadline: std::time::Duration,
    cancel: Option<&tokio_util::sync::CancellationToken>,
) -> Result<ScoreReport, ScoreError>
where
    F: std::future::Future<Output = Result<ScoreReport, ScoreError>>,
{
    // 打ち切りが無いときは「決して起きない待ち」を置く。`select!` の腕を
    // 条件で消すと分岐が 2 通りになり、片方だけ締め切りを外す事故が作れる。
    let idle = tokio_util::sync::CancellationToken::new();
    let cancel = cancel.unwrap_or(&idle);
    tokio::select! {
        biased;
        () = cancel.cancelled() => Err(ScoreError::Cancelled),
        () = tokio::time::sleep(deadline) => Err(ScoreError::Timeout),
        out = fut => out,
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
/// 返るのは [`Applied`]。圧縮しないときは**理由つき**で返す（rev4）。
/// **全部落ちたら圧縮しない** — 依頼と結果が噛み合っていないか採点の失敗で、
/// どちらでも全文を返すほうが害が小さい（`all_dropped`）。
/// **正味の削減が [`MIN_REDUCTION`] に満たないときも圧縮しない**（`below_floor`）。
#[must_use]
pub fn apply(prepared: &Prepared, scores: &[Option<f32>], threshold: f32, id: &str) -> Applied {
    let keep: Vec<bool> = (0..prepared.len())
        .map(|i| scores.get(i).copied().flatten().is_none_or(|s| s >= threshold))
        .collect();

    // 採点した段落が 1 つ以上あり、そのすべてが閾値未満なら圧縮しない。
    // **6,000 字超は採点対象ではないので `kept` 扱い**（D6）。
    let scored: Vec<usize> = (0..prepared.len())
        .filter(|i| scores.get(*i).copied().flatten().is_some())
        .collect();
    if !scored.is_empty() && scored.iter().all(|i| !keep[*i]) {
        return Applied::AllDropped;
    }
    if keep.iter().all(|k| *k) {
        return Applied::NothingDropped;
    }

    let body = render(prepared, &keep, id);
    let raw_chars = prepared.raw.chars().count();
    let kept_chars = body.chars().count();
    // **正味で測る**（rev4）。落とした字数ではなく、印を入れた後の差で判定する —
    // 印は 150 字前後あるので、少ししか落ちない本文では足すほうが多くなる。
    let ratio = if raw_chars == 0 {
        0.0
    } else {
        (raw_chars.saturating_sub(kept_chars) as f64) / (raw_chars as f64)
    };
    if ratio < MIN_REDUCTION {
        return Applied::BelowFloor { ratio };
    }

    Applied::Pruned(Pruned {
        body,
        kept_chars,
        dropped: keep.iter().filter(|k| !**k).count(),
        paragraphs: prepared.len(),
        shape: prepared.shape(),
    })
}

/// [`apply`] の結末。**圧縮しなかった理由を畳まない**（`failures.md` #72）。
///
/// rev3 までは `Option<Pruned>` で、`all_dropped` / 1 つも落ちなかった / の 2 つが
/// 同じ `None` に落ちていた。rev4 で門が 3 つ目の理由になったので、**どれで
/// 見送ったかがログから読めないと門が効いているかを数えられない**。
#[derive(Debug, Clone)]
pub enum Applied {
    /// 圧縮した。
    Pruned(Pruned),
    /// 採点した段落がすべて閾値未満。全文へ倒す。
    AllDropped,
    /// 1 つも落ちなかった。印だけ足して本文を太らせない。
    NothingDropped,
    /// 落ちたが、**正味の削減が [`MIN_REDUCTION`] に満たない**。
    BelowFloor {
        /// 実測の削減率（ログの `ratio=`）。
        ratio: f64,
    },
}

impl Applied {
    /// ログの `outcome=` に出す語。圧縮したときは呼び出し側が `ok` を書く。
    #[must_use]
    pub fn label(&self) -> &'static str {
        match self {
            Self::Pruned(_) => "ok",
            Self::AllDropped => "all_dropped",
            Self::NothingDropped => "nothing_dropped",
            Self::BelowFloor { .. } => "below_floor",
        }
    }
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

    /// 圧縮された前提で中身を取る（取れなければ理由ごと落とす）。
    #[track_caller]
    fn pruned(applied: Applied) -> Pruned {
        match applied {
            Applied::Pruned(p) => p,
            other => panic!("圧縮されること（実際は {}）", other.label()),
        }
    }

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
    fn a_json_without_a_direct_string_value_is_skipped() {
        let body = format!(r#"{{"posts":["{}"]}}"#, long(MIN_CHARS + 10));
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// 諦める枝 2/4: **トップレベルがオブジェクトでない**（配列型 = Spec 60）。
    #[test]
    fn a_top_level_array_is_skipped() {
        let body = format!(r#"[{{"content":"{}"}}]"#, long(MIN_CHARS + 10));
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// 諦める枝 3/4: **最長の文字列が本文の 60% に届かない**。
    /// 中身が本文そのものだと言えないので、差し替えても効き目が出ない。
    #[test]
    fn a_json_whose_longest_string_is_a_minority_is_skipped() {
        // 同じ長さの文字列を 2 つ入れると、最長でも全体の半分に届かない。
        let body = format!(
            r#"{{"a":"{}","b":"{}"}}"#,
            long(MIN_CHARS + 500),
            "い".repeat(MIN_CHARS + 500)
        );
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// 諦める枝 4/4: **同じ文字列が 2 回現れる**。どちらを差し替えたか読めない。
    #[test]
    fn a_json_with_a_duplicated_literal_is_skipped() {
        let inner = long(MIN_CHARS + 100);
        let body = format!(r#"{{"a":"{inner}","b":"{inner}"}}"#);
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// 諦める枝 1/4 の裏: **最長の文字列が 4,000 字に満たない**。
    /// 本文全体は 4,000 字を超えるので [`Skip::UnderMin`] では落ちない。
    #[test]
    fn a_json_whose_string_is_below_the_floor_is_skipped() {
        let body = format!(
            r#"{{"pad":"{}","content":"{}"}}"#,
            long(600),
            long(MIN_CHARS - 500)
        );
        assert!(body.chars().count() > MIN_CHARS);
        assert_eq!(
            prepare(&body, "この依頼は 20 字以上ありますので通ります").unwrap_err(),
            Skip::Structured
        );
    }

    /// **圧縮した文字列以外は 1 バイトも変わらない**（D4b の golden）。
    ///
    /// 整形・キーの順序・他の値・末尾の改行を**わざと serde の既定から外して**
    /// おく。再シリアライズで組み直す実装ならここで必ず落ちる（末尾の
    /// `assert_ne!` が、この golden が本当に判別していることの対照）。
    #[test]
    fn everything_outside_the_compressed_string_is_byte_identical() {
        let inner = (0..20)
            .map(|i| format!("段落 {i} です。{}", long(200)))
            .collect::<Vec<_>>()
            .join("

");
        let literal = serde_json::Value::String(inner.clone()).to_string();
        // キーは辞書順でない / 2 字下げ / 末尾に改行。
        let raw = format!(
            "{{
  \"zzz_last\": \"Root > a > b\",
  \"content\": {literal},
  \"aaa_first\": \"exact\"
}}
"
        );
        let start = raw.find(&literal).expect("文字列リテラルが在る");
        let (prefix, suffix) = (&raw[..start], &raw[start + literal.len()..]);

        let p = prepare(&raw, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores: Vec<Option<f32>> = (0..p.len())
            .map(|i| Some(if i < 10 { 0.05 } else { 0.9 }))
            .collect();
        let out = pruned(apply(&p, &scores, 0.2, "P1"));

        assert!(
            out.body.starts_with(prefix),
            "前半が変わった:
{:?}
{:?}",
            &out.body[..prefix.len().min(out.body.len())],
            prefix
        );
        assert!(out.body.ends_with(suffix), "後半が変わった（末尾の改行を含む）");

        // 対照: 読んで書き戻すと**この形にはならない**。だからこの golden は
        // 「差し替えを再シリアライズへ変える」変異を捕まえる。
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_ne!(serde_json::to_string(&parsed).unwrap(), raw);
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
        let out = pruned(apply(&p, &scores, 0.2, "P1"));
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
        assert_eq!(apply(&p, &scores, 0.2, "P1").label(), "all_dropped");
    }

    /// 1 つも落ちなければ圧縮しない（印だけ足して本文を太らせない）。
    #[test]
    fn keeping_everything_also_falls_back() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores = vec![Some(0.9); 5];
        assert_eq!(apply(&p, &scores, 0.2, "P1").label(), "nothing_dropped");
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
        let out = pruned(apply(&p, &scores, 0.2, "P2"));
        assert!(out.body.contains("段落 1〜4・4 段落"), "畳まれた印: {}", out.body);
        assert!(out.body.contains("id=P2"));
        assert_eq!(out.dropped, 4);
    }

    /// **正味の削減が 25% に満たなければ圧縮しない**（rev4 の門）。
    ///
    /// 20 段落のうち 1 段落だけ落とすと、落ちるのは約 5% で印が 150 字前後。
    /// 実機（2026-09-22 の P4）で観測したのはまさにこの形で、`omitted` の
    /// 読み直しまで含めると**差し引き増えていた**。
    #[test]
    fn a_small_gain_is_not_worth_the_marker() {
        let text = (0..20)
            .map(|i| format!("段落 {i}。{}", long(300)))
            .collect::<Vec<_>>()
            .join("

");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        let scores: Vec<Option<f32>> = (0..p.len())
            .map(|i| Some(if i == 3 { 0.01 } else { 0.9 }))
            .collect();

        let verdict = apply(&p, &scores, 0.2, "P1");
        assert_eq!(verdict.label(), "below_floor");
        match verdict {
            Applied::BelowFloor { ratio } => {
                assert!(ratio > 0.0 && ratio < MIN_REDUCTION, "実測の削減率: {ratio}");
            }
            other => panic!("BelowFloor を期待したが {}", other.label()),
        }
    }

    /// 門を超えれば圧縮する（上の対照）。**同じ本文で落とす数だけ変える。**
    #[test]
    fn a_large_gain_passes_the_floor() {
        let text = (0..20)
            .map(|i| format!("段落 {i}。{}", long(300)))
            .collect::<Vec<_>>()
            .join("

");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        // 半分落とす。印を足しても 25% は超える。
        let scores: Vec<Option<f32>> = (0..p.len())
            .map(|i| Some(if i < 10 { 0.01 } else { 0.9 }))
            .collect();

        let out = pruned(apply(&p, &scores, 0.2, "P1"));
        assert_eq!(out.dropped, 10);
        let raw_chars = text.chars().count();
        let ratio = (raw_chars - out.kept_chars) as f64 / raw_chars as f64;
        assert!(ratio >= MIN_REDUCTION, "削減率 {ratio} が門を超えること");
    }

    /// 採点が返らなかった段落は残す（`None` = 残す。D8 の「一部のバッチだけ失敗」）。
    #[test]
    fn an_unscored_paragraph_is_kept() {
        let text = (0..5).map(|_| long(1000)).collect::<Vec<_>>().join("\n\n");
        let p = prepare(&text, "この依頼は 20 字以上ありますので通ります").unwrap();
        // 0 は採点が返らず残る / 1 は高得点で残る / 2〜4 が落ちる。
        let scores = vec![None, Some(0.9), Some(0.01), Some(0.01), Some(0.01)];
        let out = pruned(apply(&p, &scores, 0.2, "P1"));
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
        assert_eq!(
            apply(&p, &scores, 0.2, "P1").label(),
            "all_dropped",
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
    /// 締め切りの中で終われば、結果はそのまま通る。
    #[tokio::test]
    async fn with_deadline_passes_a_result_through() {
        let got = with_deadline(
            async { Ok(ScoreReport { scores: vec![Some(0.9)], calls: 1, tokens: 7 }) },
            std::time::Duration::from_secs(5),
            None,
        )
        .await
        .expect("間に合う");
        assert_eq!(got.calls, 1);
        assert_eq!(got.tokens, 7);
    }

    /// 締め切りを超えたら [`ScoreError::Timeout`]。**済んだ分も捨てる** —
    /// ここでは future ごと落ちるので、部分適用の経路が構造的に存在しない。
    #[tokio::test]
    async fn with_deadline_gives_up_on_a_slow_scorer() {
        let err = with_deadline(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(ScoreReport::default())
            },
            std::time::Duration::from_millis(10),
            None,
        )
        .await
        .unwrap_err();
        assert_eq!(err, ScoreError::Timeout);
        assert_eq!(err.label(), "timeout");
    }

    /// 打ち切り（Spec 10）は締め切りを待たずに切る。**`biased` で打ち切りを先に
    /// 見る**ので、両方成立していても分類は `cancelled`（cancel が最優先）。
    #[tokio::test]
    async fn with_deadline_stops_on_a_cancelled_turn() {
        let token = tokio_util::sync::CancellationToken::new();
        token.cancel();
        let err = with_deadline(
            async {
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                Ok(ScoreReport::default())
            },
            std::time::Duration::from_millis(10),
            Some(&token),
        )
        .await
        .unwrap_err();
        assert_eq!(err, ScoreError::Cancelled);
    }

    /// 束ねは **16 段落・12,000 字**で切る。1 本目が 16 で切れ、残りが 2 本目。
    #[test]
    fn batch_splits_at_sixteen_paragraphs() {
        let owned: Vec<String> = (0..20).map(|i| format!("段落 {i}")).collect();
        let texts: Vec<&str> = owned.iter().map(String::as_str).collect();
        let got = batch(&texts);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].len(), BATCH_PARAGRAPHS);
        assert_eq!(got[1], vec![16, 17, 18, 19]);
    }

    /// 字数でも切る。**1 段落で上限を超えても単独の束ねとして残す**
    /// （6,000 字超は候補から外れているので、ここへ来るのは 12,000 字未満）。
    #[test]
    fn batch_splits_at_twelve_thousand_chars() {
        let big = "あ".repeat(5_000);
        let texts = vec![big.as_str(), big.as_str(), big.as_str()];
        let got = batch(&texts);
        assert_eq!(got, vec![vec![0, 1], vec![2]], "10,000 で 1 本目が満ちる");
    }
}
