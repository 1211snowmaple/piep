use super::{
    rows::{mapped_id, mark_row, mark_table, marked},
    LegacyImporter,
};
use crate::library::{Error, Library, Result, SavedSearchDefinition};
use rusqlite::params;
use std::sync::atomic::{AtomicBool, Ordering};

impl LegacyImporter {
    pub(super) fn normalize_searches(&self, target: &Library, cancel: &AtomicBool) -> Result<()> {
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        if marked(&tx, "saved_searches")? {
            return Ok(());
        }
        let mut rows = Vec::new();
        self.each_row("saved_searches", |row| {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let definition = SavedSearchDefinition {
                name: row.text("name")?.into(),
                query: row.optional_text("query")?.map(str::to_owned),
                params_json: row.text("params_json")?.into(),
            };
            definition.validate()?;
            rows.push((
                row.text("created_at")?.to_owned(),
                row.integer("id")?,
                row.time("created_at")?,
                row.time("updated_at")?,
                definition,
            ));
            Ok(())
        })?;
        // 旧画面の created_at, id の順序を固定し、同時刻の検索を入れ替えない。
        rows.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
        for (position, (_, old, created, updated, definition)) in rows.into_iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let key = old.to_string();
            let id = mapped_id(&tx, "saved_searches", &key)?;
            tx.execute("INSERT INTO saved_searches(id,user_id,name,query,params_json,definition_version,position,created_at,updated_at,generation) VALUES(?1,?2,?3,?4,?5,1,?6,?7,?8,1)",params![id.as_bytes().as_slice(),target.id.as_bytes().as_slice(),definition.name,definition.query,definition.params_json,position as i64,created,updated])?;
            mark_row(&tx, "saved_searches", &key, id)?;
        }
        mark_table(&tx, "saved_searches")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "searches_tests.rs"]
mod tests;
