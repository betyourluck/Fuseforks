//! 画像の添付 — 検証・置き場・GC（Spec 23。契約は `attachment_contract`）。
//!
//! **実体はファイル、メッセージは参照。** 添付の本体は
//! `{workspace}/attachments/{uuid}.webp` に置き、[`Attachment`] は
//! id・寸法・元ファイル名だけを運ぶ。base64 をメッセージにも redb にも積まない。
//!
//! **形式は WebP に固定する**（アイコンと同じ契約 — 変換は UI 層の責務で、
//! コアは受け入れ検証だけを持つ）。検証で共有するのは [`is_webp`] の
//! マジックバイト判定だけ。`validate_icon` とは**別の述語**にする —
//! あちらは「2 種類のアイコンが 1 つの上限を共有する」を不変条件として持ち、
//! 上限をパラメータ化するとその不変条件が消える（Spec 23 rev3）。
//!
//! アニメーション WebP は拒否する。Anthropic は先頭フレームしか読まず、
//! 「動く画像を送ったつもり」と「静止画 1 枚が届いた」の食い違いは
//! 画面からは見えないため、入口で断って理由を返す。

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, CoreResult};

/// 添付の置き場（ワークスペース直下のフォルダ名）。
pub const ATTACHMENT_DIR: &str = "attachments";

/// 変換後（WebP）の許容上限。超えたら保存せず拒否する（D5）。
///
/// 元ファイル 10MB の門は UI 層が持つ（デコード前に弾く）。ここは
/// 変換を通さず IPC へ直接流し込む経路を塞ぐ側の門。
pub const ATTACHMENT_MAX_BYTES: usize = 2 * 1024 * 1024;

/// 長辺の許容上限（px）。UI 層はこの値へ縮小してから送る契約（D3）。
///
/// 1568 は Anthropic 標準ティアの長辺。高解像度ティアの 2576 を使わないのは
/// Goal のため（1 枚 4,784 視覚トークン対 1,792。Spec 23 の表）。
pub const ATTACHMENT_MAX_EDGE_PX: u32 = 1568;

/// 保持期間。起動時の GC がこれより古いファイルを消す（D9）。
pub const ATTACHMENT_RETENTION_DAYS: u64 = 30;

/// フォルダ全体の容量上限。超過分は古い順に消す（D9）。
pub const ATTACHMENT_MAX_TOTAL_BYTES: u64 = 500 * 1024 * 1024;

/// WebP コンテナのマジックバイト判定。
///
/// **`validate_icon` と添付の検証が共有する唯一の述語**（Spec 23 rev3）。
/// 先頭 `RIFF` + オフセット 8 から `WEBP`。
pub(crate) fn is_webp(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP"
}

/// 会話メッセージが運ぶ添付の参照。
///
/// 実体（バイト列）は持たない。持つと `AgentMessage` のシリアライズ経由で
/// base64 が redb と IPC へ流れ、「実体はファイル」の契約が崩れる。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    /// ファイル名の幹（UUID v4）。実体は `{workspace}/attachments/{id}.webp`。
    pub id: String,
    /// 幅（px）。検証時に WebP ヘッダから読んだ実測値。
    pub width: u32,
    /// 高さ（px）。
    pub height: u32,
    /// 元ファイル名（表示用）。パス解決には一切使わない。
    pub file_name: String,
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

/// 添付の受け入れ検証。通れば `(幅, 高さ)` を返す。
///
/// # Errors
/// 上限超・WebP 以外・アニメーション・寸法が読めない・長辺超過は
/// [`CoreError::InvalidAttachment`]。
///
/// 検査の順は「安いものから」— サイズ → マジック → ヘッダ解析。
/// 巨大な非画像バイト列でヘッダ解析まで進まない。
pub fn validate_attachment(bytes: &[u8]) -> CoreResult<(u32, u32)> {
    if bytes.len() > ATTACHMENT_MAX_BYTES {
        return Err(CoreError::InvalidAttachment {
            reason: format!(
                "サイズが上限を超えています（{} bytes > {} bytes）",
                bytes.len(),
                ATTACHMENT_MAX_BYTES
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
    if edge > ATTACHMENT_MAX_EDGE_PX {
        return Err(CoreError::InvalidAttachment {
            reason: format!(
                "長辺が上限を超えています（{edge}px > {ATTACHMENT_MAX_EDGE_PX}px。UI 側で縮小してから送る契約）"
            ),
        });
    }
    Ok((width, height))
}

/// GC の判断材料。ファイル 1 つぶんのメタデータ。
///
/// 純関数 [`gc_plan`] の入力。I/O から切り離してあるのは、
/// 「期限切れの判定」を壁時計に依存せずテストするため
/// （`schedule.rs` の「内部で `Local::now()` を呼ばない」と同じ規律）。
#[derive(Debug, Clone)]
pub struct GcEntry {
    /// ファイル名（`{id}.webp`）。
    pub file_name: String,
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

    /// id からファイルパスを解決する。
    fn path_for(&self, id: &str) -> CoreResult<PathBuf> {
        if !Self::is_safe_id(id) {
            return Err(CoreError::UnsafeIdentifier {
                value: id.to_string(),
            });
        }
        Ok(self.dir().join(format!("{id}.webp")))
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
        let (width, height) = validate_attachment(bytes)?;
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.dir();
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| Self::io_err(&dir, e))?;
        let path = dir.join(format!("{id}.webp"));
        tokio::fs::write(&path, bytes)
            .await
            .map_err(|e| Self::io_err(&path, e))?;
        Ok(Attachment {
            id,
            width,
            height,
            file_name: file_name.to_owned(),
        })
    }

    /// id で実体を読む。**無ければ `None`**。
    ///
    /// GC で消えた添付の表示は「保持期間を過ぎて削除されました」の枠に
    /// なる（D9）ので、不在はエラーではなく通常の答え。
    pub async fn read(&self, id: &str) -> CoreResult<Option<Vec<u8>>> {
        let path = self.path_for(id)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(Self::io_err(&path, err)),
        }
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
            if path.extension().and_then(|e| e.to_str()) != Some("webp") {
                continue; // 添付以外（手で置かれた何か）には触らない
            }
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
        Ok(GcReport {
            removed: doomed.len(),
            remaining_files: remaining.len(),
            remaining_bytes: remaining.iter().map(|e| e.len).sum(),
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
                "concordia-attachment-{tag}-{}",
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
        let err = validate_attachment(&vec![0u8; ATTACHMENT_MAX_BYTES + 1]).unwrap_err();
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
            validate_attachment(&plain_webp(ATTACHMENT_MAX_EDGE_PX, 10)).unwrap(),
            (ATTACHMENT_MAX_EDGE_PX, 10)
        );
        let err =
            validate_attachment(&plain_webp(ATTACHMENT_MAX_EDGE_PX + 1, 10)).unwrap_err();
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
        assert_eq!((att.width, att.height), (320, 200));
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
            GcEntry { file_name: "new.webp".into(), modified: new, len: third },
            GcEntry { file_name: "old.webp".into(), modified: old, len: third },
            GcEntry { file_name: "mid.webp".into(), modified: mid, len: third },
        ];
        assert_eq!(gc_plan(&entries, now), vec!["old.webp".to_owned()]);
    }

    /// 上限以内なら容量では消さない。
    #[test]
    fn gc_plan_keeps_everything_under_the_cap() {
        let now = SystemTime::now();
        let entries = vec![GcEntry {
            file_name: "a.webp".into(),
            modified: now,
            len: 1024,
        }];
        assert!(gc_plan(&entries, now).is_empty());
    }
}
