use super::{LibraryId, Result};
use rusqlite::{params, OptionalExtension, Transaction};

/// 品質不明の画像は、縮小版と断定しない。確認済み原寸のみ常に優先する。
pub(super) fn prefer_saved_quality(
    tx: &Transaction<'_>,
    media: LibraryId,
    selected: Option<&[u8]>,
) -> Result<Option<(Vec<u8>, String)>> {
    let quality: Option<String> = selected
        .map(|id| {
            tx.query_row(
                "SELECT quality FROM media_renditions WHERE id=?1",
                [id],
                |r| r.get(0),
            )
        })
        .transpose()?;
    if quality.as_deref() == Some("original") {
        return Ok(None);
    }
    // 未公開の保存状態や、別の source_media に属する実体は候補にしない。
    // ファイル名も選択した実体のものを使い、縮小版の拡張子を引き継がない。
    Ok(tx.query_row(
        "SELECT r.id,(SELECT m.original_name FROM snapshot_media m JOIN revision_snapshots s ON s.id=m.snapshot_id WHERE m.rendition_id=r.id AND s.published=1 ORDER BY s.id,m.slot LIMIT 1)
         FROM media_renditions r WHERE r.media_id=?1 AND (r.quality='original' OR (?2=1 AND r.quality='master'))
         AND EXISTS(SELECT 1 FROM snapshot_media m JOIN revision_snapshots s ON s.id=m.snapshot_id WHERE m.rendition_id=r.id AND s.published=1)
         ORDER BY CASE r.quality WHEN 'original' THEN 0 ELSE 1 END,r.id LIMIT 1",
        params![media.as_bytes().as_slice(),matches!(quality.as_deref(),None|Some("thumbnail"))],
        |r|Ok((r.get(0)?,r.get(1)?)),
    ).optional()?)
}
