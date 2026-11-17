use super::repository::REQUEST_REUSED;
use super::{Capture, CaptureSource, EntityKind, Error, Library, LibraryId, Result};
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
    /// 前回の移行から行が変わっていた旧表（追いつきのとき）。
    pub changed_tables: Vec<String>,
    /// 前回から変わっていないので読み直さなかった保存版（`snapshots` にも数える）。
    pub unchanged_snapshots: usize,
    /// 前に移した古い版（旧棚の今の版でないもの）が旧棚で変わっていたが、履歴の途中に
    /// 新しい版を差し込めないので前の保存状態のままにした数（`snapshots` にも数える）。
    pub kept_old_snapshots: usize,
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
    /// 旧棚でその作品の最後の版（旧棚の今の版）か。
    latest: bool,
}

/// 追いつきで一つの保存版をどう扱ったか。
#[derive(Clone, Copy, PartialEq, Eq)]
enum VersionRead {
    /// 読んで保存した（初めての移行、または旧棚で変わった今の版）。
    Read,
    /// 前回から変わっていないので読み直さなかった。
    Unchanged,
    /// 前に移した古い版が旧棚で変わっていた。履歴の途中を書き換えないので前のまま。
    KeptOld,
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

    /// 版の画像・添付（その版に属するもの。編集画面の画像は除く）。容量が記録と違えば誤り。
    fn version_assets(
        &self,
        work_id: i64,
        json_path: &str,
    ) -> Result<Vec<(i64, super::work_capture::StagedAsset)>> {
        let mut out = Vec::new();
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
            out.push((
                asset_id,
                super::work_capture::StagedAsset {
                    role,
                    name,
                    path: asset_path,
                    url,
                    mime_type: mime,
                },
            ));
        }
        Ok(out)
    }

    /// 旧棚の各作品の今の版を、アプリの保存の入力の形で渡す（取得元・鍵・応答・画像と添付）。
    /// 移した棚へ同じ入力で保存し直して、版が増えないことを確かめるのに使う。読めない作品は
    /// `each` に誤りで渡す。
    pub fn current_save_inputs(
        &self,
        mut each: impl FnMut(
            &str,
            &str,
            Result<(Value, Vec<super::work_capture::StagedAsset>)>,
        ) -> Result<()>,
    ) -> Result<()> {
        let mut works = self.conn.prepare(
            "SELECT id,source,source_id,json_path FROM downloads WHERE json_path IS NOT NULL ORDER BY id",
        )?;
        let rows = works
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(works);
        for (work_id, provider, key, json_path) in rows {
            let input = (|| {
                let path = self.managed_path(&json_path)?;
                let mut raw = Vec::new();
                File::open(&path)?
                    .take(16 * 1024 * 1024 + 1)
                    .read_to_end(&mut raw)?;
                let data: Value = serde_json::from_slice(&raw)?;
                let assets = self
                    .version_assets(work_id, &json_path)?
                    .into_iter()
                    .map(|(_, asset)| asset)
                    .collect();
                Ok((data, assets))
            })();
            each(&provider, &key, input)?;
        }
        Ok(())
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
        self.prepare_job_refresh(target, cancel)?;
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
        // 作品ごとの更新の監視は、この列のない古い棚にはない。
        let watch = if self.has_column("downloads", "watch_updates")? {
            "watch_updates"
        } else {
            "0"
        };
        let mut stmt = self.conn.prepare(&format!(
            "SELECT id,source,source_id,title,author_name,favorite,{watch} FROM downloads ORDER BY id"
        ))?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
                (r.get::<_, bool>(5)?, r.get::<_, bool>(6)?),
            ))
        })?;
        for row in rows {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let (old_id, provider, key, title, author, (favorite, watching)) = row?;
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
            let editions = editions.collect::<rusqlite::Result<Vec<_>>>()?;
            let last = editions.len().saturating_sub(1);
            let mut work = None;
            for (index, edition) in editions.into_iter().enumerate() {
                let (version_id, number, json_path, kind, created_at) = edition;
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
                                latest: index == last,
                            },
                        )
                    });
                match result {
                    Ok((saved, read)) => {
                        work = Some(saved.entity_id);
                        report.snapshots += 1;
                        match read {
                            VersionRead::Read => {}
                            VersionRead::Unchanged => report.unchanged_snapshots += 1,
                            VersionRead::KeptOld => report.kept_old_snapshots += 1,
                        }
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
                // 旧棚の今のお気に入りと監視に合わせる。追いつきでは外した印もそのまま写す。
                let state = target.user_state(work, target.id)?;
                if state.favorite != favorite || state.watching != watching {
                    let request = migration_request(
                        target,
                        "user_state",
                        &format!("{work}:{}", state.generation),
                    )?;
                    target.set_user_state(
                        request,
                        work,
                        target.id,
                        state.generation,
                        crate::library::UserStateChange {
                            favorite: Some(favorite),
                            watching: Some(watching),
                        },
                    )?;
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
            "更新ジョブのキュー・履歴と項目・操作記録の移行",
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
    ) -> Result<(super::CaptureResult, VersionRead)> {
        let LegacyVersion {
            work_id,
            version_id,
            number,
            provider,
            key,
            json_path,
            kind,
            acquired_at,
            latest,
        } = version;
        let migration_key = format!("{work_id}:{version_id}:{number}");
        let path = self.managed_path(json_path)?;
        // 追いつき：前回この版を移した時から、版の行・作品の資産の行・原本が変わっていなければ、
        // 読み直さずに前回の保存状態を使う。
        let fingerprint = self.version_fingerprint(work_id, version_id, &path)?;
        if let Some(saved) = moved_version(target, &migration_key, Some(&fingerprint))? {
            return Ok((saved, VersionRead::Unchanged));
        }
        // まず前と同じ要求 ID で送る。中身が同じなら前の保存状態が返るだけで、保存状態は増えない
        // （要求を分けると、同じ中身でも取得の記録が一つ増える）。一度移し終えた版の中身が旧棚で
        // 変わっていて断られたときだけ、読み直した中身（指紋）ごとの要求で送り直す。
        let imported = imported_before(target, &migration_key)?;
        let mut request_key = migration_key.clone();
        let (saved, request_id, included_assets) = loop {
            let request_id = migration_request(target, "download_versions", &request_key)?;
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
            let fields = super::work_capture::work_fields(provider, &data);
            let (asset_ids, staged): (Vec<i64>, Vec<_>) =
                self.version_assets(work_id, json_path)?.into_iter().unzip();
            let (media, attachments, slots) =
                super::work_capture::staged_assets(provider, &data, staged)?;
            let included_assets: Vec<(i64, String)> = asset_ids.into_iter().zip(slots).collect();
            let result = target.capture_with_legacy_classification(
                Capture {
                    acquired_at,
                    request_id,
                    provider: provider.into(),
                    external_key: key.into(),
                    kind: EntityKind::Work,
                    route: "legacy-import".into(),
                    raw: CaptureSource::File(path.clone()),
                    metadata: fields.metadata,
                    field_states: fields.field_states,
                    body: fields.body,
                    media,
                    attachments,
                },
                Some(kind),
            );
            match result {
                Err(Error::Integrity(message))
                    if imported && request_key == migration_key && message == REQUEST_REUSED =>
                {
                    // 古い版を読み直すと、旧棚の今の版より後ろに積まれて今の版になってしまう。
                    if !latest {
                        if let Some(saved) = moved_version(target, &migration_key, None)? {
                            return Ok((saved, VersionRead::KeptOld));
                        }
                    }
                    request_key = format!("{migration_key}#{fingerprint}");
                }
                result => break (result?, request_id, included_assets),
            }
        };
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        rows::mark_row(&tx, "download_versions", &migration_key, saved.snapshot_id)?;
        tx.execute(
            "INSERT INTO migration_fingerprints VALUES('download_versions',?1,?2) ON CONFLICT(source_table,source_key) DO UPDATE SET fingerprint=excluded.fingerprint",
            params![migration_key, fingerprint.0.as_slice()],
        )?;
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
        Ok((saved, VersionRead::Read))
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
                // 追いつき（旧棚を使い続けたあとの再移行）では、変わった表の原記録を今の行に
                // 差し替える。前の原記録は棚に残る（消す操作はまだない）。各領域の正規化が、
                // 今の行との差を取り込む。
                let changed = old.is_some_and(|old| old != file.id.0);
                if changed {
                    report.changed_tables.push(table.clone());
                }
                // 空になった表も、前に移した行を片付けるために正規化をもう一度通す。
                let status = if (count == 0 && !changed)
                    || matches!(table.as_str(), "downloads" | "download_versions")
                    || APPLICATION_SETTING_TABLES.contains(&table.as_str())
                {
                    "imported"
                } else {
                    "unresolved"
                };
                // 最初の原記録と未完了の印を同時に確定する。正規化開始前に落ちても完成に見せない。
                tx.execute("INSERT INTO migration_records(source_table,source_key,status) VALUES('__normalization__','migration','unresolved') ON CONFLICT(source_table,source_key) DO UPDATE SET status='unresolved'",[])?;
                if changed {
                    // 取り込みが済んでいた表も未完了に戻し、各領域の正規化に今の行から取り込み
                    // 直させる（済めば imported に戻る）。取り込み直せない領域は未解決として残る。
                    tx.execute("UPDATE migration_records SET evidence_file_id=?2,status=?3 WHERE source_table=?1 AND source_key='__all_rows__'",params![table,file.id.0.as_slice(),status])?;
                } else {
                    tx.execute("INSERT OR IGNORE INTO migration_records(source_table,source_key,evidence_file_id,status) VALUES(?1,'__all_rows__',?2,?3)",params![table,file.id.0.as_slice(),status])?;
                }
                tx.commit()?;
            }
            fs::remove_file(path)?;
            report.preserved_tables.insert(table.clone(), count);
        }
        Ok(())
    }
}

/// 前に移し終えた保存版の、移した先の保存状態。指紋を渡すと、前回の指紋と同じときだけ返す。
fn moved_version(
    target: &Library,
    key: &str,
    fingerprint: Option<&super::Fingerprint>,
) -> Result<Option<super::CaptureResult>> {
    let conn = target.conn.lock().map_err(|_| Error::Busy)?;
    let found: Option<(Vec<u8>, Vec<u8>)> = conn
        .query_row(
            "SELECT s.entity_id,s.id FROM migration_records r
             JOIN revision_snapshots s ON s.id=r.target_id
             WHERE r.source_table='download_versions' AND r.source_key=?1 AND r.status='imported'
               AND (?2 IS NULL OR EXISTS(SELECT 1 FROM migration_fingerprints f WHERE f.source_table=r.source_table AND f.source_key=r.source_key AND f.fingerprint=?2))",
            params![key, fingerprint.map(|f| f.0.to_vec())],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((entity, snapshot)) = found else {
        return Ok(None);
    };
    let id = |bytes: &[u8]| {
        LibraryId::from_slice(bytes)
            .map_err(|_| Error::Integrity("移行対応表の ID が不正です".into()))
    };
    Ok(Some(super::CaptureResult {
        entity_id: id(&entity)?,
        snapshot_id: id(&snapshot)?,
        created_revision: false,
    }))
}

/// この旧棚の保存版を、前に一度移し終えているか。
fn imported_before(target: &Library, key: &str) -> Result<bool> {
    let conn = target.conn.lock().map_err(|_| Error::Busy)?;
    Ok(conn
        .query_row(
            "SELECT 1 FROM migration_records WHERE source_table='download_versions' AND source_key=?1 AND status='imported'",
            [key],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

/// 移行が書く一つの変更の要求 ID。同じ鍵では前回と同じ ID を返すので、途中で止まった移行を
/// 再開しても二重に書かない。移行先の利用者の変更と見分ける印にもなる。
fn migration_request(target: &Library, table: &str, key: &str) -> Result<LibraryId> {
    let conn = target.conn.lock().map_err(|_| Error::Busy)?;
    let request: Option<Vec<u8>> = conn
        .query_row(
            "SELECT request_id FROM migration_requests WHERE source_table=?1 AND source_key=?2",
            params![table, key],
            |r| r.get(0),
        )
        .optional()?;
    match request {
        Some(id) => LibraryId::from_slice(&id)
            .map_err(|_| Error::Integrity("移行要求 ID が不正です".into())),
        None => {
            let id = LibraryId::now_v7();
            conn.execute(
                "INSERT INTO migration_requests VALUES(?1,?2,?3)",
                params![table, key, id.as_bytes().as_slice()],
            )?;
            Ok(id)
        }
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
            assert_eq!(first.unchanged_snapshots, 0);
            assert_eq!(second.unchanged_snapshots, 1);
            // 旧棚で資産の行が変われば、その版は読み直す。
            Connection::open(old.join("piep.db"))
                .unwrap()
                .execute(
                    "UPDATE assets SET original_url='https://example.invalid/cover.png' WHERE id=1",
                    [],
                )
                .unwrap();
            let third = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(third.snapshots, 1, "{:?}", third.unresolved);
            // 変わった版をもう一度追いつかせても、同じ要求で書き直さない。
            let fourth = source
                .import(&target, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert_eq!(fourth.unchanged_snapshots, 1);
            assert_eq!(third.unchanged_snapshots, 0);
            let work = &target.works(10).unwrap()[0];
            // 取得元の URL は保存版の中身に入るので、変われば版が一つ増える（二度目の追いつきでは増えない）。
            assert_eq!(target.snapshots(work.id).unwrap().len(), 2);
            assert!(target.user_state(work.id, target.id).unwrap().favorite);
            let file = target
                .media_file(work.id, work.snapshot_id, "cover")
                .unwrap();
            let mut image_bytes = Vec::new();
            target.objects.export(&file, &mut image_bytes).unwrap();
            assert_eq!(image_bytes, b"untouched image metadata");
            assert_eq!(fs::read(&raw).unwrap(), raw_bytes);
            target.verify().unwrap();
            // 未解決が残る棚は正本に切り替えない。
            assert!(target.switch_over().is_err());
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
        // 旧棚で古い版の画像の取得元だけが変わっても、追いつきは履歴の途中に版を足さない。
        // 足すと、それが今の版になって旧棚の今の版と食い違う。
        Connection::open(old.join("piep.db"))
            .unwrap()
            .execute(
                "UPDATE assets SET original_url='https://example.invalid/a.png' WHERE id=1",
                [],
            )
            .unwrap();
        let again = LegacyImporter::open(&old)
            .unwrap()
            .import(&target, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(again.unresolved_records, 0, "{:?}", again.unresolved);
        assert_eq!(
            (again.kept_old_snapshots, again.unchanged_snapshots),
            (1, 0)
        );
        assert_eq!(target.snapshots(work).unwrap().len(), 2);
        assert_eq!(target.work_card(work).unwrap().version_number, 2);
        target.verify().unwrap();
        // 正本に切り替えた棚へは、もう旧棚から移さない。
        assert_eq!(target.switched_at().unwrap(), None);
        let at = target.switch_over().unwrap();
        assert_eq!(target.switch_over().unwrap(), at);
        assert!(LegacyImporter::open(&old)
            .unwrap()
            .import(&target, &AtomicBool::new(false), |_| {})
            .is_err());
        drop(target);
        fs::remove_dir_all(temp).unwrap();
    }
}
