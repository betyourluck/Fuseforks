//! 同梱 `blackboard` ツール（Spec 55）— 村の黒板の書き込みをツール 1 本へ寄せる。
//!
//! # なぜツールか
//!
//! 黒板の規約（置き場・綴り・状態）をサーヴァントへ伝える経路は条例だけだった。
//! 条例は利用者が任意に書く文章で、新しい村では空。コアのプロンプトは黒板に
//! 1 字も触れないので、**条例に節が無い村では付箋が 1 枚も書かれない**。
//! 規約の運び手を、条例からこのツールの説明文と schema へ移す。
//!
//! # モデルに書かせないもの
//!
//! モデルが渡すのは仕事名・本文・行き先の状態だけ。
//! `blackboard/<状態>/<agent_id> - <仕事名>.md` はここが組む。
//! 「書けるのは自分の付箋だけ」は [`ToolContext::agent_id`] で決まる構造で、文言ではない。
//!
//! # 提示
//!
//! `BUNDLED_TOOL_NAMES` の外に居る（`rag` と同じ棚）。門は [`is_offered`] の述語 1 本で、
//! `spec_for` と `call` の先頭が同じ関数を呼ぶ。
//!
//! # 読みは読み手と共有する
//!
//! 盤面は [`read_blackboard_dir`]（黒板タブと同じ読み手）で引く。走査を 2 つ目に
//! 書くと、画面に出ている付箋をツールが見つけられない形で割れる。
//! **3 値（[`STATES`]）を知っているのは書き手のこのツールだけ** — 読み手は状態名を
//! 検査しない。

use std::path::Path;

use async_trait::async_trait;
use serde_json::Value;

use crate::blackboard::{
    BLACKBOARD_DIR, BlackboardNote, NOTE_EXTENSION, NOTE_SEPARATOR, read_blackboard_dir,
    split_note_name,
};
use crate::compute::spawn_rayon;
use crate::error::CoreResult;
use crate::llm::ToolSpec;
use crate::model::AgentId;
use crate::tool::{AgentTool, ToolContext};
use crate::tools::fs::{MAX_OUTPUT_CHARS, resolve_creatable, resolve_in_work_dir, work_dir_missing};
use crate::world::Language;

/// 仕事の状態 = `blackboard/` 直下のフォルダ名。閉じた 3 値（Spec 54 rev3）。
/// 並びは盤面の並び。フロントの `blackboardLanes.ts` の `STATES` と同じ綴り。
pub const STATES: [&str; 3] = ["doing", "on-hold", "done"];

/// 新しい付箋の置き場（`write` は常にここへ作る）。
const STATE_NEW: &str = STATES[0];
/// 動かせない状態。ここにある付箋は `move` も `append` も受けない。
const STATE_FROZEN: &str = STATES[2];

/// 仕事名の上限（code point）。
const TASK_NAME_MAX_CHARS: usize = 80;

/// 付箋 1 枚の上限（字）。**`file read` の 1 回の返却上限と同じ** —
/// ツールで書いた付箋は `read` で必ず全文が返るので、続きを取る引数も分割の案内も要らない。
const NOTE_MAX_CHARS: usize = MAX_OUTPUT_CHARS;

/// 仕事名に入れられない文字（Windows のファイル名の禁止文字）。制御文字と合わせて `_` へ寄せる。
const FORBIDDEN_NAME_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

/// 提示と実行の門（Spec 55 凍結 1）。**`spec_for` と `call` が同じこの関数を呼ぶ。**
///
/// 提示集合は既に実行フィルタとして働く（`spec_for` が `None` を返したツールは
/// `executable` に入らない）が、書き込み系なので `call` でも 1 回見る。
fn is_offered(ctx: &ToolContext) -> bool {
    ctx.work_dir.is_some() && ctx.uses_blackboard
}

/// 仕事名を受け付けられない理由。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TaskNameError {
    /// 正規化すると何も残らない。
    Empty,
    /// 上限超え（正規化後の字数）。
    TooLong(usize),
}

/// 仕事名の正規化（Spec 55 凍結 4）。**全 op がこの 1 本を通す** — `write` と `read` で
/// 寄せ方が違うと、作った付箋を同じ名前で引けなくなる。
///
/// - 前後の空白と、末尾の `.` を落とす（Windows は末尾の `.` と空白を黙って落とすので、
///   残すと「作った名前」と「ディスク上の名前」が食い違う）
/// - 末尾の `.md` を落とす（盤面の表示は拡張子なしだが、モデルは拡張子つきで返してくることが
///   ある。落とさないと `調査.md.md` ができ、`調査` では引けない）
/// - `\ / : * ? " < > |` と制御文字を `_` へ
/// - 空は拒否 / 上限 80 字（code point）。**切り詰めない** — 黙って切ると、別の仕事が
///   同じ名前へ落ちる
///
/// 冪等（正規化済みの名前を通しても変わらない）。
pub(crate) fn normalize_task_name(raw: &str) -> Result<String, TaskNameError> {
    let mut rest = raw.trim();
    loop {
        let before = rest;
        rest = rest.trim_end_matches(|c: char| c == '.' || c.is_whitespace());
        let cut = rest.len().saturating_sub(NOTE_EXTENSION.len());
        if rest.len() >= NOTE_EXTENSION.len()
            && rest.is_char_boundary(cut)
            && rest[cut..].eq_ignore_ascii_case(NOTE_EXTENSION)
        {
            rest = &rest[..cut];
        }
        if rest == before {
            break;
        }
    }
    let name: String = rest
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN_NAME_CHARS.contains(&c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    if name.is_empty() {
        return Err(TaskNameError::Empty);
    }
    let chars = name.chars().count();
    if chars > TASK_NAME_MAX_CHARS {
        return Err(TaskNameError::TooLong(chars));
    }
    Ok(name)
}

/// 付箋のファイル名を組む。
fn note_file_name(owner: &str, task: &str) -> String {
    format!("{owner}{NOTE_SEPARATOR}{task}{NOTE_EXTENSION}")
}

/// 付箋の場所。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Place {
    /// 3 状態のどれか。
    State(&'static str),
    /// `blackboard/` 直下の平置き（状態なし）。
    Root,
    /// 3 値の外のフォルダ。
    Other(String),
}

impl Place {
    fn of(note: &BlackboardNote) -> Self {
        match note.state.as_deref() {
            None => Self::Root,
            Some(folder) => match STATES.iter().find(|s| **s == folder) {
                Some(state) => Self::State(state),
                None => Self::Other(folder.to_owned()),
            },
        }
    }

    /// 盤面の並び（`doing → on-hold → done → 直下 → その他`）。
    fn order(&self) -> (usize, &str) {
        match self {
            Self::State(state) => (STATES.iter().position(|s| s == state).unwrap_or(0), ""),
            Self::Root => (STATES.len(), ""),
            Self::Other(folder) => (STATES.len() + 1, folder.as_str()),
        }
    }

    /// 計器へ出す綴り。**フォルダ名は出さない**（3 値の外の名前は任意の文字列）。
    fn log(&self) -> &'static str {
        match self {
            Self::State(state) => state,
            Self::Root => "root",
            Self::Other(_) => "other",
        }
    }

    /// モデルへ見せる呼び名。
    fn label(&self, language: Language) -> String {
        match self {
            Self::State(state) => format!("`{state}`"),
            Self::Root => language
                .pick("状態なし（`blackboard/` 直下）", "unfiled (directly under `blackboard/`)")
                .to_owned(),
            Self::Other(folder) => pick(
                language,
                format!("その他のフォルダ `{folder}`"),
                format!("other folder `{folder}`"),
            ),
        }
    }

    /// 作業フォルダからの相対パス（フォルダ）。
    fn rel_dir(&self) -> String {
        match self {
            Self::State(state) => format!("{BLACKBOARD_DIR}/{state}"),
            Self::Root => BLACKBOARD_DIR.to_owned(),
            Self::Other(folder) => format!("{BLACKBOARD_DIR}/{folder}"),
        }
    }
}

/// op の結末（計器の `outcome=`。閉じた列挙）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    Ok,
    /// 同じ状態への `move`。何もしていない。
    Noop,
    /// `write` の重複（create-only）。
    Exists,
    /// 他人の付箋への `append` / `move` / `remove`。
    NotOwner,
    /// `done` にある付箋への `move` / `append`。**`op=` と組で読む。**
    Frozen,
    NotFound,
    /// 付箋の上限（12,000 字）超え。
    TooLarge,
    /// 門（作業フォルダなし / 黒板を使わない設定）。
    Disabled,
    /// 引数の誤り（op・仕事名・`to`・本文の欠け）。
    Invalid,
    /// 3 状態の外（直下・「その他」）にある付箋への `append` / `move`。
    Misplaced,
    /// 同じ `id + 仕事名` が 2 箇所以上にある（`run` や手作業でできる形）。
    Ambiguous,
    /// ディスクの失敗。
    Error,
}

impl Outcome {
    fn log(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Noop => "noop",
            Self::Exists => "exists",
            Self::NotOwner => "not_owner",
            Self::Frozen => "frozen",
            Self::NotFound => "not_found",
            Self::TooLarge => "too_large",
            Self::Disabled => "disabled",
            Self::Invalid => "invalid",
            Self::Misplaced => "misplaced",
            Self::Ambiguous => "ambiguous",
            Self::Error => "error",
        }
    }
}

/// op 1 回の結果。`text` はモデルへ、残りは計器へ。
#[derive(Debug)]
pub(crate) struct OpResult {
    pub(crate) text: String,
    pub(crate) outcome: Outcome,
    /// 触った（触ろうとした）付箋の場所。無ければ `-`。
    state: &'static str,
    /// `move` の宛先。
    to: Option<&'static str>,
}

impl OpResult {
    fn new(outcome: Outcome, text: String) -> Self {
        Self { text, outcome, state: "-", to: None }
    }

    fn at(mut self, place: &Place) -> Self {
        self.state = place.log();
        self
    }
}

/// モデルが渡した引数。
#[derive(Debug, Default, Clone)]
struct Request {
    op: String,
    name: Option<String>,
    body: Option<String>,
    to: Option<String>,
    owner: Option<String>,
}

/// 計器へ出す op 名。**モデルが書いた文字列をそのまま出さない**（閉じた 6 つの外は `unknown`）。
fn op_log(op: &str) -> &'static str {
    match op {
        "list" => "list",
        "read" => "read",
        "write" => "write",
        "append" => "append",
        "move" => "move",
        "remove" => "remove",
        _ => "unknown",
    }
}

fn pick(language: Language, ja: String, en: String) -> String {
    match language {
        Language::Ja => ja,
        Language::En => en,
    }
}

/// op を実行する側が共有する文脈。
struct Board<'a> {
    work_dir: &'a Path,
    me: &'a str,
    names: &'a [(AgentId, String)],
    language: Language,
    notes: &'a [BlackboardNote],
}

impl Board<'_> {
    /// `id（表示名）`。村に居ない id はそう書く（旧形式の付箋 / 消えた個体の残り）。
    fn owner_label(&self, owner: &str) -> String {
        let name = self.names.iter().find(|(id, _)| id.as_str() == owner).map(|(_, name)| name.as_str());
        match (name, owner == self.me) {
            (Some(name), true) => pick(
                self.language,
                format!("{owner}（{name}・自分）"),
                format!("{owner} ({name}, you)"),
            ),
            (Some(name), false) => pick(self.language, format!("{owner}（{name}）"), format!("{owner} ({name})")),
            (None, _) => pick(
                self.language,
                format!("{owner}（村に居ない持ち主）"),
                format!("{owner} (no such servant in the village)"),
            ),
        }
    }

    /// `owner` の `task` の付箋。盤面の並びで返す。
    fn find(&self, owner: &str, task: &str) -> Vec<&BlackboardNote> {
        let mut hits: Vec<&BlackboardNote> = self
            .notes
            .iter()
            .filter(|note| split_note_name(&note.name) == Some((owner, task)))
            .collect();
        hits.sort_by(|a, b| Place::of(a).order().cmp(&Place::of(b).order()));
        hits
    }

    /// 同じ仕事名の、他人の付箋の持ち主（id 順・重複なし）。
    fn other_owners_of(&self, task: &str) -> Vec<String> {
        let mut owners: Vec<String> = self
            .notes
            .iter()
            .filter_map(|note| split_note_name(&note.name))
            .filter(|(owner, name)| *name == task && *owner != self.me)
            .map(|(owner, _)| owner.to_owned())
            .collect();
        owners.sort();
        owners.dedup();
        owners
    }

    fn places(&self, hits: &[&BlackboardNote]) -> String {
        hits.iter().map(|note| Place::of(note).label(self.language)).collect::<Vec<_>>().join(" / ")
    }

    /// `owner` 引数を id へ解く。id にも表示名にも当たらなければそのまま使う
    /// （旧形式の付箋は前半が表示名なので、その綴りで引ける）。
    fn resolve_owner(&self, raw: &str) -> String {
        let raw = raw.trim();
        if self.names.iter().any(|(id, _)| id.as_str() == raw) {
            return raw.to_owned();
        }
        self.names
            .iter()
            .find(|(_, name)| name == raw)
            .map(|(id, _)| id.as_str().to_owned())
            .unwrap_or_else(|| raw.to_owned())
    }

    /// 自分の付箋を 1 枚に決める。決まらなければ、そのまま返せる結果。
    ///
    /// `append` / `move` / `remove` の共通の入口。**他人の付箋は持ち主を名指しして断る** —
    /// op は仕事名しか取らないので、他人の付箋を狙う形は 2 つ: `owner` に他人を書く /
    /// 盤面で見た他人の仕事名をそのまま渡す。
    fn own_notes(&self, req: &Request, task: &str) -> Result<Vec<&BlackboardNote>, OpResult> {
        if let Some(owner) = req.owner.as_deref() {
            let owner = self.resolve_owner(owner);
            if owner != self.me {
                return Err(self.not_owner(&[owner]));
            }
        }
        let hits = self.find(self.me, task);
        if !hits.is_empty() {
            return Ok(hits);
        }
        let others = self.other_owners_of(task);
        if !others.is_empty() {
            return Err(self.not_owner(&others));
        }
        Err(self.not_found(self.me, task))
    }

    fn not_owner(&self, owners: &[String]) -> OpResult {
        let labels = owners.iter().map(|o| self.owner_label(o)).collect::<Vec<_>>().join(" / ");
        OpResult::new(
            Outcome::NotOwner,
            pick(
                self.language,
                format!(
                    "その付箋の持ち主は {labels} です。書けるのは自分の付箋だけです（何も変えていません）。\
                     読むだけなら read に `owner` を付けてください。自分のメモは自分の付箋へ append してください。"
                ),
                format!(
                    "That note belongs to {labels}. You can only write to your own notes (nothing was changed). \
                     To just read it, use read with `owner`. Put your own memo into your own note with append."
                ),
            ),
        )
    }

    fn not_found(&self, owner: &str, task: &str) -> OpResult {
        let label = self.owner_label(owner);
        OpResult::new(
            Outcome::NotFound,
            pick(
                self.language,
                format!(
                    "{label} の付箋「{task}」はありません。list で盤面の仕事名を確かめてください。\
                     新しい仕事なら write で作ってください。"
                ),
                format!(
                    "There is no note \"{task}\" owned by {label}. Check the task names with list. \
                     For a new task, create the note with write."
                ),
            ),
        )
    }

    fn ambiguous(&self, task: &str, hits: &[&BlackboardNote]) -> OpResult {
        let places = self.places(hits);
        OpResult::new(
            Outcome::Ambiguous,
            pick(
                self.language,
                format!(
                    "付箋「{task}」が {} 箇所にあります（{places}）。どれを指すか決められないので、何も変えていません。\
                     read で中身を確かめ、要らなければ remove で全部をごみ箱へ移してから write で作り直してください。",
                    hits.len()
                ),
                format!(
                    "The note \"{task}\" exists in {} places ({places}). It is unclear which one you mean, so nothing was changed. \
                     Check the content with read; if you do not need them, remove sends all of them to the recycle bin, then create it again with write.",
                    hits.len()
                ),
            ),
        )
    }

    fn io_error(&self, what: &str, detail: impl std::fmt::Display) -> OpResult {
        OpResult::new(
            Outcome::Error,
            pick(
                self.language,
                format!("{what}に失敗しました: {detail}\n黒板は変わっていません。利用者に伝えてください。"),
                format!("Failed to {what}: {detail}\nThe blackboard was not changed. Tell the user."),
            ),
        )
    }

    // -----------------------------------------------------------------------
    // op
    // -----------------------------------------------------------------------

    fn list(&self) -> OpResult {
        if self.notes.is_empty() {
            return OpResult::new(
                Outcome::Ok,
                pick(
                    self.language,
                    "黒板に付箋はありません。仕事に着手したら write で付箋を作ってください。".to_owned(),
                    "The blackboard has no notes. When you start a task, create a note with write.".to_owned(),
                ),
            );
        }

        let mut sorted: Vec<&BlackboardNote> = self.notes.iter().collect();
        sorted.sort_by(|a, b| {
            (Place::of(a).order(), a.name.as_str()).cmp(&(Place::of(b).order(), b.name.as_str()))
        });

        let mut out = pick(
            self.language,
            format!("黒板: 付箋 {} 枚（本文は read で読む）\n", sorted.len()),
            format!("Blackboard: {} note(s) (read shows the body)\n", sorted.len()),
        );
        let mut current: Option<Place> = None;
        // 上限で落とした付箋（場所ごと）。**行の境界でだけ止め、数は最後まで数える。**
        let mut dropped: Vec<(String, usize)> = Vec::new();
        for note in sorted {
            let place = Place::of(note);
            let mut entry = String::new();
            if current.as_ref() != Some(&place) {
                entry.push_str(&format!("[{}]\n", place.label(self.language)));
            }
            entry.push_str(&self.list_line(note, &place));
            if out.chars().count() + entry.chars().count() > MAX_OUTPUT_CHARS {
                let label = place.label(self.language);
                match dropped.last_mut() {
                    Some((last, count)) if *last == label => *count += 1,
                    _ => dropped.push((label, 1)),
                }
                continue;
            }
            out.push_str(&entry);
            current = Some(place);
        }
        if !dropped.is_empty() {
            let detail = dropped
                .iter()
                .map(|(label, count)| format!("{label} {count}"))
                .collect::<Vec<_>>()
                .join(" / ");
            out.push_str(&pick(
                self.language,
                format!(
                    "\n（上限 {MAX_OUTPUT_CHARS} 字に達したため表示していない付箋: {detail}。\
                     同じ引数で呼び直しても同じ範囲が返ります。名前が分かっている付箋は read で読めます。\
                     終わった付箋が溜まっているなら、利用者に黒板タブで消してもらってください）\n"
                ),
                format!(
                    "\n(Not shown because the {MAX_OUTPUT_CHARS}-character limit was reached: {detail}. \
                     Calling again with the same arguments returns the same range. A note whose name you know can be read with read. \
                     If finished notes have piled up, ask the user to delete them in the blackboard tab.)\n"
                ),
            ));
        }
        OpResult::new(Outcome::Ok, out)
    }

    fn list_line(&self, note: &BlackboardNote, place: &Place) -> String {
        let chars = note.content.chars().count();
        let when = format_time(note.modified_ms);
        match split_note_name(&note.name) {
            Some((owner, task)) => pick(
                self.language,
                format!("- {}: 「{task}」 {chars} 字・{when}\n", self.owner_label(owner)),
                format!("- {}: \"{task}\" {chars} chars, {when}\n", self.owner_label(owner)),
            ),
            // 持ち主を名乗っていないファイル（旧い `まとめ.md` など）。このツールでは引けないので、
            // 読める経路（`file` の read。読み取りは囲いの外）をパスごと書く。
            None => {
                let path = format!("{}/{}", place.rel_dir(), note.name);
                pick(
                    self.language,
                    format!("- （持ち主なし）`{path}` {chars} 字・{when} — 読むなら `file` の read\n"),
                    format!("- (no owner) `{path}` {chars} chars, {when} — read it with `file` read\n"),
                )
            }
        }
    }

    fn read(&self, req: &Request, task: &str) -> OpResult {
        let owner = match req.owner.as_deref() {
            Some(raw) if !raw.trim().is_empty() => self.resolve_owner(raw),
            _ => self.me.to_owned(),
        };
        let hits = self.find(&owner, task);
        let Some(note) = hits.first() else {
            // 自分の付箋として探して無かったとき、同じ仕事名の他人の付箋があれば名指しする
            // （盤面で見た他人の付箋を `owner` なしで読もうとした形）。
            let others = if req.owner.is_none() { self.other_owners_of(task) } else { Vec::new() };
            let mut result = self.not_found(&owner, task);
            if !others.is_empty() {
                let labels = others.iter().map(|o| self.owner_label(o)).collect::<Vec<_>>().join(" / ");
                result.text.push_str(&pick(
                    self.language,
                    format!("\n同じ仕事名の付箋を {labels} が持っています。読むなら `owner` にその id を付けてください。"),
                    format!("\n{labels} has a note with the same task name. To read it, pass that id as `owner`."),
                ));
            }
            return result;
        };
        let place = Place::of(note);
        let chars = note.content.chars().count();
        let mut text = pick(
            self.language,
            format!("{} / {} / 「{task}」（{chars} 字）\n\n", place.label(self.language), self.owner_label(&owner)),
            format!("{} / {} / \"{task}\" ({chars} chars)\n\n", place.label(self.language), self.owner_label(&owner)),
        );
        if chars > NOTE_MAX_CHARS {
            // ツールで書いた付箋はここへ来ない。`run` や手作業で置かれた上限超えの付箋だけ。
            let head: String = note.content.chars().take(NOTE_MAX_CHARS).collect();
            let path = format!("{}/{}", place.rel_dir(), note.name);
            text.push_str(&head);
            text.push_str(&pick(
                self.language,
                format!(
                    "\n\n（先頭 {NOTE_MAX_CHARS} 字。残り {} 字は省略しました。**続きを読む引数はありません** — \
                     同じ引数で読み直しても同じ範囲が返ります。残りが要るなら `grep` で `{path}` を探してください）",
                    chars - NOTE_MAX_CHARS
                ),
                format!(
                    "\n\n(First {NOTE_MAX_CHARS} characters; the remaining {} were omitted. **There is no argument for reading further** — \
                     reading again with the same arguments returns the same range. If you need the rest, search `{path}` with `grep`.)",
                    chars - NOTE_MAX_CHARS
                ),
            ));
        } else {
            text.push_str(&note.content);
        }
        if hits.len() > 1 {
            let places = self.places(&hits);
            text.push_str(&pick(
                self.language,
                format!("\n\n（同じ名前の付箋が {} 箇所にあります: {places}。上は最初の 1 枚です）", hits.len()),
                format!("\n\n(The same note exists in {} places: {places}. The above is the first one.)", hits.len()),
            ));
        }
        OpResult::new(Outcome::Ok, text).at(&place)
    }

    /// **create-only**。置き場は `doing` 固定。
    fn write(&self, task: &str, body: &str) -> OpResult {
        let chars = body.chars().count();
        if chars > NOTE_MAX_CHARS {
            return OpResult::new(
                Outcome::TooLarge,
                pick(
                    self.language,
                    format!(
                        "本文が {chars} 字あり、付箋 1 枚の上限 {NOTE_MAX_CHARS} 字を超えます（何も書いていません）。\
                         付箋は途中経過のメモです。要点へ絞るか、長い成果物は `file` で作業フォルダへ書き、付箋にはそのパスを書いてください。"
                    ),
                    format!(
                        "The body is {chars} characters, over the {NOTE_MAX_CHARS}-character limit of one note (nothing was written). \
                         A note is a progress memo. Trim it to the key points, or write a long deliverable into the working folder with `file` and put its path in the note."
                    ),
                ),
            );
        }

        // 重複の検査は 3 状態 + 直下 +「その他」の全部。
        let existing = self.find(self.me, task);
        if let Some(first) = existing.first() {
            return self.exists(task, &Place::of(first), &existing);
        }

        let file = note_file_name(self.me, task);
        let rel = format!("{BLACKBOARD_DIR}/{STATE_NEW}/{file}");
        let path = match resolve_creatable(self.work_dir, &rel) {
            Ok((path, _)) => path,
            Err(reason) => return OpResult::new(Outcome::Error, reason),
        };
        // 置き場のフォルダはツールが作る（新しい村に `blackboard/` は無い）。
        if let Some(parent) = path.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            return self.io_error(self.language.pick("置き場のフォルダの作成", "create the folder"), err);
        }
        // `create_new` で作る。盤面を読んでからここまでの間に同じ名前ができていても上書きしない
        // （大文字小文字だけが違う名前も、Windows ではここで止まる）。
        use std::io::Write as _;
        let opened = std::fs::OpenOptions::new().write(true).create_new(true).open(&path);
        let mut handle = match opened {
            Ok(handle) => handle,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                return self.exists(task, &Place::State(STATE_NEW), &[]);
            }
            Err(err) => return self.io_error(self.language.pick("付箋の作成", "create the note"), err),
        };
        if let Err(err) = handle.write_all(body.as_bytes()) {
            return self.io_error(self.language.pick("付箋の書き込み", "write the note"), err);
        }
        OpResult::new(
            Outcome::Ok,
            pick(
                self.language,
                format!(
                    "付箋「{task}」を `{STATE_NEW}` に作りました（{chars} 字）。\
                     以後のメモは append で足し、仕事が終わったら move で `{STATE_FROZEN}` へ移してください。"
                ),
                format!(
                    "Created the note \"{task}\" in `{STATE_NEW}` ({chars} chars). \
                     Add further memos with append, and when the task is finished move it to `{STATE_FROZEN}`."
                ),
            ),
        )
        .at(&Place::State(STATE_NEW))
    }

    /// `write` の重複の拒否。**その場所で実際に通る手だけを案内する**（凍結 7）。
    fn exists(&self, task: &str, place: &Place, hits: &[&BlackboardNote]) -> OpResult {
        let where_ = if hits.len() > 1 { self.places(hits) } else { place.label(self.language) };
        let next = match place {
            Place::State(state) if *state != STATE_FROZEN => pick(
                self.language,
                "続きのメモなら append、状態を変えるなら move、別の仕事なら別の仕事名で write してください。".to_owned(),
                "To add a memo use append, to change the state use move, and for a different task write with another task name.".to_owned(),
            ),
            Place::State(_) => pick(
                self.language,
                format!(
                    "`{STATE_FROZEN}` の付箋は動かせず、追記もできません。やり直すなら remove してから write するか、別の仕事名で write してください。"
                ),
                format!(
                    "A note in `{STATE_FROZEN}` cannot be moved or appended to. To redo the task, remove it and then write, or write with another task name."
                ),
            ),
            Place::Root | Place::Other(_) => pick(
                self.language,
                "3 つの状態の外にある付箋へは append も move もできません。remove してから write するか、別の仕事名で write してください。".to_owned(),
                "A note outside the three states cannot be appended to or moved. Remove it and then write, or write with another task name.".to_owned(),
            ),
        };
        OpResult::new(
            Outcome::Exists,
            pick(
                self.language,
                format!("付箋「{task}」は既にあります（{where_}）。write は新規作成だけなので、何も書いていません。{next}"),
                format!("The note \"{task}\" already exists ({where_}). write only creates new notes, so nothing was written. {next}"),
            ),
        )
        .at(place)
    }

    fn append(&self, req: &Request, task: &str, body: &str) -> OpResult {
        let hits = match self.own_notes(req, task) {
            Ok(hits) => hits,
            Err(result) => return result,
        };
        if hits.len() > 1 {
            return self.ambiguous(task, &hits);
        }
        let note = hits[0];
        let place = Place::of(note);
        match &place {
            Place::State(state) if *state == STATE_FROZEN => return self.frozen(task).at(&place),
            Place::State(_) => {}
            Place::Root | Place::Other(_) => return self.misplaced(task, &place),
        }

        // メモは 1 件ずつの追記なので、前の行と繋がらないよう改行を挟む。
        let glue = if note.content.is_empty() || note.content.ends_with('\n') { "" } else { "\n" };
        let added = glue.chars().count() + body.chars().count();
        let total = note.content.chars().count() + added;
        if total > NOTE_MAX_CHARS {
            return OpResult::new(
                Outcome::TooLarge,
                pick(
                    self.language,
                    format!(
                        "追記すると付箋「{task}」が {total} 字になり、上限 {NOTE_MAX_CHARS} 字を超えます（何も書いていません）。\
                         続きは別の仕事名（例:「{task} 2」）で write してください。"
                    ),
                    format!(
                        "Appending would make the note \"{task}\" {total} characters, over the {NOTE_MAX_CHARS}-character limit (nothing was written). \
                         Continue in a new note: write with another task name (for example \"{task} 2\")."
                    ),
                ),
            )
            .at(&place);
        }

        let rel = format!("{}/{}", place.rel_dir(), note.name);
        let path = match resolve_in_work_dir(self.work_dir, &rel) {
            Ok((path, _)) => path,
            Err(reason) => return OpResult::new(Outcome::Error, reason).at(&place),
        };
        use std::io::Write as _;
        let written = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .and_then(|mut handle| handle.write_all(format!("{glue}{body}").as_bytes()));
        if let Err(err) = written {
            return self.io_error(self.language.pick("付箋への追記", "append to the note"), err).at(&place);
        }
        OpResult::new(
            Outcome::Ok,
            pick(
                self.language,
                format!("付箋「{task}」（{}）へ {added} 字を追記しました（合計 {total} 字）。", place.label(self.language)),
                format!("Appended {added} characters to the note \"{task}\" ({}); {total} in total.", place.label(self.language)),
            ),
        )
        .at(&place)
    }

    fn frozen(&self, task: &str) -> OpResult {
        OpResult::new(
            Outcome::Frozen,
            pick(
                self.language,
                format!(
                    "付箋「{task}」は `{STATE_FROZEN}` にあります。終わった付箋は動かせず、追記もできません（何も変えていません）。\
                     やり直すなら、remove してから write するか、別の仕事名で write してください。"
                ),
                format!(
                    "The note \"{task}\" is in `{STATE_FROZEN}`. A finished note cannot be moved or appended to (nothing was changed). \
                     To redo the task, remove it and then write, or write with another task name."
                ),
            ),
        )
    }

    fn misplaced(&self, task: &str, place: &Place) -> OpResult {
        OpResult::new(
            Outcome::Misplaced,
            pick(
                self.language,
                format!(
                    "付箋「{task}」は 3 つの状態の外（{}）にあります。ここにある付箋へは append も move もできません（何も変えていません）。\
                     read で中身を確かめ、remove してから write で作り直してください。",
                    place.label(self.language)
                ),
                format!(
                    "The note \"{task}\" is outside the three states ({}). Notes there cannot be appended to or moved (nothing was changed). \
                     Check the content with read, then remove it and create it again with write.",
                    place.label(self.language)
                ),
            ),
        )
        .at(place)
    }

    fn move_to(&self, req: &Request, task: &str, to: &'static str) -> OpResult {
        let hits = match self.own_notes(req, task) {
            Ok(hits) => hits,
            Err(result) => return result,
        };
        if hits.len() > 1 {
            return self.ambiguous(task, &hits);
        }
        let note = hits[0];
        let place = Place::of(note);
        let from = match &place {
            // **`done` の凍結が no-op より先** — 宛先が `done` でも frozen。
            Place::State(state) if *state == STATE_FROZEN => {
                let mut result = self.frozen(task).at(&place);
                result.to = Some(to);
                return result;
            }
            Place::State(state) => *state,
            Place::Root | Place::Other(_) => {
                let mut result = self.misplaced(task, &place);
                result.to = Some(to);
                return result;
            }
        };
        if from == to {
            let mut result = OpResult::new(
                Outcome::Noop,
                pick(
                    self.language,
                    format!("付箋「{task}」は既に `{to}` にあります。何も変えていません。"),
                    format!("The note \"{task}\" is already in `{to}`. Nothing was changed."),
                ),
            )
            .at(&place);
            result.to = Some(to);
            return result;
        }

        let done = |outcome: Outcome, text: String| {
            let mut result = OpResult::new(outcome, text).at(&place);
            result.to = Some(to);
            result
        };
        let src = match resolve_in_work_dir(self.work_dir, &format!("{}/{}", place.rel_dir(), note.name)) {
            Ok((path, _)) => path,
            Err(reason) => return done(Outcome::Error, reason),
        };
        let dest = match resolve_creatable(self.work_dir, &format!("{BLACKBOARD_DIR}/{to}/{}", note.name)) {
            Ok((path, _)) => path,
            Err(reason) => return done(Outcome::Error, reason),
        };
        if dest.exists() {
            // 盤面では 1 枚だったのに宛先にある = 大文字小文字だけが違う名前か、読んだ後にできたもの。
            return done(
                Outcome::Exists,
                pick(
                    self.language,
                    format!("`{to}` に同じ名前のファイルが既にあります。上書きしないので、何も変えていません。利用者に伝えてください。"),
                    format!("A file with the same name already exists in `{to}`. Nothing is overwritten, so nothing was changed. Tell the user."),
                ),
            );
        }
        if let Some(parent) = dest.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            let mut result = self.io_error(self.language.pick("置き場のフォルダの作成", "create the folder"), err).at(&place);
            result.to = Some(to);
            return result;
        }
        if let Err(err) = std::fs::rename(&src, &dest) {
            let mut result = self.io_error(self.language.pick("付箋の移動", "move the note"), err).at(&place);
            result.to = Some(to);
            return result;
        }
        // **実際の遷移を必ず書く** — モデルが覚えていた状態とずれていれば、この 1 行で分かる。
        done(
            Outcome::Ok,
            pick(
                self.language,
                format!("付箋「{task}」を移しました: `{from}` → `{to}`。"),
                format!("Moved the note \"{task}\": `{from}` → `{to}`."),
            ),
        )
    }

    /// OS のごみ箱へ。場所を問わない。同じ名前が複数あれば全部（それが重複を解く唯一の手）。
    fn remove(&self, req: &Request, task: &str) -> OpResult {
        let hits = match self.own_notes(req, task) {
            Ok(hits) => hits,
            Err(result) => return result,
        };
        let first = Place::of(hits[0]);
        for note in &hits {
            let place = Place::of(note);
            let path = match resolve_in_work_dir(self.work_dir, &format!("{}/{}", place.rel_dir(), note.name)) {
                Ok((path, _)) => path,
                Err(reason) => return OpResult::new(Outcome::Error, reason).at(&place),
            };
            // 完全削除の経路は無い。ごみ箱が使えない環境では消さずに失敗を返す（`file remove` と同じ）。
            if let Err(err) = trash::delete(&path) {
                return OpResult::new(
                    Outcome::Error,
                    pick(
                        self.language,
                        format!(
                            "付箋「{task}」をごみ箱へ移せません: {err}\n\
                             この環境ではごみ箱が使えないようです。完全削除は行わないので、利用者に削除を頼んでください。"
                        ),
                        format!(
                            "Cannot move the note \"{task}\" to the recycle bin: {err}\n\
                             The recycle bin seems unavailable here. Nothing is deleted for good, so ask the user to delete it."
                        ),
                    ),
                )
                .at(&place);
            }
        }
        let places = self.places(&hits);
        OpResult::new(
            Outcome::Ok,
            pick(
                self.language,
                format!("付箋「{task}」（{places}）をごみ箱へ移しました（完全には削除していません）。"),
                format!("Moved the note \"{task}\" ({places}) to the recycle bin (nothing is deleted for good)."),
            ),
        )
        .at(&first)
    }

    /// 仕事名を受け取って正規化する。op の共通の入口。
    fn task_name(&self, req: &Request) -> Result<String, OpResult> {
        let Some(raw) = req.name.as_deref() else {
            return Err(OpResult::new(
                Outcome::Invalid,
                pick(
                    self.language,
                    format!("{} には `name`（仕事名）が必要です。", req.op),
                    format!("{} needs `name` (the task name).", req.op),
                ),
            ));
        };
        normalize_task_name(raw).map_err(|err| {
            OpResult::new(
                Outcome::Invalid,
                match err {
                    TaskNameError::Empty => pick(
                        self.language,
                        "仕事名が空です（記号と空白を除くと何も残りません）。仕事の内容が分かる短い名前を付けてください。".to_owned(),
                        "The task name is empty (nothing is left after removing symbols and spaces). Give a short name that tells what the task is.".to_owned(),
                    ),
                    TaskNameError::TooLong(chars) => pick(
                        self.language,
                        format!("仕事名が {chars} 字あり、上限 {TASK_NAME_MAX_CHARS} 字を超えます。短い名前にしてください（詳細は本文へ）。"),
                        format!("The task name is {chars} characters, over the {TASK_NAME_MAX_CHARS}-character limit. Use a shorter name (details go in the body)."),
                    ),
                },
            )
        })
    }

    fn body<'r>(&self, req: &'r Request) -> Result<&'r str, OpResult> {
        req.body.as_deref().ok_or_else(|| {
            OpResult::new(
                Outcome::Invalid,
                pick(
                    self.language,
                    format!("{} には `body`（本文）が必要です。", req.op),
                    format!("{} needs `body` (the text).", req.op),
                ),
            )
        })
    }

    fn run(&self, req: &Request) -> OpResult {
        // 自分の id がファイル名の前半に使えない形なら、どの付箋も自分のものとして引けない。
        if self.me.contains(NOTE_SEPARATOR) || self.me.contains(['/', '\\']) || self.me.is_empty() {
            return OpResult::new(
                Outcome::Invalid,
                pick(
                    self.language,
                    "この個体の id は付箋のファイル名に使えません。利用者に伝えてください。".to_owned(),
                    "This servant's id cannot be used in a note's file name. Tell the user.".to_owned(),
                ),
            );
        }
        let result = (|| -> Result<OpResult, OpResult> {
            Ok(match req.op.as_str() {
                "list" => self.list(),
                "read" => self.read(req, &self.task_name(req)?),
                "write" => {
                    let task = self.task_name(req)?;
                    self.write(&task, self.body(req)?)
                }
                "append" => {
                    let task = self.task_name(req)?;
                    self.append(req, &task, self.body(req)?)
                }
                "move" => {
                    let task = self.task_name(req)?;
                    let to = req.to.as_deref().map(str::trim).unwrap_or_default();
                    let Some(to) = STATES.iter().find(|s| **s == to) else {
                        return Err(OpResult::new(
                            Outcome::Invalid,
                            pick(
                                self.language,
                                format!("move の `to` は {} のいずれかです（指定値: `{to}`）。", STATES.map(|s| format!("`{s}`")).join(" / ")),
                                format!("`to` for move is one of {} (given: `{to}`).", STATES.map(|s| format!("`{s}`")).join(" / ")),
                            ),
                        ));
                    };
                    self.move_to(req, &task, to)
                }
                "remove" => self.remove(req, &self.task_name(req)?),
                other => OpResult::new(
                    Outcome::Invalid,
                    pick(
                        self.language,
                        format!("`op` は list / read / write / append / move / remove のいずれかです（指定値: `{other}`）。"),
                        format!("`op` is one of list / read / write / append / move / remove (given: `{other}`)."),
                    ),
                ),
            })
        })();
        result.unwrap_or_else(|refusal| refusal)
    }
}

/// 更新時刻（epoch ms）を端末の時刻で書く。取れなければ `-`。
fn format_time(modified_ms: u64) -> String {
    use chrono::TimeZone as _;
    i64::try_from(modified_ms)
        .ok()
        .filter(|ms| *ms > 0)
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|when| when.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| "-".to_owned())
}

/// 同梱 `blackboard` ツール本体。状態を持たない。
pub struct BlackboardTool;

impl BlackboardTool {
    /// 門 → 盤面の読み → op。`call` はこれに計器を足すだけ。
    pub(crate) async fn execute(&self, ctx: &ToolContext, args: &Value) -> CoreResult<OpResult> {
        let language = ctx.language;
        if !is_offered(ctx) {
            let text = if ctx.work_dir.is_none() {
                work_dir_missing()
            } else {
                language
                    .pick(
                        "この個体は黒板を使わない設定です。黒板は使わずに仕事を進めてください。",
                        "This servant is set not to use the blackboard. Carry on without it.",
                    )
                    .to_owned()
            };
            return Ok(OpResult::new(Outcome::Disabled, text));
        }
        let work_dir = ctx.work_dir.clone().expect("is_offered が work_dir を確かめた");
        // 作業フォルダが実在しないまま進むと、`write` がフォルダごと掘ってしまう。
        if !tokio::fs::metadata(&work_dir).await.map(|meta| meta.is_dir()).unwrap_or(false) {
            return Ok(OpResult::new(
                Outcome::Error,
                pick(
                    language,
                    format!("作業フォルダ `{}` が存在しません。設定を確認してください。", work_dir.display()),
                    format!("The working folder `{}` does not exist. Check the settings.", work_dir.display()),
                ),
            ));
        }

        let text_arg = |key: &str| args.get(key).and_then(Value::as_str).map(str::to_owned);
        let req = Request {
            op: text_arg("op").unwrap_or_default(),
            name: text_arg("name"),
            body: text_arg("body"),
            to: text_arg("to"),
            owner: text_arg("owner"),
        };

        let notes = match read_blackboard_dir(&work_dir).await {
            Ok(notes) => notes,
            Err(err) => {
                return Ok(OpResult::new(
                    Outcome::Error,
                    pick(
                        language,
                        format!("黒板を読めません: {err}\n利用者に伝えてください。"),
                        format!("Cannot read the blackboard: {err}\nTell the user."),
                    ),
                ));
            }
        };

        let me = ctx.agent_id.as_str().to_owned();
        let names = ctx.agent_names.clone();
        // ファイル I/O は Tokio ワーカーを塞がない側へ逃がす（既存ツールと同じ規律）。
        spawn_rayon(move || {
            Board { work_dir: &work_dir, me: &me, names: &names, language, notes: &notes }.run(&req)
        })
        .await
    }
}

#[async_trait]
impl AgentTool for BlackboardTool {
    fn name(&self) -> &str {
        "blackboard"
    }

    /// **規約を運ぶのはこの説明文**（Spec 55 凍結 11）。条例が空の村でも黒板が動くよう、
    /// 5 点を入れる — 1 仕事 1 付箋 / 着手時に list / 状態の意味 / write は新規だけ・
    /// done は動かせない / 答えは返信で返す。全員の毎ターンに乗る固定費なので足さない。
    fn description(&self, language: Language) -> String {
        language
            .pick(
                "村の黒板 = 村のみんなで共有する作業メモ（各自の長期記憶とは別物）を読み書きする。\
                 **付箋は仕事 1 つに 1 枚。** 新しい仕事に着手したら、まず list で盤面を見て、関係する付箋だけ read する\
                 （同じ仕事の進行中は読み直さない）。自分の仕事は write で付箋を作り、途中経過・気づき・引き継ぎのメモを append で足す。\
                 状態は 3 つ: doing（進行中。write はここへ作る）/ on-hold（止めるよう言われて、自分で移した仕事）/ done（終わった仕事）。\
                 状態は move で変える — 仕事が終わったら、最終の報告を返す前に done へ移す。\
                 **write は新規作成だけ**（同じ仕事名が既にあるなら append か、別の仕事名）。\
                 **done の付箋は動かせず、追記もできない**（やり直すなら remove してから write か、別の仕事名で write）。\
                 書けるのは自分の付箋だけ。読むのは誰の付箋でもよい。置き場とファイル名はこのツールが決める。\
                 **頼まれた仕事の答えは、付箋ではなく返信で返す。**",
                "Read and write the village blackboard: working memos shared by the whole village (separate from each servant's long-term memory). \
                 **One note per task.** When you start a new task, first look at the board with list and read only the related notes \
                 (do not re-read during the same task). For your own task, create a note with write, then add progress, findings and hand-over memos with append. \
                 There are three states: doing (in progress; write creates notes here) / on-hold (work you were told to pause and moved yourself) / done (finished). \
                 Change the state with move — when the task is finished, move the note to done before you send your final report. \
                 **write only creates new notes** (if the task name already exists, use append or another task name). \
                 **A note in done cannot be moved or appended to** (to redo it, remove then write, or write with another task name). \
                 You can only write to your own notes; you may read anyone's. This tool decides the location and the file name. \
                 **Return the answer to a request as a reply, not as a note.**",
            )
            .to_owned()
    }

    fn parameters(&self, language: Language) -> Value {
        let d = |ja: &str, en: &str| language.pick(ja, en).to_owned();
        serde_json::json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "string",
                    "enum": ["list", "read", "write", "append", "move", "remove"],
                    "description": d("list: 盤面（本文なし）/ read: 付箋 1 枚の本文 / write: 自分の付箋を新しく作る / append: 自分の付箋へ足す / move: 自分の付箋の状態を変える / remove: 自分の付箋をごみ箱へ",
                                     "list: the board (no bodies) / read: one note's body / write: create a new note of your own / append: add to your own note / move: change the state of your own note / remove: send your own note to the recycle bin")
                },
                "name": {
                    "type": "string",
                    "description": d("仕事名（付箋の名前）。list 以外で必須。80 字まで。ファイル名に使えない文字は `_` になる",
                                     "Task name (the note's name). Required except for list. Up to 80 characters; characters a file name cannot hold become `_`")
                },
                "body": {
                    "type": "string",
                    "description": d("write の本文、または append で足すメモ。付箋 1 枚は 12,000 字まで",
                                     "The body for write, or the memo to add for append. One note holds up to 12,000 characters")
                },
                "to": {
                    "type": "string",
                    "enum": STATES,
                    "description": d("move の行き先の状態", "Destination state for move")
                },
                "owner": {
                    "type": "string",
                    "description": d("read で他人の付箋を読むときの持ち主（list に出る id）。省略は自分",
                                     "Owner when reading someone else's note with read (the id shown by list). Omit for your own")
                }
            },
            "required": ["op"],
            "additionalProperties": false
        })
    }

    async fn spec_for(&self, ctx: &ToolContext) -> Option<ToolSpec> {
        is_offered(ctx).then(|| self.spec(ctx.language))
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        let op = args.get("op").and_then(Value::as_str).unwrap_or_default();
        let result = self.execute(ctx, args).await?;
        // `tool:` 行は `args_chars` しか持たないので、どの op が何で断られたかはここでしか読めない。
        // **本文と仕事名は出さない**（モデルの出力を記録する計器は秘密の転送経路になる — #71）。
        let to = result.to.map(|to| format!(" to={to}")).unwrap_or_default();
        crate::note!(
            "blackboard op: agent={} op={} state={}{to} outcome={}",
            ctx.agent_id,
            op_log(op),
            result.state,
            result.outcome.log()
        );
        Ok(result.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fuseforks-bbtool-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// `blackboard/` の下へ、ツールを通さずに置く（手作業・`run`・旧形式の代役）。
        fn put(&self, rel: &str, content: &str) {
            let path = self.0.join(BLACKBOARD_DIR).join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }

        fn note(&self, rel: &str) -> Option<String> {
            std::fs::read_to_string(self.0.join(BLACKBOARD_DIR).join(rel)).ok()
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ctx_as(dir: &TempDir, agent: &str) -> ToolContext {
        ToolContext {
            agent_id: AgentId::from(agent),
            work_dir: Some(dir.0.clone()),
            cancel: None,
            rag_roots: Vec::new(),
            agent_names: vec![
                (AgentId::from("agent"), "ザリ".to_owned()),
                (AgentId::from("agent_2"), "ジェミー".to_owned()),
            ],
            uses_blackboard: true,
            language: Language::Ja,
        }
    }

    async fn run(ctx: &ToolContext, args: Value) -> OpResult {
        BlackboardTool.execute(ctx, &args).await.unwrap()
    }

    fn ja_chars(s: &str) -> usize {
        s.chars()
            .filter(|c| matches!(*c as u32, 0x3040..=0x309F | 0x30A0..=0x30FF | 0x4E00..=0x9FFF))
            .count()
    }

    #[test]
    fn task_names_are_normalized_by_one_rule() {
        let ok = |raw: &str| normalize_task_name(raw).unwrap();
        assert_eq!(ok("  調査  "), "調査");
        assert_eq!(ok("調査."), "調査", "末尾の `.` は Windows が黙って落とす");
        assert_eq!(ok("調査 . ."), "調査");
        assert_eq!(ok("調査.md"), "調査", "拡張子つきで返してきても同じ付箋を引ける");
        assert_eq!(ok("調査.MD."), "調査");
        assert_eq!(ok(r#"a\b/c:d*e?f"g<h>i|j"#), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(ok("1 行目\n2 行目"), "1 行目_2 行目", "制御文字も `_`");
        assert_eq!(ok("Spec 55 - P2"), "Spec 55 - P2", "仕事名の中の区切りはそのまま");
        assert_eq!(ok("???"), "___");

        assert_eq!(normalize_task_name("  . "), Err(TaskNameError::Empty));
        assert_eq!(normalize_task_name(".md"), Err(TaskNameError::Empty));
        assert_eq!(ok(&"あ".repeat(80)).chars().count(), 80, "上限は code point で数える");
        assert_eq!(normalize_task_name(&"あ".repeat(81)), Err(TaskNameError::TooLong(81)));

        for raw in ["  調査.md. ", r"a\b", "Spec 55 - P2", "x.md.md"] {
            let once = ok(raw);
            assert_eq!(ok(&once), once, "冪等: {raw}");
        }
    }

    /// 門は述語 1 本。提示（`spec_for`）と実行（`call`）が同じ答えを返す。
    #[tokio::test]
    async fn the_gate_is_the_same_for_presenting_and_for_running() {
        let dir = TempDir::new("gate");
        let open = ctx_as(&dir, "agent");
        assert!(BlackboardTool.spec_for(&open).await.is_some());

        let mut opted_out = ctx_as(&dir, "agent");
        opted_out.uses_blackboard = false;
        let mut no_dir = ctx_as(&dir, "agent");
        no_dir.work_dir = None;
        for closed in [&opted_out, &no_dir] {
            assert!(BlackboardTool.spec_for(closed).await.is_none());
            let result = run(closed, serde_json::json!({ "op": "write", "name": "調査", "body": "x" })).await;
            assert_eq!(result.outcome, Outcome::Disabled);
        }
        assert!(!dir.0.join(BLACKBOARD_DIR).exists(), "断った呼び出しはディスクに触らない");
    }

    /// 新しい村に `blackboard/` は無い。置き場はツールが作り、ファイル名もツールが組む。
    #[tokio::test]
    async fn write_builds_the_place_and_the_file_name() {
        let dir = TempDir::new("write");
        let result = run(
            &ctx_as(&dir, "agent"),
            serde_json::json!({ "op": "write", "name": " 調査: 単価表.md ", "body": "着手" }),
        )
        .await;
        assert_eq!(result.outcome, Outcome::Ok, "{}", result.text);
        assert_eq!(dir.note("doing/agent - 調査_ 単価表.md").as_deref(), Some("着手"));
        assert!(result.text.contains("調査_ 単価表"), "正規化後の名前を結果に書く: {}", result.text);
    }

    /// **create-only。** 同じ `id + 仕事名` がどこにあっても書かず、その場所で通る手だけを案内する。
    #[tokio::test]
    async fn write_never_overwrites_and_names_a_next_step_that_works_there() {
        let dir = TempDir::new("exists");
        let ctx = ctx_as(&dir, "agent");
        dir.put("doing/agent - 進行中.md", "元");
        dir.put("done/agent - 済み.md", "元");
        dir.put("agent - 直下.md", "元");
        dir.put("archive/agent - 他.md", "元");

        for (task, rel, must, must_not) in [
            ("進行中", "doing/agent - 進行中.md", "append", "remove してから"),
            ("済み", "done/agent - 済み.md", "remove してから write", "続きのメモなら append"),
            ("直下", "agent - 直下.md", "remove してから write", "続きのメモなら append"),
            ("他", "archive/agent - 他.md", "remove してから write", "続きのメモなら append"),
        ] {
            let result = run(&ctx, serde_json::json!({ "op": "write", "name": task, "body": "上書き" })).await;
            assert_eq!(result.outcome, Outcome::Exists, "{task}: {}", result.text);
            assert_eq!(dir.note(rel).as_deref(), Some("元"), "{task}: 既存は 1 バイトも変わらない");
            assert!(result.text.contains(must), "{task}: {}", result.text);
            assert!(!result.text.contains(must_not), "{task}: 通らない手を案内しない: {}", result.text);
        }
        assert!(dir.note("doing/agent - 済み.md").is_none(), "別の場所に 2 枚目を作らない");

        // 他人の同じ仕事名は重複ではない（主キーは id + 仕事名）。
        let other = run(&ctx_as(&dir, "agent_2"), serde_json::json!({ "op": "write", "name": "進行中", "body": "別人" })).await;
        assert_eq!(other.outcome, Outcome::Ok, "{}", other.text);
    }

    #[tokio::test]
    async fn append_grows_own_notes_in_doing_and_on_hold_only() {
        let dir = TempDir::new("append");
        let ctx = ctx_as(&dir, "agent");
        dir.put("doing/agent - a.md", "1 行目");
        dir.put("on-hold/agent - b.md", "1 行目\n");
        dir.put("done/agent - c.md", "終わり");
        dir.put("agent - d.md", "直下");

        let a = run(&ctx, serde_json::json!({ "op": "append", "name": "a", "body": "2 行目" })).await;
        assert_eq!(a.outcome, Outcome::Ok, "{}", a.text);
        assert_eq!(dir.note("doing/agent - a.md").as_deref(), Some("1 行目\n2 行目"), "前のメモと繋げない");
        let b = run(&ctx, serde_json::json!({ "op": "append", "name": "b", "body": "2 行目" })).await;
        assert_eq!(b.outcome, Outcome::Ok);
        assert_eq!(dir.note("on-hold/agent - b.md").as_deref(), Some("1 行目\n2 行目"));

        let c = run(&ctx, serde_json::json!({ "op": "append", "name": "c", "body": "追記" })).await;
        assert_eq!(c.outcome, Outcome::Frozen, "{}", c.text);
        assert_eq!(dir.note("done/agent - c.md").as_deref(), Some("終わり"));
        let d = run(&ctx, serde_json::json!({ "op": "append", "name": "d", "body": "追記" })).await;
        assert_eq!(d.outcome, Outcome::Misplaced, "{}", d.text);
        assert_eq!(dir.note("agent - d.md").as_deref(), Some("直下"));

        let missing = run(&ctx, serde_json::json!({ "op": "append", "name": "無い", "body": "x" })).await;
        assert_eq!(missing.outcome, Outcome::NotFound);
        assert!(dir.note("doing/agent - 無い.md").is_none(), "append は付箋を作らない");
    }

    /// 付箋の上限 = `file read` の返却上限。`write` の本文と `append` 後の全体に掛かる。
    #[tokio::test]
    async fn a_note_never_grows_past_what_read_returns() {
        let dir = TempDir::new("size");
        let ctx = ctx_as(&dir, "agent");
        let big = run(&ctx, serde_json::json!({ "op": "write", "name": "大", "body": "あ".repeat(NOTE_MAX_CHARS + 1) })).await;
        assert_eq!(big.outcome, Outcome::TooLarge);
        assert!(dir.note("doing/agent - 大.md").is_none());

        let full = "あ".repeat(NOTE_MAX_CHARS);
        assert_eq!(run(&ctx, serde_json::json!({ "op": "write", "name": "満", "body": full })).await.outcome, Outcome::Ok);
        let over = run(&ctx, serde_json::json!({ "op": "append", "name": "満", "body": "い" })).await;
        assert_eq!(over.outcome, Outcome::TooLarge, "{}", over.text);
        assert!(over.text.contains("別の仕事名"), "次の手を名指しする: {}", over.text);
        assert_eq!(dir.note("doing/agent - 満.md").unwrap().chars().count(), NOTE_MAX_CHARS);

        let read = run(&ctx, serde_json::json!({ "op": "read", "name": "満" })).await;
        assert!(read.text.ends_with(&full), "ツールで書いた付箋は read で全文が返る");
    }

    /// **書けるのは自分の付箋だけ。** 判定は ctx の id で、仕事名を知っていても届かない。
    #[tokio::test]
    async fn other_servants_notes_are_readable_but_never_writable() {
        let dir = TempDir::new("owner");
        dir.put("doing/agent_2 - 検索.md", "ジェミーのメモ");
        let ctx = ctx_as(&dir, "agent");

        for args in [
            serde_json::json!({ "op": "append", "name": "検索", "body": "横から" }),
            serde_json::json!({ "op": "move", "name": "検索", "to": "done" }),
            serde_json::json!({ "op": "remove", "name": "検索" }),
            serde_json::json!({ "op": "remove", "name": "検索", "owner": "agent_2" }),
            serde_json::json!({ "op": "append", "name": "何でも", "body": "x", "owner": "ジェミー" }),
        ] {
            let result = run(&ctx, args.clone()).await;
            assert_eq!(result.outcome, Outcome::NotOwner, "{args}: {}", result.text);
            assert!(result.text.contains("agent_2（ジェミー）"), "持ち主を名指しする: {}", result.text);
        }
        assert_eq!(dir.note("doing/agent_2 - 検索.md").as_deref(), Some("ジェミーのメモ"));

        // 読むのは id でも表示名でも通る。
        for owner in ["agent_2", "ジェミー"] {
            let read = run(&ctx, serde_json::json!({ "op": "read", "name": "検索", "owner": owner })).await;
            assert_eq!(read.outcome, Outcome::Ok);
            assert!(read.text.ends_with("ジェミーのメモ"), "{}", read.text);
        }
        // `owner` なしでは自分の付箋として探す。無ければ、同じ仕事名の持ち主を案内する。
        let mine = run(&ctx, serde_json::json!({ "op": "read", "name": "検索" })).await;
        assert_eq!(mine.outcome, Outcome::NotFound);
        assert!(mine.text.contains("agent_2（ジェミー）"), "{}", mine.text);
    }

    /// 遷移: `doing ⇄ on-hold`、`→ done`。**`done` からは動かない。凍結は no-op より先。**
    #[tokio::test]
    async fn move_follows_the_transition_table_and_done_is_frozen() {
        let dir = TempDir::new("move");
        let ctx = ctx_as(&dir, "agent");
        run(&ctx, serde_json::json!({ "op": "write", "name": "t", "body": "x" })).await;
        let mv = |to: &'static str| serde_json::json!({ "op": "move", "name": "t", "to": to });

        let noop = run(&ctx, mv("doing")).await;
        assert_eq!(noop.outcome, Outcome::Noop);

        let held = run(&ctx, mv("on-hold")).await;
        assert_eq!(held.outcome, Outcome::Ok, "{}", held.text);
        assert!(held.text.contains("`doing` → `on-hold`"), "実際の遷移を書く: {}", held.text);
        assert!(dir.note("on-hold/agent - t.md").is_some() && dir.note("doing/agent - t.md").is_none());

        assert_eq!(run(&ctx, mv("doing")).await.outcome, Outcome::Ok);
        let done = run(&ctx, mv("done")).await;
        assert!(done.text.contains("`doing` → `done`"), "{}", done.text);

        for to in ["doing", "on-hold", "done"] {
            let frozen = run(&ctx, mv(to)).await;
            assert_eq!(frozen.outcome, Outcome::Frozen, "done → {to}: {}", frozen.text);
        }
        assert_eq!(dir.note("done/agent - t.md").as_deref(), Some("x"), "盤面は変わらない");

        let bad = run(&ctx, serde_json::json!({ "op": "move", "name": "t", "to": "archive" })).await;
        assert_eq!(bad.outcome, Outcome::Invalid);
        assert!(!dir.0.join(BLACKBOARD_DIR).join("archive").exists(), "3 値の外のフォルダを作らない");

        dir.put("agent - 直下.md", "x");
        let misplaced = run(&ctx, serde_json::json!({ "op": "move", "name": "直下", "to": "done" })).await;
        assert_eq!(misplaced.outcome, Outcome::Misplaced);
    }

    /// `remove` は場所を問わない。ごみ箱が無い環境では消さずに失敗する。
    #[tokio::test]
    async fn remove_reaches_own_notes_wherever_they_are() {
        let dir = TempDir::new("remove");
        let ctx = ctx_as(&dir, "agent");
        for rel in ["done/agent - a.md", "agent - b.md", "archive/agent - c.md"] {
            dir.put(rel, "x");
        }
        for (task, rel) in [("a", "done/agent - a.md"), ("b", "agent - b.md"), ("c", "archive/agent - c.md")] {
            let result = run(&ctx, serde_json::json!({ "op": "remove", "name": task })).await;
            if result.outcome == Outcome::Error {
                assert!(dir.note(rel).is_some(), "失敗時に完全削除へ倒さない");
                continue;
            }
            assert_eq!(result.outcome, Outcome::Ok, "{}", result.text);
            assert!(dir.note(rel).is_none());
        }
    }

    /// `run` や手作業で同じ名前が 2 箇所にできた形。どちらかを黙って選ばない。
    #[tokio::test]
    async fn duplicates_are_refused_by_name_until_removed() {
        let dir = TempDir::new("dup");
        let ctx = ctx_as(&dir, "agent");
        dir.put("doing/agent - t.md", "A");
        dir.put("done/agent - t.md", "B");

        for args in [
            serde_json::json!({ "op": "append", "name": "t", "body": "x" }),
            serde_json::json!({ "op": "move", "name": "t", "to": "on-hold" }),
        ] {
            assert_eq!(run(&ctx, args).await.outcome, Outcome::Ambiguous);
        }
        assert_eq!(dir.note("doing/agent - t.md").as_deref(), Some("A"));
        let read = run(&ctx, serde_json::json!({ "op": "read", "name": "t" })).await;
        assert!(read.text.contains("2 箇所"), "{}", read.text);
    }

    /// 盤面は本文を返さず、持ち主を `id（表示名）` で書く。旧形式と持ち主なしも場所つきで出る。
    #[tokio::test]
    async fn list_shows_the_board_without_bodies() {
        let dir = TempDir::new("list");
        dir.put("doing/agent - 調査.md", "秘密の本文");
        dir.put("done/agent_2 - 検索.md", "x");
        dir.put("doing/ルナ - 旧形式.md", "x");
        dir.put("まとめ.md", "x");

        let result = run(&ctx_as(&dir, "agent"), serde_json::json!({ "op": "list" })).await;
        let text = result.text;
        assert!(!text.contains("秘密の本文"), "{text}");
        assert!(text.contains("agent（ザリ・自分）: 「調査」"), "{text}");
        assert!(text.contains("agent_2（ジェミー）: 「検索」"), "{text}");
        assert!(text.contains("ルナ（村に居ない持ち主）"), "{text}");
        assert!(text.contains("`blackboard/まとめ.md`"), "{text}");
        let pos = |needle: &str| text.find(needle).unwrap_or_else(|| panic!("{needle}: {text}"));
        assert!(pos("[`doing`]") < pos("[`done`]") && pos("[`done`]") < pos("[状態なし"), "盤面の並び: {text}");
    }

    /// 付箋が溜まっても出力は有界。落とした数を場所ごとに書く。
    #[tokio::test]
    async fn list_is_bounded_and_counts_what_it_dropped() {
        let dir = TempDir::new("list-big");
        for i in 0..400 {
            dir.put(&format!("done/agent - {}{i:03}.md", "長い仕事名".repeat(6)), "x");
        }
        let result = run(&ctx_as(&dir, "agent"), serde_json::json!({ "op": "list" })).await;
        let shown = result.text.matches("\n- ").count();
        assert!(shown < 400, "全部は出ない");
        assert!(result.text.contains(&format!("`done` {}", 400 - shown)), "落とした数が合う: …{}", &result.text[result.text.len().saturating_sub(400)..]);
        // 打ち切り文ぶんだけ上限を超えてよい（行の境界でしか止めない）。
        assert!(result.text.chars().count() < MAX_OUTPUT_CHARS + 600);
    }

    #[tokio::test]
    async fn bad_arguments_are_refused_with_the_missing_piece_named() {
        let dir = TempDir::new("args");
        let ctx = ctx_as(&dir, "agent");
        for args in [
            serde_json::json!({ "op": "write", "body": "x" }),
            serde_json::json!({ "op": "write", "name": "t" }),
            serde_json::json!({ "op": "write", "name": " . ", "body": "x" }),
            serde_json::json!({ "op": "delete", "name": "t" }),
            serde_json::json!({}),
        ] {
            assert_eq!(run(&ctx, args.clone()).await.outcome, Outcome::Invalid, "{args}");
        }
        assert!(!dir.0.join(BLACKBOARD_DIR).exists());
    }

    /// 英語の村へ届く面（提示と結果）に日本語が混ざらない。
    #[tokio::test]
    async fn the_english_face_contains_no_japanese() {
        let spec = BlackboardTool.spec(Language::En);
        let rendered = format!("{}{}", spec.description, spec.parameters);
        assert_eq!(ja_chars(&rendered), 0, "{rendered}");

        let dir = TempDir::new("en");
        let mut ctx = ctx_as(&dir, "agent");
        ctx.language = Language::En;
        ctx.agent_names = vec![(AgentId::from("agent"), "Zari".to_owned())];
        dir.put("done/agent - old.md", "x");
        for args in [
            serde_json::json!({ "op": "write", "name": "survey", "body": "x" }),
            serde_json::json!({ "op": "write", "name": "survey", "body": "x" }),
            serde_json::json!({ "op": "append", "name": "old", "body": "x" }),
            serde_json::json!({ "op": "move", "name": "survey", "to": "done" }),
            serde_json::json!({ "op": "move", "name": "nope", "to": "done" }),
            serde_json::json!({ "op": "list" }),
            serde_json::json!({ "op": "bogus" }),
        ] {
            let result = run(&ctx, args.clone()).await;
            assert_eq!(ja_chars(&result.text), 0, "{args}: {}", result.text);
        }
    }

    /// 説明文は全員の毎ターンに乗る固定費。5 点が入っていることと、字数を留める。
    #[test]
    fn the_description_carries_the_five_rules() {
        let ja = BlackboardTool.description(Language::Ja);
        for needle in ["仕事 1 つに 1 枚", "まず list", "on-hold（止めるよう言われて、自分で移した", "write は新規作成だけ", "done の付箋は動かせず", "返信で返す"] {
            assert!(ja.contains(needle), "{needle}");
        }
        assert!(ja.chars().count() < 700, "ja {} 字", ja.chars().count());
    }
}
