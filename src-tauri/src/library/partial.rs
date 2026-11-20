//! 棚の部分写し。作者・シリーズのアーカイブとして、選んだ作品だけを自立した棚の形で写し、
//! あとで同じ棚へ取り込み直す。
//!
//! 行は識別子ごと写す。保存状態の指紋が作品・作者・タグ・ファイルの識別子を含むので、付け替えると
//! 検査を通らない。だから取り込めるのは同じ棚（同じ棚の写しを含む）から作った部分写しだけで、
//! 今の棚にある行は書き換えず、無い行だけを足す。同じ作品や作者が別の識別子で保存し直されて
//! いれば、その作品は足さずに数だけ伝える。
//!
//! 写しの作成も取り込みも同じ手順で行う。作成は空の棚へ、取り込みは今の棚へ足すだけの違い。
use super::{
    repository::PreparedFile,
    store::{regular_directory, sync_directory},
    Error, Fingerprint, Library, LibraryId, Result,
};
use rusqlite::{Connection, ErrorCode, OptionalExtension, ToSql, TransactionBehavior};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// 部分写しの形式の記録。棚の `library.json` とは名前を分け、棚として開けないようにする。
pub const PART_MARKER: &str = "library-part.json";

/// 作品ごとの写し・取り込みの結果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PartTransfer {
    /// 新しく足した作品。
    pub added: u64,
    /// 棚から外していた作品を、残っていた記録のまま棚へ戻した数。
    pub restored: u64,
    /// すでに棚にあって、そのままにした作品。
    pub already: u64,
    /// 同じ作品・作者・タグなどが別の記録として保存し直されていて、足せなかった作品。
    pub conflicts: u64,
}

/// 一作品ぶんの範囲を置く一時表。写す側の行を選ぶのに使う。
const SCOPE: &str = "
CREATE TEMP TABLE IF NOT EXISTS px_e(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_s(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_ns(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_r(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_f(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_ne(id BLOB PRIMARY KEY);
CREATE TEMP TABLE IF NOT EXISTS px_nb(snapshot_id BLOB, recipe INTEGER, PRIMARY KEY(snapshot_id,recipe));
CREATE TEMP TABLE IF NOT EXISTS px_w(id BLOB PRIMARY KEY);
DELETE FROM px_w;";
const CLEAR: &str = "DELETE FROM px_e; DELETE FROM px_s; DELETE FROM px_ns; DELETE FROM px_r;
DELETE FROM px_f; DELETE FROM px_ne; DELETE FROM px_nb;";
const DROP_SCOPE: &str = "DROP TABLE IF EXISTS temp.px_e; DROP TABLE IF EXISTS temp.px_s;
DROP TABLE IF EXISTS temp.px_ns; DROP TABLE IF EXISTS temp.px_r; DROP TABLE IF EXISTS temp.px_f;
DROP TABLE IF EXISTS temp.px_ne; DROP TABLE IF EXISTS temp.px_nb; DROP TABLE IF EXISTS temp.px_w;";

/// 作品から、その保存状態が指す作者・シリーズへ広げる（作者の紹介の保存状態が指す先も）。
const CLOSURE: &str = "INSERT OR IGNORE INTO px_e
SELECT p.person_id FROM src.snapshot_people p JOIN src.revision_snapshots s ON s.id=p.snapshot_id
 WHERE s.entity_id IN px_e AND s.published=1
UNION SELECT q.series_id FROM src.snapshot_series q JOIN src.revision_snapshots s ON s.id=q.snapshot_id
 WHERE s.entity_id IN px_e AND s.published=1";
/// 写す取得の記録。結果が未確定の保存状態を指すものは写さない。
const OBSERVATIONS: &str =
    "entity_id IN px_e AND (result_snapshot_id IS NULL OR result_snapshot_id IN px_s)";
const EDITS: &str = "work_id IN px_e AND sealed=1";

/// 写す範囲の行が指すファイル。
fn files_sql() -> String {
    format!(
        "INSERT OR IGNORE INTO px_f
 SELECT raw_file_id FROM src.acquisition_observations WHERE {OBSERVATIONS} AND raw_file_id IS NOT NULL
 UNION SELECT body_file_id FROM src.acquisition_observations WHERE {OBSERVATIONS} AND body_file_id IS NOT NULL
 UNION SELECT request_hash FROM src.acquisition_observations WHERE {OBSERVATIONS}
 UNION SELECT body_file_id FROM src.revision_snapshots WHERE id IN px_s AND body_file_id IS NOT NULL
 UNION SELECT file_id FROM src.snapshot_files WHERE snapshot_id IN px_s
 UNION SELECT file_id FROM src.media_renditions WHERE id IN px_r AND file_id IS NOT NULL
 UNION SELECT content_file_id FROM src.edit_revisions WHERE {EDITS}
 UNION SELECT file_id FROM src.edit_media WHERE edit_id IN (SELECT id FROM src.edit_revisions WHERE {EDITS})
 UNION SELECT body_file_id FROM src.reader_binding_sets WHERE snapshot_id IN px_s
 UNION SELECT evidence_file_id FROM src.repair_attempt_markers WHERE entity_id IN px_e AND snapshot_id IN px_s AND evidence_file_id IS NOT NULL
 UNION SELECT evidence_file_id FROM src.asset_audit_markers WHERE entity_id IN px_e AND snapshot_id IN px_s AND evidence_file_id IS NOT NULL"
    )
}

/// 表ごとの列と主キー。写す先の表の形に合わせる（写す元も同じ版へ移行済み）。
struct Tables(HashMap<&'static str, (Vec<String>, Vec<String>)>);

const TABLES: &[&str] = &[
    "entities",
    "external_identities",
    "works",
    "people",
    "series",
    "source_revisions",
    "revision_snapshots",
    "acquisition_observations",
    "tags",
    "source_media",
    "media_renditions",
    "media_observations",
    "snapshot_observations",
    "snapshot_files",
    "snapshot_media",
    "snapshot_people",
    "snapshot_series",
    "snapshot_tags",
    "reader_binding_sets",
    "reader_bindings",
    "entity_current",
    "source_entity_state",
    "edit_revisions",
    "edit_media",
    "edit_blocks",
    "edit_documents",
    "edit_archive",
    "work_user_state",
    "reading_positions",
    "user_tags",
    "work_user_tag_state",
    "work_user_tags",
    "repair_attempt_markers",
    "asset_audit_markers",
    "migration_records",
    "body_text_stats",
    "ai_notes",
    "library_items",
    "work_links",
    "link_evidence",
];

impl Tables {
    fn read(conn: &Connection) -> Result<Self> {
        let mut tables = HashMap::new();
        for &table in TABLES {
            let mut stmt = conn.prepare(&format!("PRAGMA main.table_info({table})"))?;
            let mut columns = Vec::new();
            let mut keys = Vec::new();
            for row in stmt.query_map([], |r| Ok((r.get::<_, String>(1)?, r.get::<_, i64>(5)?)))? {
                let (name, key) = row?;
                if key > 0 {
                    keys.push((key, name.clone()));
                }
                columns.push(name);
            }
            keys.sort();
            if columns.is_empty() || keys.is_empty() {
                return Err(Error::Integrity(format!("{table} の形を読めません")));
            }
            tables.insert(table, (columns, keys.into_iter().map(|(_, k)| k).collect()));
        }
        Ok(Self(tables))
    }

    /// 写す元の `filter` に合う行のうち、写す先に主キーの無い行を足す。`overrides` は列の値を
    /// 置き換える式（封をする前の形で入れるのに使う）。足した行の数を返す。
    fn copy(
        &self,
        conn: &Connection,
        table: &str,
        filter: &str,
        overrides: &[(&str, &str)],
        values: &[&dyn ToSql],
    ) -> Result<usize> {
        let (columns, keys) = self
            .0
            .get(table)
            .ok_or_else(|| Error::Integrity(format!("{table} は写す表にありません")))?;
        let selected = columns
            .iter()
            .map(|c| {
                overrides
                    .iter()
                    .find(|(name, _)| name == c)
                    .map_or_else(|| format!("x.\"{c}\""), |(_, value)| (*value).to_owned())
            })
            .collect::<Vec<_>>()
            .join(",");
        let names = columns
            .iter()
            .map(|c| format!("\"{c}\""))
            .collect::<Vec<_>>()
            .join(",");
        let same = keys
            .iter()
            .map(|k| format!("y.\"{k}\" IS x.\"{k}\""))
            .collect::<Vec<_>>()
            .join(" AND ");
        let sql = format!(
            "INSERT INTO main.\"{table}\"({names}) SELECT {selected} FROM src.\"{table}\" AS x
             WHERE ({filter}) AND NOT EXISTS(SELECT 1 FROM main.\"{table}\" AS y WHERE {same})"
        );
        Ok(conn.prepare_cached(&sql)?.execute(values)?)
    }
}

/// 別の識別子で保存し直された行とぶつかった（足せない）か。それ以外の失敗は全体を止める。
fn conflicted(error: &Error) -> bool {
    matches!(error, Error::Sql(rusqlite::Error::SqliteFailure(e, _)) if e.code == ErrorCode::ConstraintViolation)
}

fn exists(conn: &Connection, sql: &str, id: &[u8]) -> Result<bool> {
    Ok(conn.query_row(sql, [id], |_| Ok(())).optional()?.is_some())
}

impl Library {
    /// 部分写しか（形式の記録で見分ける）。
    pub fn is_part(root: &Path) -> bool {
        root.join(PART_MARKER).is_file()
    }

    /// 選んだ作品を、その作者・シリーズ・版・編集・読書位置・画像・添付ごと、まだ無いフォルダー
    /// `destination` へ自立した部分写しとして写す。写し終えて全体を検査するまでは置かない。
    pub fn copy_works(&self, works: &[LibraryId], destination: &Path) -> Result<PartTransfer> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        regular_directory(parent)?;
        if destination.try_exists()? {
            return Err(Error::UnsafePath(destination.into()));
        }
        // 写す途中で、写し元の台帳が参照しなくなった実体を回収されないようにする。
        let _turn = self.storage_gate.lock().map_err(|_| Error::Busy)?;
        let stage = parent.join(format!(".piep-part-{}.part", LibraryId::now_v7()));
        let made = (|| {
            let part = Library::create_backup_stage(&stage, self.id)?;
            let outcome = {
                let mut conn = part.conn.lock().map_err(|_| Error::Busy)?;
                transfer(self, &part, &mut conn, works)?
            };
            if outcome.conflicts != 0 || outcome.restored != 0 {
                return Err(Error::Integrity(
                    "写しの作品が一部しか写せませんでした".into(),
                ));
            }
            part.verify()?;
            part.conn
                .lock()
                .map_err(|_| Error::Busy)?
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
            Ok(outcome)
        })();
        let outcome = match made {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = fs::remove_dir_all(&stage);
                return Err(error);
            }
        };
        if destination.try_exists()? {
            let _ = fs::remove_dir_all(&stage);
            return Err(Error::UnsafePath(destination.into()));
        }
        fs::rename(&stage, destination)?;
        sync_directory(parent)?;
        fs::rename(
            destination.join(".backup-marker.json"),
            destination.join(PART_MARKER),
        )?;
        sync_directory(destination)?;
        Ok(outcome)
    }

    /// 部分写しの作品を、この棚へ足す。今の棚にある行は書き換えない。
    pub fn merge_part(&self, part: &Library) -> Result<PartTransfer> {
        if part.id != self.id {
            return Err(Error::Unsupported(
                "別の棚から作った写しは取り込めません".into(),
            ));
        }
        let _turn = self.storage_gate.lock().map_err(|_| Error::Busy)?;
        // 実体は先に写す。台帳の書き込みを長く止めない。
        let files = {
            let theirs = part.reader()?;
            let ours = self.reader()?;
            let ids = theirs
                .prepare("SELECT id FROM content_files WHERE sealed=1 ORDER BY id")?
                .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            let mut missing = Vec::new();
            for id in ids {
                if !exists(&ours, "SELECT 1 FROM content_files WHERE id=?1", &id)? {
                    missing.push(Self::file_from_conn(&theirs, fingerprint(id)?)?);
                }
            }
            missing
        };
        for file in &files {
            part.objects.replicate(file, &self.objects)?;
        }
        let works = part
            .reader()?
            .prepare("SELECT work_id FROM library_items ORDER BY added_at,work_id")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .map(|id| {
                LibraryId::from_slice(&id?)
                    .map_err(|_| Error::Integrity("作品 ID が不正です".into()))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        transfer(part, self, &mut conn, &works)
    }
}

fn fingerprint(id: Vec<u8>) -> Result<Fingerprint> {
    Ok(Fingerprint(id.try_into().map_err(|_| {
        Error::Integrity("ファイル ID が不正です".into())
    })?))
}

/// `from` の作品を `into`（書き込みの接続 `conn`）へ足す。
fn transfer(
    from: &Library,
    into: &Library,
    conn: &mut Connection,
    works: &[LibraryId],
) -> Result<PartTransfer> {
    let source = from.root().join("library.db");
    let source = source
        .to_str()
        .ok_or_else(|| Error::UnsafePath(source.clone()))?
        .to_owned();
    conn.execute("ATTACH DATABASE ?1 AS src", [source])?;
    let result = conn
        .execute_batch(SCOPE)
        .map_err(Error::from)
        .and_then(|()| transfer_attached(from, into, conn, works));
    let dropped = conn.execute_batch(DROP_SCOPE);
    let detached = conn.execute_batch("DETACH DATABASE src");
    let outcome = result?;
    dropped?;
    detached?;
    Ok(outcome)
}

fn transfer_attached(
    from: &Library,
    into: &Library,
    conn: &mut Connection,
    works: &[LibraryId],
) -> Result<PartTransfer> {
    let tables = Tables::read(conn)?;
    let source = from.reader()?;
    let mut tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let mut outcome = PartTransfer::default();
    for &work in works {
        let w = work.as_bytes().as_slice();
        if exists(&tx, "SELECT 1 FROM main.library_items WHERE work_id=?1", w)? {
            outcome.already += 1;
            continue;
        }
        if !exists(&tx, "SELECT 1 FROM src.library_items WHERE work_id=?1", w)? {
            return Err(Error::Integrity(format!(
                "作品 {work} は写す元の棚にありません"
            )));
        }
        let known = exists(&tx, "SELECT 1 FROM main.entities WHERE id=?1", w)?;
        let copied = {
            let point = tx.savepoint()?;
            match copy_work(from, into, &source, &point, &tables, work) {
                Ok(()) => point.commit().map_err(Error::from),
                // 足せなかった作品は、書きかけの行ごと戻す（点を落とすと巻き戻る）。
                Err(error) => Err(error),
            }
        };
        match copied {
            Ok(()) if known => outcome.restored += 1,
            Ok(()) => outcome.added += 1,
            Err(error) if conflicted(&error) => {
                // 外していた作品の記録が残っていれば、残っている記録のまま棚へ戻す。
                let back = known
                    && exists(
                        &tx,
                        "SELECT 1 FROM main.entity_current WHERE entity_id=?1",
                        w,
                    )?
                    && {
                        let point = tx.savepoint()?;
                        let inserted =
                            tables.copy(&point, "library_items", "work_id=?1", &[], &[&w]);
                        match inserted {
                            Ok(n) => {
                                point.commit()?;
                                n == 1
                            }
                            Err(error) if conflicted(&error) => false,
                            Err(error) => return Err(error),
                        }
                    };
                if back {
                    outcome.restored += 1;
                } else {
                    outcome.conflicts += 1;
                    continue;
                }
            }
            Err(error) => return Err(error),
        }
        tx.execute("INSERT INTO px_w VALUES(?1)", [w])?;
    }
    // 作品どうしのつながりは、両端が棚にあるものだけ。
    tables.copy(
        &tx,
        "work_links",
        "(from_work_id IN px_w OR to_work_id IN px_w)
         AND from_work_id IN (SELECT id FROM main.works) AND to_work_id IN (SELECT id FROM main.works)
         AND NOT EXISTS(SELECT 1 FROM main.work_links z WHERE z.from_work_id=x.from_work_id
           AND z.to_work_id=x.to_work_id AND z.relation=x.relation)",
        &[],
        &[],
    )?;
    tables.copy(
        &tx,
        "link_evidence",
        "link_id IN (SELECT y.id FROM main.work_links y JOIN src.work_links z ON z.id=y.id)",
        &[],
        &[],
    )?;
    let changed = tx
        .prepare("SELECT id FROM px_w")?
        .query_map([], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for work in &changed {
        let work = LibraryId::from_slice(work)
            .map_err(|_| Error::Integrity("作品 ID が不正です".into()))?;
        into.refresh_work_list(&tx, work)?;
    }
    if !changed.is_empty() {
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
    }
    tx.commit()?;
    Ok(outcome)
}

/// 一作品と、それが指す作者・シリーズの行を、封をする前の形で入れてから封をする。
fn copy_work(
    from: &Library,
    into: &Library,
    source: &Connection,
    conn: &Connection,
    tables: &Tables,
    work: LibraryId,
) -> Result<()> {
    let w = work.as_bytes().as_slice();
    conn.execute_batch(CLEAR)?;
    conn.execute("INSERT INTO px_e VALUES(?1)", [w])?;
    while conn.execute(CLOSURE, [])? > 0 {}
    conn.execute_batch(
        "INSERT INTO px_s SELECT id FROM src.revision_snapshots WHERE entity_id IN px_e AND published=1;
         INSERT INTO px_ns SELECT id FROM px_s WHERE id NOT IN (SELECT id FROM main.revision_snapshots);
         INSERT OR IGNORE INTO px_r SELECT rendition_id FROM src.snapshot_media
          WHERE snapshot_id IN px_ns AND rendition_id IS NOT NULL;",
    )?;
    conn.execute(&files_sql(), [])?;

    // ファイルは実体を確かめてから台帳へ載せる。無ければ写す元から写す。
    let missing = conn
        .prepare("SELECT id FROM px_f WHERE id NOT IN (SELECT id FROM main.content_files)")?
        .query_map([], |r| r.get::<_, Vec<u8>>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    for id in missing {
        let file = Library::file_from_conn(source, fingerprint(id)?)?;
        let stored = match into.objects.inspect(&file) {
            Ok(stored) => stored,
            Err(_) => {
                from.objects.replicate(&file, &into.objects)?;
                into.objects.inspect(&file)?
            }
        };
        Library::register_file(conn, &PreparedFile::from(stored))?;
    }

    let copy = |table: &str, filter: &str| tables.copy(conn, table, filter, &[], &[]);
    copy("entities", "id IN px_e")?;
    copy("external_identities", "entity_id IN px_e")?;
    for table in ["works", "people", "series"] {
        copy(table, "id IN px_e")?;
    }
    copy("source_revisions", "entity_id IN px_e")?;
    tables.copy(
        conn,
        "revision_snapshots",
        "id IN px_ns",
        &[("published", "0")],
        &[],
    )?;
    copy("acquisition_observations", OBSERVATIONS)?;
    copy(
        "tags",
        "id IN (SELECT tag_id FROM src.snapshot_tags WHERE snapshot_id IN px_ns)",
    )?;
    copy(
        "source_media",
        "id IN (SELECT media_id FROM src.snapshot_media WHERE snapshot_id IN px_ns)",
    )?;
    copy("media_renditions", "id IN px_r")?;
    copy("media_observations", "rendition_id IN px_r")?;
    for table in [
        "snapshot_observations",
        "snapshot_files",
        "snapshot_media",
        "snapshot_people",
        "snapshot_series",
        "snapshot_tags",
    ] {
        copy(table, "snapshot_id IN px_ns")?;
    }
    conn.execute(
        "UPDATE main.revision_snapshots SET published=1 WHERE id IN px_ns",
        [],
    )?;

    // 本文の参照の対応は、公開した保存状態に封をする前の形で入れる。
    conn.execute(
        "INSERT INTO px_nb SELECT snapshot_id,recipe FROM src.reader_binding_sets x WHERE snapshot_id IN px_s
          AND NOT EXISTS(SELECT 1 FROM main.reader_binding_sets y WHERE y.snapshot_id=x.snapshot_id AND y.recipe=x.recipe)",
        [],
    )?;
    let bound = "(snapshot_id,recipe) IN (SELECT snapshot_id,recipe FROM px_nb)";
    tables.copy(conn, "reader_binding_sets", bound, &[("sealed", "0")], &[])?;
    copy("reader_bindings", bound)?;
    conn.execute(
        &format!("UPDATE main.reader_binding_sets SET sealed=1 WHERE {bound}
          AND (snapshot_id,recipe) IN (SELECT snapshot_id,recipe FROM src.reader_binding_sets WHERE sealed=1)"),
        [],
    )?;
    copy("entity_current", "entity_id IN px_e")?;
    copy("source_entity_state", "entity_id IN px_e")?;

    // 編集は親から順に。子は封をした親しか指せない。
    loop {
        conn.execute("DELETE FROM px_ne", [])?;
        let ready = conn.execute(
            &format!("INSERT INTO px_ne SELECT id FROM src.edit_revisions x WHERE {EDITS}
              AND id NOT IN (SELECT id FROM main.edit_revisions)
              AND (parent_id IS NULL OR parent_id IN (SELECT id FROM main.edit_revisions WHERE sealed=1))"),
            [],
        )?;
        if ready == 0 {
            break;
        }
        tables.copy(
            conn,
            "edit_revisions",
            "id IN px_ne",
            &[("sealed", "0")],
            &[],
        )?;
        copy("edit_media", "edit_id IN px_ne")?;
        copy("edit_blocks", "edit_id IN px_ne")?;
        conn.execute(
            "UPDATE main.edit_revisions SET sealed=1 WHERE id IN px_ne",
            [],
        )?;
    }
    copy("edit_documents", "work_id IN px_e")?;
    copy("edit_archive", "work_id IN px_e")?;
    copy("work_user_state", "work_id IN px_e")?;
    copy("reading_positions", "work_id IN px_e")?;
    copy(
        "user_tags",
        "id IN (SELECT tag_id FROM src.work_user_tags WHERE work_id IN px_e)",
    )?;
    copy("work_user_tag_state", "work_id IN px_e")?;
    copy("work_user_tags", "work_id IN px_e")?;
    // 保守の記録（修復を試した・画像を点検した）は、旧棚から移した原記録と照合する。その原記録の行も持つ。
    copy(
        "migration_records",
        "source_key='__all_rows__' AND source_table IN ('incomplete_save_repairs','saved_asset_audits')
         AND evidence_file_id IN px_f",
    )?;
    for table in ["repair_attempt_markers", "asset_audit_markers"] {
        copy(table, "entity_id IN px_e AND snapshot_id IN px_s")?;
    }
    copy("body_text_stats", "file_id IN px_f")?;
    // 作品の覚え書き（あらすじなど）は作り直すと重いので一緒に持つ。鍵は作品 ID の文字列。
    let key = work.to_string();
    tables.copy(
        conn,
        "ai_notes",
        "subject_type='work' AND subject_key=?1",
        &[],
        &[&key],
    )?;
    tables.copy(conn, "library_items", "work_id=?1", &[], &[&w])?;
    Ok(())
}

#[cfg(test)]
#[path = "partial_tests.rs"]
mod tests;
