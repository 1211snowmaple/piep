//! 反映中の同じ取得元の版の編集だけを、挿絵の不足の判定へ反映する。
use super::{BodyAssetReference, Error, Library, LibraryId, Result, SourceBody};
use regex::Regex;
use rusqlite::{params, OptionalExtension};
use std::{collections::BTreeSet, sync::LazyLock};

static REFERENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(?:pixivimage|uploadedimage):[^\]]+\]").unwrap());

// 明示した 1 ページ目と省略形を同じ指定として比べる。解釈できない指定は元の字句で残す。
fn key(token: &str) -> String {
    match SourceBody::PixivText(token.into())
        .asset_references()
        .ok()
        .as_deref()
    {
        Some([BodyAssetReference::PixivUploaded { image_id }]) => format!("uploaded:{image_id}"),
        Some(
            [BodyAssetReference::PixivIllustration {
                illustration_id,
                page,
            }],
        ) => format!("illustration:{illustration_id}:{page}"),
        _ => token.into(),
    }
}

impl Library {
    /// 原本の欠損は書き換えず、今読む編集でまだ表示する未解決の指定だけを返す。
    pub(crate) fn pixiv_references_left_by_active_edit(
        &self,
        work: LibraryId,
        snapshot: LibraryId,
        mut missing: Vec<String>,
    ) -> Result<Vec<String>> {
        if missing.is_empty() {
            return Ok(missing);
        }
        let revision = |snapshot: LibraryId| -> Result<Vec<u8>> {
            let conn = self.reader()?;
            conn.query_row("SELECT s.revision_id FROM revision_snapshots s JOIN works w ON w.id=s.entity_id JOIN external_identities x ON x.entity_id=w.id AND x.kind='work' AND x.provider='pixiv' WHERE s.entity_id=?1 AND s.id=?2 AND s.published=1", params![work.as_bytes().as_slice(), snapshot.as_bytes().as_slice()], |r|r.get(0)).optional()?.ok_or(Error::ResourceUnavailable)
        };
        let source_revision = revision(snapshot)?;
        let Some(active) = self.edit_state(work, self.id)?.active else {
            return Ok(missing);
        };
        let edit = self.edit(work, active)?;
        if revision(edit.base)? != source_revision {
            return Ok(missing);
        }
        let mut available = BTreeSet::new();
        for media in &edit.content.media {
            let file = self.file(media.file)?;
            let mut present = file.length > 0;
            for chunk in &file.chunks {
                present &= self.objects.has_object(chunk.id)?;
            }
            if present {
                available.insert(media.slot.as_str());
            }
        }
        let mut shown = BTreeSet::new();
        for block in &edit.content.blocks {
            let text = if block.kind == "image" {
                if block
                    .media_slot
                    .as_deref()
                    .is_some_and(|slot| available.contains(slot))
                {
                    continue;
                }
                block
                    .attrs
                    .get("sourceRef")
                    .and_then(serde_json::Value::as_str)
            } else {
                block.text.as_deref()
            };
            if let Some(text) = text {
                shown.extend(REFERENCE.find_iter(text).map(|m| key(m.as_str())));
            }
        }
        missing.retain(|reference| shown.contains(&key(reference)));
        Ok(missing)
    }
}

#[cfg(test)]
#[path = "pixiv_edit_gaps_tests.rs"]
mod tests;
