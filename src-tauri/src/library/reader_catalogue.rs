use super::{
    protocol::{Id, ReaderWork, WorkReadingView},
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

    /// 携帯の一覧は、デスクトップと同じ保存日と作品 ID の索引から、必要な列だけ引く。
    pub fn reader_works(
        &self,
        subject: LibraryId,
        limit: u8,
        after: Option<(i64, LibraryId)>,
    ) -> Result<Vec<(ReaderWork, i64)>> {
        let conn = self.reader()?;
        let boundary = if after.is_some() {
            "WHERE (p.saved_at,p.work_id)<(?3,?4)"
        } else {
            ""
        };
        let mut statement = conn.prepare_cached(&format!(
            "SELECT p.work_id,p.snapshot_id,d.active_id,p.provider,p.external_key,p.title,
                    p.author_name,p.content_type,p.saved_at
             FROM work_current_projection p
             LEFT JOIN edit_documents d ON d.work_id=p.work_id AND d.user_id=?1
             {boundary} ORDER BY p.saved_at DESC,p.work_id DESC LIMIT ?2",
        ))?;
        let user = subject.as_bytes().as_slice();
        let mut rows = match after {
            Some((time, work)) => {
                statement.query(params![user, limit, time, work.as_bytes().as_slice()])?
            }
            None => statement.query(params![user, limit])?,
        };
        let mut items = Vec::new();
        while let Some(row) = rows.next()? {
            items.push((
                ReaderWork {
                    view: view(row)?,
                    provider: row.get(3)?,
                    key: row.get(4)?,
                    title: row.get(5)?,
                    author_name: row.get(6)?,
                    content_type: row.get(7)?,
                },
                row.get(8)?,
            ));
        }
        Ok(items)
    }
}
