//! 画像添付の JPEG フォールバック（Spec 23 D3）を、HTTP 経路ごと踏む。
//!
//! # なぜスタブを立てるか
//!
//! この分岐は**応答コードで決まる**（`OpenAiCompat` × 400 × WebP 添付あり）ので、
//! 純関数の単体テストでは配線まで届かない。変換（`with_jpeg_attachments`）と
//! 発火条件（`has_webp_attachments`）は `client.rs` の単体で留めてあるが、
//! 「400 を受けて実際に再送するか」「無関係な 400 で再送しないか」は
//! `LlmBackend::chat` を通さないと分からない。
//!
//! P0 の実測では 5 系統すべてが WebP を受け入れたため、**拒む接続先が実在しない**。
//! 実機で踏めない経路をテスト無しで残すのは、**エラー回復の経路が必要になった
//! 瞬間に壊れている**という一番痛い形になる。
//!
//! # スタブの作り
//!
//! ループバックに `TcpListener` を 1 本立て、リクエスト本文を見て応答を返すだけ。
//! HTTP/1.1 の最小の形（ステータス行 + `content-length` + 本文）しか喋らない —
//! reqwest の相手として成立する範囲で十分で、それ以上は実装しない。
//! 受け取った本文は全部記録するので、**何回・どの形式で送ったか**を後から数えられる。

use std::sync::{Arc, Mutex};

use fuseforks_core::llm::{ChatMessage, ChatRequest, ImageAttachment, ImageMediaType, LlmBackend};
use fuseforks_core::model::{CredentialSource, ModelTemplate};
use fuseforks_core::{HttpLlmBackend, InMemorySecretStore, LlmConfig, Provider};

/// スタブが返す応答の決め方。**接続先の性格を 3 つに分けている。**
#[derive(Clone, Copy, PartialEq, Eq)]
enum Policy {
    /// WebP を 400 で拒み、それ以外は成功。**フォールバックが通る接続先。**
    WebpUnsupported,
    /// 画像が何であれ 400。**画像を受け付けない接続先。**
    NoImagesAtAll,
    /// 何を送っても 400。**画像とは無関係な失敗**（パラメータ誤りなど）。
    AlwaysBadRequest,
}

/// 受け取ったリクエスト本文の記録。
type Bodies = Arc<Mutex<Vec<String>>>;

/// ループバックに最小の OpenAI 互換スタブを立て、`(base_url, 記録)` を返す。
async fn spawn_stub(policy: Policy) -> (String, Bodies) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let bodies: Bodies = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&bodies);

    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let recorded = Arc::clone(&recorded);
            tokio::spawn(async move {
                // ヘッダと本文をまとめて読む。`content-length` を見て、本文が
                // 揃うまで読み足す（リクエストは 1 本 1 接続で来る前提）。
                let mut raw = Vec::new();
                let mut buf = [0u8; 8192];
                loop {
                    let Ok(n) = socket.read(&mut buf).await else {
                        return;
                    };
                    if n == 0 {
                        break;
                    }
                    raw.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&raw);
                    let Some(head_end) = text.find("\r\n\r\n") else {
                        continue;
                    };
                    let len: usize = text[..head_end]
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())?
                        })
                        .unwrap_or(0);
                    if raw.len() >= head_end + 4 + len {
                        break;
                    }
                }

                let text = String::from_utf8_lossy(&raw).into_owned();
                let body = text
                    .split_once("\r\n\r\n")
                    .map(|(_, body)| body.to_owned())
                    .unwrap_or_default();
                let has_webp = body.contains("data:image/webp");
                let has_jpeg = body.contains("data:image/jpeg");
                recorded.lock().unwrap().push(body);

                let reject = match policy {
                    Policy::WebpUnsupported => has_webp,
                    Policy::NoImagesAtAll => has_webp || has_jpeg,
                    Policy::AlwaysBadRequest => true,
                };
                let payload = if reject {
                    // 400 の本文はプロバイダごとに形が違うので、素の JSON で返す。
                    r#"{"error":{"message":"unsupported image format"}}"#.to_owned()
                } else {
                    r#"{"choices":[{"message":{"role":"assistant","content":"見えました"},
                        "finish_reason":"stop"}],
                        "usage":{"prompt_tokens":1,"completion_tokens":1}}"#
                        .replace(['\n', ' '], "")
                };
                let status = if reject { "400 Bad Request" } else { "200 OK" };
                let response = format!(
                    "HTTP/1.1 {status}\r\ncontent-type: application/json\r\n\
                     content-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            });
        }
    });

    (format!("http://127.0.0.1:{port}/v1"), bodies)
}

/// スタブへ向けたバックエンドを組む。**再試行は 1 回**（バックオフで待たない）。
fn backend(base_url: &str, provider: Provider) -> HttpLlmBackend {
    let mut template = ModelTemplate::new("tpl_stub", "スタブ", "stub-model");
    template.base_url = base_url.to_owned();
    template.credential = CredentialSource::NotRequired;
    template.provider = Some(provider);
    // 一過性エラーの再試行と、フォールバックの再送を混ぜないため 1 回に固定する。
    template.max_retries = 1;

    let config = LlmConfig::from_template(&template, &InMemorySecretStore::new()).unwrap();
    HttpLlmBackend::new(config).unwrap()
}

/// WebP を 1 枚積んだリクエスト。
fn request_with_image() -> ChatRequest {
    ChatRequest::plain(
        "stub-model",
        vec![ChatMessage::user_with_attachments(
            "何が見える？",
            vec![ImageAttachment {
                media_type: ImageMediaType::Webp,
                // 4×4 の実 WebP（image crate がデコードできる形でないと変換が失敗する）。
                data: tiny_webp_base64(),
            }],
        )],
        64,
    )
}

/// 実在の WebP を base64 で作る。手組みのヘッダではデコーダが本文を要求して落ちる。
fn tiny_webp_base64() -> String {
    use base64::Engine as _;
    let img =
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(4, 4, image::Rgb([1, 2, 3])));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::WebP).unwrap();
    base64::engine::general_purpose::STANDARD.encode(out.into_inner())
}

/// **WebP が 400 で拒まれたら JPEG へ落として通る**（D3 の本体）。
///
/// 1 回目は WebP、2 回目は JPEG。**2 回で止まる**（無限に形式を試さない）。
#[tokio::test]
async fn a_webp_rejection_retries_once_as_jpeg_and_succeeds() {
    let (url, bodies) = spawn_stub(Policy::WebpUnsupported).await;
    let response = backend(&url, Provider::OpenAiCompat)
        .chat(request_with_image())
        .await
        .expect("JPEG へ落ちて成功すること");

    assert_eq!(response.text.as_deref(), Some("見えました"));

    let bodies = bodies.lock().unwrap();
    assert_eq!(bodies.len(), 2, "送ったのは 2 回だけ");
    assert!(bodies[0].contains("data:image/webp"), "1 回目は WebP");
    assert!(bodies[1].contains("data:image/jpeg"), "2 回目は JPEG");
    assert!(
        !bodies[1].contains("data:image/webp"),
        "再送に WebP が残っていないこと"
    );
}

/// **両方拒まれたら、画像が原因だと分かる文面で返す。**
///
/// 生の 400 本文だけでは、利用者は「画像が原因」へ辿り着けない。
#[tokio::test]
async fn refusing_both_formats_names_the_image_as_the_cause() {
    let (url, bodies) = spawn_stub(Policy::NoImagesAtAll).await;
    let err = backend(&url, Provider::OpenAiCompat)
        .chat(request_with_image())
        .await
        .expect_err("両方拒まれたら失敗すること");

    let text = err.to_string();
    assert!(
        text.contains("この接続先は画像を受け付けません"),
        "画像が原因だと名指しすること: {text}"
    );
    // プロバイダの応答も落とさない（診断の材料を捨てない）。
    assert!(text.contains("unsupported image format"), "{text}");
    assert_eq!(bodies.lock().unwrap().len(), 2, "試すのは 2 回まで");
}

/// **添付の無いリクエストは再送しない。**
///
/// ここが崩れると、画像と無関係な 400（パラメータ誤りなど）で
/// **もう 1 回課金して同じ失敗を繰り返す**。
#[tokio::test]
async fn a_plain_400_is_not_retried() {
    let (url, bodies) = spawn_stub(Policy::AlwaysBadRequest).await;
    let request = ChatRequest::plain("stub-model", vec![ChatMessage::user("やあ")], 64);

    backend(&url, Provider::OpenAiCompat)
        .chat(request)
        .await
        .expect_err("400 は失敗のまま返ること");

    assert_eq!(bodies.lock().unwrap().len(), 1, "再送しないこと");
}

/// **Anthropic 経路では発火しない。**
///
/// フォールバックは互換層のためのもの。Anthropic は P0 の実測で WebP を
/// 受けており、ここで再送すると**通るはずの経路で無駄に 2 回払う**。
///
/// スタブは Anthropic の形（`/messages`）を喋らないので 400 が返るが、
/// **見たいのは「2 回目を送らないこと」**なので応答の中身は問わない。
#[tokio::test]
async fn the_anthropic_path_never_falls_back() {
    let (url, bodies) = spawn_stub(Policy::AlwaysBadRequest).await;
    backend(&url, Provider::Anthropic)
        .chat(request_with_image())
        .await
        .expect_err("スタブは 400 を返す");

    assert_eq!(
        bodies.lock().unwrap().len(),
        1,
        "Anthropic では JPEG 再送を試みないこと"
    );
}
