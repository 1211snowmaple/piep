//! 作者の書いたものが変わらないのに分かれた最後の改稿を、前の版へ畳む（一度きりの手入れ）。
//!
//! 本文の比較規則 1 は、FANBOX の本文に貼られた**別の投稿のカード**のいいね数なども比べていた。
//! そのため、リンク先のいいね数が増えただけで改稿として新しい版ができた。規則 2 はそれを除く
//! （body.rs）。規則 2 で前の版と同じになる版は、作者が何も変えていないので消して前の版に戻す。
//!
//! 畳むのは、作品の**今の版**が、その改稿のただ一つの版であるときだけ。途中の版や、補完で版が
//! 重なった改稿は触らない（後の版の比較の基準や補完を失わないため）。下書き・保存の途中の記録が
//! 指している版も触らない。読書位置・取得・確認の記録は前の版へ付け替える（本文は同じ）。
//!
//! 付け替えると記録の意味が変わるものがある版も触らない（`keeps_what_folding_would_lose`）。
//! - その版を基準に始めた取得（確定済みの入力の券が今の版を指す）
//! - 前の版と画像の枠・実体・画質・大きさのどれかが違う（原寸が取れたなど。同じバイト列でも
//!   画質の記録が違えば別の選択肢になる。畳むと今の版からも版の履歴からも選べなくなる）、
//!   前の版に無い添付のファイル
//! - 取得の読む形の記録があるのに、前の版の同じ作り方の対応表が中身まで同じでない（確定済みの
//!   入力の対応表と、付け替え先の対応表が食い違う）
//! - 送信台帳に、その版を名指しした要求（読書位置など）が残っている（同じ要求の再送が、消した
//!   版を指して断られる）。保存の結果（`capture`）は版ごとに必ずあり、券は確定で閉じていて同じ
//!   要求が再び来ないので数えない

use super::{integrity, Error, Library, LibraryId, Result};
use rusqlite::{params, Connection};

/// 公開済みの版と、その版に結んだ記録を書き換えさせない見張り。畳む間だけ外し、同じ定義で戻す。
/// 取得の完了と記録の一致を確かめる見張り（acquisition_completion_link など）は外さない。
const GUARDS: &[&str] = &[
    "published_snapshot_immutable",
    "snapshot_files_published_delete",
    "snapshot_media_published_delete",
    "snapshot_people_published_delete",
    "snapshot_series_published_delete",
    "snapshot_tags_published_delete",
    "reader_binding_header_delete",
    "reader_binding_header_update",
    "reader_binding_delete",
    "acquisition_reader_update",
    "acquisition_observations_immutable",
    "acquisition_header_immutable",
];

struct Candidate {
    entity: LibraryId,
    current: LibraryId,
    revision: Vec<u8>,
    previous: LibraryId,
}

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("版の識別子が不正です".into()))
}

impl Library {
    /// 棚を開いたあとに、移行が残した手入れを行う。失敗しても棚は開き、次に開いたときに
    /// やり直す（手入れは一つの書き込みの中で行うので、途中で落ちても何も変わらない）。
    pub(super) fn run_library_tasks(&self) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let pending: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM library_tasks WHERE name='fold_volatile_revisions')",
            [],
            |r| r.get(0),
        )?;
        if !pending {
            return Ok(());
        }
        let tx = conn.transaction()?;
        match self.fold_volatile_revisions(&tx) {
            Ok(folded) => {
                tx.execute(
                    "DELETE FROM library_tasks WHERE name='fold_volatile_revisions'",
                    [],
                )?;
                tx.commit()?;
                if folded > 0 {
                    log::info!("作者の書いたものが変わらない版を {folded} 件畳みました");
                }
            }
            Err(error) => {
                log::warn!("変わらない版を畳めませんでした。次に開いたときにやり直します: {error}");
            }
        }
        Ok(())
    }

    pub(super) fn fold_volatile_revisions(&self, tx: &Connection) -> Result<usize> {
        let candidates = self.volatile_revisions(tx)?;
        if candidates.is_empty() {
            return Ok(0);
        }
        let guards = GUARDS
            .iter()
            .map(|name| {
                tx.query_row(
                    "SELECT sql FROM sqlite_master WHERE type='trigger' AND name=?1",
                    [name],
                    |r| r.get::<_, String>(0),
                )
                .map_err(Error::from)
            })
            .collect::<Result<Vec<_>>>()?;
        for name in GUARDS {
            tx.execute_batch(&format!("DROP TRIGGER {name};"))?;
        }
        let now = chrono::Utc::now().timestamp_millis();
        for candidate in &candidates {
            fold(tx, candidate, now)?;
        }
        // 付け替えた確認の記録で、前の版の構成の指紋が変わる。内容の指紋は変わらない
        // （変われば止まる）。読書用の対応表と取得の読む形の記録も一緒に移す。
        tx.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS unclassified_snapshots(id BLOB PRIMARY KEY);
             DELETE FROM temp.unclassified_snapshots;",
        )?;
        for candidate in &candidates {
            tx.execute(
                "INSERT OR IGNORE INTO temp.unclassified_snapshots(id) VALUES(?1)",
                [candidate.previous.as_bytes().as_slice()],
            )?;
        }
        integrity::refingerprint_unclassified(tx)?;
        tx.execute_batch("DROP TABLE temp.unclassified_snapshots;")?;
        for sql in &guards {
            tx.execute_batch(&format!("{sql};"))?;
        }
        // 一覧の行は今の版を指しているので、確かめる前に作り直す。
        for candidate in &candidates {
            self.refresh_work_list(tx, candidate.entity)?;
        }
        let broken: Option<String> = tx.query_row(
            "SELECT group_concat(\"table\" || '→' || parent, ', ') FROM pragma_foreign_key_check",
            [],
            |r| r.get(0),
        )?;
        if let Some(broken) = broken {
            return Err(Error::Integrity(format!(
                "版を畳んだあとに参照の切れた行があります: {broken}"
            )));
        }
        Ok(candidates.len())
    }

    /// 今の版がその改稿のただ一つの版で、今の比較規則では前の版と内容が同じ作品。
    fn volatile_revisions(&self, tx: &Connection) -> Result<Vec<Candidate>> {
        let rows = tx
            .prepare(
                "SELECT c.entity_id,c.snapshot_id,s.revision_id,
                        (SELECT p.id FROM revision_snapshots p WHERE p.entity_id=s.entity_id
                            AND p.published=1 AND p.sequence<s.sequence ORDER BY p.sequence DESC LIMIT 1)
                   FROM entity_current c JOIN works w ON w.id=c.entity_id
                   JOIN revision_snapshots s ON s.id=c.snapshot_id
                  WHERE (SELECT count(*) FROM revision_snapshots o WHERE o.revision_id=s.revision_id)=1
                    AND EXISTS(SELECT 1 FROM revision_snapshots p WHERE p.entity_id=s.entity_id
                            AND p.published=1 AND p.sequence<s.sequence)
                    AND NOT EXISTS(SELECT 1 FROM edit_revisions e WHERE e.base_snapshot_id=s.id)
                    AND NOT EXISTS(SELECT 1 FROM incomplete_save_attempts i WHERE i.snapshot_id=s.id)",
            )?
            .query_map([], |r| {
                Ok((
                    r.get::<_, Vec<u8>>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, Vec<u8>>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut found = Vec::new();
        for (entity, current, revision, previous) in rows {
            let (entity, current, previous) = (id(entity)?, id(current)?, id(previous)?);
            // 本文以外が違えば、本文を読むまでもなく別の版。
            if integrity::source_content_with_body(tx, current, None)?
                != integrity::source_content_with_body(tx, previous, None)?
            {
                continue;
            }
            let body = |snapshot| self.body_semantic_now(tx, entity, snapshot);
            let (now, before) = (body(current)?, body(previous)?);
            if integrity::source_content_with_body(tx, current, now.as_ref().map(|f| &f.0[..]))?
                == integrity::source_content_with_body(
                    tx,
                    previous,
                    before.as_ref().map(|f| &f.0[..]),
                )?
                // 守りの確かめは取得・送信の台帳を引くので、内容が同じと分かった組にだけ行う
                // （本文が違う、ふつうの改稿の作品にまで走らせない）。
                && !keeps_what_folding_would_lose(tx, entity, current, previous)?
            {
                found.push(Candidate {
                    entity,
                    current,
                    revision,
                    previous,
                });
            }
        }
        Ok(found)
    }
}

/// 畳むと失うもの・食い違うものを、今の版が持っているか。
pub(super) fn keeps_what_folding_would_lose(
    tx: &Connection,
    entity: LibraryId,
    current: LibraryId,
    previous: LibraryId,
) -> Result<bool> {
    Ok(tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM acquisition_attempts WHERE base_snapshot_id=?1)
             OR EXISTS(SELECT m.slot,m.role,m.media_id,r.file_id,r.mime_type,r.quality,r.width,r.height
                        FROM snapshot_media m LEFT JOIN media_renditions r ON r.id=m.rendition_id
                        WHERE m.snapshot_id=?1
                       EXCEPT SELECT m.slot,m.role,m.media_id,r.file_id,r.mime_type,r.quality,r.width,r.height
                        FROM snapshot_media m LEFT JOIN media_renditions r ON r.id=m.rendition_id
                        WHERE m.snapshot_id=?2)
             OR EXISTS(SELECT file_id FROM snapshot_files WHERE snapshot_id=?1 AND role<>'raw'
                       EXCEPT SELECT file_id FROM snapshot_files WHERE snapshot_id=?2 AND role<>'raw')
             OR EXISTS(SELECT 1 FROM acquisition_reader_outputs o WHERE o.snapshot_id=?1
                        AND (NOT EXISTS(SELECT 1 FROM reader_binding_sets b
                                         WHERE b.snapshot_id=?2 AND b.recipe=o.recipe)
                             OR EXISTS(SELECT reference_json,media_slot,file_slot,unavailable FROM reader_bindings
                                        WHERE snapshot_id=?1 AND recipe=o.recipe
                                       EXCEPT SELECT reference_json,media_slot,file_slot,unavailable FROM reader_bindings
                                        WHERE snapshot_id=?2 AND recipe=o.recipe)
                             OR EXISTS(SELECT reference_json,media_slot,file_slot,unavailable FROM reader_bindings
                                        WHERE snapshot_id=?2 AND recipe=o.recipe
                                       EXCEPT SELECT reference_json,media_slot,file_slot,unavailable FROM reader_bindings
                                        WHERE snapshot_id=?1 AND recipe=o.recipe)))
             OR EXISTS(SELECT 1 FROM outbox WHERE entity_id=?3 AND operation NOT IN ('capture','fold_revision')
                        AND instr(payload_json,?4)>0)",
        params![
            current.as_bytes().as_slice(),
            previous.as_bytes().as_slice(),
            entity.as_bytes().as_slice(),
            current.to_string()
        ],
        |r| r.get(0),
    )?)
}

/// 一つの版を消して、作品を前の版へ戻す。
///
/// その版を作った取得と、その版に結んだ確認の記録は消さず、前の版の記録として付け替える
/// （取得の世代は増えるだけで戻せないので、取得を消すと世代の記録と食い違う）。付け替えた取得は
/// もう改稿を作っていないので、そのように記録し直す。前の版の構成の指紋は、あとでまとめて
/// 計算し直す（`refingerprint_unclassified`）。
fn fold(tx: &Connection, candidate: &Candidate, now: i64) -> Result<()> {
    let entity = candidate.entity.as_bytes().as_slice();
    let current = candidate.current.as_bytes().as_slice();
    let previous = candidate.previous.as_bytes().as_slice();
    let moved = tx.execute(
        "UPDATE entity_current SET snapshot_id=?3,generation=generation+1 WHERE entity_id=?1 AND snapshot_id=?2",
        params![entity, current, previous],
    )?;
    if moved != 1 {
        return Err(Error::Integrity("畳む版が今の版ではありません".into()));
    }
    for sql in [
        "UPDATE reading_positions SET snapshot_id=?3 WHERE work_id=?1 AND snapshot_id=?2",
        // 読む形の記録・確認の記録・取得の順に付け替える（取得の完了の見張りが、記録と取得の
        // 指す版が一致していることを確かめる）。
        "UPDATE acquisition_reader_outputs SET snapshot_id=?3 WHERE entity_id=?1 AND snapshot_id=?2",
        "UPDATE acquisition_observations SET result_snapshot_id=?3,created_revision=0 WHERE entity_id=?1 AND result_snapshot_id=?2",
        "UPDATE acquisition_attempts SET result_snapshot_id=?3 WHERE entity_id=?1 AND result_snapshot_id=?2",
        "UPDATE acquisition_attempts SET base_snapshot_id=?3 WHERE entity_id=?1 AND base_snapshot_id=?2",
        "INSERT INTO snapshot_observations(entity_id,snapshot_id,observation_id)
           SELECT entity_id,?3,observation_id FROM snapshot_observations o WHERE o.entity_id=?1 AND o.snapshot_id=?2
              AND NOT EXISTS(SELECT 1 FROM snapshot_observations p WHERE p.snapshot_id=?3 AND p.observation_id=o.observation_id)",
    ] {
        tx.execute(sql, params![entity, current, previous])?;
    }
    for sql in [
        "DELETE FROM snapshot_observations WHERE snapshot_id=?1",
        "DELETE FROM reader_bindings WHERE snapshot_id=?1",
        "DELETE FROM reader_binding_sets WHERE snapshot_id=?1",
        "DELETE FROM snapshot_files WHERE snapshot_id=?1",
        "DELETE FROM snapshot_media WHERE snapshot_id=?1",
        "DELETE FROM snapshot_people WHERE snapshot_id=?1",
        "DELETE FROM snapshot_series WHERE snapshot_id=?1",
        "DELETE FROM snapshot_tags WHERE snapshot_id=?1",
        "DELETE FROM asset_repair_state WHERE snapshot_id=?1",
        "DELETE FROM revision_snapshots WHERE id=?1",
    ] {
        tx.execute(sql, [current])?;
    }
    tx.execute(
        "DELETE FROM source_revisions WHERE id=?1",
        [candidate.revision.as_slice()],
    )?;
    tx.execute(
        "INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'fold_revision',?3,?4)",
        params![
            LibraryId::now_v7().as_bytes().as_slice(),
            entity,
            serde_json::json!({"removed": candidate.current, "restored": candidate.previous}).to_string(),
            now
        ],
    )?;
    Ok(())
}

#[cfg(test)]
#[path = "revision_folds_tests.rs"]
mod tests;
