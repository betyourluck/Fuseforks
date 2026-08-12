//! **carries 表と adapter の実装が食い違わないことを機械で留める**
//! （Spec 36 D2「提示と判定の集合を分けない」）。
//!
//! `Provider::carries` は送信入口の門（Spec 36 P3）が読む述語で、adapter は
//! 実際にワイヤへ組み立てる側。**2 つは別の場所に書かれているので、片方だけ
//! 直すと「門は通すのに黙って落ちる」か「門が拒むのに実は送れる」になる。**
//! どちらもコンパイラにも lint にも掛からない（`defaultEnabledTools.test.ts` や
//! `toolLabel.test.ts` が Rust の表と TS の表を突き合わせているのと同じ網）。
//!
//! **adapter が落とすのは最後の砦であって、正面の門ではない。** 実運用では
//! P3 の `carries` が先に断るので、ここへ来る「運べない添付」は存在しない。
//! それでも両者を突き合わせるのは、**門を後から緩めたときに adapter の沈黙が
//! 露出する**形を塞ぐため。
//!
//! 表の出どころは P0 の probe 18 発（2026-08-12）で、契約は
//! `data_contract.yaml` の `attachment_contract` にある。

use fuseforks_core::attachment::AttachmentKind;
use fuseforks_core::llm::{
    ChatMessage, ChatRequest, PromptAttachment, PromptMediaType, Provider,
};

/// 種別ごとの代表的な形式（音声は mp3 / wav の 2 形式があるが、
/// carries は種別の粒度なのでどちらでも同じ答えになる）。
fn media_for(kind: AttachmentKind) -> PromptMediaType {
    match kind {
        AttachmentKind::Image => PromptMediaType::Webp,
        AttachmentKind::Audio => PromptMediaType::Wav,
        AttachmentKind::Video => PromptMediaType::Mp4,
        AttachmentKind::Pdf => PromptMediaType::Pdf,
    }
}

/// 添付 1 件だけを載せたリクエスト。
fn request_with(kind: AttachmentKind) -> ChatRequest {
    ChatRequest::plain(
        "m",
        vec![ChatMessage::user_with_attachments(
            "これは何？",
            vec![PromptAttachment::new(media_for(kind), "QUJD")],
        )],
        64,
    )
}

/// そのワイヤの encode 結果に、添付のデータが実際に現れるか。
///
/// **JSON 文字列に base64 が出るかで見る** — ブロックの型名は社ごとに違う
/// （`image_url` / `input_image` / `inlineData` / `document`）ので、
/// 種別ごとに構造を書き分けると、この網自体が表の写しになってしまう。
fn adapter_emits(provider: Provider, kind: AttachmentKind) -> bool {
    let req = request_with(kind);
    let json = match provider {
        Provider::OpenAiCompat => {
            serde_json::to_string(&fuseforks_core::llm::openai_compat::encode(&req, true)).unwrap()
        }
        Provider::Anthropic => {
            serde_json::to_string(&fuseforks_core::llm::anthropic::encode(&req)).unwrap()
        }
        Provider::Gemini => {
            serde_json::to_string(&fuseforks_core::llm::gemini::encode(&req, false)).unwrap()
        }
        Provider::XaiResponses => {
            serde_json::to_string(&fuseforks_core::llm::xai_responses::encode(&req, true, false, false))
                .unwrap()
        }
        Provider::OpenAiResponses => serde_json::to_string(
            &fuseforks_core::llm::openai_responses::encode(&req, true, false, false),
        )
        .unwrap(),
        Provider::MetaResponses => {
            serde_json::to_string(&fuseforks_core::llm::meta_responses::encode(&req, true, false))
                .unwrap()
        }
    };
    json.contains("QUJD")
}

/// **全 20 マス**（4 種別 × 5 ワイヤ）で、述語と実装が一致する。
///
/// ここが落ちたら、直すのは**両方を見てから**。片方だけ合わせると、
/// 表が実装に追随しただけで「観測で決めた」という根拠が消える
/// （表の出どころは P0 の probe で、実装ではない）。
#[test]
fn adapters_match_the_carries_table() {
    // **`Provider::ALL` を使わない。** ここは「表が覆うべきマス」の期待値で、
    // 実装から導出すると 6 値目を足したときに黙って 24 マスへ増える。
    // 手で並べてあるから、variant を足した人が**この行でも決めさせられる**。
    let providers = [
        Provider::OpenAiCompat,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::XaiResponses,
        Provider::OpenAiResponses,
        Provider::MetaResponses,
    ];
    let mut checked = 0;
    for provider in providers {
        for kind in AttachmentKind::ALL {
            assert_eq!(
                provider.carries(kind),
                adapter_emits(provider, kind),
                "{provider:?} × {kind:?}: carries の表と adapter の実装が食い違っている",
            );
            checked += 1;
        }
    }
    assert_eq!(checked, 24, "表は 4 種別 × 6 ワイヤの全マスを覆う");
}

/// **表そのものを逐語で凍結する**（P0 の probe 18 発の観測結果）。
///
/// 上のテストは「述語と実装が一致する」ことしか見ないので、**両方を同時に
/// 変えると通ってしまう**。観測で決めた値はここで別に留める。
#[test]
fn the_carries_table_matches_what_the_probes_observed() {
    use AttachmentKind::{Audio, Image, Pdf, Video};
    let expected = [
        // (provider, image, audio, video, pdf)
        (Provider::OpenAiCompat, true, true, false, true),
        (Provider::Anthropic, true, false, false, true),
        (Provider::Gemini, true, true, true, true),
        // xAI の PDF は**公式文書に記述が無いのに通った**（乖離 2 例目）。
        (Provider::XaiResponses, true, false, false, true),
        (Provider::OpenAiResponses, true, false, false, true),
        // **Gemini に次ぐ 2 本目の「4 種別すべて」**（Spec 37 P0）。
        // 動画は openai_responses からの類推で ✗ と書くところを、
        // payload 無しの `input_video` の名指し 400 が実在を教えて撃てた。
        (Provider::MetaResponses, true, true, true, true),
    ];
    for (provider, image, audio, video, pdf) in expected {
        assert_eq!(provider.carries(Image), image, "{provider:?} の画像");
        assert_eq!(provider.carries(Audio), audio, "{provider:?} の音声");
        assert_eq!(provider.carries(Video), video, "{provider:?} の動画");
        assert_eq!(provider.carries(Pdf), pdf, "{provider:?} の PDF");
    }
}

/// **画像の行が全 ✓**（Spec 36 D9 の完了条件）。
///
/// 「ネイティブを選ぶと画像が黙って落ちる」3 例（gemini / xai_responses /
/// open_ai_responses。Spec 34 検収 7）が解消したことを、表の 1 行として読む。
#[test]
fn every_wire_carries_images_now() {
    for provider in [
        Provider::OpenAiCompat,
        Provider::Anthropic,
        Provider::Gemini,
        Provider::XaiResponses,
        Provider::OpenAiResponses,
        Provider::MetaResponses,
    ] {
        assert!(
            provider.carries(AttachmentKind::Image),
            "{provider:?} は画像を運ぶ（D9 の回収）",
        );
    }
}

/// **添付が無い発話は 5 ワイヤすべてでバイト等価**（Goal「使わない村の
/// 固定費はゼロ」の機械側）。
///
/// 各 adapter の単体テストが golden を持っているが、**ここでは 5 本を同じ
/// 入力で横に並べる** — 1 本だけ「常にブロック列を作る」実装へ倒れる退行は、
/// 個別の golden では気づけても表の側からは見えない。
#[test]
fn no_attachment_means_no_block_list_on_any_wire() {
    let req = ChatRequest::plain("m", vec![ChatMessage::user("こんにちは")], 64);

    let oai = serde_json::to_value(fuseforks_core::llm::openai_compat::encode(&req, true)).unwrap();
    assert_eq!(oai["messages"][0]["content"], "こんにちは", "互換は素の文字列");

    let ant = serde_json::to_value(fuseforks_core::llm::anthropic::encode(&req)).unwrap();
    assert_eq!(ant["messages"][0]["content"][0]["type"], "text");
    assert_eq!(ant["messages"][0]["content"].as_array().unwrap().len(), 1);

    let gem = serde_json::to_value(fuseforks_core::llm::gemini::encode(&req, false)).unwrap();
    assert_eq!(
        gem["contents"][0]["parts"],
        serde_json::json!([{ "text": "こんにちは" }]),
    );

    for responses in [
        serde_json::to_value(fuseforks_core::llm::xai_responses::encode(&req, true, false, false))
            .unwrap(),
        serde_json::to_value(fuseforks_core::llm::openai_responses::encode(&req, true, false, false))
            .unwrap(),
        serde_json::to_value(fuseforks_core::llm::meta_responses::encode(&req, true, false))
            .unwrap(),
    ] {
        assert_eq!(
            responses["input"][0]["content"], "こんにちは",
            "Responses も素の文字列",
        );
    }
}
