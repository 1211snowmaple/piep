use super::{
    cover_identity, image_identity, Capture, CaptureSource, CapturedAttachment, CapturedMedia,
    EntityKind, Error, Library, LibraryId, Result, SourceBody,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

mod annotations;
mod catalogue;
mod current;
mod edits;
mod filesystem;
mod inventory;
mod jobs;
mod maintenance;
mod profiles;
mod reader;
mod relations;
mod rows;
mod searches;
mod server_state;
mod state;
mod suggestions;

/// 未解決の記録がある移行先は、完成した棚として切り替えてはならない。
#[derive(Default, Debug, Serialize)]
pub struct LegacyImportReport {
    pub works: usize,
    pub snapshots: usize,
    pub profile_snapshots: usize,
    pub files: u64,
    pub managed_files: u64,
    pub unresolved_files: u64,
    /// 未解決として残った移行記録の数。切り替えの可否と件数の報告にはこれを使う。
    pub unresolved_records: u64,
    /// 未解決の説明。一つの記録に理由と記録名の二行が付くことがあるので、件数には使わない。
    pub unresolved: Vec<String>,
    pub preserved_tables: BTreeMap<String, usize>,
    /// 作り直せるので中身を保管しなかった旧表と、その行数。
    pub rebuildable_tables: BTreeMap<String, usize>,
    /// 段階ごとにかかった時間。走った順。
    pub stages: Vec<StageTime>,
    #[serde(skip)]
    lap: Option<Instant>,
}

#[derive(Debug, Serialize)]
pub struct StageTime {
    pub stage: String,
    pub seconds: f64,
}

impl LegacyImportReport {
    /// 前の区切りからここまでを一つの段階として記録する。
    fn lap(&mut self, stage: &str) {
        let now = Instant::now();
        if let Some(since) = self.lap.replace(now) {
            self.stages.push(StageTime {
                stage: stage.to_owned(),
                seconds: (now - since).as_secs_f64(),
            });
        }
    }
}

/// 保存済みの作品から作り直せる索引の状態と、旧スキーマの版の印。
/// 利用者の判断や取得した内容を含まないので、行を原記録として保管しない。
const REBUILDABLE_TABLES: &[&str] = &[
    "schema_marker",
    "search_index_meta",
    "search_index_state",
    "semantic_index_state",
];

/// アプリの設定を持つ表。棚のデータではないので、行は原記録として保管するが、
/// 正規化の対象にはしない。切り替えのときにアプリの設定として引き継ぐ。
const APPLICATION_SETTING_TABLES: &[&str] = &["search_feature_settings"];

pub struct LegacyImporter {
    root: PathBuf,
    conn: Connection,
    _source_lock: File,
}

struct LegacyVersion<'a> {
    work_id: i64,
    version_id: i64,
    number: i64,
    provider: &'a str,
    key: &'a str,
    json_path: &'a str,
    kind: &'a str,
    acquired_at: Option<i64>,
}

impl LegacyImporter {
    /// 通常のアプリと同じ錠を取り、移行中に旧棚が更新されるのを防ぐ。
    pub fn open(root: &Path) -> Result<Self> {
        super::store::regular_directory(root)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".piep-library.lock"))?;
        match file.try_lock() {
            Ok(()) => (),
            Err(std::fs::TryLockError::WouldBlock) => return Err(Error::Busy),
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
        let conn =
            Connection::open_with_flags(root.join("piep.db"), OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let required = ["downloads", "download_versions", "assets"];
        for table in required {
            let found: i64 = conn.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?1",
                [table],
                |r| r.get(0),
            )?;
            if found != 1 {
                return Err(Error::Unsupported(format!("旧棚の {table} がありません")));
            }
        }
        conn.pragma_update(None, "query_only", true)?;
        Ok(Self {
            root: root.canonicalize()?,
            conn,
            _source_lock: file,
        })
    }

    fn managed_path(&self, stored: &str) -> Result<PathBuf> {
        let path = Path::new(stored);
        // パスの推測は移行器だけ。記録が指すファイルを別の同名ファイルで代用しない。
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        for parent in path.ancestors().skip(1) {
            super::store::regular_directory(parent)?;
            if parent.canonicalize()? == self.root {
                break;
            }
        }
        let resolved = path.canonicalize()?;
        if !resolved.starts_with(&self.root) {
            return Err(Error::UnsafePath(path));
        }
        let relative = resolved
            .strip_prefix(&self.root)
            .map_err(|_| Error::UnsafePath(path.clone()))?;
        if !relative.components().next().is_some_and(|c| {
            matches!(
                c.as_os_str().to_str(),
                Some(name) if filesystem::MANAGED_ROOTS.contains(&name)
            )
        }) {
            return Err(Error::UnsafePath(path));
        }
        Ok(resolved)
    }

    /// 棚を全件読み取る。非対応の正本は原記録を保存し、未解決として報告する。
    pub fn import(
        &self,
        target: &Library,
        cancel: &AtomicBool,
        mut progress: impl FnMut(&LegacyImportReport),
    ) -> Result<LegacyImportReport> {
        if cancel.load(Ordering::Acquire) {
            return Err(Error::Canceled);
        }
        if target.root().starts_with(&self.root) || self.root.starts_with(target.root()) {
            return Err(Error::UnsafePath(target.root().into()));
        }
        self.validate_migration_target(target)?;
        // 同じファイルを作品・資産の照合・全件照合で保管し直す。保管した実体は最後の
        // `verify` で全件読むので、その間は一度確かめた実体を読み直さない。
        let _verified = target.objects.verified_session();
        let mut report = LegacyImportReport {
            lap: Some(Instant::now()),
            ..Default::default()
        };
        self.preserve_tables(target, &mut report)?;
        report.lap("preserve_tables");
        let profiles = self
            .normalize_profiles(target, cancel)
            .map(|(count, anomalies)| {
                report.profile_snapshots = count;
                report.unresolved.extend(anomalies);
            });
        self.finish_normalization(
            target,
            "profiles",
            "作者・シリーズの履歴の正規化",
            profiles,
            &mut report,
        )?;
        let mut stmt = self.conn.prepare(
            "SELECT id,source,source_id,title,author_name,favorite FROM downloads ORDER BY id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, bool>(5)?,
            ))
        })?;
        for row in rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let (old_id, provider, key, title, author, favorite) = row?;
            let date = if self.has_column("download_versions", "created_at")? {
                "created_at"
            } else {
                "NULL"
            };
            let mut versions=self.conn.prepare(&format!("SELECT id,version,coalesce(original_json_path,json_path),change_kind,{date} FROM download_versions WHERE download_id=?1 ORDER BY version,id"))?;
            let editions = versions.query_map([old_id], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                ))
            })?;
            let mut work = None;
            for edition in editions {
                let (version_id, number, json_path, kind, created_at) = edition?;
                let result = created_at
                    .as_deref()
                    .map(rows::parse_time)
                    .transpose()
                    .and_then(|acquired_at| {
                        self.import_version(
                            target,
                            LegacyVersion {
                                work_id: old_id,
                                version_id,
                                number,
                                provider: &provider,
                                key: &key,
                                json_path: &json_path,
                                kind: &kind,
                                acquired_at,
                            },
                        )
                    });
                match result {
                    Ok(saved) => {
                        work = Some(saved.entity_id);
                        report.snapshots += 1;
                    }
                    Err(error) => {
                        report
                            .unresolved
                            .push(format!("作品 {old_id} 版 {number}: {error}"));
                        let conn = target.conn.lock().map_err(|_| Error::Busy)?;
                        conn.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('download_versions',?1,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[format!("{old_id}:{version_id}:{number}")])?;
                    }
                }
            }
            if let Some(work) = work {
                if favorite {
                    let state = target.user_state(work, target.id)?;
                    if state.generation == 0 {
                        target.set_favorite(LibraryId::now_v7(), work, target.id, 0, true)?;
                    } else if !state.favorite {
                        return Err(Error::Integrity(
                            "移行先の利用者状態が変更されています".into(),
                        ));
                    }
                }
                report.works += 1;
            } else {
                report.unresolved.push(format!(
                    "作品 {old_id}: 保存版を移行できませんでした ({provider}:{key})"
                ));
                let conn = target.conn.lock().map_err(|_| Error::Busy)?;
                conn.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('downloads',?1,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[old_id.to_string()])?;
            }
            let _ = (title, author); // 現在値で過去の不明項目を埋めない。
            progress(&report);
        }
        report.lap("works");
        self.finish_normalization(
            target,
            "current_versions",
            "現在の保存版の照合",
            self.normalize_current_versions(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "catalogue",
            "整理・監視データの正規化",
            self.normalize_catalogue(target),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "edits",
            "利用者の編集の正規化",
            self.normalize_edits(target),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "asset_inventory",
            "元ファイルの全件照合",
            self.normalize_asset_inventory(target),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "reader_bindings",
            "本文と画像・添付の対応の移行",
            self.normalize_reader(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "tags",
            "タグの出どころと所属の照合",
            self.normalize_tags(target),
            &mut report,
        )?;
        let relations = self
            .normalize_source_relations(target)
            .map(|anomalies| report.unresolved.extend(anomalies));
        self.finish_normalization(
            target,
            "source_relations",
            "作者・シリーズの所属の照合",
            relations,
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "work_check_state",
            "作品の詳細確認日時の移行",
            self.normalize_work_check_state(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "saved_searches",
            "保存した検索の正規化",
            self.normalize_searches(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "job_history",
            "更新ジョブの履歴と項目・操作記録の移行",
            self.normalize_job_history(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "maintenance",
            "修復試行と旧保存検査の記録の移行",
            self.normalize_maintenance(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "collection_suggestions",
            "コレクション提案と判断の正規化",
            self.normalize_suggestions(target, cancel),
            &mut report,
        )?;
        self.finish_normalization(
            target,
            "server_state",
            "携帯の読書位置とお気に入りの変更の移行",
            self.normalize_server_state(target),
            &mut report,
        )?;
        let inventory = self
            .normalize_filesystem(target, cancel, |files| {
                report.managed_files = files;
                progress(&report);
            })
            .map(|inventory| {
                report.managed_files = inventory.files;
                report.unresolved_files = inventory.unresolved;
            });
        self.finish_normalization(
            target,
            "filesystem",
            "管理領域のファイル全件照合",
            inventory,
            &mut report,
        )?;
        report.files = target.verify()?;
        report.lap("verify");
        target.conn.lock().map_err(|_|Error::Busy)?.execute("DELETE FROM migration_records WHERE source_table='__normalization__' AND source_key='migration'",[])?;
        report.unresolved.extend(target.activation_problems()?);
        report.unresolved_records = target.reader()?.query_row(
            "SELECT count(*) FROM migration_records WHERE status='unresolved'",
            [],
            |r| r.get::<_, i64>(0),
        )? as u64;
        report.unresolved.sort();
        report.unresolved.dedup();
        Ok(report)
    }

    fn finish_normalization(
        &self,
        target: &Library,
        domain: &str,
        label: &str,
        result: Result<()>,
        report: &mut LegacyImportReport,
    ) -> Result<()> {
        if matches!(&result, Err(Error::Canceled)) {
            return Err(Error::Canceled);
        }
        report.lap(domain);
        let conn = target.conn.lock().map_err(|_| Error::Busy)?;
        match result {
            Ok(()) => {
                conn.execute("DELETE FROM migration_records WHERE source_table='__normalization__' AND source_key=?1",[domain])?;
            }
            Err(error) => {
                // 再検査の失敗を永続化する。確定済みの移行を二重適用しないよう、表の完了印とは分ける。
                conn.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('__normalization__',?1,'unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[domain])?;
                report.unresolved.push(format!("{label}: {error}"));
            }
        }
        Ok(())
    }

    fn import_version(
        &self,
        target: &Library,
        version: LegacyVersion<'_>,
    ) -> Result<super::CaptureResult> {
        let LegacyVersion {
            work_id,
            version_id,
            number,
            provider,
            key,
            json_path,
            kind,
            acquired_at,
        } = version;
        let migration_key = format!("{work_id}:{version_id}:{number}");
        let request_id = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let request:Option<Vec<u8>>=conn.query_row("SELECT request_id FROM migration_requests WHERE source_table='download_versions' AND source_key=?1",[&migration_key],|r|r.get(0)).optional()?;
            match request {
                Some(id) => LibraryId::from_slice(&id)
                    .map_err(|_| Error::Integrity("移行要求 ID が不正です".into()))?,
                None => {
                    let id = LibraryId::now_v7();
                    conn.execute(
                        "INSERT INTO migration_requests VALUES('download_versions',?1,?2)",
                        params![migration_key, id.as_bytes().as_slice()],
                    )?;
                    id
                }
            }
        };
        let path = self.managed_path(json_path)?;
        let mut raw = Vec::new();
        File::open(&path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut raw)?;
        if raw.len() > 16 * 1024 * 1024 {
            return Err(Error::Integrity(
                "原本 JSON が移行の読み込み上限を超えています".into(),
            ));
        }
        let data: Value = serde_json::from_slice(&raw)?;
        let detail = data.get("detail").unwrap_or(&data);
        let mut metadata = BTreeMap::new();
        match provider {
            "pixiv" => {
                metadata.insert("content_type".into(), json!("novel"));
            }
            "fanbox" => {
                if let Some(kind) = detail.get("type").and_then(Value::as_str) {
                    metadata.insert("content_type".into(), json!(kind));
                }
            }
            _ => (),
        }
        for name in [
            "title",
            "caption",
            "create_date",
            "series_id",
            "series_title",
            "text_length",
        ] {
            if let Some(value) = detail.get(name) {
                metadata.insert(name.to_owned(), value.clone());
            }
        }
        if let Some(user) = detail.get("user") {
            if let Some(name) = user.get("name") {
                metadata.insert("author_name".into(), name.clone());
            }
            if let Some(id) = user.get("id") {
                metadata.insert("author_key".into(), id.clone());
            }
        }
        if provider == "fanbox" {
            if let Some(creator) = detail.get("creatorId").and_then(Value::as_str) {
                metadata.insert("author_key".into(), json!(creator));
            }
            if let Some(date) = detail.get("publishedDatetime").and_then(Value::as_str) {
                metadata.insert("create_date".into(), json!(date));
            }
            // 一覧の更新日とカードの抜粋。比較の対象ではないので、変わっても改稿にしない。
            if let Some(date) = detail.get("updatedDatetime").and_then(Value::as_str) {
                metadata.insert("update_date".into(), json!(date));
            }
            if let Some(excerpt) = detail.get("excerpt").and_then(Value::as_str) {
                metadata.insert("excerpt".into(), json!(excerpt));
            }
        }
        if let Some(tags) = detail.get("tags").and_then(Value::as_array) {
            let mut names: Vec<_> = tags
                .iter()
                .filter_map(|t| t.as_str().or_else(|| t.get("name").and_then(Value::as_str)))
                .map(str::to_owned)
                .collect();
            names.sort();
            names.dedup();
            metadata.insert("tags".into(), json!(names));
        }
        let body = if provider == "fanbox" {
            data.get("body")
                .filter(|body| !body.is_null())
                .cloned()
                .map(SourceBody::FanboxBody)
        } else {
            data.get("text")
                .or_else(|| detail.get("text"))
                .and_then(Value::as_str)
                .map(|text| {
                    if provider == "pixiv" {
                        SourceBody::PixivText(text.into())
                    } else {
                        SourceBody::PlainText(text.into())
                    }
                })
        };
        let mut states = BTreeMap::new();
        // 旧棚の版は、その版のフォルダーの画像と添付がすべて。前の版にあって消えた画像を
        // 引き継ぐと、旧棚の読書画面にない画像が現在の保存状態に残る。
        states.insert("attachments".into(), "known".into());
        states.insert("media".into(), "known".into());
        if metadata
            .get("caption")
            .is_some_and(|v| v.as_str() == Some(""))
            && metadata
                .get("tags")
                .is_some_and(|v| v.as_array().is_some_and(Vec::is_empty))
        {
            states.insert("caption".into(), "unknown".into());
            states.insert("tags".into(), "unknown".into());
        }
        let mut media = Vec::new();
        let mut attachments = Vec::new();
        let mut included_assets = Vec::new();
        let mut assets=self.conn.prepare("SELECT id,asset_type,filename,local_path,original_url,mime_type,file_size_bytes FROM assets WHERE download_id=?1 ORDER BY id")?;
        for asset in assets.query_map([work_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })? {
            let (asset_id, role, name, stored, url, mime, size) = asset?;
            // 他版の欠損を現在版の欠損と誤認しない。所属を調べてからファイルを開く。
            if role == "editor_image" || !self.asset_in_version(work_id, &stored, json_path)? {
                continue;
            }
            let asset_path = self.managed_path(&stored)?;
            if size < 0 || fs::metadata(&asset_path)?.len() != size as u64 {
                return Err(Error::Integrity(format!(
                    "画像・添付 {asset_id} の容量が記録と違います"
                )));
            }
            if role == "cover" || role == "illustration" {
                let crate::library::ResourceReference::Media { slot } =
                    reader::asset_reference(&role, &name)?
                else {
                    unreachable!()
                };
                let media_key = if role == "cover" {
                    url.as_deref()
                        .or_else(|| data.get("cover_url").and_then(Value::as_str))
                        .or_else(|| detail.get("cover_url").and_then(Value::as_str))
                        .or_else(|| data.get("coverImageUrl").and_then(Value::as_str))
                        .map(|url| cover_identity(provider, url))
                } else {
                    url.as_deref().map(|url| image_identity(provider, url))
                };
                let quality = if url.as_deref().is_some_and(|u| u.contains("/c/")) {
                    "thumbnail"
                } else if url.as_deref().is_some_and(|u| u.contains("-original/")) {
                    "original"
                } else {
                    "unknown"
                };
                included_assets.push((asset_id, slot.clone()));
                media.push(CapturedMedia {
                    slot,
                    role: if role == "cover" {
                        "cover"
                    } else {
                        "illustration"
                    }
                    .into(),
                    source_key: media_key,
                    source_url: url,
                    quality: quality.into(),
                    original_name: name,
                    mime_type: mime.unwrap_or_else(|| "application/octet-stream".into()),
                    bytes: Some(CaptureSource::File(asset_path)),
                });
            } else {
                let crate::library::ResourceReference::File { slot } =
                    reader::asset_reference(&role, &name)?
                else {
                    unreachable!()
                };
                included_assets.push((asset_id, slot.clone()));
                attachments.push(CapturedAttachment {
                    slot,
                    original_name: name,
                    mime_type: mime,
                    bytes: CaptureSource::File(asset_path),
                });
            }
        }
        let saved = target.capture_with_legacy_classification(
            Capture {
                acquired_at,
                request_id,
                provider: provider.into(),
                external_key: key.into(),
                kind: EntityKind::Work,
                route: "legacy-import".into(),
                raw: CaptureSource::File(path.clone()),
                metadata,
                field_states: states,
                body,
                media,
                attachments,
            },
            Some(kind),
        )?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        rows::mark_row(&tx, "download_versions", &migration_key, saved.snapshot_id)?;
        let raw: Vec<u8> = tx.query_row(
            "SELECT raw_file_id FROM acquisition_observations WHERE request_id=?1",
            [request_id.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        self.mark_used_file(&tx, &path, &raw)?;
        for (asset, slot) in included_assets {
            rows::mark_row(
                &tx,
                "asset_placements",
                &asset.to_string(),
                saved.snapshot_id,
            )?;
            let file:Vec<u8>=tx.query_row("SELECT file_id FROM snapshot_files WHERE snapshot_id=?1 AND slot=?2 UNION ALL SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 AND m.slot=?2",params![saved.snapshot_id.as_bytes().as_slice(),slot],|r|r.get(0))?;
            tx.execute("UPDATE migration_records SET evidence_file_id=?2 WHERE source_table='asset_placements' AND source_key=?1",params![asset.to_string(),file])?;
        }
        tx.commit()?;
        Ok(saved)
    }

    fn preserve_tables(&self, target: &Library, report: &mut LegacyImportReport) -> Result<()> {
        let mut tables=self.conn.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name")?;
        for table in tables.query_map([], |r| r.get::<_, String>(0))? {
            let table = table?;
            if !table
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                return Err(Error::Unsupported("旧テーブル名が不正です".into()));
            }
            if REBUILDABLE_TABLES.contains(&table.as_str()) {
                let count =
                    self.conn
                        .query_row(&format!("SELECT count(*) FROM \"{table}\""), [], |r| {
                            r.get::<_, i64>(0)
                        })? as usize;
                // 索引の進み具合は旧アプリを使うたびに変わる。行数や中身を証跡に入れず、
                // 保管しなかった理由だけを残す。
                let evidence = target.store_bytes(&serde_json::to_vec(&json!({
                    "table": table,
                    "reason": "rebuildable_index_state",
                    "contents_preserved": false
                }))?)?;
                let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let tx = conn.transaction()?;
                Library::register_file(&tx, &evidence)?;
                tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('__normalization__','migration','unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[])?;
                tx.execute("INSERT INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES(?1,'__all_rows__',?2,'imported') ON CONFLICT(source_table,source_key) DO UPDATE SET evidence_file_id=excluded.evidence_file_id,status='imported'",params![table,evidence.file.id.0.as_slice()])?;
                tx.commit()?;
                report.rebuildable_tables.insert(table, count);
                continue;
            }
            let mut statement = self.conn.prepare(&format!("SELECT * FROM \"{table}\""))?;
            let columns: Vec<_> = statement
                .column_names()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut rows = statement.query([])?;
            let mut count = 0;
            let path = target
                .root()
                .join(format!(".legacy-{table}-{}.jsonl", LibraryId::now_v7()));
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)?;
            use std::io::Write;
            while let Some(row) = rows.next()? {
                let mut values = BTreeMap::new();
                for (i, name) in columns.iter().enumerate() {
                    use rusqlite::types::ValueRef;
                    let value = match row.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(v) => json!(v),
                        ValueRef::Real(v) => json!({"sqlite_real_bits":v.to_bits()}),
                        ValueRef::Text(v) => json!({"sqlite_text_bytes":v}),
                        ValueRef::Blob(v) => json!({"sqlite_blob_bytes":v}),
                    };
                    values.insert(name, value);
                }
                serde_json::to_writer(&mut output, &values)?;
                output.write_all(b"\n")?;
                count += 1;
            }
            output.sync_all()?;
            drop(output);
            let prepared = target.store_path(&path)?;
            let file = prepared.file.clone();
            {
                let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                let tx = conn.transaction()?;
                Library::register_file(&tx, &prepared)?;
                let old:Option<Vec<u8>>=tx.query_row("SELECT evidence_file_id FROM migration_records WHERE source_table=?1 AND source_key='__all_rows__'",[&table],|r|r.get(0)).optional()?;
                if old.is_some_and(|old| old != file.id.0) {
                    return Err(Error::Integrity(format!(
                        "旧棚の {table} が前回の移行から変更されています"
                    )));
                }
                // 最初の原記録と未完了の印を同時に確定する。正規化開始前に落ちても完成に見せない。
                tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('__normalization__','migration','unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[])?;
                tx.execute("INSERT OR IGNORE INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES(?1,'__all_rows__',?2,?3)",params![table,file.id.0.as_slice(),if count==0 || matches!(table.as_str(),"downloads"|"download_versions") || APPLICATION_SETTING_TABLES.contains(&table.as_str()) { "imported" } else { "unresolved" }])?;
                tx.commit()?;
            }
            fs::remove_file(path)?;
            report.preserved_tables.insert(table.clone(), count);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_import_keeps_all_source_rows_and_retries_without_duplicate_revisions() {
        let temp = std::env::temp_dir().join(format!("piep-legacy-test-{}", LibraryId::now_v7()));
        let old = temp.join("old");
        let next = temp.join("next");
        let version = old.join("downloads/pixiv/123/v1");
        fs::create_dir_all(version.join("data_assets/cover")).unwrap();
        let raw = version.join("original.json");
        let image = version.join("data_assets/cover/cover.png");
        let raw_bytes=b"{ \"detail\":{\"id\":123,\"title\":\"title\",\"tags\":[{\"name\":\"tag\"}]},\"text\":\"original body\" }\r\n";
        fs::write(&raw, raw_bytes).unwrap();
        fs::write(&image, b"untouched image metadata").unwrap();
        let db = Connection::open(old.join("piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER); CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT); CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER); CREATE TABLE user_decisions(id INTEGER,note BLOB);").unwrap();
        db.execute(
            "INSERT INTO downloads VALUES(1,'pixiv','123','title','author',1)",
            [],
        )
        .unwrap();
        db.execute(
            "INSERT INTO download_versions VALUES(1,1,1,?1,?1,'source')",
            [raw.to_str().unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO assets VALUES(1,1,'cover','cover.png',?1,NULL,'image/png',24)",
            [image.to_str().unwrap()],
        )
        .unwrap();
        db.execute(
            "INSERT INTO user_decisions VALUES(1,?1)",
            [b"must remain\0exact".as_slice()],
        )
        .unwrap();
        drop(db);
        {
            let source = LegacyImporter::open(&old).unwrap();
            let target = Library::create(&next).unwrap();
            let first = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(first.works, 1);
            assert_eq!(first.snapshots, 1);
            assert_eq!(first.preserved_tables["user_decisions"], 1);
            assert!(first
                .unresolved
                .iter()
                .any(|s| s.contains("user_decisions")));
            let second = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(second.snapshots, 1);
            let work = &target.works(10).unwrap()[0];
            assert_eq!(target.snapshots(work.id).unwrap().len(), 1);
            assert!(target.user_state(work.id, target.id).unwrap().favorite);
            let file = target
                .media_file(work.id, work.snapshot_id, "cover")
                .unwrap();
            let mut image_bytes = Vec::new();
            target.objects.export(&file, &mut image_bytes).unwrap();
            assert_eq!(image_bytes, b"untouched image metadata");
            assert_eq!(fs::read(&raw).unwrap(), raw_bytes);
            target.verify().unwrap();
        }
        fs::remove_dir_all(temp).unwrap();
    }

    #[test]
    fn images_dropped_by_a_later_version_are_not_carried_into_the_current_state() {
        let temp = std::env::temp_dir().join(format!("piep-legacy-media-{}", LibraryId::now_v7()));
        let old = temp.join("old");
        let db_path = old.join("downloads");
        fs::create_dir_all(&db_path).unwrap();
        let db = Connection::open(old.join("piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER); CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT); CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
INSERT INTO downloads VALUES(1,'pixiv','123','title','author',0);").unwrap();
        // 一版目は挿絵 2 枚、二版目は 1 枚。旧棚の読書画面は今の版のフォルダーの画像だけを出す。
        let mut asset = 0;
        for (number, text, images) in [
            (1, "first", vec!["a.png", "b.png"]),
            (2, "second", vec!["b.png"]),
        ] {
            let version = old.join(format!("downloads/pixiv/123/v{number}"));
            fs::create_dir_all(version.join("data_assets/illustrations")).unwrap();
            let raw = version.join("original.json");
            fs::write(
                &raw,
                serde_json::to_vec(&json!({"detail":{"id":123,"title":"title"},"text":text}))
                    .unwrap(),
            )
            .unwrap();
            db.execute(
                "INSERT INTO download_versions VALUES(?1,1,?1,?2,?2,'source')",
                params![number, raw.to_str().unwrap()],
            )
            .unwrap();
            for name in images {
                asset += 1;
                let path = version.join("data_assets/illustrations").join(name);
                let bytes = format!("{name} in v{number}");
                fs::write(&path, &bytes).unwrap();
                db.execute(
                    "INSERT INTO assets VALUES(?1,1,'illustration',?2,?3,NULL,'image/png',?4)",
                    params![asset, name, path.to_str().unwrap(), bytes.len() as i64],
                )
                .unwrap();
            }
        }
        drop(db);
        let target = Library::create(&temp.join("next")).unwrap();
        let report = LegacyImporter::open(&old)
            .unwrap()
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(report.unresolved_records, 0, "{:?}", report.unresolved);
        let work = target.works(10).unwrap()[0].id;
        let card = target.work_card(work).unwrap();
        assert_eq!((card.version_number, card.media_count), (2, 1));
        let counts: Vec<i64> = target
            .reader()
            .unwrap()
            .prepare("SELECT (SELECT count(*) FROM snapshot_media m WHERE m.snapshot_id=s.id) FROM revision_snapshots s WHERE s.entity_id=?1 ORDER BY s.sequence")
            .unwrap()
            .query_map([work.as_bytes().as_slice()], |r| r.get(0))
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(counts, vec![2, 1]);
        target.verify().unwrap();
        drop(target);
        fs::remove_dir_all(temp).unwrap();
    }
}
