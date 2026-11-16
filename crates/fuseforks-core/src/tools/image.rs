//! 同梱ツール `image` — 画像を 1 枚生成する（Spec 76 / `image_gen_contract`）。
//!
//! # 3 つの部品
//!
//! - [`ImageGenSlot`] — いま効いている設定（アプリ全体で 1 つ。ホストが埋める）。**空なら提示しない**
//!   （提示の門の 2 段目 = `is_active`。1 段目の `enabledTools` は名前の表が受ける）
//! - [`TurnImages`] — そのターンの画像の台帳。参照番号（利用者の添付 → そのターンに生成した画像の順に
//!   1 から）と、接続先へ送った回数（1 ターン 4 回まで）を持つ。**ターンの間だけ生きる**
//! - [`ImageTool`] — 引数を検査し、台帳から参照を引き、ワイヤ（`crate::image_gen`）へ送り、元を作業フォルダへ、
//!   写しを台帳へ置いて、写しをモデルへ返す
//!
//! **写しは `tool_image::normalize_bytes` の 1 実装**（S76-1）で、会話ペイン（返事の発話の添付）・モデル
//! （`ToolOutput.images`）・参照の 3 つが同じバイト列を使う。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Instant;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::error::CoreResult;
use crate::image_gen::{
    GenerateError, ImageAspect, ImageGenConfig, ImageGenFailure, ImageRequest, ImageUsage,
};
use crate::llm::PromptAttachment;
use crate::tool::{AgentTool, ToolContext, ToolOutput};
use crate::world::Language;

/// ツールの名前。
pub const IMAGE_TOOL_NAME: &str = "image";

/// 1 ターンで接続先へ送れる生成の要求の数（契約の不変条件 11。**値は P4 で見直す**）。
pub const IMAGES_PER_TURN: usize = 4;

/// 1 回の生成で渡せる参照の数。
pub const MAX_REFERENCES: usize = 3;

/// 元のバイト列を置く作業フォルダの中のフォルダ（契約の不変条件 10 (a)）。
pub const GENERATED_DIR: &str = "generated_images";

/// いま効いている画像生成の設定（**ホストが `is_active` の真偽で埋める・空ける**）。
#[derive(Debug, Clone)]
pub struct ActiveImageGen {
    /// 選んでいる接続先の枠。
    pub config: ImageGenConfig,
    /// 鍵（資格情報ストアから読んだ値。**ログにも本文にも出さない**）。
    pub key: String,
    /// 縦横比の既定（引数が省かれたとき）。
    pub default_aspect: ImageAspect,
}

/// 画像生成の設定の差し込み口。ホストとツールが同じ `Arc` を持つ。
///
/// **保存の後に差し替えると、次の呼び出しから効く**（提示も実行も呼び出しの瞬間に読む）。
#[derive(Debug, Default)]
pub struct ImageGenSlot {
    current: RwLock<Option<Arc<ActiveImageGen>>>,
}

impl ImageGenSlot {
    /// 空の差し込み口（設定が無効）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 差し替える。`None` で空ける（`is_active` が偽）。
    pub fn set(&self, active: Option<ActiveImageGen>) {
        let mut slot = self.current.write().unwrap_or_else(std::sync::PoisonError::into_inner);
        *slot = active.map(Arc::new);
    }

    /// いま効いている設定。
    pub fn current(&self) -> Option<Arc<ActiveImageGen>> {
        self.current
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// 生成した 1 枚の写し（返事の発話に付けるもの）。
#[derive(Debug, Clone)]
pub struct GeneratedCopy {
    /// 写し（JPEG か WebP）。
    pub image: PromptAttachment,
    /// 表示用の名前（作業フォルダに保存したファイル名、保存しなかったときは `generated-<番号>.<ext>`）。
    pub file_name: String,
}

#[derive(Debug, Default)]
struct TurnImagesInner {
    /// 参照番号の順（index + 1 が番号）。利用者の添付 → 生成した写し。
    images: Vec<PromptAttachment>,
    /// 生成した写し（返事の発話に付ける）。
    generated: Vec<GeneratedCopy>,
    /// 接続先へ送った生成の要求の数。
    sent: usize,
}

/// そのターンの画像の台帳（契約の不変条件 5 / 11）。
///
/// **ターンの間だけ生きる** — ターンの層が作り、`ToolContext` で呼び出しへ渡し、ターンの終わりに
/// 生成した写しを取り出す。次のターンに番号は残らない（添付の 1 ターン限りは不変）。
#[derive(Debug, Default)]
pub struct TurnImages {
    inner: Mutex<TurnImagesInner>,
}

impl TurnImages {
    /// 利用者がそのターンに添付した画像（届いた順）で始める。画像以外は呼び出し側が抜いておく。
    pub fn new(user_images: Vec<PromptAttachment>) -> Self {
        Self {
            inner: Mutex::new(TurnImagesInner {
                images: user_images,
                ..TurnImagesInner::default()
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, TurnImagesInner> {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// 番号の付いた画像の数。
    pub fn len(&self) -> usize {
        self.lock().images.len()
    }

    /// 空か。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 参照番号（1 始まり）で引く。
    pub fn get(&self, number: usize) -> Option<PromptAttachment> {
        number
            .checked_sub(1)
            .and_then(|index| self.lock().images.get(index).cloned())
    }

    /// 送る前に 1 回ぶんを数える。上限に達していれば `None`、通れば何回目か（1 始まり）。
    pub fn reserve(&self) -> Option<usize> {
        let mut inner = self.lock();
        if inner.sent >= IMAGES_PER_TURN {
            return None;
        }
        inner.sent += 1;
        Some(inner.sent)
    }

    /// 生成した写しを台帳へ置き、参照番号を返す。
    pub fn push_generated(&self, copy: GeneratedCopy) -> usize {
        let mut inner = self.lock();
        inner.images.push(copy.image.clone());
        inner.generated.push(copy);
        inner.images.len()
    }

    /// 生成した写しを取り出す（ターンの終わりに 1 回。返事の発話に付ける）。
    pub fn take_generated(&self) -> Vec<GeneratedCopy> {
        std::mem::take(&mut self.lock().generated)
    }
}

/// 同梱ツール `image`。
pub struct ImageTool {
    slot: Arc<ImageGenSlot>,
    http: reqwest::Client,
}

impl ImageTool {
    /// 差し込み口を共有して作る。
    pub fn new(slot: Arc<ImageGenSlot>) -> Self {
        Self {
            slot,
            http: reqwest::Client::new(),
        }
    }
}

/// 生成を始めずに断った理由（ログの `outcome=`）。8 区分の外。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusal {
    /// 設定が無効（呼び出しの時に見た `is_active` が偽）。
    Inactive,
    /// 1 ターンの上限。
    Limit,
    /// 参照番号が正しくない。
    BadReference,
    /// 引数が正しくない（`prompt` が無い・`aspect` が知らない語・`references` が数の配列でない）。
    BadArgs,
}

impl Refusal {
    fn as_str(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Limit => "limit",
            Self::BadReference => "bad_reference",
            Self::BadArgs => "bad_args",
        }
    }
}

/// 引数を読んだ結果。
struct Args {
    prompt: String,
    aspect: Option<ImageAspect>,
    references: Vec<usize>,
}

/// 引数を読む。読めなければ村の言語の理由。
fn parse_args(args: &Value, language: Language) -> Result<Args, String> {
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if prompt.is_empty() {
        return Err(match language {
            Language::Ja => "prompt（描く内容）が空です。".to_owned(),
            Language::En => "prompt (what to draw) is empty.".to_owned(),
        });
    }
    let aspect = match args.get("aspect") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_str().and_then(ImageAspect::parse) {
            Some(aspect) => Some(aspect),
            None => {
                return Err(match language {
                    Language::Ja => "aspect は square / landscape / portrait のどれかです。".to_owned(),
                    Language::En => "aspect must be one of square / landscape / portrait.".to_owned(),
                });
            }
        },
    };
    let references = match args.get("references") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => {
            let mut numbers = Vec::with_capacity(items.len());
            for item in items {
                match item.as_u64().and_then(|n| usize::try_from(n).ok()) {
                    Some(n) => numbers.push(n),
                    None => {
                        return Err(match language {
                            Language::Ja => "references は参照番号（1 以上の整数）の配列です。".to_owned(),
                            Language::En => {
                                "references must be an array of reference numbers (integers from 1).".to_owned()
                            }
                        });
                    }
                }
            }
            numbers
        }
        Some(_) => {
            return Err(match language {
                Language::Ja => "references は参照番号の配列です。".to_owned(),
                Language::En => "references must be an array of reference numbers.".to_owned(),
            });
        }
    };
    Ok(Args {
        prompt: prompt.to_owned(),
        aspect,
        references,
    })
}

/// 参照番号を検査する。通れば番号の並び、落ちれば村の言語の理由。
fn check_references(references: &[usize], available: usize, language: Language) -> Result<(), String> {
    if references.is_empty() {
        return Ok(());
    }
    if available == 0 {
        return Err(match language {
            Language::Ja => "このターンには参照できる画像がありません。references を省くか、画像を添付し直してもらってください。".to_owned(),
            Language::En => "There is no image to reference in this turn. Leave out references, or ask for the image to be attached again.".to_owned(),
        });
    }
    if references.len() > MAX_REFERENCES {
        return Err(match language {
            Language::Ja => format!("参照は {MAX_REFERENCES} 枚までです。"),
            Language::En => format!("At most {MAX_REFERENCES} references."),
        });
    }
    if let Some(bad) = references.iter().find(|n| **n == 0 || **n > available) {
        return Err(match language {
            Language::Ja => format!("参照番号 {bad} はありません。使える番号は 1〜{available} です。"),
            Language::En => format!("There is no reference number {bad}. Use 1 to {available}."),
        });
    }
    Ok(())
}

/// 生成を始めずに断った本文。
fn refusal_text(refusal: Refusal, detail: &str, language: Language) -> String {
    match (refusal, language) {
        (Refusal::Inactive, Language::Ja) => "画像生成は今は使えません（システム設定 ＞ 外部連携 ＞ 画像生成が無効です）。作れないことを伝えてください。".to_owned(),
        (Refusal::Inactive, Language::En) => "Image generation is not available right now (System settings > Integrations > Image generation is off). Say that you cannot make the image.".to_owned(),
        (Refusal::Limit, Language::Ja) => format!("このターンでは画像生成をもう {IMAGES_PER_TURN} 回使いました。これ以上は生成しません。できた画像で答えるか、次のターンで頼み直すよう伝えてください。"),
        (Refusal::Limit, Language::En) => format!("Image generation has already been used {IMAGES_PER_TURN} times in this turn; no more images will be made. Answer with what you have, or ask to be asked again next turn."),
        (Refusal::BadReference | Refusal::BadArgs, _) => detail.to_owned(),
    }
}

/// 8 区分の失敗の本文（理由と次の手）。
fn failure_text(failure: &ImageGenFailure, config: &ImageGenConfig, language: Language) -> String {
    let provider = config.provider.as_str();
    match (failure, language) {
        (ImageGenFailure::Auth, Language::Ja) => format!("画像生成の鍵が受け付けられませんでした（{provider} が 401 / 403 を返しました）。システム設定の鍵を確かめるよう伝えてください。"),
        (ImageGenFailure::Auth, Language::En) => format!("The image generation key was rejected ({provider} returned 401 / 403). Ask for the key in System settings to be checked."),
        (ImageGenFailure::RateLimited { detail }, Language::Ja) => format!("回数制限に当たりました（{detail}）。少し待ってから頼むよう伝えてください。"),
        (ImageGenFailure::RateLimited { detail }, Language::En) => format!("Hit a rate limit ({detail}). Ask to try again after a short wait."),
        (ImageGenFailure::Blocked { reason }, Language::Ja) => format!("接続先が生成を断りました（理由: {reason}）。内容を言い換えるか、作れないことを伝えてください。"),
        (ImageGenFailure::Blocked { reason }, Language::En) => format!("The provider refused to generate (reason: {reason}). Rephrase, or say that you cannot make it."),
        (ImageGenFailure::Timeout, Language::Ja) => format!("接続先の応答が {} 秒以内に返りませんでした。", config.timeout().as_secs()),
        (ImageGenFailure::Timeout, Language::En) => format!("The provider did not answer within {} seconds.", config.timeout().as_secs()),
        (ImageGenFailure::Network { detail }, Language::Ja) => format!("接続先へ繋がりませんでした（{detail}）。"),
        (ImageGenFailure::Network { detail }, Language::En) => format!("Could not reach the provider ({detail})."),
        (ImageGenFailure::Shape { detail }, Language::Ja) => format!("応答の形が想定と違い、画像を受け取れませんでした（{detail}）。"),
        (ImageGenFailure::Shape { detail }, Language::En) => format!("The response was not in the expected shape and no image came back ({detail})."),
        (ImageGenFailure::Api { status, detail }, Language::Ja) => format!("接続先がエラーを返しました（status={status}: {detail}）。"),
        (ImageGenFailure::Api { status, detail }, Language::En) => format!("The provider returned an error (status={status}: {detail})."),
        (ImageGenFailure::Config { detail }, Language::Ja) => format!("画像生成の設定に誤りがあります（{detail}）。システム設定の画像生成を確かめるよう伝えてください。"),
        (ImageGenFailure::Config { detail }, Language::En) => format!("The image generation settings are wrong ({detail}). Ask for them to be checked in System settings."),
    }
}

/// 打ち切りの本文。
fn interrupted_text(language: Language) -> &'static str {
    match language {
        Language::Ja => "打ち切られたため、画像の生成をやめました（接続先では生成が続いて費用がかかることがあります）。",
        Language::En => "The turn was interrupted, so image generation was abandoned (the provider may still finish and charge for it).",
    }
}

/// 作業フォルダへの保存の結果。
enum Saved {
    /// 保存した（作業フォルダからの相対パス）。
    Path(String),
    /// 作業フォルダが無い。
    NoWorkDir,
    /// 書けなかった。
    Failed(String),
}

impl Saved {
    fn label(&self) -> &'static str {
        match self {
            Self::Path(_) => "yes",
            Self::NoWorkDir => "no_work_dir",
            Self::Failed(_) => "failed",
        }
    }
}

/// 元のバイト列を `<work_dir>/generated_images/<YYYYMMDD-HHMMSS>-<agent_id>-<番号>.<ext>` へ置く。
///
/// **上書きしない**（`create_new`。在れば `-2`・`-3` を足す）。`generated_images` がリンク
/// （symlink / ジャンクション）なら書かない — 作業フォルダの外へ書かせない（`failures.md` #154 の形）。
async fn save_original(
    work_dir: &Path,
    agent_id: &str,
    number: usize,
    ext: &str,
    bytes: &[u8],
) -> Result<String, String> {
    use tokio::io::AsyncWriteExt as _;
    let dir = work_dir.join(GENERATED_DIR);
    if tokio::fs::symlink_metadata(&dir)
        .await
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(format!("{GENERATED_DIR} is a link"));
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|err| err.to_string())?;
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let stem = format!("{stamp}-{agent_id}-{number}");
    for suffix in 1..=100u32 {
        let name = if suffix == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{suffix}.{ext}")
        };
        let path: PathBuf = dir.join(&name);
        match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(mut file) => {
                file.write_all(bytes).await.map_err(|err| err.to_string())?;
                file.flush().await.map_err(|err| err.to_string())?;
                return Ok(format!("{GENERATED_DIR}/{name}"));
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.to_string()),
        }
    }
    Err("too many files with the same name".to_owned())
}

/// 成功の本文。
fn success_text(number: usize, saved: &Saved, language: Language) -> String {
    match (saved, language) {
        (Saved::Path(path), Language::Ja) => format!("画像を生成しました（参照番号 {number}）。作業フォルダの {path} に保存しました。画像はこの結果に付いており、会話の画面にも出ます。"),
        (Saved::Path(path), Language::En) => format!("Generated the image (reference number {number}). Saved to {path} in the work folder. The image is attached to this result and also appears in the conversation."),
        (Saved::NoWorkDir, Language::Ja) => format!("画像を生成しました（参照番号 {number}）。作業フォルダが設定されていないので、ファイルには保存していません（会話の画面には出ます）。"),
        (Saved::NoWorkDir, Language::En) => format!("Generated the image (reference number {number}). No work folder is set, so it was not saved as a file (it still appears in the conversation)."),
        (Saved::Failed(err), Language::Ja) => format!("画像を生成しました（参照番号 {number}）。作業フォルダへの保存には失敗しました（{err}）。会話の画面には出ます。"),
        (Saved::Failed(err), Language::En) => format!("Generated the image (reference number {number}). Saving it to the work folder failed ({err}); it still appears in the conversation."),
    }
}

/// `image gen:` の 1 行（契約の不変条件 13）。**プロンプトの本文は書かない。**
#[allow(clippy::too_many_arguments)]
fn log_line(
    agent: &str,
    config: Option<&ImageGenConfig>,
    aspect: Option<ImageAspect>,
    refs: usize,
    outcome: &str,
    elapsed_ms: u128,
    bytes: usize,
    saved: &str,
    usage: ImageUsage,
) {
    let (provider, model) = config.map_or(("-", "-".to_owned()), |c| {
        (c.provider.as_str(), c.model_or_default().to_owned())
    });
    let mut line = format!(
        "image gen: agent={agent} provider={provider} model={model} aspect={} refs={refs} outcome={outcome} elapsed_ms={elapsed_ms} bytes={bytes} saved={saved}",
        aspect.map_or("-", ImageAspect::as_str),
    );
    if let Some(n) = usage.input_tokens {
        line.push_str(&format!(" in_tokens={n}"));
    }
    if let Some(n) = usage.output_tokens {
        line.push_str(&format!(" out_tokens={n}"));
    }
    if let Some(n) = usage.cost_ticks {
        line.push_str(&format!(" cost_ticks={n}"));
    }
    crate::note!("{line}");
}

#[async_trait]
impl AgentTool for ImageTool {
    fn name(&self) -> &str {
        IMAGE_TOOL_NAME
    }

    fn description(&self, language: Language) -> String {
        match language {
            Language::Ja => format!(
                "画像を 1 枚生成する（システム設定で選んだ画像生成 API へプロンプトを送る。費用がかかる）。\
                 絵や図を頼まれたとき、説明に画像が要るときに呼ぶ。1 ターン {IMAGES_PER_TURN} 回まで。\
                 references に参照番号を渡すと、その画像をもとに描き直す・寄せる（{MAX_REFERENCES} 枚まで）。\
                 参照番号は、このターンに利用者が添付した画像が届いた順に 1 から、このターンに生成した画像がその続き\
                 （生成の結果に番号が出る）。生成した画像はこの結果に付いてあなたにも見え、作業フォルダの \
                 {GENERATED_DIR}/ に保存される。"
            ),
            Language::En => format!(
                "Generate one image (sends the prompt to the image generation API chosen in System settings; this costs money). \
                 Call it when asked for a picture or diagram, or when an explanation needs one. Up to {IMAGES_PER_TURN} times per turn. \
                 Pass reference numbers in references to redraw or follow those images (up to {MAX_REFERENCES}). \
                 Reference numbers start at 1 with the images the user attached in this turn, in the order they arrived, \
                 followed by the images generated in this turn (each result tells you its number). The generated image is \
                 attached to the result so you can see it, and is saved under {GENERATED_DIR}/ in the work folder."
            ),
        }
    }

    fn parameters(&self, language: Language) -> Value {
        let (prompt, aspect, references) = match language {
            Language::Ja => (
                "描く内容。被写体・構図・画風・色を具体的に書く",
                "縦横比。省くとシステム設定の既定",
                "参照する画像の番号（1 から。3 つまで）",
            ),
            Language::En => (
                "What to draw. Be specific about the subject, composition, style and colours",
                "Aspect ratio. Leave out to use the default from System settings",
                "Reference numbers of images to follow (from 1, up to 3)",
            ),
        };
        json!({
            "type": "object",
            "properties": {
                "prompt": { "type": "string", "description": prompt },
                "aspect": {
                    "type": "string",
                    "enum": ["square", "landscape", "portrait"],
                    "description": aspect
                },
                "references": {
                    "type": "array",
                    "items": { "type": "integer", "minimum": 1 },
                    "maxItems": MAX_REFERENCES,
                    "description": references
                }
            },
            "required": ["prompt"]
        })
    }

    /// **提示の門の 2 段目**（`is_active`）。空なら提示しない。1 段目（`enabledTools`）は名前の表が受ける。
    async fn spec_for(&self, ctx: &ToolContext) -> Option<crate::llm::ToolSpec> {
        self.slot.current().map(|_| self.spec(ctx.language))
    }

    async fn call(&self, ctx: &ToolContext, args: &Value) -> CoreResult<String> {
        self.call_with_media(ctx, args).await.map(|output| output.text)
    }

    async fn call_with_media(&self, ctx: &ToolContext, args: &Value) -> CoreResult<ToolOutput> {
        let language = ctx.language;
        let agent_label = ctx.agent_id.to_string();
        let agent = agent_label.as_str();
        let started = Instant::now();
        // 台帳はターンの層が渡す。渡っていない呼び出し（テスト）は、その場限りの空の台帳。
        let local;
        let ledger: &TurnImages = match ctx.turn_images.as_deref() {
            Some(ledger) => ledger,
            None => {
                local = TurnImages::default();
                &local
            }
        };
        let refuse = |refusal: Refusal,
                      detail: &str,
                      config: Option<&ImageGenConfig>,
                      aspect: Option<ImageAspect>,
                      refs: usize|
         -> CoreResult<ToolOutput> {
            log_line(agent, config, aspect, refs, refusal.as_str(), 0, 0, "-", ImageUsage::default());
            Ok(ToolOutput::text(refusal_text(refusal, detail, language)))
        };

        // 門は呼び出しの時にも見る（ターンの途中で設定が無効になったら送らない）。
        let Some(active) = self.slot.current() else {
            return refuse(Refusal::Inactive, "", None, None, 0);
        };
        let config = &active.config;
        let parsed = match parse_args(args, language) {
            Ok(parsed) => parsed,
            Err(reason) => return refuse(Refusal::BadArgs, &reason, Some(config), None, 0),
        };
        let aspect = parsed.aspect.unwrap_or(active.default_aspect);
        let refs_count = parsed.references.len();
        if let Err(reason) = check_references(&parsed.references, ledger.len(), language) {
            return refuse(Refusal::BadReference, &reason, Some(config), Some(aspect), refs_count);
        }
        let references: Vec<PromptAttachment> = parsed
            .references
            .iter()
            .filter_map(|n| ledger.get(*n))
            .collect();
        // 数えるのは送った要求だけ（生成を始めずに断ったものは数えない）。
        let Some(number) = ledger.reserve() else {
            return refuse(Refusal::Limit, "", Some(config), Some(aspect), refs_count);
        };

        let request = ImageRequest {
            prompt: &parsed.prompt,
            aspect,
            refs: &references,
        };
        let result =
            crate::image_gen::generate(&self.http, config, &active.key, &request, ctx.cancel.as_ref()).await;
        let elapsed_ms = started.elapsed().as_millis();
        let generated = match result {
            Ok(generated) => generated,
            Err(GenerateError::Interrupted) => {
                log_line(agent, Some(config), Some(aspect), refs_count, "interrupted", elapsed_ms, 0, "-", ImageUsage::default());
                return Ok(ToolOutput::text(interrupted_text(language)));
            }
            Err(GenerateError::Failed(failure)) => {
                log_line(agent, Some(config), Some(aspect), refs_count, failure.as_str(), elapsed_ms, 0, "-", ImageUsage::default());
                return Ok(ToolOutput::text(failure_text(&failure, config, language)));
            }
        };

        // 写しを先に作る（読めない画像なら保存もしない）。
        let copy = match crate::tool_image::normalize_bytes(&generated.bytes) {
            Ok(copy) => copy,
            Err(reason) => {
                let failure = ImageGenFailure::Shape {
                    detail: format!("the image could not be read ({})", reason.label()),
                };
                log_line(agent, Some(config), Some(aspect), refs_count, failure.as_str(), elapsed_ms, generated.bytes.len(), "-", generated.usage);
                return Ok(ToolOutput::text(failure_text(&failure, config, language)));
            }
        };

        let ext = generated.ext();
        let saved = match &ctx.work_dir {
            None => Saved::NoWorkDir,
            Some(dir) => match save_original(dir, agent, number, ext, &generated.bytes).await {
                Ok(path) => Saved::Path(path),
                Err(err) => Saved::Failed(err),
            },
        };
        let file_name = match &saved {
            Saved::Path(path) => path.rsplit('/').next().unwrap_or(path).to_owned(),
            Saved::NoWorkDir | Saved::Failed(_) => {
                let copy_ext = match copy.attachment.media_type {
                    crate::llm::PromptMediaType::Webp => "webp",
                    _ => "jpg",
                };
                format!("generated-{number}.{copy_ext}")
            }
        };
        let reference = ledger.push_generated(GeneratedCopy {
            image: copy.attachment.clone(),
            file_name,
        });
        log_line(agent, Some(config), Some(aspect), refs_count, "ok", elapsed_ms, generated.bytes.len(), saved.label(), generated.usage);
        Ok(ToolOutput {
            text: success_text(reference, &saved, language),
            images: vec![copy],
            ..ToolOutput::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::PromptMediaType;

    fn webp(n: u8) -> PromptAttachment {
        PromptAttachment::new(PromptMediaType::Webp, format!("data{n}"))
    }

    #[test]
    fn numbers_run_from_the_user_images_into_the_generated_ones() {
        let ledger = TurnImages::new(vec![webp(1), webp(2)]);
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger.get(1).unwrap().data, "data1");
        assert!(ledger.get(0).is_none());
        assert!(ledger.get(3).is_none());
        let number = ledger.push_generated(GeneratedCopy {
            image: webp(9),
            file_name: "x.webp".to_owned(),
        });
        assert_eq!(number, 3, "生成した画像は利用者の添付の続き");
        assert_eq!(ledger.get(3).unwrap().data, "data9");
        assert_eq!(ledger.take_generated().len(), 1);
        assert!(ledger.take_generated().is_empty(), "取り出しは 1 回だけ");
    }

    #[test]
    fn the_turn_cap_counts_sent_requests() {
        let ledger = TurnImages::default();
        for expected in 1..=IMAGES_PER_TURN {
            assert_eq!(ledger.reserve(), Some(expected));
        }
        assert_eq!(ledger.reserve(), None, "5 回目は送らない");
    }

    #[test]
    fn references_are_checked_before_anything_is_sent() {
        let ja = Language::Ja;
        assert!(check_references(&[], 0, ja).is_ok());
        assert!(check_references(&[1], 0, ja).unwrap_err().contains("参照できる画像がありません"));
        assert!(check_references(&[1, 2, 3, 1], 5, ja).unwrap_err().contains("3 枚まで"));
        assert!(check_references(&[0], 2, ja).unwrap_err().contains("参照番号 0"));
        assert!(check_references(&[3], 2, ja).unwrap_err().contains("1〜2"));
        assert!(check_references(&[2, 1], 2, ja).is_ok());
    }

    #[test]
    fn args_are_read_strictly() {
        let ja = Language::Ja;
        assert!(parse_args(&json!({}), ja).is_err());
        assert!(parse_args(&json!({"prompt": "  "}), ja).is_err());
        assert!(parse_args(&json!({"prompt": "cat", "aspect": "wide"}), ja).is_err());
        assert!(parse_args(&json!({"prompt": "cat", "references": "1"}), ja).is_err());
        assert!(parse_args(&json!({"prompt": "cat", "references": [-1]}), ja).is_err());
        let ok = parse_args(&json!({"prompt": " cat ", "aspect": "portrait", "references": [2]}), ja).unwrap();
        assert_eq!(ok.prompt, "cat");
        assert_eq!(ok.aspect, Some(ImageAspect::Portrait));
        assert_eq!(ok.references, vec![2]);
    }

    #[test]
    fn an_empty_slot_hides_the_tool() {
        let slot = Arc::new(ImageGenSlot::new());
        assert!(slot.current().is_none());
        slot.set(Some(ActiveImageGen {
            config: ImageGenConfig::new(crate::image_gen::ImageProvider::Xai),
            key: "k".to_owned(),
            default_aspect: ImageAspect::Square,
        }));
        assert!(slot.current().is_some());
        slot.set(None);
        assert!(slot.current().is_none());
    }
}
