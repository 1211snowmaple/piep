use super::{
    protocol::{Id, WorkReadingView},
    Error, Library, LibraryId, Result,
};
use rusqlite::{params, Row};

fn view(row: &Row<'_>) -> Result<WorkReadingView> {
    let id = |bytes: Vec<u8>| {
        let uuid = LibraryId::from_slice(&bytes)
            .map_err(|_| Error::Integrity("作品の識別子が不正です".into()))?;
        Ok::<_, Error>(Id::new(uuid)?)
    };
    Ok(WorkReadingView {
        entity_id: id(row.get(0)?)?,
        snapshot_id: id(row.get(1)?)?,
        edit_id: row.get::<_, Option<Vec<u8>>>(2)?.map(id).transpose()?,
    })
}

impl Library {
    /// 取得元の別名を含め、棚に並ぶ作品の現在の読書用の版を一度の読み取りで引く。
    pub fn reader_work_by_key(
        &self,
        subject: LibraryId,
        provider: &str,
        key: &str,
    ) -> Result<Option<WorkReadingView>> {
        let conn = self.reader()?;
        let mut statement = conn.prepare_cached(
            "SELECT p.work_id,p.snapshot_id,d.active_id
             FROM external_identities x
             JOIN work_current_projection p ON p.work_id=x.entity_id
             LEFT JOIN edit_documents d ON d.work_id=p.work_id AND d.user_id=?1
             WHERE x.provider=?2 AND x.kind='work' AND x.external_key=?3",
        )?;
        let mut rows = statement.query(params![subject.as_bytes().as_slice(), provider, key])?;
        rows.next()?.map(view).transpose()
    }
}
