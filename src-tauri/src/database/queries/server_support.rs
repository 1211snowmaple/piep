//! サーバーモードが本棚に問うこと。写しの書き出し、参照先の一覧、作品ごとの並び順。

use super::*;

impl Database {
    /// 確定済みの内容を 1 つのファイルに書き出す。WAL の中身も含まれる。
    ///
    /// 読み取り用の接続は `query_only` なので、このためだけに読み取り専用で開き直す。
    pub fn write_vacuumed_copy(&self, target: &Path) -> Result<(), String> {
        let source = {
            let conn = self.read_conn()?;
            conn.path()
                .map(str::to_string)
                .ok_or("本棚の DB の場所が分かりません")?
        };
        let conn = rusqlite::Connection::open_with_flags(
            &source,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|error| format!("本棚の DB を開けません: {error}"))?;
        conn.busy_timeout(std::time::Duration::from_secs(30))
            .map_err(|error| format!("本棚の DB を開けません: {error}"))?;
        conn.execute("VACUUM INTO ?1", [target.to_string_lossy().as_ref()])
            .map_err(|error| format!("本棚の写しを作れません: {error}"))?;
        Ok(())
    }

    /// 作品が入っているコレクションと、その中での位置。
    pub fn list_collection_positions_for_work(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Vec<(String, i64)>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare("SELECT collection_id, position FROM work_collection_members WHERE source = ?1 AND source_id = ?2")
            .map_err(|error| format!("コレクションの位置を読めません: {error}"))?;
        let rows = stmt
            .query_map([source, source_id], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|error| format!("コレクションの位置を読めません: {error}"))?;
        rows.collect::<Result<_, _>>()
            .map_err(|error| format!("コレクションの位置を読めません: {error}"))
    }

    /// 本棚の DB が指しているファイルのパス。サーバーで欠けを数えるのに使う。
    pub fn server_referenced_paths(&self) -> Result<Vec<String>, String> {
        let conn = self.read_conn()?;
        let mut paths = Vec::new();
        for (table, column) in crate::server::snapshot::PATH_COLUMNS {
            let mut stmt = conn
                .prepare(&format!("SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL AND \"{column}\" <> ''"))
                .map_err(|error| format!("{table}.{column} を読めません: {error}"))?;
            let rows = stmt
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| format!("{table}.{column} を読めません: {error}"))?;
            for row in rows {
                paths.push(row.map_err(|error| format!("{table}.{column} を読めません: {error}"))?);
            }
        }
        Ok(paths)
    }
}
