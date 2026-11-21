//! 応答本文を、溜める前に上限を見ながら 1 塊ずつ読む（Spec 77 #7b / N5）。
//!
//! `response.text()` / `response.bytes()` は本文を**全部読んでから**返す。`Content-Length` を付けずに
//! 桁外れの本文を送る相手にはその大きさのメモリを確保し、送り終えない相手には待ちの上限まで
//! 止まる。rmcp の `ReqwestOAuthHttpClient` が 1 塊ずつ読んでいるのと同じ形を、外へ出る HTTP の
//! 全経路（LLM・判断モデル・画像生成・OAuth・単価表）で共有する。
//!
//! reqwest は 2 つの版がツリーに居る（0.12 = こちらの HTTP / 0.13 = rmcp と OAuth）ので、
//! 読む側の 1 塊を [`ChunkSource`] で抽象し、**上限の判定は [`read_capped`] / [`read_prefix`] の
//! 1 実装**に置く。

use async_trait::async_trait;

/// 本文を 1 塊ずつ出す相手（reqwest の `Response`）。
#[async_trait]
pub trait ChunkSource: Send {
    /// 宣言された本文の長さ（`Content-Length`）。宣言が無ければ `None`。
    fn declared_len(&self) -> Option<u64>;
    /// 読み取りの失敗（reqwest の型をそのまま運ぶ — 待ちの上限か接続の失敗かで再試行の判断が分かれる）。
    type Error: Send;
    /// 次の塊。本文の終わりなら `None`。
    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, Self::Error>;
}

#[async_trait]
impl ChunkSource for reqwest::Response {
    type Error = reqwest::Error;

    fn declared_len(&self) -> Option<u64> {
        self.content_length()
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, reqwest::Error> {
        self.chunk().await.map(|chunk| chunk.map(|c| c.to_vec()))
    }
}

#[async_trait]
impl ChunkSource for reqwest_rmcp::Response {
    type Error = reqwest_rmcp::Error;

    fn declared_len(&self) -> Option<u64> {
        self.content_length()
    }

    async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, reqwest_rmcp::Error> {
        self.chunk().await.map(|chunk| chunk.map(|c| c.to_vec()))
    }
}

/// 上限つきで読んだ結果。
#[derive(Debug, PartialEq, Eq)]
pub enum CappedBody {
    /// 本文の全部（上限以内）。
    Complete(Vec<u8>),
    /// 上限を超えた（宣言の長さか、読んだ量で分かった時点で読むのをやめた）。
    Exceeded,
}

/// 本文を上限まで読む。上限を超えたら、残りを読まずに [`CappedBody::Exceeded`] を返す。
///
/// # Errors
/// 読み取りに失敗したとき（接続の切断・待ちの上限）。
pub async fn read_capped<S: ChunkSource>(
    source: &mut S,
    limit: usize,
) -> Result<CappedBody, S::Error> {
    if source.declared_len().is_some_and(|len| len > limit as u64) {
        return Ok(CappedBody::Exceeded);
    }
    let mut body = Vec::new();
    while let Some(chunk) = source.next_chunk().await? {
        if body.len() + chunk.len() > limit {
            return Ok(CappedBody::Exceeded);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(CappedBody::Complete(body))
}

/// 本文の先頭を `limit` バイトまで読む（エラーの本文のように、先頭だけあれば足りるもの）。
/// 上限に達したら残りを読まずに返す。読み取りの失敗は、そこまでに読めた分を返す。
pub async fn read_prefix(source: &mut impl ChunkSource, limit: usize) -> Vec<u8> {
    let mut body = Vec::new();
    while body.len() < limit {
        match source.next_chunk().await {
            Ok(Some(chunk)) => {
                let room = limit - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    body
}

/// 本文の先頭を `limit` バイトまで読み、残りは読まない（Spec 78 D7 — robots.txt は先頭 500 KiB を
/// 解析すればよく、超えても断らない）。
///
/// [`read_prefix`] と違い、**読み取りの失敗を返す** — 途中で切れた規則で判定しないため（`Disallow` の行が
/// 途中で切れて短い道に化けると、許すべきでない道を許しうる）。
///
/// # Errors
/// 読み取りに失敗したとき（接続の切断・待ちの上限）。
pub async fn read_head<S: ChunkSource>(source: &mut S, limit: usize) -> Result<Vec<u8>, S::Error> {
    let mut body = Vec::new();
    while body.len() < limit {
        match source.next_chunk().await? {
            Some(chunk) => {
                let room = limit - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(room)]);
            }
            None => break,
        }
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 塊を順に出す偽の相手。何塊目まで読まれたかを数える。
    struct Chunks {
        declared: Option<u64>,
        chunks: Vec<Vec<u8>>,
        read: usize,
    }

    #[async_trait]
    impl ChunkSource for Chunks {
        type Error = String;

        fn declared_len(&self) -> Option<u64> {
            self.declared
        }
        async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
            let next = self.chunks.get(self.read).cloned();
            if next.is_some() {
                self.read += 1;
            }
            Ok(next)
        }
    }

    fn chunks(declared: Option<u64>, sizes: &[usize]) -> Chunks {
        Chunks {
            declared,
            chunks: sizes.iter().map(|n| vec![b'x'; *n]).collect(),
            read: 0,
        }
    }

    #[tokio::test]
    async fn a_body_within_the_limit_is_read_whole() {
        let mut source = chunks(None, &[3, 4, 3]);
        assert_eq!(read_capped(&mut source, 10).await.unwrap(), CappedBody::Complete(vec![b'x'; 10]));
    }

    #[tokio::test]
    async fn reading_stops_at_the_chunk_that_crosses_the_limit() {
        let mut source = chunks(None, &[4, 4, 4, 4]);
        assert_eq!(read_capped(&mut source, 10).await.unwrap(), CappedBody::Exceeded);
        assert_eq!(source.read, 3, "4 塊目は読まない");
    }

    #[tokio::test]
    async fn a_declared_length_over_the_limit_is_refused_before_reading() {
        let mut source = chunks(Some(11), &[11]);
        assert_eq!(read_capped(&mut source, 10).await.unwrap(), CappedBody::Exceeded);
        assert_eq!(source.read, 0, "1 塊も読まない");
    }

    /// 外へ出る HTTP の応答本文は、どの経路もここの 2 本で読む（Spec 77 N5）。`text()` / `bytes()` は
    /// 全部読んでから返すので、1 か所でも戻ると上限が効かない経路が生まれる。結合テスト
    /// （`tests/response_body_cap.rs`）が叩くのは LLM と OAuth の探索だけなので、残りは字面で留める。
    #[test]
    fn every_outbound_response_is_read_through_the_cap() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for file in [
            "llm/client.rs",
            "jev.rs",
            "image_gen.rs",
            "mcp_oauth.rs",
            "tools/fetch/mod.rs",
        ] {
            let path = root.join(file);
            let src = std::fs::read_to_string(&path)
                .unwrap_or_else(|e| panic!("{} を読めない: {e}", path.display()));
            let code: String = src
                .split("#[cfg(test)]")
                .next()
                .unwrap_or("")
                .lines()
                .map(|l| l.split("//").next().unwrap_or(""))
                .collect::<Vec<_>>()
                .join("\n");
            for banned in [".text().await", ".bytes().await", ".text()).await"] {
                assert!(!code.contains(banned), "{file} が応答本文を `{banned}` で全部読んでいる");
            }
        }
    }

    #[tokio::test]
    async fn a_head_keeps_the_head_and_stops() {
        let mut source = chunks(None, &[4, 4, 4, 4]);
        assert_eq!(read_head(&mut source, 10).await.unwrap().len(), 10);
        assert_eq!(source.read, 3, "上限に達したら次の塊を読まない");
    }

    /// 失敗を飲まない（[`read_prefix`] との違い）。途中まで読めていても `Err` を返す。
    #[tokio::test]
    async fn a_head_reports_a_failed_read() {
        struct Breaks(usize);
        #[async_trait]
        impl ChunkSource for Breaks {
            type Error = String;
            fn declared_len(&self) -> Option<u64> {
                None
            }
            async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>, String> {
                self.0 += 1;
                if self.0 == 1 { Ok(Some(vec![b'x'; 4])) } else { Err("切れた".into()) }
            }
        }
        assert_eq!(read_head(&mut Breaks(0), 10).await, Err("切れた".to_owned()));
    }

    #[tokio::test]
    async fn a_prefix_keeps_the_head_and_stops() {
        let mut source = chunks(None, &[4, 4, 4, 4]);
        assert_eq!(read_prefix(&mut source, 10).await.len(), 10);
        assert_eq!(source.read, 3, "上限に達したら次の塊を読まない");
    }
}
