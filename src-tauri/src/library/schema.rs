use super::{Error, Fingerprint, LibraryId, Result};
use rusqlite::Connection;

pub(crate) const APPLICATION_ID: i64 = 0x504C5632;
pub(crate) const VERSION: i64 = 1;
const SQL: &str = include_str!("schema.sql");

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
    tx.execute_batch(SQL)?;
    tx.execute(
        "INSERT INTO library_meta(singleton,library_id) VALUES(1,?1)",
        [library.as_bytes().as_slice()],
    )?;
    tx.execute(
        "INSERT INTO schema_migrations VALUES(1,?1,?2)",
        rusqlite::params![
            migration_checksum(SQL).0.as_slice(),
            chrono::Utc::now().timestamp_millis()
        ],
    )?;
    tx.pragma_update(None, "application_id", APPLICATION_ID)?;
    tx.pragma_update(None, "user_version", VERSION)?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn open(conn: &Connection, library: LibraryId) -> Result<()> {
    // 未知の DB に WAL や DDL を適用する前に識別する。
    let app: i64 = conn.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if app != APPLICATION_ID || version != VERSION {
        return Err(Error::Unsupported(format!(
            "application_id={app}, schema={version}"
        )));
    }
    let (id,checksum): (Vec<u8>,Vec<u8>) = conn.query_row("SELECT library_id,checksum FROM library_meta,schema_migrations WHERE singleton=1 AND version=1",[],|r| Ok((r.get(0)?,r.get(1)?)))?;
    if id != library.as_bytes() || checksum != migration_checksum(SQL).0 {
        return Err(Error::Integrity(
            "ライブラリ ID またはスキーマの記録が一致しません".into(),
        ));
    }
    let (storage,reader,writer):(i64,i64,i64)=conn.query_row("SELECT storage_version,min_reader_version,min_writer_version FROM library_meta WHERE singleton=1",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
    if storage != 1 || reader > 1 || writer > 1 {
        return Err(Error::Unsupported(format!(
            "保存形式={storage}, 最低読取版={reader}, 最低書込版={writer}"
        )));
    }
    configure(conn)
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
            "UPDATE schema_migrations SET checksum=?1",
            [Fingerprint::of(lf.as_bytes()).0.as_slice()],
        )
        .unwrap();
        open(&conn, id).unwrap();
        let changed = migration_checksum(&lf.replace("generation >= 0", "generation >= 1"));
        conn.execute(
            "UPDATE schema_migrations SET checksum=?1",
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
