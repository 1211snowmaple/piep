//! 使われなくなった実体の回収。
//!
//! 台帳のどの行からも参照されないファイルを台帳から外し、その部分実体を隔離し、隔離から
//! 一定の日数が過ぎても参照されないものだけを消す。参照は「content_files を指す外部キーの
//! ある列すべて」で数える（表を足しても数え漏れない）。作品を棚から外しても、作品の識別と
//! 保存版は残るので、その実体は回収されない。
//!
//! 保存は実体を置いて（使い回して）から台帳に確定する。その間の実体を未参照と見て退けないよう、
//! 実体領域は使い回すたびに更新時刻を今にし、回収は最後に使われてから `grace` が過ぎた実体
//! だけを退ける。退けた実体が後で参照されていれば、次の回収で戻す。参照されている実体が
//! 実体領域にも隔離にもなければ、何も回収せずに失敗する。

use super::{repository::Library, Error, Fingerprint, LibraryId, Result};
use piep_storage::{StoredEntry, StoredListing};
use rusqlite::params;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// 回収の猶予。
#[derive(Clone, Copy, Debug)]
pub struct ReclaimOptions {
    /// 未参照でも、最後に使われてからこれだけ経っていない実体は退けない。
    pub grace: Duration,
    /// 隔離してからこれだけ経った実体（と、壊れて退けた実体）を消す。
    pub hold: Duration,
}

impl Default for ReclaimOptions {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(24 * 60 * 60),
            hold: Duration::from_secs(7 * 24 * 60 * 60),
        }
    }
}

/// 一回の回収で起きたこと。
#[derive(Clone, Debug, Default, Serialize, PartialEq, Eq)]
pub struct ReclaimReport {
    /// 台帳から外した、どこからも参照されないファイル。
    pub released_files: usize,
    /// 隔離した部分実体と、その保存の大きさ。
    pub quarantined_chunks: usize,
    pub quarantined_bytes: u64,
    /// 未参照だが、使われてから猶予が過ぎていないので残した部分実体。
    pub recent_chunks: usize,
    /// 隔離から猶予が過ぎて消した部分実体と、その大きさ。
    pub deleted_chunks: usize,
    pub deleted_bytes: u64,
    /// 隔離していたが参照されていたので戻した部分実体。
    pub restored_chunks: usize,
    /// 書き込み途中で残った一時ファイル・壊れて退けた実体のうち、消したもの。
    pub removed_parts: usize,
    pub removed_damaged: usize,
}

fn millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

fn fingerprint(bytes: Vec<u8>) -> Result<Fingerprint> {
    Ok(Fingerprint(bytes.try_into().map_err(|_| {
        Error::Integrity("ファイル ID が不正です".into())
    })?))
}

impl Library {
    /// 使われなくなった実体を回収する。バックアップ・検証と同時には走らない。
    pub fn reclaim(&self, options: ReclaimOptions) -> Result<ReclaimReport> {
        self.reclaim_at(options, SystemTime::now())
    }

    pub(crate) fn reclaim_at(
        &self,
        options: ReclaimOptions,
        now: SystemTime,
    ) -> Result<ReclaimReport> {
        let _turn = self.storage_gate.lock().map_err(|_| Error::Busy)?;
        let operation = LibraryId::now_v7();
        {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            conn.execute(
                "INSERT INTO storage_operations(id,kind,state,payload_json,created_at) VALUES(?1,'reclaim','prepared','{}',?2)",
                params![operation.as_bytes().as_slice(), millis(now)],
            )?;
        }
        let result = self.reclaim_inner(options, now);
        let (state, payload) = match &result {
            Ok(report) => ("complete", serde_json::to_string(report)?),
            Err(error) => (
                "failed",
                serde_json::json!({ "error": error.to_string() }).to_string(),
            ),
        };
        let conn = self.conn.lock().map_err(|_| Error::Busy)?;
        conn.execute(
            "UPDATE storage_operations SET state=?2,payload_json=?3 WHERE id=?1",
            params![operation.as_bytes().as_slice(), state, payload],
        )?;
        result
    }

    fn reclaim_inner(&self, options: ReclaimOptions, now: SystemTime) -> Result<ReclaimReport> {
        let mut report = ReclaimReport::default();
        let listing = self.objects.listing()?;
        let mut objects = HashSet::new();
        let mut quarantined: HashMap<Fingerprint, Vec<&StoredListing>> = HashMap::new();
        for item in &listing {
            match item.entry {
                StoredEntry::Object(id) => {
                    objects.insert(id);
                }
                StoredEntry::Quarantined { id, .. } => {
                    quarantined.entry(id).or_default().push(item)
                }
                _ => {}
            }
        }

        // 台帳の側：参照されないファイルを外し、残る部分実体を数える。参照されている部分
        // 実体が実体領域にも隔離にもなければ、外す前に失敗する（何も回収しない）。
        let referenced: HashSet<Fingerprint> = {
            let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
            let tx = conn.transaction()?;
            let mut columns = Vec::new();
            {
                let mut tables = tx.prepare(
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'",
                )?;
                let names = tables
                    .query_map([], |r| r.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                for table in names {
                    let mut keys =
                        tx.prepare("SELECT \"table\",\"from\" FROM pragma_foreign_key_list(?1)")?;
                    for key in keys.query_map([&table], |r| {
                        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                    })? {
                        let (parent, column) = key?;
                        if parent == "content_files"
                            && !(table == "file_chunks" && column == "file_id")
                        {
                            columns.push((table.clone(), column));
                        }
                    }
                }
            }
            if columns.is_empty() {
                return Err(Error::Integrity(
                    "ファイルを参照する列が見つかりません".into(),
                ));
            }
            let union = columns
                .iter()
                .map(|(table, column)| {
                    format!("SELECT \"{column}\" FROM \"{table}\" WHERE \"{column}\" IS NOT NULL")
                })
                .collect::<Vec<_>>()
                .join(" UNION ");
            tx.execute_batch(&format!(
                "CREATE TEMP TABLE reclaim_unused AS SELECT id FROM content_files WHERE id NOT IN ({union});"
            ))?;
            let kept = {
                let mut statement = tx.prepare(
                    "SELECT DISTINCT chunk_id FROM file_chunks WHERE file_id NOT IN (SELECT id FROM reclaim_unused)",
                )?;
                let rows = statement
                    .query_map([], |r| r.get::<_, Vec<u8>>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                rows.into_iter()
                    .map(fingerprint)
                    .collect::<Result<HashSet<_>>>()?
            };
            for id in &kept {
                if !objects.contains(id)
                    && !quarantined.contains_key(id)
                    && !self.objects.has_object(*id)?
                {
                    return Err(Error::Integrity(format!(
                        "参照されている部分実体 {id} が見つからないので、回収しません"
                    )));
                }
            }
            report.released_files =
                tx.query_row("SELECT count(*) FROM reclaim_unused", [], |r| {
                    r.get::<_, i64>(0)
                })? as usize;
            // 確定したファイルの構成は書き換えられない（トリガー）。親を先に外し、外部キーの
            // 検査は確定の時まで延ばす。
            tx.execute_batch(
                "PRAGMA defer_foreign_keys=ON;
                 DELETE FROM content_files WHERE id IN (SELECT id FROM reclaim_unused);
                 DELETE FROM file_chunks WHERE file_id IN (SELECT id FROM reclaim_unused);
                 DELETE FROM chunk_storage WHERE chunk_id NOT IN (SELECT chunk_id FROM file_chunks);
                 DELETE FROM chunks WHERE id NOT IN (SELECT chunk_id FROM file_chunks);
                 DROP TABLE temp.reclaim_unused;",
            )?;
            tx.commit()?;
            kept
        };

        // 参照されているのに隔離していたものを戻す。
        for (id, items) in &quarantined {
            if referenced.contains(id) {
                for item in items {
                    if self.objects.restore_quarantined(item)? {
                        report.restored_chunks += 1;
                    }
                }
            }
        }

        // 実体の側：参照されない実体を隔離し、隔離や壊れから日数の過ぎたものを消す。
        let unused_since = now.checked_sub(options.grace).unwrap_or(UNIX_EPOCH);
        let expired = now.checked_sub(options.hold).unwrap_or(UNIX_EPOCH);
        for item in &listing {
            match item.entry {
                StoredEntry::Object(id) if !referenced.contains(&id) => {
                    if self.objects.quarantine(id, unused_since, millis(now))? {
                        report.quarantined_chunks += 1;
                        report.quarantined_bytes += item.bytes;
                    } else {
                        report.recent_chunks += 1;
                    }
                }
                StoredEntry::Quarantined { id, at_ms }
                    if !referenced.contains(&id) && at_ms <= millis(expired) =>
                {
                    self.objects.remove_listed(item)?;
                    report.deleted_chunks += 1;
                    report.deleted_bytes += item.bytes;
                }
                StoredEntry::Part if item.modified <= unused_since => {
                    self.objects.remove_listed(item)?;
                    report.removed_parts += 1;
                }
                StoredEntry::Damaged if item.modified <= expired => {
                    self.objects.remove_listed(item)?;
                    report.removed_damaged += 1;
                }
                _ => {}
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{Capture, EntityKind};
    use serde_json::json;
    use std::collections::BTreeMap;

    struct Temp(std::path::PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn capture(key: &str, body: &str) -> Capture {
        Capture {
            acquired_at: Some(1_000),
            request_id: LibraryId::now_v7(),
            provider: "pixiv".into(),
            external_key: key.into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: serde_json::to_vec(&json!({ "key": key })).unwrap().into(),
            metadata: BTreeMap::from([("title".into(), json!(key))]),
            field_states: BTreeMap::new(),
            body: Some(body.into()),
            media: vec![],
            attachments: vec![],
        }
    }

    fn files(library: &Library) -> i64 {
        library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM content_files", [], |r| r.get(0))
            .unwrap()
    }

    #[test]
    fn unused_files_wait_out_the_grace_and_the_hold_while_saved_works_and_removed_works_stay_readable(
    ) {
        let temp = Temp(std::env::temp_dir().join(format!("piep-reclaim-{}", LibraryId::now_v7())));
        let library = Library::create(&temp.0).unwrap();
        let kept = library.capture(capture("1", "kept body")).unwrap();
        let removed = library.capture(capture("2", "removed body")).unwrap();
        let generation = library
            .library_item_generation(removed.entity_id)
            .unwrap()
            .unwrap();
        library
            .remove_work(LibraryId::now_v7(), removed.entity_id, generation)
            .unwrap();
        // 台帳に置いたが、どこからも参照されないファイル（追いつきで差し替えた原記録など）と、
        // 台帳にない実体（保存の確定の前に落ちた跡）。
        let orphan = library.store_bytes(b"replaced evidence").unwrap();
        {
            let mut conn = library.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            Library::register_file(&tx, &orphan).unwrap();
            tx.commit().unwrap();
        }
        let crashed = library.objects.put(&b"stored before a crash"[..]).unwrap();
        let before = files(&library);
        let options = ReclaimOptions::default();
        let now = SystemTime::now();

        // 使ったばかりの実体は、台帳から外しても退けない。
        let first = library.reclaim_at(options, now).unwrap();
        assert_eq!(first.released_files, 1);
        assert_eq!(first.quarantined_chunks, 0);
        assert_eq!(first.recent_chunks, 2);
        assert_eq!(files(&library), before - 1);
        assert!(library.objects.verify(&crashed).is_ok());

        // 猶予が過ぎると隔離する。
        let later = now + options.grace + Duration::from_secs(60);
        let second = library.reclaim_at(options, later).unwrap();
        assert_eq!((second.quarantined_chunks, second.deleted_chunks), (2, 0));
        assert!(library.objects.verify(&crashed).is_err());

        // 隔離から日数が過ぎたら消す。
        let third = library
            .reclaim_at(options, later + options.hold + Duration::from_secs(60))
            .unwrap();
        assert_eq!((third.restored_chunks, third.deleted_chunks), (0, 2));
        assert!(library
            .objects
            .listing()
            .unwrap()
            .iter()
            .all(|item| matches!(item.entry, StoredEntry::Object(_))));
        // 消した中身をもう一度保存すれば、置き直されて読める。
        library.objects.put(&b"stored before a crash"[..]).unwrap();
        library.objects.verify(&crashed).unwrap();

        // 残した作品も、棚から外した作品も読める。
        for work in [kept, removed] {
            let mut body = Vec::new();
            library
                .read_body(work.entity_id, work.snapshot_id, &mut body)
                .unwrap();
            assert!(!body.is_empty());
        }
        library.verify().unwrap();
        let operations: Vec<String> = library
            .reader()
            .unwrap()
            .prepare(
                "SELECT state FROM storage_operations WHERE kind='reclaim' ORDER BY created_at",
            )
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(operations, vec!["complete"; 3]);
    }

    #[test]
    fn a_referenced_chunk_found_in_quarantine_is_put_back_and_a_lost_one_stops_the_reclaim() {
        let temp = Temp(std::env::temp_dir().join(format!("piep-reclaim-{}", LibraryId::now_v7())));
        let library = Library::create(&temp.0).unwrap();
        let work = library.capture(capture("1", "body")).unwrap();
        let file = library
            .reader()
            .unwrap()
            .query_row(
                "SELECT body_file_id FROM revision_snapshots WHERE id=?1",
                [work.snapshot_id.as_bytes().as_slice()],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .unwrap();
        let file = Library::file_from_conn(&library.reader().unwrap(), fingerprint(file).unwrap())
            .unwrap();
        let chunk = file.chunks[0].id;
        // 参照されている実体が隔離されていた（回収と保存が行き違った跡）。
        let far = SystemTime::now() + Duration::from_secs(10 * 24 * 60 * 60);
        assert!(library.objects.quarantine(chunk, far, 1).unwrap());
        assert!(library.objects.verify(&file).is_err());
        let report = library.reclaim(ReclaimOptions::default()).unwrap();
        assert_eq!(report.restored_chunks, 1);
        library.objects.verify(&file).unwrap();

        // 実体がどこにもなければ、何も外さずに失敗する。
        let orphan = library.store_bytes(b"unused").unwrap();
        {
            let mut conn = library.conn.lock().unwrap();
            let tx = conn.transaction().unwrap();
            Library::register_file(&tx, &orphan).unwrap();
            tx.commit().unwrap();
        }
        let before = files(&library);
        assert!(library.objects.quarantine(chunk, far, 2).unwrap());
        for item in library.objects.listing().unwrap() {
            if matches!(item.entry, StoredEntry::Quarantined { id, .. } if id == chunk) {
                library.objects.remove_listed(&item).unwrap();
            }
        }
        assert!(library.reclaim(ReclaimOptions::default()).is_err());
        assert_eq!(files(&library), before);
        let state: String = library
            .reader()
            .unwrap()
            .query_row(
                "SELECT state FROM storage_operations ORDER BY created_at DESC, rowid DESC LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "failed");
    }
}
