//! 棚で、縮小版のまま保存されている pixiv の表紙・挿絵を探す。
//!
//! 取り直しそのものは、保存済みの応答に原寸の在り処を足して保存し直す（`commands::downloader` の
//! 保存と同じ道）。棚は画質の違いを版の違いとして扱わないので、保存し直しても版は増えない
//! （保存状態が一つ増え、同じ画像の原寸が今の画像になる）。

use crate::library::{Library, LibraryId, Result};
use std::collections::BTreeMap;

/// 縮小版の画像を持つ作品。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReducedShelfWork {
    pub work: LibraryId,
    pub covers: usize,
    pub illustrations: usize,
}

impl Library {
    /// 今の版に縮小版の表紙・挿絵が残っている pixiv の作品（作品の ID 順）。
    ///
    /// 縮小版の URL（`novel-cover-master/`・`/img-master/`）で取得した画像のうち、原寸と確かめていない
    /// ものを数える。縮小版の URL の観測から引くので、棚全体でも一度の索引の読みで済む。
    pub fn reduced_pixiv_works(&self) -> Result<Vec<ReducedShelfWork>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare(
            "WITH reduced AS (
                 SELECT DISTINCT rendition_id FROM media_observations
                 WHERE instr(url,'novel-cover-master/')>0 OR instr(url,'/img-master/')>0
             )
             SELECT DISTINCT c.entity_id, sm.slot, sm.role FROM reduced x
             JOIN media_renditions r ON r.id=x.rendition_id AND r.quality<>'original'
             JOIN snapshot_media sm ON sm.rendition_id=r.id AND sm.role IN ('cover','illustration')
             JOIN entity_current c ON c.snapshot_id=sm.snapshot_id
             JOIN library_items li ON li.work_id=c.entity_id
             JOIN source_media m ON m.id=sm.media_id AND m.provider='pixiv'",
        )?;
        let mut works: BTreeMap<LibraryId, ReducedShelfWork> = BTreeMap::new();
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, String>(2)?))
        })?;
        for row in rows {
            let (work, role) = row?;
            let work = LibraryId::from_slice(&work)
                .map_err(|_| crate::library::Error::Integrity("作品の ID が不正です".into()))?;
            let entry = works.entry(work).or_insert(ReducedShelfWork {
                work,
                covers: 0,
                illustrations: 0,
            });
            if role == "cover" {
                entry.covers += 1;
            } else {
                entry.illustrations += 1;
            }
        }
        Ok(works.into_values().collect())
    }
}

#[cfg(test)]
#[path = "library_original_images_tests.rs"]
mod tests;
