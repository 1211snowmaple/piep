use super::{Error, Fingerprint, Library, Result};
use rusqlite::Connection;

pub(super) const MAX_JOB_PAYLOAD_BYTES: u64 = 16 * 1024 * 1024;

impl Library {
    pub(crate) fn read_job_payload(&self, conn: &Connection, hash: Fingerprint) -> Result<String> {
        let file = Self::file_from_conn(conn, hash)?;
        if file.length > MAX_JOB_PAYLOAD_BYTES {
            return Err(Error::Unsupported(
                "ジョブの原記録が読み込み上限を超えています".into(),
            ));
        }
        let mut bytes = Vec::new();
        self.objects.export(&file, &mut bytes)?;
        let text = String::from_utf8(bytes)
            .map_err(|_| Error::Integrity("ジョブの原記録が UTF-8 ではありません".into()))?;
        let _: &serde_json::value::RawValue = serde_json::from_str(&text)?;
        Ok(text)
    }

    pub(super) fn verify_execution_payloads(&self, conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("SELECT DISTINCT payload_file_id FROM execution_items")?;
        for hash in stmt.query_map([], |r| r.get::<_, Vec<u8>>(0))? {
            let hash = Fingerprint(
                hash?
                    .try_into()
                    .map_err(|_| Error::Integrity("実行項目の原本識別子が不正です".into()))?,
            );
            self.read_job_payload(conn, hash)?;
        }
        Ok(())
    }
}
