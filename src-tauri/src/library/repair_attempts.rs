//! 作品の欠けを取り直した記録。取得元によらず、作品 1 件・今の版につき 1 行。
//!
//! - 欠けの状態は指紋（`gap_fingerprint`）で表す。期待した項目（FANBOX は応答が指す画像・添付の
//!   URL、pixiv は見られなかった挿絵の参照と伏せられた作品情報）と、その版が持つ画像・添付から作る。
//! - `attempts` は、同じ版・同じ指紋のまま欠けが残った保存の回数。0 は揃った。版か指紋が変われば
//!   1 から数え直す。[`MAX_REPAIR_ATTEMPTS`] に届いたら、その状態ではもう取りに行かない。
//! - 行は今の版だけを指す。新しい版を保存したら前の版の行は消える（`forget_other_versions`）。
//! - 取得の要求と結んだ行（FANBOX）は `request_id` を持ち、取得の入力の観測と突き合わせる
//!   （`asset_repairs::verify_asset_repairs`）。pixiv の補完は保存ジョブの外で数えるので持たない。
use super::{Error, Fingerprint, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension};

#[cfg(test)]
#[path = "repair_attempts_tests.rs"]
mod tests;

/// 同じ版・同じ欠けで取りに行く回数の上限。欠けたまま確定した最初の保存も 1 回に数える。
pub(crate) const MAX_REPAIR_ATTEMPTS: u32 = 2;

/// 欠けの指紋。`expected` は取得元が持つと示した項目、残りはその版が持つ画像・添付。
/// FANBOX の取得の入力に残した観測から同じ値を作り直せるよう、作り方は変えない。
pub(super) fn gap_fingerprint(
    conn: &Connection,
    snapshot: LibraryId,
    expected: &[String],
) -> Result<Fingerprint> {
    let mut expected = expected.to_vec();
    expected.sort();
    expected.dedup();
    let mut stmt=conn.prepare("SELECT 'media',m.slot,r.file_id FROM snapshot_media m LEFT JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 UNION ALL SELECT 'file',slot,file_id FROM snapshot_files WHERE snapshot_id=?1 ORDER BY 1,2")?;
    let held = stmt
        .query_map([snapshot.as_bytes().as_slice()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<Vec<u8>>>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Fingerprint::of(&serde_json::to_vec(&(
        "shelf-assets-v1",
        expected,
        held,
    ))?))
}

/// 一回の試みの結果を書く。`complete` なら 0、欠けたままなら同じ版・同じ指紋の続きとして数える。
/// 書いた回数を返す。
pub(super) fn record(
    conn: &Connection,
    work: LibraryId,
    snapshot: LibraryId,
    fingerprint: Fingerprint,
    complete: bool,
    request: Option<LibraryId>,
    at: i64,
) -> Result<u32> {
    let attempts = if complete {
        0
    } else {
        attempts(conn, work, snapshot, fingerprint)?.saturating_add(1)
    };
    write(conn, work, snapshot, fingerprint, attempts, request, at)?;
    Ok(attempts)
}

fn write(
    conn: &Connection,
    work: LibraryId,
    snapshot: LibraryId,
    fingerprint: Fingerprint,
    attempts: u32,
    request: Option<LibraryId>,
    at: i64,
) -> Result<()> {
    conn.execute(
        "INSERT INTO repair_attempts(work_id,snapshot_id,gap_fingerprint,attempts,request_id,attempted_at)
         VALUES(?1,?2,?3,?4,?5,?6)
         ON CONFLICT(work_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,
           gap_fingerprint=excluded.gap_fingerprint,attempts=excluded.attempts,
           request_id=excluded.request_id,attempted_at=excluded.attempted_at",
        params![
            work.as_bytes().as_slice(),
            snapshot.as_bytes().as_slice(),
            fingerprint.0.as_slice(),
            attempts,
            request.as_ref().map(|r| r.as_bytes().to_vec()),
            at
        ],
    )?;
    Ok(())
}

/// 保存の外で数える取り直し（pixiv の補完）を一回足す。欠けた版を作った保存は記録の外にあるので、
/// その保存を 1 回目に数える。数えた後の回数を返す。
pub(super) fn record_retry(
    conn: &Connection,
    work: LibraryId,
    snapshot: LibraryId,
    fingerprint: Fingerprint,
    at: i64,
) -> Result<u32> {
    let attempts = attempts(conn, work, snapshot, fingerprint)?
        .max(1)
        .saturating_add(1);
    write(conn, work, snapshot, fingerprint, attempts, None, at)?;
    Ok(attempts)
}

/// その版・その指紋で、欠けたまま終わった試みの回数。版か指紋が違えば 0。
pub(super) fn attempts(
    conn: &Connection,
    work: LibraryId,
    snapshot: LibraryId,
    fingerprint: Fingerprint,
) -> Result<u32> {
    Ok(conn
        .query_row(
            "SELECT attempts FROM repair_attempts WHERE work_id=?1 AND snapshot_id=?2 AND gap_fingerprint=?3",
            params![
                work.as_bytes().as_slice(),
                snapshot.as_bytes().as_slice(),
                fingerprint.0.as_slice()
            ],
            |r| r.get::<_, u32>(0),
        )
        .optional()?
        .unwrap_or(0))
}

/// 新しい版を今の版にしたとき、前の版を指す行を消す。記録は今の版でしか効かない。
pub(super) fn forget_other_versions(
    conn: &Connection,
    work: LibraryId,
    current: LibraryId,
) -> Result<()> {
    conn.execute(
        "DELETE FROM repair_attempts WHERE work_id=?1 AND snapshot_id<>?2",
        params![work.as_bytes().as_slice(), current.as_bytes().as_slice()],
    )?;
    Ok(())
}

/// どの行も今の版を指す。
pub(super) fn verify(conn: &Connection) -> Result<()> {
    let stale: i64 = conn.query_row(
        "SELECT count(*) FROM repair_attempts r WHERE NOT EXISTS(
           SELECT 1 FROM entity_current c WHERE c.entity_id=r.work_id AND c.snapshot_id=r.snapshot_id)",
        [],
        |r| r.get(0),
    )?;
    if stale != 0 {
        return Err(Error::Integrity(format!(
            "補完を試した記録 {stale} 件が今の版を指していません"
        )));
    }
    Ok(())
}
