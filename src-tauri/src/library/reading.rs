use super::{Error, Library, LibraryId, Result, SourceBody};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};

/// v1 の位置は、固定した原本の UTF-8 境界か、固定した編集ブロックに属する。
/// 画面のページ番号や端末の幅から、別の版の位置を推測しない。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadingAnchor {
    TextByte {
        offset: u64,
    },
    BodyBlock {
        index: u64,
        offset: u64,
    },
    EditBlock {
        block: LibraryId,
        offset: u64,
    },
    /// 旧系統の読書画面が記録した位置。本文のページ区切り（pixiv の `[newpage]`、編集の
    /// `page_break`）で数えたページ（0 から）と、そのページの中のスクロールの割合
    /// （百万分率）。割合は表示の幅で変わるので、文字の位置へ推測して直さない。
    /// 読書画面はこの位置から開き、次に読み進めたときに文字の位置で書き直す。
    Page {
        page: u64,
        ratio_ppm: u32,
    },
}

/// 割合の上限（百万分率で 1.0）。
pub const PAGE_RATIO_SCALE: u32 = 1_000_000;

fn page_within(pages: usize, page: u64, ratio_ppm: u32) -> Result<()> {
    let page = usize::try_from(page).map_err(|_| invalid_anchor())?;
    if page < pages && ratio_ppm <= PAGE_RATIO_SCALE {
        Ok(())
    } else {
        Err(invalid_anchor())
    }
}

/// pixiv の本文のページ数。旧系統の読書画面と同じく `[newpage]` だけで区切る。
fn pixiv_pages(text: &str) -> usize {
    text.matches("[newpage]").count() + 1
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReadingLocation {
    Source {
        snapshot: LibraryId,
        anchor: ReadingAnchor,
    },
    Edit {
        edit: LibraryId,
        anchor: ReadingAnchor,
    },
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadingPosition {
    pub location: Option<ReadingLocation>,
    pub finished: bool,
    pub device: Option<LibraryId>,
    pub updated_at: Option<i64>,
    pub generation: i64,
}

pub struct ReadingUpdate {
    pub request: LibraryId,
    pub work: LibraryId,
    pub user: LibraryId,
    /// 呼び出し側が認証済み端末から決める。端末時刻は更新順に使わない。
    pub device: LibraryId,
    pub expected: i64,
    /// None は明示的な消去。世代と再送記録を消さずに残す。
    pub location: Option<ReadingLocation>,
    pub finished: bool,
}

fn invalid_anchor() -> Error {
    Error::Integrity("読書位置が指定した本文の範囲にありません".into())
}

fn text_boundary(text: &str, offset: u64) -> Result<()> {
    let offset = usize::try_from(offset).map_err(|_| invalid_anchor())?;
    if text.is_char_boundary(offset) {
        Ok(())
    } else {
        Err(invalid_anchor())
    }
}

fn check_source_anchor(body: &SourceBody, anchor: &ReadingAnchor) -> Result<()> {
    match (body, anchor) {
        (
            SourceBody::PlainText(text) | SourceBody::PixivText(text),
            ReadingAnchor::TextByte { offset },
        ) => text_boundary(text, *offset),
        (SourceBody::FanboxBody(body), ReadingAnchor::TextByte { offset }) => text_boundary(
            body.get("text")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(invalid_anchor)?,
            *offset,
        ),
        (SourceBody::FanboxBody(body), ReadingAnchor::BodyBlock { index, offset }) => {
            let index = usize::try_from(*index).map_err(|_| invalid_anchor())?;
            let block = body
                .get("blocks")
                .and_then(serde_json::Value::as_array)
                .and_then(|b| b.get(index))
                .filter(|b| b.is_object())
                .ok_or_else(invalid_anchor)?;
            text_boundary(
                block
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(""),
                *offset,
            )
        }
        (SourceBody::PixivText(text), ReadingAnchor::Page { page, ratio_ppm }) => {
            page_within(pixiv_pages(text), *page, *ratio_ppm)
        }
        // pixiv 以外の本文は旧系統でも一続きの一ページだった。
        (
            SourceBody::PlainText(_) | SourceBody::FanboxBody(_),
            ReadingAnchor::Page { page, ratio_ppm },
        ) => page_within(1, *page, *ratio_ppm),
        _ => Err(invalid_anchor()),
    }
}

/// 編集のページ数。`page_break` のブロックで区切る。
fn edit_pages(breaks: usize) -> usize {
    breaks + 1
}

impl Library {
    pub(super) fn check_reading_location(
        &self,
        work: LibraryId,
        location: &ReadingLocation,
    ) -> Result<()> {
        match location {
            ReadingLocation::Source { snapshot, anchor } => {
                let body = self
                    .source_body(work, *snapshot)?
                    .ok_or_else(invalid_anchor)?;
                check_source_anchor(&body, anchor)
            }
            ReadingLocation::Edit { edit, anchor } => {
                let content = self.edit(work, *edit)?.content;
                match anchor {
                    ReadingAnchor::EditBlock { block, offset } => {
                        let block = content
                            .blocks
                            .iter()
                            .find(|b| b.id == *block)
                            .ok_or_else(invalid_anchor)?;
                        text_boundary(block.text.as_deref().unwrap_or(""), *offset)
                    }
                    ReadingAnchor::Page { page, ratio_ppm } => {
                        let breaks = content
                            .blocks
                            .iter()
                            .filter(|b| b.kind == "page_break")
                            .count();
                        page_within(edit_pages(breaks), *page, *ratio_ppm)
                    }
                    _ => Err(invalid_anchor()),
                }
            }
        }
    }

    pub fn reading_position(&self, work: LibraryId, user: LibraryId) -> Result<ReadingPosition> {
        let conn = self.reader()?;
        conn.query_row(
            "SELECT id FROM works WHERE id=?1",
            [work.as_bytes().as_slice()],
            |_| Ok(()),
        )?;
        let raw = conn.query_row(
            "SELECT snapshot_id,edit_id,anchor_json,finished,device_id,updated_at,generation FROM reading_positions WHERE work_id=?1 AND user_id=?2",
            params![work.as_bytes().as_slice(),user.as_bytes().as_slice()], |r|Ok((
                r.get::<_,Option<Vec<u8>>>(0)?,r.get::<_,Option<Vec<u8>>>(1)?,r.get::<_,Option<String>>(2)?,r.get(3)?,r.get::<_,Vec<u8>>(4)?,r.get(5)?,r.get(6)?
            )),
        ).optional()?;
        let Some((snapshot, edit, anchor, finished, device, updated_at, generation)) = raw else {
            return Ok(ReadingPosition::default());
        };
        let id = |b: Vec<u8>| {
            LibraryId::from_slice(&b)
                .map_err(|_| Error::Integrity("読書位置の識別子が不正です".into()))
        };
        let location = match (snapshot, edit, anchor) {
            (Some(s), None, Some(a)) => Some(ReadingLocation::Source {
                snapshot: id(s)?,
                anchor: serde_json::from_str(&a)?,
            }),
            (None, Some(e), Some(a)) => Some(ReadingLocation::Edit {
                edit: id(e)?,
                anchor: serde_json::from_str(&a)?,
            }),
            (None, None, None) => None,
            _ => return Err(invalid_anchor()),
        };
        Ok(ReadingPosition {
            location,
            finished,
            device: Some(id(device)?),
            updated_at: Some(updated_at),
            generation,
        })
    }

    pub fn set_reading_position(&self, update: ReadingUpdate) -> Result<ReadingPosition> {
        let ReadingUpdate {
            request,
            work,
            user,
            device,
            expected,
            location,
            finished,
        } = update;
        if expected < 0 || (location.is_none() && finished) {
            return Err(Error::Integrity("読書位置の更新が不正です".into()));
        }
        // 本文の復元・位置検証中に書き込みの錠を持たない。参照先の版は不変。
        if let Some(location) = &location {
            self.check_reading_location(work, location)?;
        }
        let payload = serde_json::json!({"work":work,"user":user,"device":device,"expected":expected,"location":location,"finished":finished});
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let receipt: Option<(String, String)> = tx
            .query_row(
                "SELECT operation,payload_json FROM outbox WHERE request_id=?1",
                [request.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((operation, stored)) = receipt {
            let stored: serde_json::Value = serde_json::from_str(&stored)?;
            if operation != "reading_position" || stored["request"] != payload {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる更新に使われました".into(),
                ));
            }
            return Ok(serde_json::from_value(stored["result"].clone())?);
        }
        let generation: i64 = tx
            .query_row(
                "SELECT generation FROM reading_positions WHERE work_id=?1 AND user_id=?2",
                params![work.as_bytes().as_slice(), user.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(0);
        if generation != expected {
            return Err(Error::Integrity(format!(
                "読書位置が競合しました。現在の世代は {generation} です"
            )));
        }
        let next = generation
            .checked_add(1)
            .ok_or_else(|| Error::Integrity("更新世代が上限を超えました".into()))?;
        let now = chrono::Utc::now().timestamp_millis();
        let (snapshot, edit, anchor) = match &location {
            Some(ReadingLocation::Source { snapshot, anchor }) => (
                Some(snapshot.as_bytes().to_vec()),
                None,
                Some(serde_json::to_string(anchor)?),
            ),
            Some(ReadingLocation::Edit { edit, anchor }) => (
                None,
                Some(edit.as_bytes().to_vec()),
                Some(serde_json::to_string(anchor)?),
            ),
            None => (None, None, None),
        };
        tx.execute("INSERT INTO reading_positions(work_id,user_id,snapshot_id,edit_id,anchor_json,cleared,finished,device_id,updated_at,generation) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)
            ON CONFLICT(work_id,user_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,edit_id=excluded.edit_id,anchor_json=excluded.anchor_json,cleared=excluded.cleared,finished=excluded.finished,device_id=excluded.device_id,updated_at=excluded.updated_at,generation=excluded.generation",
            params![work.as_bytes().as_slice(),user.as_bytes().as_slice(),snapshot,edit,anchor,location.is_none(),finished,device.as_bytes().as_slice(),now,next])?;
        let result = ReadingPosition {
            location,
            finished,
            device: Some(device),
            updated_at: Some(now),
            generation: next,
        };
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'reading_position',?3,?4)",params![request.as_bytes().as_slice(),work.as_bytes().as_slice(),serde_json::json!({"request":payload,"result":result}).to_string(),now])?;
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        tx.commit()?;
        Ok(result)
    }

    pub(super) fn verify_reading_positions(&self, conn: &rusqlite::Connection) -> Result<()> {
        let mut stmt = conn.prepare(
            "SELECT work_id,snapshot_id,edit_id,anchor_json FROM reading_positions WHERE cleared=0",
        )?;
        let mut rows = stmt.query([])?;
        let id = |b: Vec<u8>| LibraryId::from_slice(&b).map_err(|_| invalid_anchor());
        while let Some(row) = rows.next()? {
            let work = id(row.get(0)?)?;
            let snapshot: Option<Vec<u8>> = row.get(1)?;
            let edit: Option<Vec<u8>> = row.get(2)?;
            let anchor: ReadingAnchor = serde_json::from_str(&row.get::<_, String>(3)?)?;
            match (snapshot, edit) {
                (Some(snapshot), None) => {
                    let body = self
                        .source_body_from_conn(conn, work, id(snapshot)?)?
                        .ok_or_else(invalid_anchor)?;
                    check_source_anchor(&body, &anchor)?;
                }
                (None, Some(edit)) => match anchor {
                    ReadingAnchor::EditBlock { block, offset } => {
                        let text:Option<String> = conn.query_row("SELECT b.text FROM edit_blocks b JOIN edit_revisions e ON e.id=b.edit_id WHERE e.work_id=?1 AND e.id=?2 AND e.sealed=1 AND b.block_id=?3",params![work.as_bytes().as_slice(),edit,block.as_bytes().as_slice()],|r|r.get(0))?;
                        text_boundary(text.as_deref().unwrap_or(""), offset)?;
                    }
                    ReadingAnchor::Page { page, ratio_ppm } => {
                        let breaks: i64 = conn.query_row("SELECT count(*) FROM edit_blocks b JOIN edit_revisions e ON e.id=b.edit_id WHERE e.work_id=?1 AND e.id=?2 AND e.sealed=1 AND b.kind='page_break'",params![work.as_bytes().as_slice(),edit],|r|r.get(0))?;
                        let breaks = usize::try_from(breaks).map_err(|_| invalid_anchor())?;
                        page_within(edit_pages(breaks), page, ratio_ppm)?;
                    }
                    _ => return Err(invalid_anchor()),
                },
                _ => return Err(invalid_anchor()),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EditBlock, EditContent, EditRequest, EntityKind};
    use std::{collections::BTreeMap, fs, path::PathBuf};
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("piep-reading-{}", LibraryId::now_v7())))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn capture(key: &str, body: SourceBody) -> Capture {
        Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: b"{}".to_vec().into(),
            metadata: BTreeMap::new(),
            field_states: BTreeMap::new(),
            body: Some(body),
            media: vec![],
            attachments: vec![],
        }
    }

    #[test]
    fn reading_updates_are_version_pinned_retryable_and_clear_without_losing_the_generation() {
        let temp = Temp::new();
        let library = Library::create(&temp.0.join("source")).unwrap();
        let first = library.capture(capture("one", "あいう".into())).unwrap();
        let request = LibraryId::now_v7();
        let user = LibraryId::now_v7();
        let device = LibraryId::now_v7();
        let location = ReadingLocation::Source {
            snapshot: first.snapshot_id,
            anchor: ReadingAnchor::TextByte { offset: 3 },
        };
        let update = |request, device, expected, location| ReadingUpdate {
            request,
            work: first.entity_id,
            user,
            device,
            expected,
            location,
            finished: false,
        };
        assert_eq!(
            library.reading_position(first.entity_id, user).unwrap(),
            ReadingPosition::default()
        );
        let position = library
            .set_reading_position(update(request, device, 0, Some(location.clone())))
            .unwrap();
        let changed = library
            .capture(capture("one", "別の短い本文".into()))
            .unwrap();
        assert_ne!(first.snapshot_id, changed.snapshot_id);
        assert_eq!(
            library.reading_position(first.entity_id, user).unwrap(),
            position
        );
        assert!(library
            .set_reading_position(update(
                LibraryId::now_v7(),
                LibraryId::now_v7(),
                0,
                Some(location.clone())
            ))
            .is_err());
        let clear = library
            .set_reading_position(update(LibraryId::now_v7(), device, 1, None))
            .unwrap();
        assert_eq!(clear.generation, 2);
        assert!(clear.location.is_none());
        assert_eq!(
            library
                .set_reading_position(update(request, device, 0, Some(location.clone())))
                .unwrap(),
            position
        );
        assert_eq!(
            library.reading_position(first.entity_id, user).unwrap(),
            clear
        );
        assert!(library
            .set_reading_position(update(request, device, 0, None))
            .is_err());
        assert_eq!(
            library
                .reading_position(first.entity_id, LibraryId::now_v7())
                .unwrap()
                .generation,
            0
        );
        library.verify().unwrap();
        let backup = temp.0.join("backup");
        library.backup(&backup).unwrap();
        drop(library);
        fs::rename(temp.0.join("source"), temp.0.join("unavailable-source")).unwrap();
        let copy = Library::open(&backup).unwrap();
        assert_eq!(copy.reading_position(first.entity_id, user).unwrap(), clear);
        assert_eq!(
            copy.conn
                .lock()
                .unwrap()
                .query_row(
                    "SELECT count(*) FROM outbox WHERE operation='reading_position'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            2
        );
    }

    #[test]
    fn anchors_reject_other_works_invalid_utf8_boundaries_and_missing_body_blocks() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let work = library.capture(capture("one", "あいう".into())).unwrap();
        let other = library.capture(capture("two", "other".into())).unwrap();
        let mut rich = capture(
            "rich",
            SourceBody::FanboxBody(
                serde_json::json!({"blocks":[{"type":"p","text":"文章"},{"type":"image","imageId":"i"}],"imageMap":{}}),
            ),
        );
        rich.provider = "fanbox".into();
        let rich = library.capture(rich).unwrap();
        let set = |work, snapshot, anchor| {
            library.set_reading_position(ReadingUpdate {
                request: LibraryId::now_v7(),
                work,
                user: library.id,
                device: library.id,
                expected: 0,
                location: Some(ReadingLocation::Source { snapshot, anchor }),
                finished: false,
            })
        };
        assert!(set(
            work.entity_id,
            other.snapshot_id,
            ReadingAnchor::TextByte { offset: 0 }
        )
        .is_err());
        for offset in [1, 10, u64::MAX] {
            assert!(set(
                work.entity_id,
                work.snapshot_id,
                ReadingAnchor::TextByte { offset }
            )
            .is_err());
        }
        assert!(set(
            work.entity_id,
            work.snapshot_id,
            ReadingAnchor::BodyBlock {
                index: 0,
                offset: 0
            }
        )
        .is_err());
        assert!(set(
            rich.entity_id,
            rich.snapshot_id,
            ReadingAnchor::BodyBlock {
                index: 2,
                offset: 0
            }
        )
        .is_err());
        assert!(set(
            rich.entity_id,
            rich.snapshot_id,
            ReadingAnchor::BodyBlock {
                index: 1,
                offset: 1
            }
        )
        .is_err());
        let saved = set(
            rich.entity_id,
            rich.snapshot_id,
            ReadingAnchor::BodyBlock {
                index: 1,
                offset: 0,
            },
        )
        .unwrap();
        assert_eq!(saved.generation, 1);
        assert_eq!(
            library
                .reading_position(work.entity_id, library.id)
                .unwrap()
                .generation,
            0
        );
        library.verify().unwrap();
        library
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE reading_positions SET anchor_json=?1",
                [serde_json::to_string(&ReadingAnchor::BodyBlock {
                    index: 100,
                    offset: 0,
                })
                .unwrap()],
            )
            .unwrap();
        assert!(library.verify().is_err());
    }

    #[test]
    fn page_positions_follow_the_text_page_breaks_and_stay_within_the_page() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let paged = library
            .capture(capture(
                "paged",
                SourceBody::PixivText("前[newpage]後".into()),
            ))
            .unwrap();
        let plain = library.capture(capture("plain", "一続き".into())).unwrap();
        let set = |saved: &crate::library::CaptureResult, page, ratio_ppm| {
            library.set_reading_position(ReadingUpdate {
                request: LibraryId::now_v7(),
                work: saved.entity_id,
                user: library.id,
                device: library.id,
                expected: library
                    .reading_position(saved.entity_id, library.id)
                    .unwrap()
                    .generation,
                location: Some(ReadingLocation::Source {
                    snapshot: saved.snapshot_id,
                    anchor: ReadingAnchor::Page { page, ratio_ppm },
                }),
                finished: false,
            })
        };
        assert!(set(&paged, 1, PAGE_RATIO_SCALE).is_ok());
        assert!(set(&paged, 2, 0).is_err());
        assert!(set(&paged, 0, PAGE_RATIO_SCALE + 1).is_err());
        assert!(set(&plain, 0, 500_000).is_ok());
        assert!(set(&plain, 1, 0).is_err());
        library.verify().unwrap();
    }

    #[test]
    fn reading_an_edit_uses_its_own_stable_blocks_and_never_the_current_source() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let work = library.capture(capture("one", "原本".into())).unwrap();
        let block = LibraryId::now_v7();
        let document = EditContent {
            format: "piep-edit-v1".into(),
            title: None,
            blocks: vec![EditBlock {
                id: block,
                ordinal: 0,
                kind: "paragraph".into(),
                text: Some("編集本文".into()),
                attrs: serde_json::json!({}),
                media_slot: None,
            }],
            media: vec![],
            provenance: None,
        };
        let edit = library
            .save_edit(EditRequest {
                request_id: LibraryId::now_v7(),
                work: work.entity_id,
                user: library.id,
                base: work.snapshot_id,
                parent: None,
                expected: 0,
                content: &document,
                activate: true,
            })
            .unwrap();
        let update = |block, offset, expected| ReadingUpdate {
            request: LibraryId::now_v7(),
            work: work.entity_id,
            user: library.id,
            device: library.id,
            expected,
            location: Some(ReadingLocation::Edit {
                edit,
                anchor: ReadingAnchor::EditBlock { block, offset },
            }),
            finished: false,
        };
        assert!(library
            .set_reading_position(update(LibraryId::now_v7(), 0, 0))
            .is_err());
        assert!(library.set_reading_position(update(block, 1, 0)).is_err());
        let saved = library.set_reading_position(update(block, 6, 0)).unwrap();
        library.capture(capture("one", "改稿後".into())).unwrap();
        assert_eq!(
            library
                .reading_position(work.entity_id, library.id)
                .unwrap(),
            saved
        );
        library.verify().unwrap();
        let other = library.capture(capture("other", "他作品".into())).unwrap();
        let conn = library.conn.lock().unwrap();
        // FK でも、別作品の編集を読む位置として登録できない。
        assert!(conn
            .execute(
                "UPDATE reading_positions SET work_id=?1",
                [other.entity_id.as_bytes().as_slice()]
            )
            .is_err());
        // 同じ作品でも、原本用と編集用のアンカーは混在できない。
        assert!(conn
            .execute(
                "UPDATE reading_positions SET snapshot_id=?1,edit_id=NULL",
                [work.snapshot_id.as_bytes().as_slice()]
            )
            .is_ok());
        // 本文版に編集用のアンカーを付け替えた改変は検証で検出する。
        drop(conn);
        assert!(library.verify().is_err());
    }
}
