//! RAG ソースの索引。
//!
//! 実運用では埋め込みモデルを呼ぶことになるが、この層の役割は
//! 「**検索が Rayon 側で走り、Tokio 側をブロックしない**」という接続の型を確定させること。
//! 埋め込み器は [`Embedder`] trait として差し替え点にしてあり、
//! 既定の [`HashEmbedder`] はモデルを持たずに決定論的なベクトルを返す。
//!
//! `HashEmbedder` は意味を捉えないので検索品質は出ない。実装を差し替えるまでの
//! 骨組みとして置いてあり、その旨を隠さないために名前で明示している。

use std::collections::BTreeMap;

use crate::compute::{self, Scored};
use crate::error::CoreResult;

/// 埋め込みベクトルの次元数。
pub const EMBEDDING_DIM: usize = 256;

/// テキストを埋め込みベクトルへ変換する差し替え点。
pub trait Embedder: Send + Sync {
    /// 1 件の文書を埋め込む。
    fn embed(&self, text: &str) -> Vec<f32>;
}

/// モデルを持たない決定論的な埋め込み器。
///
/// 語をハッシュして次元に振り分けるだけの bag-of-words で、意味的な近さは捉えない。
/// 実モデルを繋ぐまでの土台であり、検索の**配線**（Rayon への逃がし、上位 k 件の取得、
/// UI への引き渡し）を先に固めるために置いている。
pub struct HashEmbedder;

impl Embedder for HashEmbedder {
    fn embed(&self, text: &str) -> Vec<f32> {
        let mut vector = vec![0.0f32; EMBEDDING_DIM];
        for token in text.split_whitespace() {
            let mut hash: u64 = 1469598103934665603; // FNV-1a offset basis
            for byte in token.to_lowercase().bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(1099511628211);
            }
            vector[(hash as usize) % EMBEDDING_DIM] += 1.0;
        }
        vector
    }
}

/// 索引に格納する 1 断片。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagChunk {
    /// 断片の一意 ID。
    pub id: String,
    /// 所属する RAG ソース名。エージェントの `rag_sources` と突き合わせる。
    pub source: String,
    /// 本文。
    pub text: String,
}

/// メモリ常駐の RAG 索引。
///
/// ソース名ごとに断片を保持し、検索時に対象ソースだけを走査する。
/// エージェントごとに参照ソースが違うので、全件走査を避ける構造にしてある。
pub struct RagIndex {
    embedder: Box<dyn Embedder>,
    by_source: BTreeMap<String, Vec<(RagChunk, Vec<f32>)>>,
}

impl RagIndex {
    /// 埋め込み器を指定して空の索引を作る。
    pub fn new(embedder: Box<dyn Embedder>) -> Self {
        Self {
            embedder,
            by_source: BTreeMap::new(),
        }
    }

    /// 断片を索引へ追加する。埋め込みは追加時に 1 回だけ計算する。
    pub fn insert(&mut self, chunk: RagChunk) {
        let embedding = self.embedder.embed(&chunk.text);
        self.by_source
            .entry(chunk.source.clone())
            .or_default()
            .push((chunk, embedding));
    }

    /// 登録済みのソース名一覧。GUI の選択パネルに出す。
    pub fn sources(&self) -> Vec<String> {
        self.by_source.keys().cloned().collect()
    }

    /// 指定ソース群の総断片数。
    pub fn len(&self) -> usize {
        self.by_source.values().map(Vec::len).sum()
    }

    /// 索引が空かどうか。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 指定ソース群から上位 `k` 件を検索する。
    ///
    /// 類似度計算は [`compute::spawn_rayon`] 経由で Rayon プールへ逃がすため、
    /// 呼び出し元の Tokio ワーカーはこの間も他のエージェントを捌ける。
    ///
    /// # Errors
    /// Rayon 側が結果を返さずに終了した場合にエラーを返す。
    pub async fn search(
        &self,
        sources: &[String],
        query: &str,
        k: usize,
    ) -> CoreResult<Vec<Scored<RagChunk>>> {
        // 対象ソースの断片だけを集める。埋め込み済みなのでここは複製のみ。
        let corpus: Vec<(RagChunk, Vec<f32>)> = sources
            .iter()
            .filter_map(|s| self.by_source.get(s))
            .flatten()
            .cloned()
            .collect();

        if corpus.is_empty() {
            return Ok(Vec::new());
        }

        let query_vector = self.embedder.embed(query);
        compute::spawn_rayon(move || compute::top_k_similar(&query_vector, &corpus, k)).await
    }
}

impl Default for RagIndex {
    fn default() -> Self {
        Self::new(Box::new(HashEmbedder))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, source: &str, text: &str) -> RagChunk {
        RagChunk {
            id: id.into(),
            source: source.into(),
            text: text.into(),
        }
    }

    #[tokio::test]
    async fn search_is_scoped_to_the_requested_sources() {
        let mut index = RagIndex::default();
        index.insert(chunk("1", "wiki_db", "tauri rust desktop"));
        index.insert(chunk("2", "wiki_db", "vue frontend"));
        index.insert(chunk("3", "other_db", "tauri rust desktop"));

        let hits = index
            .search(&["wiki_db".to_owned()], "tauri rust", 5)
            .await
            .unwrap();

        assert_eq!(hits.len(), 2, "wiki_db の 2 件だけが対象");
        assert_eq!(hits[0].item.id, "1");
        assert!(hits[0].score > hits[1].score);
    }

    #[tokio::test]
    async fn search_on_unknown_source_returns_empty_rather_than_error() {
        let index = RagIndex::default();
        let hits = index.search(&["missing".to_owned()], "何か", 3).await.unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn embedding_is_deterministic() {
        let embedder = HashEmbedder;
        assert_eq!(embedder.embed("同じ 入力"), embedder.embed("同じ 入力"));
        assert_eq!(embedder.embed("x").len(), EMBEDDING_DIM);
    }
}
