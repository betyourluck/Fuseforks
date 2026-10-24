//! MCP のツール結果で返った画像を、モデルへ送れる形へ寄せる（Spec 70 / `mcp_image_result_contract`）。
//!
//! 純機構。I/O を持たない。ここに在るのは 3 つ —
//!
//! 1. **1 枚の正規化**（[`normalize`]）— 再エンコードしない経路と、JPEG へ寄せる経路の 2 つ（D2）
//! 2. **ターンの `messages` の上の操作**（[`cap_tool_images`] / [`strip_tool_images`]）— 1 要求 20 枚まで（D2b）と、
//!    画像を受けないモデルへの送り直し（D5）
//! 3. **コアがツール結果に足す文**（D6）— どれも村の言語を引数に取る。呼ぶのは言語を知っている層
//!    （`McpTool` とターンの層）だけで、adapter（LLM の層）は 1 つも呼ばない（`failures.md` #149）
//!
//! **画像の中身はどこにも書かない**（計器もログも）。書くのは枚数・形式・大きさだけ。

use base64::Engine as _;
use sha2::{Digest, Sha256};

use crate::attachment::{ATTACHMENT_IMAGE_MAX_BYTES, ATTACHMENT_IMAGE_MAX_EDGE_PX};
use crate::llm::{ChatMessage, PromptAttachment, PromptMediaType, Role};
use crate::world::Language;

/// 1 つのツール結果から送る画像の上限（D2）。5 枚目以降は [`DropReason::TooMany`]。
pub const TOOL_IMAGES_PER_RESULT: usize = 4;

/// 1 要求に載せるツール結果の画像の上限（D2b）。超えたら古いものから外す。
///
/// Anthropic の 1 要求 100 枚・「20 枚を超えると 1 枚の上限が 2000px へ下がる」の手前に置いた値。
pub const TOOL_IMAGES_PER_REQUEST: usize = 20;

/// 再エンコードの JPEG の品質（D2）。
pub const REENCODE_JPEG_QUALITY: u8 = 85;

/// デコードしてよい 1 辺の上限。**伸張爆弾の歯止め**で、送る大きさの上限（1568px）とは別。
///
/// 長辺 1568px を超える画像は縮めて送る（D2）ので、ここで落とすのは「縮める前に展開するだけで
/// メモリを食い尽くす」大きさだけ。超えたら [`DropReason::TooLarge`]。
pub const TOOL_IMAGE_DECODE_MAX_EDGE_PX: u32 = 16_384;

/// 画像を落とした理由（D7。`ToolOutput::dropped` に入り、**計器だけが読む**）。
///
/// モデルへは [`dropped_note`] の 1 文で伝わる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropReason {
    /// 運べない形式（GIF・BMP など。送るのは JPEG / WebP / PNG だけ）。
    Format,
    /// 画像として読めない（base64 が壊れている・どの形式とも判別できない・デコードに失敗）。
    Decode,
    /// 大きすぎる（展開の上限を超える・再エンコードしても 2 MB を超える）。
    TooLarge,
    /// 1 つの結果につき [`TOOL_IMAGES_PER_RESULT`] 枚を超えた分。
    TooMany,
}

impl DropReason {
    /// ログと注記で並べる順。
    pub const ALL: [Self; 4] = [Self::Format, Self::Decode, Self::TooLarge, Self::TooMany];

    /// 計器の語（`tool image dropped: … reason=format:1`）。
    pub fn label(self) -> &'static str {
        match self {
            Self::Format => "format",
            Self::Decode => "decode",
            Self::TooLarge => "too_large",
            Self::TooMany => "too_many",
        }
    }

    /// 注記に入れる語（村の言語）。
    fn phrase(self, language: Language) -> String {
        match (self, language) {
            (Self::Format, Language::Ja) => "対応していない形式".to_owned(),
            (Self::Format, Language::En) => "unsupported format".to_owned(),
            (Self::Decode, Language::Ja) => "読み取れない画像".to_owned(),
            (Self::Decode, Language::En) => "could not be decoded".to_owned(),
            (Self::TooLarge, Language::Ja) => "大きすぎる画像".to_owned(),
            (Self::TooLarge, Language::En) => "too large".to_owned(),
            (Self::TooMany, Language::Ja) => {
                format!("1 回 {TOOL_IMAGES_PER_RESULT} 枚を超えた分")
            }
            (Self::TooMany, Language::En) => {
                format!("over the limit of {TOOL_IMAGES_PER_RESULT} per result")
            }
        }
    }
}

/// モデルへ送る 1 枚（D2 を通ったもの）。
#[derive(Debug, Clone)]
pub struct ToolImage {
    /// ワイヤへ載せる添付（JPEG か WebP）。
    pub attachment: PromptAttachment,
    /// **接続先へ送るバイト列**の SHA-256（D4。RepeatGuard の判定に足す）。
    pub digest: [u8; 32],
    /// 送るバイト列の長さ（ツールの出力の表示に使う。D8）。
    pub bytes: usize,
    /// JPEG へ再エンコードしたか（計器用）。
    pub reencoded: bool,
}

impl ToolImage {
    fn new(media_type: PromptMediaType, bytes: &[u8], reencoded: bool) -> Self {
        let engine = base64::engine::general_purpose::STANDARD;
        Self {
            attachment: PromptAttachment::new(media_type, engine.encode(bytes)),
            digest: Sha256::digest(bytes).into(),
            bytes: bytes.len(),
            reencoded,
        }
    }
}

/// 正規化の上限。本番は [`Bounds::DEFAULT`] だけで、テストが小さい値を渡して境界を踏む。
#[derive(Debug, Clone, Copy)]
struct Bounds {
    /// 送る 1 枚の大きさの上限（バイト）。
    max_bytes: usize,
    /// 送る 1 枚の長辺の上限（px）。
    max_edge: u32,
}

impl Bounds {
    /// 利用者の添付と**同じ定数**（予算は共有しない — D2）。
    const DEFAULT: Self = Self {
        max_bytes: ATTACHMENT_IMAGE_MAX_BYTES,
        max_edge: ATTACHMENT_IMAGE_MAX_EDGE_PX,
    };
}

/// MCP の `ImageContent.data`（base64）を 1 枚、送れる形へ寄せる（D2）。
///
/// - **再エンコードしない**: JPEG / WebP（アニメーションでない）で、長辺 1568px 以下・2 MB 以下。
///   MCP が返したバイト列をそのまま送る（寸法を読むためにデコードはする）
/// - **再エンコードする**: PNG・アニメーション WebP・上限を超えた JPEG / WebP。長辺 1568px へ縮め、
///   JPEG（品質 85）へ。透明は白で塗る（この経路だけ）。再エンコード後も 2 MB を超えたら落とす
///
/// **形式は MCP の `mimeType` ではなくバイト列で決める** — `mimeType` はサーバーの自己申告で、
/// 送る `media_type` がバイト列と食い違うと接続先が 400 を返す。
///
/// # Errors
/// 落とした理由（[`DropReason`]）。呼び出し側は注記を書き、計器へ数える。
pub fn normalize(data_b64: &str) -> Result<ToolImage, DropReason> {
    normalize_within(data_b64, Bounds::DEFAULT)
}

fn normalize_within(data_b64: &str, bounds: Bounds) -> Result<ToolImage, DropReason> {
    let engine = base64::engine::general_purpose::STANDARD;
    let bytes = engine
        .decode(data_b64.trim())
        .map_err(|_| DropReason::Decode)?;
    let format = image::guess_format(&bytes).map_err(|_| DropReason::Decode)?;
    let media_type = match format {
        image::ImageFormat::Jpeg => Some(PromptMediaType::Jpeg),
        image::ImageFormat::WebP => Some(PromptMediaType::Webp),
        image::ImageFormat::Png => None,
        _ => return Err(DropReason::Format),
    };

    let decoded = decode(&bytes, format)?;
    let edge = decoded.width().max(decoded.height());
    let animated = format == image::ImageFormat::WebP && crate::attachment::is_animated_webp(&bytes);

    if let Some(media_type) = media_type
        && !animated
        && edge <= bounds.max_edge
        && bytes.len() <= bounds.max_bytes
    {
        return Ok(ToolImage::new(media_type, &bytes, false));
    }

    let resized = if edge > bounds.max_edge {
        decoded.resize(
            bounds.max_edge,
            bounds.max_edge,
            image::imageops::FilterType::Triangle,
        )
    } else {
        decoded
    };
    let rgb = flatten_onto_white(&resized);
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, REENCODE_JPEG_QUALITY)
        .encode_image(&rgb)
        .map_err(|_| DropReason::Decode)?;
    if jpeg.len() > bounds.max_bytes {
        return Err(DropReason::TooLarge);
    }
    Ok(ToolImage::new(PromptMediaType::Jpeg, &jpeg, true))
}

/// 展開の上限つきでデコードする。上限に当たったら [`DropReason::TooLarge`]、それ以外の失敗は
/// [`DropReason::Decode`]。
fn decode(bytes: &[u8], format: image::ImageFormat) -> Result<image::DynamicImage, DropReason> {
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(bytes), format);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(TOOL_IMAGE_DECODE_MAX_EDGE_PX);
    limits.max_image_height = Some(TOOL_IMAGE_DECODE_MAX_EDGE_PX);
    reader.limits(limits);
    reader.decode().map_err(|err| match err {
        image::ImageError::Limits(_) => DropReason::TooLarge,
        _ => DropReason::Decode,
    })
}

/// 透明を白で塗って RGB へ落とす（JPEG は透明を持たない。D2）。
///
/// 透明を持たない画像はそのまま RGB へ。持つものは `out = c·α + 255·(1−α)`（四捨五入）。
fn flatten_onto_white(image: &image::DynamicImage) -> image::RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    image::RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let pixel = rgba.get_pixel(x, y);
        let alpha = u32::from(pixel[3]);
        let blend = |channel: u8| {
            let value = (u32::from(channel) * alpha + 255 * (255 - alpha) + 127) / 255;
            u8::try_from(value).unwrap_or(u8::MAX)
        };
        image::Rgb([blend(pixel[0]), blend(pixel[1]), blend(pixel[2])])
    })
}

/// 落とした理由を計器の 1 欄へ畳む（`format:1,too_many:2`）。順は [`DropReason::ALL`]。
pub fn reasons_label(dropped: &[DropReason]) -> String {
    DropReason::ALL
        .iter()
        .filter_map(|reason| {
            let count = dropped.iter().filter(|d| *d == reason).count();
            (count > 0).then(|| format!("{}:{count}", reason.label()))
        })
        .collect::<Vec<_>>()
        .join(",")
}

/// 画像を落としたことを書く 1 文（D7。`McpTool` が本文へ足す）。落としていなければ `None`。
pub fn dropped_note(language: Language, dropped: &[DropReason]) -> Option<String> {
    if dropped.is_empty() {
        return None;
    }
    let parts: Vec<String> = DropReason::ALL
        .iter()
        .filter_map(|reason| {
            let count = dropped.iter().filter(|d| *d == reason).count();
            (count > 0).then(|| match language {
                Language::Ja => format!("{} {count} 枚", reason.phrase(language)),
                Language::En => format!("{}: {count}", reason.phrase(language)),
            })
        })
        .collect();
    let total = dropped.len();
    Some(match language {
        Language::Ja => format!("（画像 {total} 枚は渡せませんでした: {}）", parts.join("、")),
        Language::En => format!(
            "({total} image(s) could not be passed on: {})",
            parts.join("; ")
        ),
    })
}

/// 本文が無く画像だけが返った結果の本文（`McpTool`）。「空の結果」と書くと嘘になる。
pub fn images_only_text(language: Language, count: usize) -> String {
    match language {
        Language::Ja => format!("（画像 {count} 枚）"),
        Language::En => format!("({count} image(s))"),
    }
}

/// 代替（直後の利用者の発話）の本文（D1。ターンの層が組んで canonical の欄で渡す）。
///
/// どのツールの結果かを名乗る — 名乗らないと、モデルは利用者が貼った画像と取り違える。
pub fn caption(language: Language, tool: &str, count: usize) -> String {
    match language {
        Language::Ja => format!("（{tool} の結果の画像 {count} 枚）"),
        Language::En => format!("({count} image(s) from the {tool} result)"),
    }
}

/// 前の周の画像を外した注記（D2b。ターンの層）。
pub fn omitted_note(language: Language) -> &'static str {
    match language {
        Language::Ja => "（前の周の画像は省きました）",
        Language::En => "(Images from earlier rounds were left out.)",
    }
}

/// 画像をモデルへ送れなかった注記（D5。ターンの層）。
pub fn fallback_note(language: Language) -> &'static str {
    match language {
        Language::Ja => "（画像はこのモデルへ送れませんでした）",
        Language::En => "(The images could not be sent to this model.)",
    }
}

/// ツールの出力の表示（Spec 57 のリング）へ足す 1 行（D8）。**画像そのものは持たない。**
///
/// 例: 「（画像 1 枚: JPEG 21 KB）」。モデルへ送る本文には足さない（表示だけ）。
pub fn display_suffix(language: Language, images: &[ToolImage]) -> Option<String> {
    if images.is_empty() {
        return None;
    }
    let items: Vec<String> = images
        .iter()
        .map(|image| {
            let format = match image.attachment.media_type {
                PromptMediaType::Jpeg => "JPEG",
                PromptMediaType::Webp => "WebP",
                other => other.as_str(),
            };
            let kib = (image.bytes + 512) / 1024;
            format!("{format} {} KB", kib.max(1))
        })
        .collect();
    let count = images.len();
    Some(match language {
        Language::Ja => format!("（画像 {count} 枚: {}）", items.join("、")),
        Language::En => format!("({count} image(s): {})", items.join(", ")),
    })
}

/// `messages` に載っているツール結果の画像の枚数。
pub fn count_tool_images(messages: &[ChatMessage]) -> usize {
    messages
        .iter()
        .filter(|m| m.role == Role::Tool)
        .map(|m| m.attachments.len())
        .sum()
}

/// ツール結果へ画像を載せる（D1。ターンの層）。代替の本文も同時に組む — 枚数を名乗るので、
/// 画像と本文を別の場所で数えると食い違う。
pub fn attach(message: &mut ChatMessage, images: Vec<PromptAttachment>, language: Language) {
    if images.is_empty() {
        return;
    }
    let tool = message.tool_name.clone().unwrap_or_default();
    message.attachment_caption = Some(caption(language, &tool, images.len()));
    message.attachments = images;
}

/// 1 要求に載せるツール結果の画像を、**新しいものから `limit` 枚まで**に絞る（D2b）。外した枚数を返す。
///
/// 外した結果の本文に [`omitted_note`] を足し、代替の本文（枚数を名乗る）を組み直す。
/// 1 つの結果の中で一部だけ外すときは、**先頭から残す**（MCP が返した順）。
///
/// **`messages` を直に書き換える** — 次の周も同じバイト列で組み直されるので、前方一致が切れるのは
/// 外した周の 1 回だけ（D3）。
pub fn cap_tool_images(messages: &mut [ChatMessage], limit: usize, language: Language) -> usize {
    let mut remaining = limit;
    let mut removed = 0;
    for message in messages.iter_mut().rev() {
        if message.role != Role::Tool || message.attachments.is_empty() {
            continue;
        }
        let keep = message.attachments.len().min(remaining);
        remaining -= keep;
        if keep == message.attachments.len() {
            continue;
        }
        removed += message.attachments.len() - keep;
        message.attachments.truncate(keep);
        append_note(&mut message.content, omitted_note(language));
        message.attachment_caption = (keep > 0).then(|| {
            caption(
                language,
                message.tool_name.as_deref().unwrap_or_default(),
                keep,
            )
        });
    }
    removed
}

/// ツール結果の画像を**すべて**外す（D5 の送り直し。ターンの層）。外した枚数を返す。
///
/// 外した結果の本文に [`fallback_note`] を足す。
pub fn strip_tool_images(messages: &mut [ChatMessage], language: Language) -> usize {
    let mut removed = 0;
    for message in messages.iter_mut() {
        if message.role != Role::Tool || message.attachments.is_empty() {
            continue;
        }
        removed += message.attachments.len();
        message.attachments.clear();
        message.attachment_caption = None;
        append_note(&mut message.content, fallback_note(language));
    }
    removed
}

/// 本文の末尾へ注記を 1 行足す。**同じ注記が既に末尾にあれば足さない**（同じ結果を 2 度絞ったとき）。
pub fn append_note(content: &mut String, note: &str) {
    if content.ends_with(note) {
        return;
    }
    if !content.is_empty() {
        content.push('\n');
    }
    content.push_str(note);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn encode(image: &image::DynamicImage, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image.write_to(&mut out, format).expect("書けること");
        out.into_inner()
    }

    fn rgb(width: u32, height: u32) -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(width, height, |x, _| {
            if x < width / 2 {
                image::Rgb([0, 200, 0])
            } else {
                image::Rgb([200, 0, 200])
            }
        }))
    }

    fn decoded(image: &ToolImage) -> image::DynamicImage {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&image.attachment.data)
            .expect("base64");
        image::load_from_memory(&bytes).expect("送る画像は読めること")
    }

    /// 上限の内側の JPEG は **バイト列をそのまま**送る（D2 の再エンコードしない経路）。
    /// NIK の `live_view_image`（JPEG 640×424・21 KB）がこの形。
    #[test]
    fn a_jpeg_within_bounds_is_sent_byte_for_byte() {
        let bytes = encode(&rgb(64, 48), image::ImageFormat::Jpeg);
        let image = normalize(&b64(&bytes)).expect("通ること");
        assert!(!image.reencoded);
        assert_eq!(image.attachment.media_type, PromptMediaType::Jpeg);
        assert_eq!(image.attachment.data, b64(&bytes), "元のバイト列のまま");
        let expected: [u8; 32] = Sha256::digest(&bytes).into();
        assert_eq!(image.digest, expected, "要約値は送るバイト列で取る（D4）");
        assert_eq!(image.bytes, bytes.len());
    }

    /// WebP も同じ（7 ワイヤとも WebP を受ける。透明は残る）。
    #[test]
    fn a_webp_within_bounds_is_sent_byte_for_byte() {
        let rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([10, 20, 30, 0]),
        ));
        let bytes = encode(&rgba, image::ImageFormat::WebP);
        let image = normalize(&b64(&bytes)).expect("通ること");
        assert!(!image.reencoded);
        assert_eq!(image.attachment.media_type, PromptMediaType::Webp);
        assert_eq!(image.attachment.data, b64(&bytes));
    }

    /// **形式は `mimeType` ではなくバイト列で決める** — 呼び出し側は mime を渡さない。
    /// PNG は JPEG へ寄せ、透明は白で塗る（この経路だけ）。
    #[test]
    fn a_png_is_reencoded_to_jpeg_with_transparency_on_white() {
        let transparent = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            16,
            16,
            image::Rgba([0, 0, 0, 0]),
        ));
        let image = normalize(&b64(&encode(&transparent, image::ImageFormat::Png)))
            .expect("通ること");
        assert!(image.reencoded);
        assert_eq!(image.attachment.media_type, PromptMediaType::Jpeg);
        let pixel = decoded(&image).to_rgb8().get_pixel(8, 8).0;
        assert!(
            pixel.iter().all(|c| *c > 240),
            "透明は黒ではなく白で塗ること: {pixel:?}"
        );
    }

    /// 長辺 1568px を超える JPEG は縮めて JPEG へ（縦横比を保つ）。
    #[test]
    fn an_oversized_jpeg_is_shrunk_to_the_edge_limit() {
        let bytes = encode(&rgb(2000, 1000), image::ImageFormat::Jpeg);
        let image = normalize(&b64(&bytes)).expect("通ること");
        assert!(image.reencoded);
        let out = decoded(&image);
        assert_eq!(out.width(), ATTACHMENT_IMAGE_MAX_EDGE_PX);
        assert_eq!(out.height(), 784, "縦横比を保つこと");
    }

    /// 大きさ（バイト）が上限を超えた JPEG も再エンコードの経路へ回る。
    #[test]
    fn a_jpeg_over_the_byte_limit_is_reencoded() {
        let bytes = encode(&rgb(64, 48), image::ImageFormat::Jpeg);
        let image = normalize_within(
            &b64(&bytes),
            Bounds {
                max_bytes: bytes.len() - 1,
                max_edge: ATTACHMENT_IMAGE_MAX_EDGE_PX,
            },
        );
        // 再エンコードしても元より小さくなるとは限らないので、結末は 2 つのどちらか。
        // **そのまま送られていないこと**が要点。
        match image {
            Ok(image) => assert!(image.reencoded),
            Err(reason) => assert_eq!(reason, DropReason::TooLarge),
        }
    }

    /// 再エンコードしても上限を超えたら落とす（TooLarge）。
    #[test]
    fn an_image_still_too_large_after_reencoding_is_dropped() {
        let bytes = encode(&rgb(64, 48), image::ImageFormat::Png);
        let reason = normalize_within(
            &b64(&bytes),
            Bounds {
                max_bytes: 10,
                max_edge: ATTACHMENT_IMAGE_MAX_EDGE_PX,
            },
        )
        .expect_err("落ちること");
        assert_eq!(reason, DropReason::TooLarge);
    }

    /// 展開の上限（伸張爆弾の歯止め）を超える画像は、展開せずに TooLarge。
    #[test]
    fn an_image_beyond_the_decode_limit_is_too_large() {
        let wide = image::DynamicImage::ImageLuma8(image::GrayImage::new(
            TOOL_IMAGE_DECODE_MAX_EDGE_PX + 1,
            1,
        ));
        let reason = normalize(&b64(&encode(&wide, image::ImageFormat::Png)))
            .expect_err("落ちること");
        assert_eq!(reason, DropReason::TooLarge);
    }

    /// GIF は運べない形式（Format）、判別できないバイト列と壊れた base64 は Decode。
    #[test]
    fn unsupported_and_unreadable_images_are_named() {
        assert_eq!(
            normalize(&b64(b"GIF89a\x01\x00\x01\x00\x00\x00\x00;")).expect_err("GIF"),
            DropReason::Format
        );
        assert_eq!(
            normalize(&b64(b"data")).expect_err("判別できない"),
            DropReason::Decode
        );
        assert_eq!(
            normalize("@@not base64@@").expect_err("base64"),
            DropReason::Decode
        );
        // 頭だけ JPEG で中身が壊れている。
        assert_eq!(
            normalize(&b64(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0])).expect_err("壊れた JPEG"),
            DropReason::Decode
        );
    }

    #[test]
    fn reasons_are_folded_in_a_fixed_order() {
        let dropped = [
            DropReason::TooMany,
            DropReason::Format,
            DropReason::TooMany,
        ];
        assert_eq!(reasons_label(&dropped), "format:1,too_many:2");
        assert_eq!(reasons_label(&[]), "");
    }

    /// 注記は村の言語で書く（D6）。英語の村に日本語を混ぜない（#149）。
    #[test]
    fn notes_follow_the_village_language() {
        let dropped = [DropReason::Format, DropReason::TooMany, DropReason::TooMany];
        let ja = dropped_note(Language::Ja, &dropped).expect("注記");
        assert_eq!(
            ja,
            "（画像 3 枚は渡せませんでした: 対応していない形式 1 枚、1 回 4 枚を超えた分 2 枚）"
        );
        let en = dropped_note(Language::En, &dropped).expect("注記");
        assert!(en.is_ascii(), "英語の注記に日本語が混ざらないこと: {en}");
        assert!(dropped_note(Language::Ja, &[]).is_none());
        assert!(caption(Language::En, "camera__snapshot", 2).is_ascii());
        assert!(omitted_note(Language::En).is_ascii());
        assert!(fallback_note(Language::En).is_ascii());
        assert!(images_only_text(Language::En, 1).is_ascii());
        assert_eq!(
            caption(Language::Ja, "camera__snapshot", 1),
            "（camera__snapshot の結果の画像 1 枚）"
        );
    }

    fn tool_message(name: &str, images: usize) -> ChatMessage {
        let mut message = ChatMessage::tool_result(format!("id_{name}"), name, "本文");
        let jpeg = encode(&rgb(4, 4), image::ImageFormat::Jpeg);
        let attachments = (0..images)
            .map(|_| PromptAttachment::new(PromptMediaType::Jpeg, b64(&jpeg)))
            .collect();
        attach(&mut message, attachments, Language::Ja);
        message
    }

    /// 1 要求 20 枚まで — **新しいものから残し**、外した結果に一文、代替の本文の枚数を組み直す（D2b）。
    #[test]
    fn capping_keeps_the_newest_images() {
        let mut messages = vec![
            ChatMessage::user("依頼"),
            tool_message("old", 3),
            tool_message("mid", 4),
            tool_message("new", 4),
        ];
        let removed = cap_tool_images(&mut messages, 6, Language::Ja);
        assert_eq!(removed, 5);
        assert_eq!(count_tool_images(&messages), 6);
        assert_eq!(messages[3].attachments.len(), 4, "最新はそのまま");
        assert_eq!(messages[2].attachments.len(), 2, "1 つ前は先頭から 2 枚残す");
        assert_eq!(messages[1].attachments.len(), 0, "最も古いものは全部外す");
        assert_eq!(
            messages[2].attachment_caption.as_deref(),
            Some("（mid の結果の画像 2 枚）"),
            "代替の本文は残した枚数を名乗ること"
        );
        assert!(messages[1].attachment_caption.is_none());
        assert!(messages[1].content.ends_with("（前の周の画像は省きました）"));
        assert_eq!(messages[3].content, "本文", "外していない結果の本文は変えない");

        // もう 1 度絞っても注記を重ねない（次の周で同じ上限に当たったとき）。
        let again = cap_tool_images(&mut messages, 5, Language::Ja);
        assert_eq!(again, 1);
        assert_eq!(
            messages[2].content.matches("前の周の画像").count(),
            1,
            "注記は 1 度だけ: {}",
            messages[2].content
        );
    }

    #[test]
    fn capping_within_the_limit_touches_nothing() {
        let mut messages = vec![tool_message("a", 4), tool_message("b", 4)];
        let before = messages.clone();
        assert_eq!(cap_tool_images(&mut messages, 20, Language::Ja), 0);
        assert_eq!(
            serde_json::to_string(&messages).unwrap(),
            serde_json::to_string(&before).unwrap()
        );
    }

    /// 送り直し（D5）は画像を全部外し、外した結果にだけ一文を足す。
    #[test]
    fn stripping_removes_every_tool_image_and_says_so() {
        let mut messages = vec![
            ChatMessage::user("依頼"),
            tool_message("a", 2),
            ChatMessage::tool_result("id_text", "text_only", "文字だけ"),
            tool_message("b", 1),
        ];
        assert_eq!(strip_tool_images(&mut messages, Language::En), 3);
        assert_eq!(count_tool_images(&messages), 0);
        assert!(messages[1].content.ends_with(fallback_note(Language::En)));
        assert!(messages[1].attachment_caption.is_none());
        assert_eq!(messages[2].content, "文字だけ", "画像の無い結果には足さない");
    }

    #[test]
    fn the_display_suffix_names_format_and_size_only() {
        let bytes = encode(&rgb(64, 48), image::ImageFormat::Jpeg);
        let image = normalize(&b64(&bytes)).expect("通ること");
        let suffix = display_suffix(Language::Ja, &[image]).expect("1 枚");
        assert!(suffix.starts_with("（画像 1 枚: JPEG "), "実際: {suffix}");
        assert!(suffix.ends_with(" KB）"));
        assert!(display_suffix(Language::Ja, &[]).is_none());
    }
}
