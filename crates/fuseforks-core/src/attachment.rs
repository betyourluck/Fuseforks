//! 添付 — 検証・置き場・GC（Spec 23 = 画像 / Spec 36 = 音声・動画・PDF。
//! 契約は `attachment_contract`）。
//!
//! **実体はファイル、メッセージは参照。** 添付の本体は
//! `{workspace}/attachments/{uuid}.{ext}` に置き、[`Attachment`] は
//! id・形式・寸法・元ファイル名だけを運ぶ。base64 をメッセージにも redb にも積まない。
//!
//! **形式は閉じた列挙**（[`AttachmentFormat`]）。**判定は常にマジックバイトで、
//! 利用者のファイル名の拡張子は読まない** — 拡張子は誰でも書けるので、
//! それを信じると「中身と種別が食い違う添付」がワイヤまで届く。
//!
//! **変換するのは画像だけ**（UI 層が WebP を作る）。音声・動画は無変換で通す —
//! 変換器を同梱すると依存の桁が変わる（ffmpeg 系は純 Rust で揃わない）ので、
//! 受け付けない形式は入口で断る（Spec 36 D3）。
//!
//! **述語は種別ごとに別で、上限も種別ごとに別の定数**（Spec 36 D4 / D13）。
//! 1 つの定数を共有すると、片方を変えたときもう片方が黙って動く。
//! `validate_icon` とも**別の述語**にする — あちらは「2 種類のアイコンが
//! 1 つの上限を共有する」を不変条件として持ち、上限をパラメータ化すると
//! その不変条件が消える（Spec 23 rev3）。共有するのは [`is_webp`] だけ。
//!
//! アニメーション WebP は拒否する。Anthropic は先頭フレームしか読まず、
//! 「動く画像を送ったつもり」と「静止画 1 枚が届いた」の食い違いは
//! 画面からは見えないため、入口で断って理由を返す。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// 添付の置き場（ワークスペース直下のフォルダ名）。
pub const ATTACHMENT_DIR: &str = "attachments";

/// 画像（変換後 WebP）の許容上限。超えたら保存せず拒否する。
///
/// 元ファイル 10MB の門は UI 層が持つ（デコード前に弾く）。ここは
/// 変換を通さず IPC へ直接流し込む経路を塞ぐ側の門。
///
/// **名前が種別を名乗るのは Spec 36 D13。** 旧名 `ATTACHMENT_MAX_BYTES` は
/// 汎用に読めるので、種別が増えた時点で新種別の実装者が必ず誤用する
/// （「画像専用。触るな」の弁解コメントを要する名前は間違っている —
/// `shell.json` → `run.json` と同じ規律）。**値は 1 バイトも変えていない。**
pub const ATTACHMENT_IMAGE_MAX_BYTES: usize = 2 * 1024 * 1024;

/// 長辺の許容上限（px）。UI 層はこの値へ縮小してから送る契約（Spec 23 D3）。
///
/// 1568 は Anthropic 標準ティアの長辺。高解像度ティアの 2576 を使わないのは
/// Goal のため（1 枚 4,784 視覚トークン対 1,792。Spec 23 の表）。
pub const ATTACHMENT_IMAGE_MAX_EDGE_PX: u32 = 1568;

/// 音声の許容上限（Spec 36 D4）。
///
/// 基準は**その種別を運ぶ最も狭いワイヤ** = Gemini のインライン要求全体 20MB。
/// 10MB は base64 で 13.3MB になり、プロンプトの余白が 6.7MB 残る。
pub const ATTACHMENT_AUDIO_MAX_BYTES: usize = 10 * 1024 * 1024;

/// 動画の許容上限（Spec 36 D4）。
///
/// 12MB は base64 で 16.0MB。Gemini のインライン 20MB に対して余白 4.0MB で、
/// そこへシステムプロンプト・履歴・JSON の枠が乗る。
/// **短尺クリップ・画面録画向けで、長尺は対象外**（Files API を採らない D10 の
/// 帰結。画面にそう書く）。
pub const ATTACHMENT_VIDEO_MAX_BYTES: usize = 12 * 1024 * 1024;

/// PDF の許容上限（Spec 36 D4）。
///
/// Anthropic は単独なら 32MB・OpenAI は 50MB を許すが、**門は種別ごとに 1 つ**で
/// 基準は最も狭い Gemini（20MB）。ワイヤ別の門を作らないのは、同じファイルが
/// 宛先によって通ったり落ちたりすると規則が画面から読めないため。
pub const ATTACHMENT_PDF_MAX_BYTES: usize = 10 * 1024 * 1024;

/// 保持期間。起動時の GC がこれより古いファイルを消す（Spec 23 D9）。
pub const ATTACHMENT_RETENTION_DAYS: u64 = 30;

/// フォルダ全体の容量上限。超過分は古い順に消す（Spec 23 D9）。
///
/// **種別ごとのクォータは持たない**（Spec 36 D11）。動画が総量を食って他人の
/// 画像を押し出すことは許容する — 30 日の期限が主機構で、総量は非常弁。
/// 押し出しの頻度は [`GcReport::remaining_by_kind`] で観測してから判断する。
pub const ATTACHMENT_MAX_TOTAL_BYTES: u64 = 500 * 1024 * 1024;

/// 添付の種別（Spec 36。**閉じた列挙**）。
///
/// **`carries(provider, kind)` の引数になる側**（どのワイヤがどの種別を運べるか。
/// 実装は Spec 36 P3）。上限と検証の述語もこの粒度で分かれる。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentKind {
    /// 画像（WebP のみ。UI 層が変換する）。
    Image,
    /// 音声（mp3 / wav）。
    Audio,
    /// 動画（mp4）。
    Video,
    /// PDF。
    Pdf,
}

impl AttachmentKind {
    /// 全種別（GC の内訳や表の網羅検査で使う）。
    pub const ALL: [Self; 4] = [Self::Image, Self::Audio, Self::Video, Self::Pdf];

    /// この種別の許容上限（bytes）。
    pub fn max_bytes(self) -> usize {
        match self {
            Self::Image => ATTACHMENT_IMAGE_MAX_BYTES,
            Self::Audio => ATTACHMENT_AUDIO_MAX_BYTES,
            Self::Video => ATTACHMENT_VIDEO_MAX_BYTES,
            Self::Pdf => ATTACHMENT_PDF_MAX_BYTES,
        }
    }

    /// ログと計器に出す安定した名前（`kinds=image:N` の形）。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Audio => "audio",
            Self::Video => "video",
            Self::Pdf => "pdf",
        }
    }
}

/// 添付の形式（Spec 36。**閉じた列挙**）。
///
/// **種別より細かい** — 音声は mp3 と wav の 2 形式があり、保存する拡張子と
/// ワイヤへ載せる MIME はここで決まる。**メッセージが持つのはこちら**で、
/// 種別は [`AttachmentFormat::kind`] で導出する（ディスク上の真実を 1 つにする）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttachmentFormat {
    /// 画像。**既定** — Spec 23 の時代に書かれた記録は形式欄を持たず、
    /// それらはすべて WebP の画像だった。
    Webp,
    /// 音声（MPEG Audio Layer III）。
    Mp3,
    /// 音声（RIFF WAVE）。
    Wav,
    /// 動画（ISO BMFF / MP4）。
    Mp4,
    /// PDF。
    Pdf,
}

impl Default for AttachmentFormat {
    /// 既定は WebP。**欄を持たない既存レコードの読み戻し用**で、
    /// 新しい添付は必ず [`detect_format`] が決める。
    fn default() -> Self {
        Self::Webp
    }
}

impl AttachmentFormat {
    /// この形式が属する種別。
    pub fn kind(self) -> AttachmentKind {
        match self {
            Self::Webp => AttachmentKind::Image,
            Self::Mp3 | Self::Wav => AttachmentKind::Audio,
            Self::Mp4 => AttachmentKind::Video,
            Self::Pdf => AttachmentKind::Pdf,
        }
    }

    /// 保存に使う拡張子（**利用者のファイル名からは取らない**）。
    pub fn ext(self) -> &'static str {
        match self {
            Self::Webp => "webp",
            Self::Mp3 => "mp3",
            Self::Wav => "wav",
            Self::Mp4 => "mp4",
            Self::Pdf => "pdf",
        }
    }

    /// ワイヤへ載せる MIME 型（各社の inline_data / document / input_file 用）。
    pub fn mime(self) -> &'static str {
        match self {
            Self::Webp => "image/webp",
            Self::Mp3 => "audio/mpeg",
            Self::Wav => "audio/wav",
            Self::Mp4 => "video/mp4",
            Self::Pdf => "application/pdf",
        }
    }

    /// 保存済みファイルの拡張子から形式を引く（GC と読み出しの解決に使う）。
    ///
    /// **こちらは自分が書いた拡張子を読むので安全** — 利用者のファイル名を
    /// 解釈する経路ではない。
    fn from_ext(ext: &str) -> Option<Self> {
        match ext {
            "webp" => Some(Self::Webp),
            "mp3" => Some(Self::Mp3),
            "wav" => Some(Self::Wav),
            "mp4" => Some(Self::Mp4),
            "pdf" => Some(Self::Pdf),
            _ => None,
        }
    }
}

/// WebP コンテナのマジックバイト判定。
///
/// **`validate_icon` と添付の検証が共有する唯一の述語**（Spec 23 rev3）。
/// 先頭 `RIFF` + オフセット 8 から `WEBP`。
pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// RIFF WAVE のマジックバイト判定。
///
/// **WebP と先頭 4 バイトが同じ**（どちらも `RIFF`）ので、オフセット 8 の
/// フォーム型まで見ないと区別できない。
fn is_wav(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE"
}

/// MPEG Audio（mp3）のマジックバイト判定。
///
/// 2 経路ある — ID3v2 タグ付き（`ID3`）と、素のフレーム同期
/// （先頭 11 bit がすべて 1 = `0xFF` + 上位 3 bit）。
fn is_mp3(bytes: &[u8]) -> bool {
    if bytes.len() >= 3 && &bytes[0..3] == b"ID3" {
        return true;
    }
    bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0
}

/// ISO BMFF の major brand のうち、**動画として受け入れるもの**。
///
/// **閉じた許容にする理由は `ftyp` が動画専用ではないこと** — 同じ
/// コンテナ形式を `M4A `（音声）/ `heic` `mif1` `avif`（画像）/ `qt  `
/// （QuickTime）も名乗る。ブランドを見ずに通すと、**HEIC の写真が
/// 「動画」としてワイヤへ載る**（そして相手が拒む理由を画面で説明できない）。
const MP4_VIDEO_BRANDS: [&[u8; 4]; 10] = [
    b"isom", b"iso2", b"iso4", b"iso5", b"iso6", b"mp41", b"mp42", b"avc1", b"dash", b"mmp4",
];

/// MP4（動画）のマジックバイト判定。`ftyp` + 受け入れブランド。
fn is_mp4_video(bytes: &[u8]) -> bool {
    if bytes.len() < 12 || &bytes[4..8] != b"ftyp" {
        return false;
    }
    let brand: &[u8] = &bytes[8..12];
    MP4_VIDEO_BRANDS.iter().any(|b| b.as_slice() == brand)
}

/// PDF のマジックバイト判定。
fn is_pdf(bytes: &[u8]) -> bool {
    bytes.len() >= 5 && &bytes[0..5] == b"%PDF-"
}

/// バイト列から形式を判定する（**閉じた許容**。判らなければ `None`）。
///
/// **利用者のファイル名を一切見ない。** 拡張子は誰でも書けるので、それを
/// 信じると中身と種別が食い違ったままワイヤへ届く。
///
/// 判定順は「曖昧さの無いものから」— RIFF 系（WebP / WAV）はフォーム型で
/// 確定し、`ftyp` はブランドで確定し、mp3 の素フレーム同期は**最後**に置く
/// （2 バイトしか見ないので最も緩い判定）。
pub fn detect_format(bytes: &[u8]) -> Option<AttachmentFormat> {
    if is_webp(bytes) {
        return Some(AttachmentFormat::Webp);
    }
    if is_wav(bytes) {
        return Some(AttachmentFormat::Wav);
    }
    if is_pdf(bytes) {
        return Some(AttachmentFormat::Pdf);
    }
    if is_mp4_video(bytes) {
        return Some(AttachmentFormat::Mp4);
    }
    if is_mp3(bytes) {
        return Some(AttachmentFormat::Mp3);
    }
    None
}

/// 会話メッセージが運ぶ添付の参照。
///
/// 実体（バイト列）は持たない。持つと `AgentMessage` のシリアライズ経由で
/// base64 が redb と IPC へ流れ、「実体はファイル」の契約が崩れる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// ファイル名の幹（UUID v4）。実体は `{workspace}/attachments/{id}.{ext}`。
    pub id: String,
    /// 形式。**種別はここから導出する**（[`AttachmentFormat::kind`]）。
    ///
    /// **加算欄**（Spec 36）— この欄を持たない既存レコード（Spec 23 の時代に
    /// 書かれたもの）は既定の [`AttachmentFormat::Webp`] として読める。
    /// **それが事実として正しい**（当時の添付はすべて WebP の画像）。
    #[serde(default)]
    pub format: AttachmentFormat,
    /// 幅（px）。**画像のときだけ**（検証時に WebP ヘッダから読んだ実測値）。
    ///
    /// 音声・PDF に寸法は無い。0 で埋めずに `None` にするのは、
    /// **無い値を型で無いと言う**ため（0 を入れると「0px の画像」と区別できない）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// 高さ（px）。画像のときだけ。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// 元ファイル名（表示用）。**パス解決にも形式判定にも一切使わない。**
    pub file_name: String,
}

impl Attachment {
    /// 種別（`carries` の引数・表示・GC の内訳で使う）。
    pub fn kind(&self) -> AttachmentKind {
        self.format.kind()
    }
}

/// WebP から読み取った特徴。検証の中間表現。
struct WebpFeatures {
    /// 幅・高さ。VP8X のキャンバス寸法を優先し、無ければビットストリーム
    /// （VP8 / VP8L）の寸法。
    dimensions: Option<(u32, u32)>,
    /// アニメーションか（VP8X のフラグ、または ANIM / ANMF チャンクの存在）。
    animated: bool,
}

/// u24 リトルエンディアンを読む（VP8X の寸法欄）。
fn u24_le(bytes: &[u8]) -> u32 {
    u32::from(bytes[0]) | (u32::from(bytes[1]) << 8) | (u32::from(bytes[2]) << 16)
}

/// WebP のチャンクを歩いて寸法とアニメーションの有無を集める。
///
/// 壊れたチャンク（宣言サイズがファイル末尾を超える等）に当たったら
/// そこで打ち切り、**それまでに読めた分だけ**を返す。検証側は寸法が
/// 読めなければ拒否するので、壊れたファイルは通らない。
fn parse_features(bytes: &[u8]) -> WebpFeatures {
    let mut features = WebpFeatures {
        dimensions: None,
        animated: false,
    };
    let mut bitstream: Option<(u32, u32)> = None;
    let mut offset = 12usize;
    while offset + 8 <= bytes.len() {
        let fourcc: [u8; 4] = bytes[offset..offset + 4].try_into().unwrap_or([0; 4]);
        let size = u32::from_le_bytes(
            bytes[offset + 4..offset + 8].try_into().unwrap_or([0; 4]),
        ) as usize;
        let start = offset + 8;
        let Some(end) = start.checked_add(size) else {
            break;
        };
        if end > bytes.len() {
            break;
        }
        let payload = &bytes[start..end];
        match &fourcc {
            b"VP8X" if payload.len() >= 10 => {
                // フラグ: 予約(2) ICC(0x20) Alpha(0x10) EXIF(0x08) XMP(0x04)
                //         Animation(0x02) 予約(0x01)
                if payload[0] & 0x02 != 0 {
                    features.animated = true;
                }
                let w = u24_le(&payload[4..7]) + 1;
                let h = u24_le(&payload[7..10]) + 1;
                features.dimensions = Some((w, h));
            }
            b"ANIM" | b"ANMF" => {
                // VP8X のフラグが落ちていても ANIM 系チャンクがあれば
                // アニメーション（フラグだけを見ると、フラグを 0 にした
                // 細工ファイルが素通りする）。
                features.animated = true;
            }
            b"VP8 " if bitstream.is_none()
                && payload.len() >= 10
                && payload[3..6] == [0x9d, 0x01, 0x2a] =>
            {
                let w = u32::from(u16::from_le_bytes([payload[6], payload[7]]) & 0x3FFF);
                let h = u32::from(u16::from_le_bytes([payload[8], payload[9]]) & 0x3FFF);
                bitstream = Some((w, h));
            }
            b"VP8L" if bitstream.is_none() && payload.len() >= 5 && payload[0] == 0x2F => {
                let b = u32::from_le_bytes([payload[1], payload[2], payload[3], payload[4]]);
                let w = (b & 0x3FFF) + 1;
                let h = ((b >> 14) & 0x3FFF) + 1;
                bitstream = Some((w, h));
            }
            _ => {}
        }
        // チャンクは偶数境界へパディングされる。
        offset = end + (size & 1);
    }
    if features.dimensions.is_none() {
        features.dimensions = bitstream;
    }
    features
}

/// **画像の**受け入れ検証。通れば `(幅, 高さ)` を返す。
///
/// # Errors
/// 上限超・WebP 以外・アニメーション・寸法が読めない・長辺超過は
/// [`CoreError::InvalidAttachment`]。
///
/// 検査の順は「安いものから」— サイズ → マジック → ヘッダ解析。
/// 巨大な非画像バイト列でヘッダ解析まで進まない。
///
/// **Spec 36 でも中身は変えていない**（参照する定数名が種別を名乗るように
/// なっただけ）。他の種別は別の述語が持つ — 1 本に畳むと、片方の上限を
/// 変えたときもう片方が黙って動く。
pub fn validate_attachment(bytes: &[u8]) -> CoreResult<(u32, u32)> {
    if bytes.len() > ATTACHMENT_IMAGE_MAX_BYTES {
        return Err(CoreError::InvalidAttachment {
            reason: format!(
                "サイズが上限を超えています（{} bytes > {} bytes）",
                bytes.len(),
                ATTACHMENT_IMAGE_MAX_BYTES
            ),
        });
    }
    if !is_webp(bytes) {
        return Err(CoreError::InvalidAttachment {
            reason: "WebP 形式ではありません（UI 側で変換してから送る契約）".to_owned(),
        });
    }
    let features = parse_features(bytes);
    if features.animated {
        return Err(CoreError::InvalidAttachment {
            reason: "アニメーション WebP は送れません（モデルには先頭フレームしか渡らないため）"
                .to_owned(),
        });
    }
    let Some((width, height)) = features.dimensions else {
        return Err(CoreError::InvalidAttachment {
            reason: "画像の寸法を読み取れません（WebP のヘッダが壊れています）".to_owned(),
        });
    };
    let edge = width.max(height);
    if edge > ATTACHMENT_IMAGE_MAX_EDGE_PX {
        return Err(CoreError::InvalidAttachment {
            reason: format!(
                "長辺が上限を超えています（{edge}px > {ATTACHMENT_IMAGE_MAX_EDGE_PX}px。UI 側で縮小してから送る契約）"
            ),
        });
    }
    Ok((width, height))
}

/// **音声の**受け入れ検証（mp3 / wav）。通れば確定した形式を返す。
///
/// # Errors
/// 上限超・mp3 でも wav でもない場合は [`CoreError::InvalidAttachment`]。
///
/// **変換しない**ので長さ（秒）は見ない — デコーダを持たないこの層では
/// 尺を知る手段が無く、**確かめられないことを確かめたふりの検査で覆わない**
/// （Spec 15 の「実行可能性は見ない」と同じ規律）。尺のコストは画面の注記が伝える。
pub fn validate_audio(bytes: &[u8]) -> CoreResult<AttachmentFormat> {
    check_size(bytes, AttachmentKind::Audio)?;
    if is_wav(bytes) {
        return Ok(AttachmentFormat::Wav);
    }
    if is_mp3(bytes) {
        return Ok(AttachmentFormat::Mp3);
    }
    Err(CoreError::InvalidAttachment {
        reason: "音声は mp3 か wav だけ送れます（中身のマジックバイトで判定します）".to_owned(),
    })
}

/// **動画の**受け入れ検証（mp4）。
///
/// # Errors
/// 上限超・mp4 でない・**動画でない ISO BMFF**（HEIC / M4A / QuickTime）は
/// [`CoreError::InvalidAttachment`]。
pub fn validate_video(bytes: &[u8]) -> CoreResult<AttachmentFormat> {
    check_size(bytes, AttachmentKind::Video)?;
    if is_mp4_video(bytes) {
        return Ok(AttachmentFormat::Mp4);
    }
    Err(CoreError::InvalidAttachment {
        reason: "動画は mp4 だけ送れます（中身のマジックバイトで判定します）".to_owned(),
    })
}

/// **PDF の**受け入れ検証。
///
/// # Errors
/// 上限超・`%PDF-` で始まらない場合は [`CoreError::InvalidAttachment`]。
///
/// **頁数は数えない** — 数えるにはパーサが要り、依存を 1 本増やしてまで
/// 守る境界ではない（重い PDF はサイズの門に当たる）。
pub fn validate_pdf(bytes: &[u8]) -> CoreResult<AttachmentFormat> {
    check_size(bytes, AttachmentKind::Pdf)?;
    if is_pdf(bytes) {
        return Ok(AttachmentFormat::Pdf);
    }
    Err(CoreError::InvalidAttachment {
        reason: "PDF ではありません（中身のマジックバイトで判定します）".to_owned(),
    })
}

/// 種別ごとの上限を掛ける（**種別を跨いで共有するのはこの 1 行の形だけ**で、
/// 値は [`AttachmentKind::max_bytes`] が種別ごとに持つ）。
fn check_size(bytes: &[u8], kind: AttachmentKind) -> CoreResult<()> {
    let max = kind.max_bytes();
    if bytes.len() > max {
        return Err(CoreError::InvalidAttachment {
            reason: format!(
                "サイズが上限を超えています（{} bytes > {} bytes。種別: {}）",
                bytes.len(),
                max,
                kind.as_str()
            ),
        });
    }
    Ok(())
}

/// 検証を通った添付の中身（[`validate_any`] の戻り値）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedAttachment {
    /// 確定した形式。
    pub format: AttachmentFormat,
    /// 寸法（画像のときだけ）。
    pub dimensions: Option<(u32, u32)>,
}

/// 形式を判定し、その種別の述語へ振り分ける（保存の入口）。
///
/// # Errors
/// どの形式にも当たらない場合と、種別ごとの検証に落ちた場合は
/// [`CoreError::InvalidAttachment`]。
///
/// **判定してから検証する順序が要点** — 先に種別を決めないと、どの上限を
/// 掛けるべきかが決まらない。**利用者の申告（ファイル名・MIME）は使わない**。
pub fn validate_any(bytes: &[u8]) -> CoreResult<ValidatedAttachment> {
    let Some(format) = detect_format(bytes) else {
        return Err(CoreError::InvalidAttachment {
            reason: "対応していない形式です（画像 webp / 音声 mp3・wav / 動画 mp4 / pdf）"
                .to_owned(),
        });
    };
    match format.kind() {
        AttachmentKind::Image => {
            let (width, height) = validate_attachment(bytes)?;
            Ok(ValidatedAttachment {
                format,
                dimensions: Some((width, height)),
            })
        }
        AttachmentKind::Audio => Ok(ValidatedAttachment {
            format: validate_audio(bytes)?,
            dimensions: None,
        }),
        AttachmentKind::Video => Ok(ValidatedAttachment {
            format: validate_video(bytes)?,
            dimensions: None,
        }),
        AttachmentKind::Pdf => Ok(ValidatedAttachment {
            format: validate_pdf(bytes)?,
            dimensions: None,
        }),
    }
}

/// GC の判断材料。ファイル 1 つぶんのメタデータ。
///
/// 純関数 [`gc_plan`] の入力。I/O から切り離してあるのは、
/// 「期限切れの判定」を壁時計に依存せずテストするため
/// （`schedule.rs` の「内部で `Local::now()` を呼ばない」と同じ規律）。
#[derive(Debug, Clone)]
pub struct GcEntry {
    /// ファイル名（`{id}.{ext}`）。
    pub file_name: String,
    /// 形式（拡張子から引いたもの。内訳の集計に使う）。
    pub format: AttachmentFormat,
    /// 最終更新時刻。添付は一度書いたら書き換えないので、実質は作成時刻。
    pub modified: SystemTime,
    /// ファイルサイズ（bytes）。
    pub len: u64,
}

/// GC の実行結果。呼び出し側（起動時）がログ 1 行に出す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcReport {
    /// 削除した数。
    pub removed: usize,
    /// 残った数。
    pub remaining_files: usize,
    /// 残った合計サイズ（bytes）。
    pub remaining_bytes: u64,
    /// **残った容量の種別内訳**（Spec 36 D11。全 4 種別を必ず含み、0 も出す）。
    ///
    /// 種別クォータを作らない代わりに置いた計器。動画が総量を食って画像を
    /// 押し出す事態が**実際に起きているか**をここで観測してから、クォータの
    /// 要否を判断する（頻度が未知の事象なので観測が先）。
    ///
    /// **0 を省かないのは #72 の規律** — 欄が無いことと 0 であることを
    /// 画面で区別できないと、そこへ落ちたものは存在しなかったことになる。
    pub remaining_by_kind: BTreeMap<AttachmentKind, u64>,
}

impl GcReport {
    /// 種別内訳をログ 1 行の形にする（`image:N,audio:N,video:N,pdf:N`）。
    pub fn kinds_line(&self) -> String {
        AttachmentKind::ALL
            .iter()
            .map(|k| format!("{}:{}", k.as_str(), self.remaining_by_kind.get(k).copied().unwrap_or(0)))
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// 全種別 0 で埋めた内訳（欄の欠落と 0 を混同させない）。
fn empty_by_kind() -> BTreeMap<AttachmentKind, u64> {
    AttachmentKind::ALL.iter().map(|k| (*k, 0u64)).collect()
}

/// 消すべきファイル名の一覧を決める（純関数・D9）。
///
/// 2 段で決める:
/// 1. **期限**: `now` から 30 日より古いものは消す
/// 2. **容量**: 残りの合計が 500MB を超えていたら、古い順に超過が
///    解消するまで消す
///
/// 会話ログからの参照の有無は**見ない** — redb とファイルの 2 つの真実を
/// 同期させない（`attachment_contract` 凍結 8）。
pub fn gc_plan(entries: &[GcEntry], now: SystemTime) -> Vec<String> {
    let retention = Duration::from_secs(ATTACHMENT_RETENTION_DAYS * 24 * 60 * 60);
    let mut doomed: Vec<String> = Vec::new();
    let mut survivors: Vec<&GcEntry> = Vec::new();
    for entry in entries {
        let expired = now
            .duration_since(entry.modified)
            .map(|age| age > retention)
            .unwrap_or(false); // 未来の mtime は「新しい」として残す
        if expired {
            doomed.push(entry.file_name.clone());
        } else {
            survivors.push(entry);
        }
    }
    // 古い順（同時刻はファイル名で決定的に）。
    survivors.sort_by(|a, b| {
        a.modified
            .cmp(&b.modified)
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
    let mut total: u64 = survivors.iter().map(|e| e.len).sum();
    let mut index = 0;
    while total > ATTACHMENT_MAX_TOTAL_BYTES && index < survivors.len() {
        total -= survivors[index].len;
        doomed.push(survivors[index].file_name.clone());
        index += 1;
    }
    doomed
}

/// 添付の置き場。検証してから書き、id で読み、起動時に GC する。
///
/// `ConfigStore` に足さないのは、添付が設定ではないから
/// （`sessions.redb` が `SessionStore` を別に持つのと同じ分業）。
#[derive(Debug, Clone)]
pub struct AttachmentStore {
    root: PathBuf,
}

impl AttachmentStore {
    /// ワークスペースのルートを指定して作る。
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 添付フォルダのパス（`{workspace}/attachments/`）。
    fn dir(&self) -> PathBuf {
        self.root.join(ATTACHMENT_DIR)
    }

    /// id が UUID の字種（英数字と `-`）だけで出来ているか。
    ///
    /// パス解決に使う値なので、`AgentId::is_safe` と同じ発想で入口を絞る。
    /// `..` や区切り文字が混ざった id は、ここで弾かれて I/O に届かない。
    fn is_safe_id(id: &str) -> bool {
        !id.is_empty()
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
    }

    /// id と形式からファイルパスを解決する（**書き込み側**）。
    fn path_for(&self, id: &str, format: AttachmentFormat) -> CoreResult<PathBuf> {
        if !Self::is_safe_id(id) {
            return Err(CoreError::UnsafeIdentifier {
                value: id.to_string(),
            });
        }
        Ok(self.dir().join(format!("{id}.{}", format.ext())))
    }

    /// id から候補パスを並べる（**読み出し側**。閉じた拡張子の集合を順に試す）。
    ///
    /// **読み口が id しか受け取らないのは意図的。** 表示の経路
    /// （IPC `read_attachment`）はチップの id だけを運び、形式を運ばない。
    /// id は UUID なので当たるファイルは高々 1 つで、外れは `NotFound` として
    /// 素通りする（stat が最大 5 回。読み口へ形式を足すより配線が 1 本少ない）。
    fn candidates(&self, id: &str) -> CoreResult<Vec<PathBuf>> {
        if !Self::is_safe_id(id) {
            return Err(CoreError::UnsafeIdentifier {
                value: id.to_string(),
            });
        }
        let dir = self.dir();
        Ok([
            AttachmentFormat::Webp,
            AttachmentFormat::Mp3,
            AttachmentFormat::Wav,
            AttachmentFormat::Mp4,
            AttachmentFormat::Pdf,
        ]
        .iter()
        .map(|f| dir.join(format!("{id}.{}", f.ext())))
        .collect())
    }

    /// I/O エラーへパス情報を添える（`ConfigStore` と同じ形）。
    fn io_err(path: &Path, source: std::io::Error) -> CoreError {
        CoreError::ConfigIo {
            path: path.display().to_string(),
            source,
        }
    }

    /// 検証して保存し、参照を返す。
    ///
    /// id はここで生成する（UUID v4）。呼び出し側に選ばせないのは、
    /// 上書きと衝突の余地を入口で消すため。
    ///
    /// # Errors
    /// 検証に失敗すれば [`CoreError::InvalidAttachment`]（何も書かない）。
    pub async fn save(&self, file_name: &str, bytes: &[u8]) -> CoreResult<Attachment> {
        let validated = validate_any(bytes)?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Self::io_err(&dir, e))?;
        // 拡張子は**検証で確定した形式**から決める（利用者の file_name は表示専用）。
        let path = self.path_for(&id, validated.format)?;
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| Self::io_err(&path, e))?;
        Ok(Attachment {
            id,
            format: validated.format,
            width: validated.dimensions.map(|(w, _)| w),
            height: validated.dimensions.map(|(_, h)| h),
            file_name: file_name.to_owned(),
        })
    }

    /// id で実体を読む。**無ければ `None`**。
    ///
    /// GC で消えた添付の表示は「保持期間を過ぎて削除されました」の枠に
    /// なる（D9）ので、不在はエラーではなく通常の答え。
    pub async fn read(&self, id: &str) -> CoreResult<Option<Vec<u8>>> {
        for path in self.candidates(id)? {
            match tokio::fs::read(&path).await {
                Ok(bytes) => return Ok(Some(bytes)),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
                Err(err) => return Err(Self::io_err(&path, err)),
            }
        }
        Ok(None)
    }

    /// 保持期間と容量上限で古い添付を消す（D9。呼ぶのは起動時）。
    ///
    /// 現在時刻は引数で受け取る — 内部で `SystemTime::now()` を読むと、
    /// テストが壁時計に依存して特定の時刻でだけ落ちるものになる。
    pub async fn gc(&self, now: SystemTime) -> CoreResult<GcReport> {
        let dir = self.dir();
        let mut reader = match tokio::fs::read_dir(&dir).await {
            Ok(reader) => reader,
            // フォルダが無い = 添付を一度も使っていない村。GC は何もしない。
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(GcReport {
                    removed: 0,
                    remaining_files: 0,
                    remaining_bytes: 0,
                    remaining_by_kind: empty_by_kind(),
                });
            }
            Err(err) => return Err(Self::io_err(&dir, err)),
        };
        let mut entries: Vec<GcEntry> = Vec::new();
        while let Some(entry) = reader
            .next_entry()
            .await
            .map_err(|e| Self::io_err(&dir, e))?
        {
            let path = entry.path();
            // 添付以外（手で置かれた何か）には触らない。**閉じた集合**で照合する
            // ので、種別が増えても「知らない拡張子は消さない」は保たれる。
            let Some(format) = path
                .extension()
                .and_then(|e| e.to_str())
                .and_then(AttachmentFormat::from_ext)
            else {
                continue;
            };
            let Ok(meta) = entry.metadata().await else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let Ok(modified) = meta.modified() else {
                continue;
            };
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            entries.push(GcEntry {
                file_name: name.to_owned(),
                format,
                modified,
                len: meta.len(),
            });
        }
        let doomed = gc_plan(&entries, now);
        for name in &doomed {
            let path = dir.join(name);
            match tokio::fs::remove_file(&path).await {
                Ok(()) => {}
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
                Err(err) => return Err(Self::io_err(&path, err)),
            }
        }
        let doomed_set: std::collections::HashSet<&str> =
            doomed.iter().map(String::as_str).collect();
        let remaining: Vec<&GcEntry> = entries
            .iter()
            .filter(|e| !doomed_set.contains(e.file_name.as_str()))
            .collect();
        let mut remaining_by_kind = empty_by_kind();
        for entry in &remaining {
            *remaining_by_kind.entry(entry.format.kind()).or_insert(0) += entry.len;
        }
        Ok(GcReport {
            removed: doomed.len(),
            remaining_files: remaining.len(),
            remaining_bytes: remaining.iter().map(|e| e.len).sum(),
            remaining_by_kind,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "fuseforks-attachment-{tag}-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// チャンク 1 つ（fourcc + サイズ + payload + 奇数パディング）。
    fn chunk(fourcc: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(fourcc);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    /// RIFF コンテナへ包む（サイズ欄も正しく書く）。
    fn container(chunks: &[Vec<u8>]) -> Vec<u8> {
        let body_len: usize = chunks.iter().map(Vec::len).sum();
        let mut out = Vec::new();
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&((body_len + 4) as u32).to_le_bytes());
        out.extend_from_slice(b"WEBP");
        for c in chunks {
            out.extend_from_slice(c);
        }
        out
    }

    /// VP8L（可逆）のヘッダ。幅・高さは 14 bit ずつの詰め込み。
    fn vp8l_payload(w: u32, h: u32) -> Vec<u8> {
        let packed = (w - 1) | ((h - 1) << 14);
        let mut out = vec![0x2F];
        out.extend_from_slice(&packed.to_le_bytes());
        out
    }

    /// VP8X（拡張ヘッダ）。flags と canvas 寸法。
    fn vp8x_payload(flags: u8, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![flags, 0, 0, 0];
        out.extend_from_slice(&(w - 1).to_le_bytes()[0..3]);
        out.extend_from_slice(&(h - 1).to_le_bytes()[0..3]);
        out
    }

    /// 素の VP8L 1 チャンクだけの WebP。
    fn plain_webp(w: u32, h: u32) -> Vec<u8> {
        container(&[chunk(b"VP8L", &vp8l_payload(w, h))])
    }

    /// WebP でないバイト列は拒否（PNG のマジック）。
    #[test]
    fn rejects_non_webp() {
        let err = validate_attachment(b"\x89PNG\r\n\x1a\n rest of file").unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// 変換後 2MB の門。中身を見る前にサイズで落とす。
    #[test]
    fn rejects_oversized_bytes() {
        let err = validate_attachment(&vec![0u8; ATTACHMENT_IMAGE_MAX_BYTES + 1]).unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// アニメーション WebP は VP8X のフラグで拒否。
    #[test]
    fn rejects_animated_webp_by_flag() {
        let bytes = container(&[
            chunk(b"VP8X", &vp8x_payload(0x02, 100, 100)),
            chunk(b"ANIM", &[0; 6]),
        ]);
        let err = validate_attachment(&bytes).unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// フラグが 0 でも ANIM チャンクがあれば拒否（フラグだけを見ると
    /// フラグを落とした細工ファイルが素通りする）。
    #[test]
    fn rejects_anim_chunk_even_without_flag() {
        let bytes = container(&[
            chunk(b"VP8X", &vp8x_payload(0x00, 100, 100)),
            chunk(b"ANIM", &[0; 6]),
        ]);
        let err = validate_attachment(&bytes).unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// 長辺 1568 は通り、1569 は拒否（境界値）。
    #[test]
    fn enforces_max_edge_boundary() {
        assert_eq!(
            validate_attachment(&plain_webp(ATTACHMENT_IMAGE_MAX_EDGE_PX, 10)).unwrap(),
            (ATTACHMENT_IMAGE_MAX_EDGE_PX, 10)
        );
        let err =
            validate_attachment(&plain_webp(ATTACHMENT_IMAGE_MAX_EDGE_PX + 1, 10)).unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// VP8（非可逆）のヘッダからも寸法が読める。
    #[test]
    fn reads_dimensions_from_lossy_vp8() {
        let mut payload = vec![0u8, 0, 0, 0x9d, 0x01, 0x2a];
        payload.extend_from_slice(&640u16.to_le_bytes());
        payload.extend_from_slice(&480u16.to_le_bytes());
        let bytes = container(&[chunk(b"VP8 ", &payload)]);
        assert_eq!(validate_attachment(&bytes).unwrap(), (640, 480));
    }

    /// VP8X があればキャンバス寸法が勝つ（ビットストリームと食い違う
    /// 細工ファイルで、大きい方を見逃さないため）。
    #[test]
    fn vp8x_canvas_wins_over_bitstream() {
        let bytes = container(&[
            chunk(b"VP8X", &vp8x_payload(0x00, 800, 600)),
            chunk(b"VP8L", &vp8l_payload(10, 10)),
        ]);
        assert_eq!(validate_attachment(&bytes).unwrap(), (800, 600));
    }

    /// 寸法の読めない WebP（マジックだけ）は拒否。
    #[test]
    fn rejects_webp_without_readable_dimensions() {
        let mut bytes = b"RIFF\0\0\0\0WEBP".to_vec();
        bytes.extend_from_slice(b"garbage");
        let err = validate_attachment(&bytes).unwrap_err();
        assert_eq!(err.code(), "INVALID_ATTACHMENT");
    }

    /// 保存 → 読みの往復。id は UUID の字種で、寸法は実測値。
    #[tokio::test]
    async fn save_roundtrips_and_reports_dimensions() {
        let dir = TempDir::new("roundtrip");
        let store = AttachmentStore::new(&dir.0);
        let bytes = plain_webp(320, 200);

        let att = store.save("screenshot.png", &bytes).await.unwrap();
        assert_eq!((att.width, att.height), (Some(320), Some(200)));
        assert_eq!(att.format, AttachmentFormat::Webp);
        assert_eq!(att.kind(), AttachmentKind::Image);
        assert_eq!(att.file_name, "screenshot.png");
        assert!(AttachmentStore::is_safe_id(&att.id), "id は UUID の字種");
        assert!(
            dir.0.join(ATTACHMENT_DIR).join(format!("{}.webp", att.id)).exists(),
            "置き場は {{workspace}}/attachments/{{id}}.webp"
        );

        assert_eq!(store.read(&att.id).await.unwrap(), Some(bytes));
    }

    /// 検証に落ちたら何も書かない。
    #[tokio::test]
    async fn rejected_bytes_leave_no_file() {
        let dir = TempDir::new("no-file");
        let store = AttachmentStore::new(&dir.0);
        store.save("x.png", b"not webp").await.unwrap_err();
        assert!(
            !dir.0.join(ATTACHMENT_DIR).exists(),
            "拒否したらフォルダも作らない"
        );
    }

    /// 無い id は None（GC 済みの表示に使う通常の答え）。
    #[tokio::test]
    async fn read_missing_returns_none() {
        let dir = TempDir::new("missing");
        let store = AttachmentStore::new(&dir.0);
        assert_eq!(store.read("0000-none").await.unwrap(), None);
    }

    /// パス区切りの混ざった id は I/O に届く前に拒否。
    #[tokio::test]
    async fn read_rejects_traversal_id() {
        let dir = TempDir::new("traversal");
        let store = AttachmentStore::new(&dir.0);
        for bad in ["../evil", "a/b", "a\\b", "", "a.webp"] {
            let err = store.read(bad).await.unwrap_err();
            assert_eq!(err.code(), "UNSAFE_IDENTIFIER", "{bad:?} は拒否");
        }
    }

    /// 期限内は消えず、期限切れは消える（現在時刻は引数なので、
    /// mtime を細工せずに「30 日後の起動」を再現できる）。
    #[tokio::test]
    async fn gc_keeps_fresh_and_deletes_expired() {
        let dir = TempDir::new("gc");
        let store = AttachmentStore::new(&dir.0);
        let a = store.save("a.png", &plain_webp(10, 10)).await.unwrap();
        let b = store.save("b.png", &plain_webp(20, 20)).await.unwrap();

        let now = SystemTime::now();
        let report = store.gc(now).await.unwrap();
        assert_eq!(report.removed, 0, "書いた直後は何も消えない");
        assert_eq!(report.remaining_files, 2);

        let later = now + Duration::from_secs((ATTACHMENT_RETENTION_DAYS + 1) * 24 * 60 * 60);
        let report = store.gc(later).await.unwrap();
        assert_eq!(report.removed, 2, "31 日後の起動で両方消える");
        assert_eq!(store.read(&a.id).await.unwrap(), None);
        assert_eq!(store.read(&b.id).await.unwrap(), None);
    }

    /// 添付フォルダが無い村では GC は何もしない。
    #[tokio::test]
    async fn gc_on_absent_folder_is_a_noop() {
        let dir = TempDir::new("gc-absent");
        let store = AttachmentStore::new(&dir.0);
        let report = store.gc(SystemTime::now()).await.unwrap();
        assert_eq!(report.removed, 0);
    }

    /// 容量上限の超過は古い順に消える（純関数）。
    #[test]
    fn gc_plan_evicts_oldest_first_over_size_cap() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(3 * 60 * 60);
        let mid = now - Duration::from_secs(2 * 60 * 60);
        let new = now - Duration::from_secs(60 * 60);
        // 3 つで上限を 1 つぶん超えている状態。
        let third = ATTACHMENT_MAX_TOTAL_BYTES / 2;
        let entries = vec![
            GcEntry { file_name: "new.webp".into(), format: AttachmentFormat::Webp, modified: new, len: third },
            GcEntry { file_name: "old.webp".into(), format: AttachmentFormat::Webp, modified: old, len: third },
            GcEntry { file_name: "mid.webp".into(), format: AttachmentFormat::Webp, modified: mid, len: third },
        ];
        assert_eq!(gc_plan(&entries, now), vec!["old.webp".to_owned()]);
    }

    /// 上限以内なら容量では消さない。
    #[test]
    fn gc_plan_keeps_everything_under_the_cap() {
        let now = SystemTime::now();
        let entries = vec![GcEntry {
            file_name: "a.webp".into(),
            format: AttachmentFormat::Webp,
            modified: now,
            len: 1024,
        }];
        assert!(gc_plan(&entries, now).is_empty());
    }

    // ---- Spec 36 P1: 多モーダル（音声・動画・PDF） ----

    /// RIFF WAVE（先頭 4 バイトは WebP と同じ）。
    fn wav_bytes() -> Vec<u8> {
        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&36u32.to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&[0u8; 24]);
        out
    }

    /// ID3v2 タグ付きの mp3。
    fn mp3_bytes() -> Vec<u8> {
        let mut out = b"ID3\x04\x00\x00".to_vec();
        out.extend_from_slice(&[0u8; 32]);
        out
    }

    /// ID3 の無い素の MPEG フレーム同期。
    fn mp3_bare_frame() -> Vec<u8> {
        let mut out = vec![0xFF, 0xFB, 0x90, 0x00];
        out.extend_from_slice(&[0u8; 32]);
        out
    }

    /// ISO BMFF（ブランドを指定できる）。
    fn ftyp_bytes(brand: &[u8; 4]) -> Vec<u8> {
        let mut out = 32u32.to_be_bytes().to_vec();
        out.extend_from_slice(b"ftyp");
        out.extend_from_slice(brand);
        out.extend_from_slice(&[0u8; 16]);
        out
    }

    fn pdf_bytes() -> Vec<u8> {
        let mut out = b"%PDF-1.4\n".to_vec();
        out.extend_from_slice(b"1 0 obj\n<< >>\nendobj\n%%EOF\n");
        out
    }

    /// 4 種別すべてがマジックバイトから判る。
    #[test]
    fn detects_every_supported_format() {
        assert_eq!(detect_format(&plain_webp(10, 10)), Some(AttachmentFormat::Webp));
        assert_eq!(detect_format(&wav_bytes()), Some(AttachmentFormat::Wav));
        assert_eq!(detect_format(&mp3_bytes()), Some(AttachmentFormat::Mp3));
        assert_eq!(detect_format(&mp3_bare_frame()), Some(AttachmentFormat::Mp3));
        assert_eq!(detect_format(&ftyp_bytes(b"isom")), Some(AttachmentFormat::Mp4));
        assert_eq!(detect_format(&pdf_bytes()), Some(AttachmentFormat::Pdf));
        assert_eq!(detect_format(b"\x89PNG\r\n\x1a\n rest"), None);
    }

    /// **WebP と WAV は先頭 4 バイトが同じ。** フォーム型まで見ないと
    /// WAV が画像として通り、画像の述語（寸法を要求する）で落ちる —
    /// 「音声を送ったのに画像のエラーが出る」形になる。
    #[test]
    fn riff_form_type_separates_webp_from_wav() {
        assert_eq!(&plain_webp(10, 10)[0..4], &wav_bytes()[0..4], "先頭は同じ");
        assert_eq!(detect_format(&plain_webp(10, 10)), Some(AttachmentFormat::Webp));
        assert_eq!(detect_format(&wav_bytes()), Some(AttachmentFormat::Wav));
    }

    /// **`ftyp` は動画専用ではない。** HEIC（画像）・M4A（音声）・QuickTime も
    /// 同じコンテナを名乗るので、ブランドを見ずに通すと写真が「動画」として
    /// ワイヤへ載る。閉じた許容で弾く。
    #[test]
    fn ftyp_brand_allowlist_rejects_non_video_containers() {
        assert_eq!(detect_format(&ftyp_bytes(b"mp42")), Some(AttachmentFormat::Mp4));
        for brand in [b"heic", b"mif1", b"avif", b"M4A ", b"qt  "] {
            assert_eq!(
                detect_format(&ftyp_bytes(brand)),
                None,
                "{} は動画として受けない",
                String::from_utf8_lossy(brand)
            );
        }
    }

    /// **上限は種別ごとに別の定数**（D13）。1 つに畳むと片方を変えたとき
    /// もう片方が黙って動く。画像の 2MB が音声・動画・PDF を縛らないことを見る。
    #[test]
    fn each_kind_has_its_own_size_gate() {
        assert_eq!(AttachmentKind::Image.max_bytes(), ATTACHMENT_IMAGE_MAX_BYTES);
        assert!(
            AttachmentKind::Audio.max_bytes() > ATTACHMENT_IMAGE_MAX_BYTES,
            "音声の門は画像の門と別"
        );
        // 画像なら落ちる大きさの音声が通る。
        let mut big = mp3_bytes();
        big.resize(ATTACHMENT_IMAGE_MAX_BYTES + 1, 0);
        assert_eq!(validate_audio(&big).unwrap(), AttachmentFormat::Mp3);
        // その音声も自分の門は超えられない。
        let mut too_big = mp3_bytes();
        too_big.resize(ATTACHMENT_AUDIO_MAX_BYTES + 1, 0);
        assert_eq!(
            validate_audio(&too_big).unwrap_err().code(),
            "INVALID_ATTACHMENT"
        );
    }

    /// 動画と PDF も自分の門を持つ。
    #[test]
    fn video_and_pdf_enforce_their_own_caps() {
        let mut video = ftyp_bytes(b"isom");
        video.resize(ATTACHMENT_VIDEO_MAX_BYTES + 1, 0);
        assert_eq!(
            validate_video(&video).unwrap_err().code(),
            "INVALID_ATTACHMENT"
        );
        let mut pdf = pdf_bytes();
        pdf.resize(ATTACHMENT_PDF_MAX_BYTES + 1, 0);
        assert_eq!(validate_pdf(&pdf).unwrap_err().code(), "INVALID_ATTACHMENT");
    }

    /// 種別違いの述語には通らない（音声の述語に PDF を渡す等）。
    #[test]
    fn kind_predicates_do_not_accept_other_kinds() {
        assert!(validate_audio(&pdf_bytes()).is_err());
        assert!(validate_video(&wav_bytes()).is_err());
        assert!(validate_pdf(&mp3_bytes()).is_err());
        assert!(validate_attachment(&wav_bytes()).is_err());
    }

    /// 振り分けは形式を確定し、**寸法を返すのは画像のときだけ**。
    #[test]
    fn validate_any_dispatches_and_only_images_carry_dimensions() {
        assert_eq!(
            validate_any(&plain_webp(320, 200)).unwrap(),
            ValidatedAttachment {
                format: AttachmentFormat::Webp,
                dimensions: Some((320, 200)),
            }
        );
        for (bytes, format) in [
            (wav_bytes(), AttachmentFormat::Wav),
            (mp3_bytes(), AttachmentFormat::Mp3),
            (ftyp_bytes(b"isom"), AttachmentFormat::Mp4),
            (pdf_bytes(), AttachmentFormat::Pdf),
        ] {
            let v = validate_any(&bytes).unwrap();
            assert_eq!(v.format, format);
            assert_eq!(v.dimensions, None, "{format:?} に寸法は無い");
        }
        assert_eq!(
            validate_any(b"\x89PNG\r\n\x1a\n").unwrap_err().code(),
            "INVALID_ATTACHMENT"
        );
    }

    /// **保存の拡張子は検証で確定した形式から決める。**
    /// 利用者のファイル名が嘘（`.png` の PDF）でも、置き場は `.pdf` になる。
    #[tokio::test]
    async fn saved_extension_comes_from_magic_not_file_name() {
        let dir = TempDir::new("ext");
        let store = AttachmentStore::new(&dir.0);
        let att = store.save("報告書.png", &pdf_bytes()).await.unwrap();

        assert_eq!(att.format, AttachmentFormat::Pdf);
        assert_eq!(att.kind(), AttachmentKind::Pdf);
        assert_eq!(att.width, None, "PDF に寸法は無い");
        assert_eq!(att.file_name, "報告書.png", "表示名は嘘でもそのまま持つ");
        assert!(
            dir.0.join(ATTACHMENT_DIR).join(format!("{}.pdf", att.id)).exists(),
            "置き場の拡張子は中身で決まる"
        );
        // 読み口は id しか受け取らないが、拡張子を総当たりして当てる。
        assert_eq!(store.read(&att.id).await.unwrap(), Some(pdf_bytes()));
    }

    /// 4 種別すべてが往復する。
    #[tokio::test]
    async fn every_kind_roundtrips() {
        let dir = TempDir::new("kinds");
        let store = AttachmentStore::new(&dir.0);
        for (bytes, kind, ext) in [
            (plain_webp(10, 10), AttachmentKind::Image, "webp"),
            (wav_bytes(), AttachmentKind::Audio, "wav"),
            (mp3_bytes(), AttachmentKind::Audio, "mp3"),
            (ftyp_bytes(b"isom"), AttachmentKind::Video, "mp4"),
            (pdf_bytes(), AttachmentKind::Pdf, "pdf"),
        ] {
            let att = store.save("x.bin", &bytes).await.unwrap();
            assert_eq!(att.kind(), kind);
            assert_eq!(att.format.ext(), ext);
            assert_eq!(store.read(&att.id).await.unwrap(), Some(bytes));
        }
    }

    /// **形式欄を持たない既存レコードは WebP の画像として読める**（Spec 23 の
    /// 時代に redb へ書かれたものが、実際そうだった）。ここが割れると
    /// 再起動で過去の会話の添付が全部読めなくなる。
    #[test]
    fn old_records_without_format_read_as_webp_image() {
        let old = r#"{"id":"abc-123","width":320,"height":200,"fileName":"a.png"}"#;
        let att: Attachment = serde_json::from_str(old).unwrap();
        assert_eq!(att.format, AttachmentFormat::Webp);
        assert_eq!(att.kind(), AttachmentKind::Image);
        assert_eq!((att.width, att.height), (Some(320), Some(200)));
    }

    /// 寸法の無い種別は寸法欄ごと出さない（`null` を書かない）。
    #[test]
    fn non_image_serialization_omits_dimensions() {
        let att = Attachment {
            id: "abc-123".into(),
            format: AttachmentFormat::Mp3,
            width: None,
            height: None,
            file_name: "voice.mp3".into(),
        };
        let json = serde_json::to_string(&att).unwrap();
        assert_eq!(
            json,
            r#"{"id":"abc-123","format":"mp3","fileName":"voice.mp3"}"#
        );
        assert_eq!(serde_json::from_str::<Attachment>(&json).unwrap(), att);
    }

    /// **GC の判断は種別で変わらない**（D11 — クォータは持たない）。
    /// 期限と容量だけで決まることを、種別を混ぜた入力で見る。
    #[test]
    fn gc_plan_ignores_kind() {
        let now = SystemTime::now();
        let old = now - Duration::from_secs(3 * 60 * 60);
        let new = now - Duration::from_secs(60 * 60);
        let half = ATTACHMENT_MAX_TOTAL_BYTES / 2 + 1;
        let entries = vec![
            GcEntry { file_name: "new.mp4".into(), format: AttachmentFormat::Mp4, modified: new, len: half },
            GcEntry { file_name: "old.webp".into(), format: AttachmentFormat::Webp, modified: old, len: half },
        ];
        // 古いほうが消える — 動画だから守られる / 画像だから守られる、は無い。
        assert_eq!(gc_plan(&entries, now), vec!["old.webp".to_owned()]);
    }

    /// GC の内訳は**全種別を必ず含み、0 も出す**（#72 — 欄の欠落と 0 を
    /// 画面で区別できないと、そこへ落ちたものは無かったことになる）。
    #[tokio::test]
    async fn gc_reports_bytes_per_kind_including_zeros() {
        let dir = TempDir::new("gc-kinds");
        let store = AttachmentStore::new(&dir.0);
        let image = plain_webp(10, 10);
        let pdf = pdf_bytes();
        store.save("a.webp", &image).await.unwrap();
        store.save("b.pdf", &pdf).await.unwrap();

        let report = store.gc(SystemTime::now()).await.unwrap();
        assert_eq!(report.removed, 0);
        assert_eq!(report.remaining_files, 2);
        assert_eq!(report.remaining_by_kind.len(), AttachmentKind::ALL.len());
        assert_eq!(
            report.remaining_by_kind[&AttachmentKind::Image],
            image.len() as u64
        );
        assert_eq!(report.remaining_by_kind[&AttachmentKind::Pdf], pdf.len() as u64);
        assert_eq!(report.remaining_by_kind[&AttachmentKind::Audio], 0);
        assert_eq!(report.remaining_by_kind[&AttachmentKind::Video], 0);
        assert_eq!(report.kinds_line(), format!("image:{},audio:0,video:0,pdf:{}", image.len(), pdf.len()));
    }

    /// **知らない拡張子には触らない**（手で置かれた何か）。種別が増えても
    /// 「閉じた集合の外は消さない」が保たれることを見る。
    #[tokio::test]
    async fn gc_leaves_unknown_extensions_alone() {
        let dir = TempDir::new("gc-unknown");
        let store = AttachmentStore::new(&dir.0);
        store.save("a.webp", &plain_webp(10, 10)).await.unwrap();
        let stray = dir.0.join(ATTACHMENT_DIR).join("notes.txt");
        std::fs::write(&stray, b"human wrote this").unwrap();

        let later = SystemTime::now()
            + Duration::from_secs((ATTACHMENT_RETENTION_DAYS + 1) * 24 * 60 * 60);
        let report = store.gc(later).await.unwrap();
        assert_eq!(report.removed, 1, "添付だけが消える");
        assert!(stray.exists(), "添付以外は期限を過ぎても触らない");
    }
}
