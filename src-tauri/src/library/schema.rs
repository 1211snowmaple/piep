use super::{Error, Fingerprint, LibraryId, Result};
use rusqlite::Connection;

pub(crate) const APPLICATION_ID: i64 = 0x504C5632;
pub(crate) const VERSION: i64 = 8;
const SQL: &str = include_str!("schema.sql");
const MIGRATIONS: &[&str] = &[
    SQL,
    include_str!("migration_002_suggestions.sql"),
    include_str!("migration_003_acquisitions.sql"),
    include_str!("migration_004_source_checks.sql"),
    include_str!("migration_005_saved_searches.sql"),
    include_str!("migration_006_reader_bindings.sql"),
    include_str!("migration_007_acquisition_readers.sql"),
    include_str!("migration_008_job_history.sql"),
];

fn migration_checksum(sql: &str) -> Fingerprint {
    // Git のチェックアウト設定による CRLF/LF の違いは、スキーマの変更ではない。
    Fingerprint::of(sql.replace("\r\n", "\n").as_bytes())
}

pub(crate) fn create(conn: &mut Connection, library: LibraryId) -> Result<()> {
    if conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
        [],
        |r| r.get::<_, i64>(0),
    )? != 0
    {
        return Err(Error::Unsupported(
            "新規作成先に既存のテーブルがあります".into(),
        ));
    }
    configure(conn)?;
    let tx = conn.transaction()?;
    for sql in MIGRATIONS {
        tx.execute_batch(sql)?;
    }
    tx.execute(
        "INSERT INTO library_meta(singleton,library_id) VALUES(1,?1)",
        [library.as_bytes().as_slice()],
    )?;
    for (index, sql) in MIGRATIONS.iter().enumerate() {
        record_migration(&tx, index as i64 + 1, sql)?;
    }
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn open(conn: &Connection, library: LibraryId) -> Result<()> {
    // 未知の DB に WAL や DDL を適用する前に識別する。
    let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app != APPLICATION_ID || !(1..=VERSION).contains(&version) {
        return Err(Error::Unsupported(format!(
            "application_id={app}, schema={version}"
        )));
    }
    let id: Vec<u8> = conn.query_row(
        "SELECT library_id FROM library_meta WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    if id != library.as_bytes() {
        return Err(Error::Integrity("ライブラリ ID が一致しません".into()));
    }
    let mut stmt =
        conn.prepare("SELECT version,checksum FROM schema_migrations ORDER BY version")?;
    let records = stmt
        .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if records.len() != version as usize
        || records
            .iter()
            .enumerate()
            .any(|(index, (number, checksum))| {
                *number != index as i64 + 1 || checksum != &migration_checksum(MIGRATIONS[index]).0
            })
    {
        return Err(Error::Integrity("スキーマの記録が一致しません".into()));
    }
    drop(stmt);
    let (storage,reader,writer):(i64,i64,i64)=conn.query_row("SELECT storage_version,min_reader_version,min_writer_version FROM library_meta WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if storage != 1 || reader > 1 || writer > 1 {
        return Err(Error::Unsupported(format!(
            "保存形式={storage}, 最低読取版={reader}, 最低書込版={writer}"
        )));
    }
    configure(conn)?;
    if version < VERSION {
        let tx = conn.unchecked_transaction()?;
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(version as usize) {
            tx.execute_batch(sql)?;
            record_migration(&tx, index as i64 + 1, sql)?;
        }
        tx.pragma_update(None, "user_version", VERSION)?;
        tx.commit()?;
    }
    Ok(())
}

fn record_migration(conn: &Connection, version: i64, sql: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO schema_migrations VALUES(?1,?2,?3)",
        rusqlite::params![
            version,
            migration_checksum(sql).0.as_slice(),
            chrono::Utc::now().timestamp_millis()
        ],
    )?;
    Ok(())
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", true)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn version_one(id: LibraryId) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        configure(&conn).unwrap();
        conn.execute_batch(SQL).unwrap();
        conn.execute(
            "INSERT INTO library_meta(singleton,library_id,generation) VALUES(1,?1,42)",
            [id.as_bytes().as_slice()],
        )
        .unwrap();
        record_migration(&conn, 1, SQL).unwrap();
        conn.pragma_update(None, "application_id", APPLICATION_ID)
            .unwrap();
        conn.pragma_update(None, "user_version", 1).unwrap();
        conn
    }

    #[test]
    fn the_source_check_extension_keeps_profile_dates_and_does_not_invent_a_deep_check() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(2) {
            conn.execute_batch(sql).unwrap();
            record_migration(&conn, index as i64 + 1, sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 3).unwrap();
        let person = LibraryId::now_v7();
        conn.execute(
            "INSERT INTO entities(id,kind,created_at) VALUES(?1,'person',0)",
            [person.as_bytes().as_slice()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO source_entity_state VALUES(?1,1234,5678)",
            [person.as_bytes().as_slice()],
        )
        .unwrap();
        open(&conn, id).unwrap();
        let dates=conn.query_row("SELECT last_checked_at,last_fetched_at,last_deep_checked_at FROM source_entity_state",[],|r|Ok((r.get::<_,Option<i64>>(0)?,r.get::<_,Option<i64>>(1)?,r.get::<_,Option<i64>>(2)?))).unwrap();
        assert_eq!(dates, (Some(1234), Some(5678), None));
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
    }

    #[test]
    fn saved_search_migration_keeps_outbox_rows_and_its_deleted_sequence_high_water_mark() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(3) {
            conn.execute_batch(sql).unwrap();
            record_migration(&conn, index as i64 + 1, sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 4).unwrap();
        conn.execute(
            "INSERT INTO entities(id,kind,created_at) VALUES(?1,'work',0)",
            [id.as_bytes().as_slice()],
        )
        .unwrap();
        for sequence in [2, 7, 200] {
            conn.execute("INSERT INTO outbox(sequence,request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,?3,'favorite','{ \"old\": true }',123)",rusqlite::params![sequence,LibraryId::now_v7().as_bytes().as_slice(),id.as_bytes().as_slice()]).unwrap();
        }
        conn.execute("DELETE FROM outbox WHERE sequence=200", [])
            .unwrap();
        let original = || {
            conn.prepare("SELECT sequence,request_id,entity_id,operation,payload_json,created_at FROM outbox ORDER BY sequence").unwrap().query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,Vec<u8>>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,i64>(5)?))).unwrap().collect::<std::result::Result<Vec<_>,_>>().unwrap()
        };
        let before = original();
        let search = LibraryId::now_v7();
        conn.execute("INSERT INTO saved_searches VALUES(?1,'keep',NULL,'{ \"future\":9007199254740993 }',1,12,34)",[search.as_bytes().as_slice()]).unwrap();
        // 途中の DDL まで巻き戻り、outbox を失わない。
        conn.execute_batch("CREATE TABLE outbox_next(blocked TEXT)")
            .unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(original(), before);
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            4
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM pragma_table_info('saved_searches') WHERE name='user_id'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.execute_batch("DROP TABLE outbox_next").unwrap();
        open(&conn, id).unwrap();
        assert_eq!(original(), before);
        let kept:(Vec<u8>,String,i64,i64,i64)=conn.query_row("SELECT user_id,params_json,created_at,updated_at,generation FROM saved_searches WHERE id=?1",[search.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).unwrap();
        assert_eq!(
            kept,
            (
                id.as_bytes().to_vec(),
                "{ \"future\":9007199254740993 }".into(),
                12,
                34,
                1
            )
        );
        conn.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'favorite','{}',0)",rusqlite::params![LibraryId::now_v7().as_bytes().as_slice(),id.as_bytes().as_slice()]).unwrap();
        assert_eq!(conn.last_insert_rowid(), 201);
        open(&conn, id).unwrap();
        assert_eq!(conn.last_insert_rowid(), 201);
    }

    #[test]
    fn unsupported_saved_search_definitions_keep_the_old_table_and_schema_on_failure() {
        for (definition, version) in [("{}", 2), ("[]", 1)] {
            let id = LibraryId::now_v7();
            let conn = version_one(id);
            for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(3) {
                conn.execute_batch(sql).unwrap();
                record_migration(&conn, index as i64 + 1, sql).unwrap();
            }
            conn.pragma_update(None, "user_version", 4).unwrap();
            let search = LibraryId::now_v7();
            conn.execute(
                "INSERT INTO saved_searches VALUES(?1,'keep',NULL,?2,?3,12,34)",
                rusqlite::params![search.as_bytes().as_slice(), definition, version],
            )
            .unwrap();
            assert!(open(&conn, id).is_err());
            assert_eq!(
                conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                    .unwrap(),
                4
            );
            let kept: (String, i64) = conn
                .query_row(
                    "SELECT filters_json,filter_version FROM saved_searches WHERE id=?1",
                    [search.as_bytes().as_slice()],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(kept, (definition.into(), version));
        }
    }

    #[test]
    fn reader_binding_migration_rolls_back_on_failure_and_requires_backfill_for_imported_history() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(4) {
            conn.execute_batch(sql).unwrap();
            record_migration(&conn, index as i64 + 1, sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 5).unwrap();
        conn.execute("INSERT INTO migration_records(source_table,source_key,target_id,status) VALUES('download_versions','old',?1,'imported')",[id.as_bytes().as_slice()]).unwrap();
        conn.execute_batch("CREATE TABLE reader_bindings(blocked TEXT)")
            .unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='reader_binding_sets'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        conn.execute_batch("DROP TABLE reader_bindings").unwrap();
        open(&conn, id).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            VERSION
        );
        assert_eq!(conn.query_row("SELECT status FROM migration_records WHERE source_table='__normalization__' AND source_key='reader_bindings'",[],|r|r.get::<_,String>(0)).unwrap(),"unresolved");
        assert_eq!(conn.query_row("SELECT target_id FROM migration_records WHERE source_table='download_versions' AND source_key='old'",[],|r|r.get::<_,Vec<u8>>(0)).unwrap(),id.as_bytes());
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
    }

    #[test]
    fn atomic_reader_completion_migration_keeps_v6_state_and_rolls_back_partial_ddl() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(5) {
            conn.execute_batch(sql).unwrap();
            record_migration(&conn, index as i64 + 1, sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 6).unwrap();
        let old_checksums = conn
            .prepare("SELECT checksum FROM schema_migrations ORDER BY version")
            .unwrap()
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        conn.execute_batch("CREATE TRIGGER acquisition_reader_completion BEFORE UPDATE ON acquisition_attempts BEGIN SELECT 1; END;").unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            6
        );
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('acquisition_reader_outputs','acquisition_reader_scope')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        conn.execute_batch("DROP TRIGGER acquisition_reader_completion;")
            .unwrap();
        open(&conn, id).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            VERSION
        );
        let kept = conn
            .prepare("SELECT checksum FROM schema_migrations WHERE version<=6 ORDER BY version")
            .unwrap()
            .query_map([], |r| r.get::<_, Vec<u8>>(0))
            .unwrap()
            .collect::<std::result::Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(kept, old_checksums);
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM acquisition_reader_outputs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn job_history_forward_migration_preserves_v7_state_and_rolls_back_all_new_tables_on_failure() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        for (index, sql) in MIGRATIONS.iter().enumerate().skip(1).take(6) {
            conn.execute_batch(sql).unwrap();
            record_migration(&conn, index as i64 + 1, sql).unwrap();
        }
        conn.pragma_update(None, "user_version", 7).unwrap();
        conn.execute_batch("CREATE TABLE job_history_logs(blocked TEXT)")
            .unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            7
        );
        assert_eq!(conn.query_row("SELECT count(*) FROM sqlite_master WHERE name IN ('job_history','job_history_items','job_history_item_scope')",[],|r|r.get::<_,i64>(0)).unwrap(),0);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            7
        );
        conn.execute_batch("DROP TABLE job_history_logs").unwrap();
        open(&conn, id).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            VERSION
        );
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            8
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM acquisition_reader_outputs", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
        open(&conn, id).unwrap();
    }

    #[test]
    fn forward_migrations_preserve_identity_and_roll_back_every_change_on_failure() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        // 2 番目の CREATE で失敗させ、先に実行された DDL も巻き戻ることを確かめる。
        conn.execute_batch("CREATE TABLE collection_suggestion_seeds(obstruction TEXT); INSERT INTO collection_suggestion_seeds VALUES('keep');").unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='collection_suggestions'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT obstruction FROM collection_suggestion_seeds",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
            "keep"
        );
        conn.execute_batch("DROP TABLE collection_suggestion_seeds")
            .unwrap();
        open(&conn, id).unwrap();
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            VERSION
        );
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        let before: Vec<u8> = conn
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version=1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        open(&conn, id).unwrap();
        assert_eq!(before, migration_checksum(SQL).0);
        assert_eq!(
            conn.query_row("SELECT count(*) FROM schema_migrations", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            VERSION
        );
    }

    #[test]
    fn unsupported_minimum_versions_and_tampered_history_stop_before_forward_migration() {
        let id = LibraryId::now_v7();
        let conn = version_one(id);
        conn.execute("UPDATE library_meta SET min_writer_version=2", [])
            .unwrap();
        assert!(matches!(open(&conn, id), Err(Error::Unsupported(_))));
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        conn.execute("UPDATE library_meta SET min_writer_version=1", [])
            .unwrap();
        conn.execute("UPDATE schema_migrations SET checksum=zeroblob(32)", [])
            .unwrap();
        assert!(matches!(open(&conn, id), Err(Error::Integrity(_))));
        assert_eq!(
            conn.pragma_query_value(None, "user_version", |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='collection_suggestions'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn migration_checksums_ignore_checkout_newlines_and_still_detect_schema_changes() {
        let id = LibraryId::now_v7();
        let mut conn = Connection::open_in_memory().unwrap();
        create(&mut conn, id).unwrap();
        let lf = SQL.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        assert_eq!(migration_checksum(&lf), migration_checksum(&crlf));
        // LF でビルドされた別環境の記録でも、この環境から開ける。
        conn.execute(
            "UPDATE schema_migrations SET checksum=?1 WHERE version=1",
            [Fingerprint::of(lf.as_bytes()).0.as_slice()],
        )
        .unwrap();
        open(&conn, id).unwrap();
        let changed = migration_checksum(&lf.replace("generation >= 0", "generation >= 1"));
        conn.execute(
            "UPDATE schema_migrations SET checksum=?1 WHERE version=1",
            [changed.0.as_slice()],
        )
        .unwrap();
        assert!(matches!(open(&conn, id), Err(Error::Integrity(_))));
    }
    #[test]
    fn a_newer_required_writer_is_rejected_without_resetting_the_library() {
        let id = LibraryId::now_v7();
        let mut conn = Connection::open_in_memory().unwrap();
        create(&mut conn, id).unwrap();
        conn.execute(
            "UPDATE library_meta SET min_writer_version=2,generation=42",
            [],
        )
        .unwrap();
        assert!(matches!(open(&conn, id), Err(Error::Unsupported(_))));
        assert_eq!(
            conn.query_row("SELECT generation FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            42
        );
        assert_eq!(
            conn.query_row("SELECT min_writer_version FROM library_meta", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
    #[test]
    fn new_schema_has_enforced_ownership_and_never_resets_a_foreign_database() {
        let id = LibraryId::now_v7();
        let mut conn = Connection::open_in_memory().unwrap();
        create(&mut conn, id).unwrap();
        open(&conn, id).unwrap();
        let work = LibraryId::now_v7();
        conn.execute(
            "INSERT INTO entities(id,kind,created_at) VALUES(?1,'work',0)",
            [work.as_bytes().as_slice()],
        )
        .unwrap();
        assert!(conn
            .execute(
                "INSERT INTO people(id) VALUES(?1)",
                [work.as_bytes().as_slice()]
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO external_identities VALUES('pixiv','person','123',?1)",
                [work.as_bytes().as_slice()]
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO chunks VALUES(?1,-1)",
                [Fingerprint::of(b"x").0.as_slice()]
            )
            .is_err());
        conn.pragma_update(None, "user_version", 99).unwrap();
        assert!(open(&conn, id).is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM entities", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(create(&mut conn, id).is_err());
    }
}
