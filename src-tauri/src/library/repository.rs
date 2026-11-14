use super::{
    schema, store::regular_directory, BodyFormat, ContentFile, Error, Fingerprint, LibraryId,
    ObjectStore, Result, SourceBody,
};
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub(super) const SOURCE_FIELDS_V1: &[&str] = &[
    "title",
    "caption",
    "tags",
    "series_id",
    "author_key",
    "author_role",
    "create_date",
    "content_type",
    "display_name",
    "description",
    "links",
    "profile_items",
    "is_concluded",
    "published_content_count",
];

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Work,
    Person,
    Series,
    Collection,
}
impl EntityKind {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Work => "work",
            Self::Person => "person",
            Self::Series => "series",
            Self::Collection => "collection",
        }
    }
}

/// 一つの観測で取得した画像。品質・取得元の識別とバイト列の識別を分離する。
pub enum CaptureSource {
    Bytes(Vec<u8>),
    File(PathBuf),
}
impl From<Vec<u8>> for CaptureSource {
    fn from(bytes: Vec<u8>) -> Self {
        Self::Bytes(bytes)
    }
}
impl CaptureSource {
    /// 保管と復元検査を一度で済ませ、DB へ登録できる形で返す。
    pub(super) fn store(&self, objects: &ObjectStore) -> Result<PreparedFile> {
        Ok(match self {
            Self::Bytes(bytes) => objects.put_stored(Cursor::new(bytes))?,
            Self::File(path) => objects.put_path_stored(path)?,
        }
        .into())
    }
}

pub struct CapturedMedia {
    pub slot: String,
    pub role: String,
    pub source_key: Option<String>,
    pub source_url: Option<String>,
    pub quality: String,
    pub original_name: String,
    pub mime_type: String,
    pub bytes: Option<CaptureSource>,
}

pub struct CapturedAttachment {
    pub slot: String,
    pub original_name: String,
    pub mime_type: Option<String>,
    pub bytes: CaptureSource,
}

/// 同じ request_id の再送は同じ結果になる。未知の項目は metadata に入れない。
pub struct Capture {
    pub request_id: LibraryId,
    /// 元の取得日時。移行時の記録日時とは別に保持する。
    pub acquired_at: Option<i64>,
    pub provider: String,
    pub external_key: String,
    pub kind: EntityKind,
    pub route: String,
    pub raw: CaptureSource,
    pub metadata: BTreeMap<String, Value>,
    pub field_states: BTreeMap<String, String>,
    pub body: Option<SourceBody>,
    pub media: Vec<CapturedMedia>,
    pub attachments: Vec<CapturedAttachment>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CaptureResult {
    pub entity_id: LibraryId,
    pub snapshot_id: LibraryId,
    pub created_revision: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkSummary {
    pub id: LibraryId,
    pub snapshot_id: LibraryId,
    pub title: String,
    pub author_name: String,
    pub text_length: u64,
    pub logical_size: u64,
    pub published_at: Option<i64>,
}

#[derive(Serialize, Deserialize)]
struct FormatMarker {
    format: String,
    version: u32,
    library_id: LibraryId,
}

#[derive(Serialize)]
struct StagedMedia<'a> {
    slot: &'a str,
    role: &'a str,
    source_key: &'a Option<String>,
    source_url: &'a Option<String>,
    quality: &'a str,
    original_name: &'a str,
    mime_type: &'a str,
    file: Option<ContentFile>,
}

struct StagedBody {
    file: ContentFile,
    format: BodyFormat,
    semantic: Fingerprint,
    text_length: u64,
}
struct StoredBody {
    file: Option<Vec<u8>>,
    format: Option<String>,
    semantic: Option<Vec<u8>>,
    rule: i64,
}

/// 新形式の棚だけを開く。未知の DB を初期化したり、旧棚を自動移行したりしない。
pub struct Library {
    root: PathBuf,
    pub id: LibraryId,
    pub objects: ObjectStore,
    /// 書き込みはこの一本だけが行う。
    pub(crate) conn: Mutex<Connection>,
    /// 使い終えた読み取り専用の接続。`reader` が貸し出す。
    readers: Mutex<Vec<Connection>>,
    _process_lock: File,
}

/// 読み取り専用の接続。借りているあいだは同じ時点の状態を読み、手放すと戻る。
/// WAL なので、保存の確定や長い検証と互いに待たない。
pub(crate) struct Reader<'a> {
    conn: Option<Connection>,
    pool: &'a Mutex<Vec<Connection>>,
}

impl std::ops::Deref for Reader<'_> {
    type Target = Connection;
    fn deref(&self) -> &Connection {
        self.conn.as_ref().expect("借りている間は接続がある")
    }
}

impl Drop for Reader<'_> {
    fn drop(&mut self) {
        let Some(conn) = self.conn.take() else { return };
        // 読み取りを閉じられなかった接続は、古い時点を見せ続けないよう捨てる。
        if conn.execute_batch("ROLLBACK").is_err() {
            return;
        }
        if let Ok(mut pool) = self.pool.lock() {
            if pool.len() < Library::IDLE_READERS {
                pool.push(conn);
            }
        }
    }
}

/// 実体を読んで確かめ終えた、DB へ登録できるファイル。
pub(crate) struct PreparedFile {
    pub(crate) file: ContentFile,
    encodings: Vec<piep_storage::ChunkEncoding>,
}

impl From<piep_storage::StoredFile> for PreparedFile {
    fn from(stored: piep_storage::StoredFile) -> Self {
        Self {
            file: stored.file,
            encodings: stored.encodings,
        }
    }
}

fn sql_size(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::Integrity("DB に記録できるサイズを超えました".into()))
}

fn read_size(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}

fn uuid(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("UUID の長さが不正です".into()))
}

impl Library {
    pub fn create(root: &Path) -> Result<Self> {
        Self::create_with_id(root, LibraryId::now_v7())
    }

    pub(crate) fn create_with_id(root: &Path, id: LibraryId) -> Result<Self> {
        Self::initialize(root, id, false)
    }

    pub(super) fn create_backup_stage(root: &Path, id: LibraryId) -> Result<Self> {
        Self::initialize(root, id, true)
    }

    fn initialize(root: &Path, id: LibraryId, backup_stage: bool) -> Result<Self> {
        fs::create_dir_all(root)?;
        regular_directory(root)?;
        let lock = Self::lock(root)?;
        let pending_name = if backup_stage {
            ".backup-marker.json"
        } else {
            ".creation.json"
        };
        let pending = root.join(pending_name);
        let resuming = pending.try_exists()?;
        // 作成操作の記録がある場合だけ再開する。未知の既存フォルダーを棚にしない。
        for entry in fs::read_dir(root)? {
            let name = entry?.file_name();
            if name != ".library.lock"
                && !(resuming
                    && (name == pending_name
                        || matches!(
                            name.to_str(),
                            Some("library.db" | "library.db-wal" | "library.db-shm" | "objects")
                        )))
            {
                return Err(Error::Unsupported(
                    "新規作成先は空のフォルダーにしてください".into(),
                ));
            }
        }
        let mut marker = FormatMarker {
            format: "piep-library".into(),
            version: 1,
            library_id: id,
        };
        if resuming {
            super::store::regular_file(&pending)?;
            let bytes = fs::read(&pending)?;
            if bytes.len() > 65536 {
                return Err(Error::Unsupported("作成操作の記録が不正です".into()));
            }
            marker = serde_json::from_slice(&bytes)?;
            if marker.format != "piep-library" || marker.version != 1 {
                return Err(Error::Unsupported("作成操作の形式が異なります".into()));
            }
        } else {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&pending)?;
            output.write_all(&serde_json::to_vec_pretty(&marker)?)?;
            output.sync_all()?;
            super::store::sync_directory(root)?;
        }
        let id = marker.library_id;
        if root.join("library.db").try_exists()? {
            super::store::regular_file(&root.join("library.db"))?;
        }
        let mut conn = Connection::open(root.join("library.db"))?;
        let tables: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )?;
        if tables == 0 {
            schema::create(&mut conn, id)?;
        } else {
            schema::open(&conn, id)?;
        }
        let objects = Self::open_objects(root)?;
        if !backup_stage {
            fs::rename(pending, root.join("library.json"))?;
            super::store::sync_directory(root)?;
        }
        let library = Self {
            root: root.canonicalize()?,
            id,
            objects,
            conn: Mutex::new(conn),
            readers: Mutex::new(Vec::new()),
            _process_lock: lock,
        };
        library.ensure_work_list()?;
        Ok(library)
    }

    pub fn open(root: &Path) -> Result<Self> {
        regular_directory(root)?;
        let lock = Self::lock(root)?;
        super::store::regular_file(&root.join("library.json"))?;
        let mut input = Vec::new();
        File::open(root.join("library.json"))?
            .take(65537)
            .read_to_end(&mut input)?;
        if input.len() > 65536 {
            return Err(Error::Unsupported("形式の記録が大きすぎます".into()));
        }
        let marker: FormatMarker = serde_json::from_slice(&input)?;
        if marker.format != "piep-library" || marker.version != 1 {
            return Err(Error::Unsupported(
                "保存形式または最低対応版が異なります".into(),
            ));
        }
        super::store::regular_file(&root.join("library.db"))?;
        let conn = Connection::open_with_flags(
            root.join("library.db"),
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )?;
        schema::open(&conn, marker.library_id)?;
        regular_directory(&root.join("objects"))?;
        regular_directory(&root.join("objects").join("v1"))?;
        let objects = Self::open_objects(root)?;
        let library = Self {
            root: root.canonicalize()?,
            id: marker.library_id,
            objects,
            conn: Mutex::new(conn),
            readers: Mutex::new(Vec::new()),
            _process_lock: lock,
        };
        library.ensure_work_list()?;
        Ok(library)
    }

    fn open_objects(root: &Path) -> Result<ObjectStore> {
        let parent = root.join("objects");
        match fs::create_dir(&parent) {
            Ok(()) => super::store::sync_directory(root)?,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(error) => return Err(error.into()),
        }
        regular_directory(&parent)?;
        ObjectStore::open(&parent.join("v1")).map_err(Into::into)
    }

    fn lock(root: &Path) -> Result<File> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(root.join(".library.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => Err(Error::Busy),
            Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 同時に何本でも借りられるが、使い終えて残すのはこの本数まで。
    const IDLE_READERS: usize = 4;

    /// 読み取り専用の接続を借りる。書き込み用の一本をふさがないので、
    /// 一覧・読書・配信・検証が保存の確定を待たず、互いにも待たない。
    pub(crate) fn reader(&self) -> Result<Reader<'_>> {
        let idle = self.readers.lock().map_err(|_| Error::Busy)?.pop();
        let conn = match idle {
            Some(conn) => conn,
            None => {
                let conn = Connection::open_with_flags(
                    self.root.join("library.db"),
                    rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                        | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
                )?;
                conn.busy_timeout(std::time::Duration::from_secs(5))?;
                conn.pragma_update(None, "query_only", true)?;
                conn
            }
        };
        // 複数の問い合わせが、途中で確定した保存をまたいで別々の時点を読まないようにする。
        conn.execute_batch("BEGIN")?;
        Ok(Reader {
            conn: Some(conn),
            pool: &self.readers,
        })
    }

    /// 未解決の移行記録を、再起動後も確認できる。本番への切り替え前に必ず検査する。
    pub fn activation_problems(&self) -> Result<Vec<String>> {
        let conn = self.reader()?;
        let mut statement=conn.prepare("SELECT source_table,source_key FROM migration_records WHERE status='unresolved' ORDER BY source_table,source_key")?;
        let result = statement
            .query_map([], |r| {
                Ok(format!(
                    "{}: {}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(result)
    }

    /// 保管済みのファイルを DB へ登録する前に、実体を一度だけ読んで確かめる。
    /// いま保管したばかりのものは `store_bytes`・`store_path` を使い、読み直しを重ねない。
    pub(crate) fn prepare_file(&self, file: &ContentFile) -> Result<PreparedFile> {
        Ok(self.objects.inspect(file)?.into())
    }

    /// 保管の中で読み戻して確かめた結果を、そのまま登録に使う。
    pub(crate) fn store_bytes(&self, bytes: &[u8]) -> Result<PreparedFile> {
        Ok(self.objects.put_stored(Cursor::new(bytes))?.into())
    }

    pub(crate) fn store_path(&self, path: &Path) -> Result<PreparedFile> {
        Ok(self.objects.put_path_stored(path)?.into())
    }

    pub(crate) fn register_file(tx: &Transaction<'_>, prepared: &PreparedFile) -> Result<()> {
        let file = &prepared.file;
        let existing: Option<(i64, bool)> = tx
            .query_row(
                "SELECT original_size,sealed FROM content_files WHERE id=?1",
                [file.id.0.as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if let Some((size, sealed)) = existing {
            if !sealed
                || size != sql_size(file.length)?
                || Self::file_from_conn(tx, file.id)? != *file
            {
                return Err(Error::Integrity(
                    "既存の元ファイルの記録が不完全か不一致です".into(),
                ));
            }
            return Ok(());
        }
        tx.execute(
            "INSERT INTO content_files(id,original_size) VALUES(?1,?2)",
            params![file.id.0.as_slice(), sql_size(file.length)?],
        )?;
        let size: i64 = tx.query_row(
            "SELECT original_size FROM content_files WHERE id=?1",
            [file.id.0.as_slice()],
            |r| r.get(0),
        )?;
        if size != sql_size(file.length)? {
            return Err(Error::Integrity(
                "元ファイルのサイズの記録が不一致です".into(),
            ));
        }
        let mut offset = 0;
        if file.chunks.len() != prepared.encodings.len() {
            return Err(Error::Integrity(
                "部分実体と保存表現の個数が一致しません".into(),
            ));
        }
        for (i, (chunk, encoding)) in file.chunks.iter().zip(&prepared.encodings).enumerate() {
            tx.execute(
                "INSERT OR IGNORE INTO chunks VALUES(?1,?2)",
                params![chunk.id.0.as_slice(), sql_size(chunk.length)?],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO chunk_storage VALUES(?1,?2,?3,?4)",
                params![
                    chunk.id.0.as_slice(),
                    encoding.codec,
                    sql_size(encoding.stored_bytes)?,
                    chrono::Utc::now().timestamp_millis()
                ],
            )?;
            tx.execute(
                "INSERT OR IGNORE INTO file_chunks VALUES(?1,?2,?3,?4)",
                params![
                    file.id.0.as_slice(),
                    sql_size(i as u64)?,
                    sql_size(offset)?,
                    chunk.id.0.as_slice()
                ],
            )?;
            offset += chunk.length;
        }
        if Self::file_from_conn(tx, file.id)? != *file {
            return Err(Error::Integrity(
                "既存の元ファイルと部分実体の記録が不一致です".into(),
            ));
        }
        tx.execute(
            "UPDATE content_files SET sealed=1 WHERE id=?1",
            [file.id.0.as_slice()],
        )?;
        Ok(())
    }

    pub(crate) fn file_from_conn(conn: &Connection, id: Fingerprint) -> Result<ContentFile> {
        let length = conn.query_row(
            "SELECT original_size FROM content_files WHERE id=?1",
            [id.0.as_slice()],
            |r| read_size(r, 0),
        )?;
        let mut statement = conn.prepare("SELECT c.id,c.original_size,f.ordinal,f.offset FROM file_chunks f JOIN chunks c ON c.id=f.chunk_id WHERE f.file_id=?1 ORDER BY f.ordinal")?;
        let mut chunks = Vec::new();
        let mut expected_offset = 0;
        for row in statement.query_map([id.0.as_slice()], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                read_size(r, 1)?,
                read_size(r, 2)?,
                read_size(r, 3)?,
            ))
        })? {
            let (hash, length, ordinal, offset) = row?;
            if ordinal != chunks.len() as u64 || offset != expected_offset {
                return Err(Error::Integrity(
                    "ファイルの部分実体の順序または位置が不正です".into(),
                ));
            }
            expected_offset += length;
            let hash: [u8; 32] = hash
                .try_into()
                .map_err(|_| Error::Integrity("部分実体の ID が不正です".into()))?;
            chunks.push(super::FileChunk {
                id: Fingerprint(hash),
                length,
            });
        }
        Ok(ContentFile { id, length, chunks })
    }

    pub fn file(&self, id: Fingerprint) -> Result<ContentFile> {
        let conn = self.reader()?;
        Self::file_from_conn(&conn, id)
    }

    /// 編集時に追加する画像等を、元バイト列のまま保管する。
    pub fn store_original(&self, source: &CaptureSource) -> Result<ContentFile> {
        let prepared = source.store(&self.objects)?;
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        Self::register_file(&tx, &prepared)?;
        tx.commit()?;
        Ok(prepared.file)
    }

    /// オフライン入力の取り込み用。ネットワーク取得は開始記録を作ってから完了する。
    #[cfg(test)]
    pub(super) fn capture(&self, capture: Capture) -> Result<CaptureResult> {
        self.capture_prepared(capture, None, None)
    }

    pub(super) fn capture_with_legacy_classification(
        &self,
        capture: Capture,
        legacy_kind: Option<&str>,
    ) -> Result<CaptureResult> {
        self.capture_prepared(capture, legacy_kind, None)
    }

    pub(super) fn capture_prepared(
        &self,
        mut capture: Capture,
        legacy_kind: Option<&str>,
        context: Option<super::acquisitions::CompletionContext>,
    ) -> Result<CaptureResult> {
        if capture
            .acquired_at
            .is_some_and(|time| chrono::DateTime::from_timestamp_millis(time).is_none())
        {
            return Err(Error::Integrity("取得日時が不正です".into()));
        }
        if let Some(Value::Array(tags)) = capture.metadata.get_mut("tags") {
            if tags.iter().all(Value::is_string) {
                tags.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
                tags.dedup();
            }
        }
        for field in ["author_key", "series_id"] {
            if let Some(value) = capture.metadata.get_mut(field) {
                if let Some(number) = value.as_u64() {
                    *value = Value::String(number.to_string());
                }
            }
        }
        for field in ["create_date", "update_date"] {
            if let Some(value) = capture.metadata.get_mut(field) {
                if let Some(date) = value
                    .as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                {
                    *value = serde_json::json!(date.timestamp_millis());
                }
            }
        }
        if capture.external_key.is_empty()
            || capture.provider.is_empty()
            || capture.kind == EntityKind::Collection
        {
            return Err(Error::Integrity("取得対象の識別が不正です".into()));
        }
        for state in capture.field_states.values() {
            if !matches!(
                state.as_str(),
                "known" | "unknown" | "restricted" | "uninterpretable"
            ) {
                return Err(Error::Integrity("項目の取得状態が不正です".into()));
            }
        }
        let mut slots = std::collections::HashSet::new();
        for media in &capture.media {
            if media.slot.is_empty() || !slots.insert(media.slot.as_str()) {
                return Err(Error::Integrity("画像の位置が空か重複しています".into()));
            }
        }
        let mut attachment_slots = std::collections::HashSet::new();
        for attachment in &capture.attachments {
            if attachment.slot.is_empty() || !attachment_slots.insert(attachment.slot.as_str()) {
                return Err(Error::Integrity("添付の位置が空か重複しています".into()));
            }
        }
        let baseline = {
            let conn = self.conn.lock().map_err(|_| Error::Busy)?;
            conn.query_row("SELECT c.snapshot_id FROM entity_current c JOIN external_identities e ON e.entity_id=c.entity_id WHERE e.provider=?1 AND e.kind=?2 AND e.external_key=?3",params![capture.provider,capture.kind.name(),capture.external_key],|r|r.get::<_,Vec<u8>>(0)).optional()?
        };
        // 保管の中で読み戻して確かめた結果を集め、登録前にもう一度読むことを避ける。
        let raw_prepared = capture.raw.store(&self.objects)?;
        let raw = raw_prepared.file.clone();
        let mut stored = Vec::new();
        let input_body = capture
            .body
            .as_ref()
            .map(|body| -> Result<StagedBody> {
                let prepared = self.store_bytes(body.storage_bytes()?.as_ref())?;
                let file = prepared.file.clone();
                stored.push(prepared);
                Ok(StagedBody {
                    file,
                    format: body.format(),
                    semantic: body.semantic_fingerprint()?,
                    text_length: body.text_content().chars().count() as u64,
                })
            })
            .transpose()?;
        let body = input_body.as_ref().filter(|_| {
            capture
                .field_states
                .get("body")
                .is_none_or(|state| state == "known")
        });
        if input_body.is_none()
            && capture
                .field_states
                .get("body")
                .is_some_and(|state| state == "known")
        {
            return Err(Error::Integrity(
                "取得済みの本文は、空の場合も本文として渡す必要があります".into(),
            ));
        }
        let media: Vec<_> = capture
            .media
            .iter()
            .map(|m| {
                Ok(StagedMedia {
                    slot: &m.slot,
                    role: &m.role,
                    source_key: &m.source_key,
                    source_url: &m.source_url,
                    quality: &m.quality,
                    original_name: &m.original_name,
                    mime_type: &m.mime_type,
                    file: match &m.bytes {
                        Some(source) => {
                            let prepared = source.store(&self.objects)?;
                            let file = prepared.file.clone();
                            stored.push(prepared);
                            Some(file)
                        }
                        None => None,
                    },
                })
            })
            .collect::<Result<_>>()?;
        let attachments: Vec<_> = capture
            .attachments
            .iter()
            .map(|a| {
                let prepared = a.bytes.store(&self.objects)?;
                let file = prepared.file.clone();
                stored.push(prepared);
                Ok((&a.slot, &a.original_name, &a.mime_type, file))
            })
            .collect::<Result<_>>()?;
        let request_bytes = serde_json::to_vec(&(
            "capture-request-v1",
            &capture.provider,
            &capture.external_key,
            capture.kind,
            &capture.route,
            capture.acquired_at,
            raw.id,
            &capture.metadata,
            &capture.field_states,
            input_body
                .as_ref()
                .map(|body| (&body.file, body.format, body.semantic)),
            &media,
            &attachments,
            legacy_kind,
        ))?;
        let request_bytes = if let Some(context) = &context {
            super::acquisitions::encode_manifest(context, request_bytes)?
        } else {
            request_bytes
        };
        let request_prepared = self.store_bytes(&request_bytes)?;
        let request_hash = request_prepared.file.id;
        // 復元検査・ファイル読み込みは DB の書き込みトランザクションより前に済ませてある。
        let mut prepared = vec![raw_prepared, request_prepared];
        prepared.extend(stored);
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        let receipt = tx.query_row("SELECT entity_id,result_snapshot_id,created_revision,request_hash FROM acquisition_observations WHERE request_id=?1",[capture.request_id.as_bytes().as_slice()],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,bool>(2)?,r.get::<_,Vec<u8>>(3)?))).optional()?;
        if let Some((entity, snapshot, created, hash)) = receipt {
            if hash != request_hash.0 {
                return Err(Error::Integrity(
                    "同じ要求 ID が異なる内容で再送されました".into(),
                ));
            }
            if let Some(context) = &context {
                let info = super::acquisitions::load_info(&tx, context.ticket.id)?;
                if info.state != super::AcquisitionState::Complete
                    || info.input_manifest != Some(request_hash)
                {
                    return Err(Error::Integrity("取得結果と完了記録が一致しません".into()));
                }
                match (&context.reader_bindings, &info.reader) {
                    (Some(bindings), Some(reader))
                        if reader.entity.as_bytes() == entity.as_slice()
                            && reader.snapshot.as_bytes() == snapshot.as_slice()
                            && reader.recipe == 1 =>
                    {
                        let plan =
                            Self::reader_bindings_from_conn(&tx, reader.entity, reader.snapshot)?;
                        if plan.bindings_hash != reader.bindings_hash || &plan.bindings != bindings
                        {
                            return Err(Error::Integrity(
                                "取得結果と読書用の完了記録が一致しません".into(),
                            ));
                        }
                    }
                    (None, None) => {}
                    _ => {
                        return Err(Error::Integrity(
                            "取得結果と読書用の完了記録が一致しません".into(),
                        ))
                    }
                }
            }
            return Ok(CaptureResult {
                entity_id: uuid(entity)?,
                snapshot_id: uuid(snapshot)?,
                created_revision: created,
            });
        }
        let now = chrono::Utc::now().timestamp_millis();
        if let Some(context) = &context {
            let info = super::acquisitions::load_info(&tx, context.ticket.id)?;
            if info.ticket != context.ticket
                || super::acquisitions::raw_parts(&tx, context.ticket.id)? != context.parts
            {
                return Err(Error::Integrity(
                    "保存の準備中に受信記録が変わりました。確認し直してください".into(),
                ));
            }
            match info.state {
                super::AcquisitionState::Pending => {}
                super::AcquisitionState::Cancelled => return Err(Error::Canceled),
                super::AcquisitionState::Superseded
                    if info.input_manifest == Some(request_hash) =>
                {
                    return Err(Error::Superseded)
                }
                _ => return Err(Error::Integrity("取得の状態と入力が一致しません".into())),
            }
            for file in &prepared {
                Self::register_file(&tx, file)?;
            }
            let mut inputs = vec![("raw", "", raw.id), ("request", "", request_hash)];
            if let Some(body) = &input_body {
                inputs.push(("body", "", body.file.id));
            }
            for item in &media {
                if let Some(file) = &item.file {
                    inputs.push(("media", item.slot, file.id));
                }
            }
            for (slot, _, _, file) in &attachments {
                inputs.push(("attachment", slot.as_str(), file.id));
            }
            for (role, slot, file) in inputs {
                tx.execute(
                    "INSERT INTO acquisition_inputs VALUES(?1,?2,?3,?4)",
                    params![
                        context.ticket.id.as_bytes().as_slice(),
                        role,
                        slot,
                        file.0.as_slice()
                    ],
                )?;
            }
            let (epoch,current,generation):(i64,Option<Vec<u8>>,Option<i64>)=tx.query_row("SELECT h.epoch,c.snapshot_id,c.generation FROM acquisition_heads h LEFT JOIN entity_current c ON c.entity_id=h.entity_id WHERE h.entity_id=?1",[context.ticket.entity.as_bytes().as_slice()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
            if epoch != context.ticket.epoch
                || generation.unwrap_or(0) != context.ticket.base_generation
                || current.as_deref()
                    != context
                        .ticket
                        .base_snapshot
                        .as_ref()
                        .map(|id| id.as_bytes().as_slice())
            {
                tx.execute("UPDATE acquisition_attempts SET state='superseded',input_manifest_file_id=?2,finished_at=?3 WHERE id=?1",params![context.ticket.id.as_bytes().as_slice(),request_hash.0.as_slice(),now])?;
                tx.execute(
                    "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
                    [],
                )?;
                tx.commit()?;
                return Err(Error::Superseded);
            }
        } else {
            let reserved: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM acquisition_attempts WHERE id=?1)",
                [capture.request_id.as_bytes().as_slice()],
                |r| r.get(0),
            )?;
            if reserved {
                return Err(Error::Integrity(
                    "この要求 ID は開始済みの取得に予約されています".into(),
                ));
            }
        }
        let acquired_at = capture.acquired_at.unwrap_or(now);
        let entity = match tx.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind=?2 AND external_key=?3",params![capture.provider,capture.kind.name(),capture.external_key],|r|r.get::<_,Vec<u8>>(0)).optional()? {
            Some(id)=>uuid(id)?,
            None=>{
                let id = LibraryId::now_v7();
                tx.execute("INSERT INTO entities(id,kind,created_at) VALUES(?1,?2,?3)",params![id.as_bytes().as_slice(),capture.kind.name(),now])?;
                tx.execute("INSERT INTO external_identities VALUES(?1,?2,?3,?4)",params![capture.provider,capture.kind.name(),capture.external_key,id.as_bytes().as_slice()])?;
                match capture.kind {
                    EntityKind::Work=>{tx.execute("INSERT INTO works(id,content_type) VALUES(?1,'unknown')",[id.as_bytes().as_slice()])?;},
                    EntityKind::Person=>{tx.execute("INSERT INTO people(id) VALUES(?1)",[id.as_bytes().as_slice()])?;},
                    EntityKind::Series=>{tx.execute("INSERT INTO series(id) VALUES(?1)",[id.as_bytes().as_slice()])?;},
                    EntityKind::Collection=>unreachable!(),
                }
                id
            }
        };
        let previous = tx.query_row("SELECT s.id,s.revision_id,s.metadata_json,s.body_file_id,s.field_states_json,s.body_format,s.body_semantic_hash,s.body_semantic_rule FROM entity_current c JOIN revision_snapshots s ON s.id=c.snapshot_id WHERE c.entity_id=?1",[entity.as_bytes().as_slice()],|r|Ok((r.get::<_,Vec<u8>>(0)?,r.get::<_,Vec<u8>>(1)?,r.get::<_,String>(2)?,StoredBody {file:r.get(3)?,format:r.get(5)?,semantic:r.get(6)?,rule:r.get(7)?},r.get::<_,String>(4)?))).optional()?;
        if context.is_none() && previous.as_ref().map(|p| &p.0) != baseline.as_ref() {
            return Err(Error::Integrity(
                "保存の準備中に別の取得が確定しました。最新の状態で比較し直してください".into(),
            ));
        }
        let mut merged: BTreeMap<String, Value> = previous
            .as_ref()
            .map(|p| serde_json::from_str(&p.2))
            .transpose()?
            .unwrap_or_default();
        let mut states: BTreeMap<String, String> = previous
            .as_ref()
            .map(|p| serde_json::from_str(&p.4))
            .transpose()?
            .unwrap_or_default();
        let mut changed = previous.is_none();
        for (key, value) in &capture.metadata {
            if capture.field_states.get(key).is_some_and(|s| s != "known") {
                continue;
            }
            if SOURCE_FIELDS_V1.contains(&key.as_str())
                && merged.get(key).is_some_and(|old| {
                    super::integrity::source_field_v1(key, old)
                        != super::integrity::source_field_v1(key, value)
                })
            {
                changed = true;
            }
            merged.insert(key.clone(), value.clone());
            states.insert(key.clone(), "known".into());
        }
        for (key, state) in &capture.field_states {
            if !merged.contains_key(key)
                && (legacy_kind.is_some()
                    || state == "known"
                    || states.get(key).is_none_or(|old| old != "known"))
            {
                states.insert(key.clone(), state.clone());
            }
        }
        if previous.as_ref().is_some_and(|p| p.3.rule != 1) {
            return Err(Error::Unsupported(
                "本文の比較規則に対応していません".into(),
            ));
        }
        let previous_body = previous.as_ref().and_then(|p| p.3.semantic.as_deref());
        if let (Some(old), Some(next)) = (previous_body, body.as_ref()) {
            if old != next.semantic.0 {
                changed = true;
            }
        }
        let body_id = body
            .as_ref()
            .map(|b| b.file.id.0.to_vec())
            .or_else(|| previous.as_ref().and_then(|p| p.3.file.clone()));
        let body_format = body
            .map(|b| b.format.name().to_owned())
            .or_else(|| previous.as_ref().and_then(|p| p.3.format.clone()));
        let body_semantic = body
            .map(|b| b.semantic.0.to_vec())
            .or_else(|| previous.as_ref().and_then(|p| p.3.semantic.clone()));
        if body_id.is_some() {
            states.insert("body".into(), "known".into());
        }
        // 画像の取得品質は変化の理由にしない。同じ slot の取得元識別が変わったときだけ改稿。
        let mut identities = BTreeMap::<String, Option<String>>::new();
        if let Some(previous) = &previous {
            let old_snapshot = &previous.0;
            let mut stmt = tx.prepare("SELECT s.slot,m.source_key FROM snapshot_media s JOIN source_media m ON m.id=s.media_id WHERE s.snapshot_id=?1 ORDER BY s.slot")?;
            for row in stmt.query_map([old_snapshot], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
            })? {
                let (slot, key) = row?;
                identities.insert(slot, key);
            }
            for next in &media {
                if let Some(key) = next.source_key.as_ref() {
                    let old: Option<Option<String>> = tx.query_row("SELECT m.source_key FROM snapshot_media s JOIN source_media m ON m.id=s.media_id WHERE s.snapshot_id=?1 AND s.slot=?2",params![old_snapshot,next.slot],|r|r.get(0)).optional()?;
                    if old.flatten().is_some_and(|old| old != *key) {
                        changed = true;
                    }
                }
            }
        }
        if capture
            .field_states
            .get("media")
            .is_some_and(|s| s == "known")
        {
            let previously_complete = previous
                .as_ref()
                .map(|p| serde_json::from_str::<BTreeMap<String, String>>(&p.4))
                .transpose()?
                .is_some_and(|s| s.get("media").is_some_and(|v| v == "known"));
            if previously_complete
                && (identities.keys().any(|slot| !slots.contains(slot.as_str()))
                    || slots.iter().any(|slot| !identities.contains_key(*slot)))
            {
                changed = true;
            }
            identities.retain(|slot, _| slots.contains(slot.as_str()));
        }
        for media in &media {
            identities.insert(media.slot.to_string(), media.source_key.clone());
        }
        let mut attachment_identity = BTreeMap::<String, Vec<u8>>::new();
        if let Some(previous) = &previous {
            let mut stmt = tx.prepare(
                "SELECT slot,file_id FROM snapshot_files WHERE snapshot_id=?1 ORDER BY slot",
            )?;
            for row in stmt.query_map([&previous.0], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, Vec<u8>>(1)?))
            })? {
                let (slot, file) = row?;
                attachment_identity.insert(slot, file);
            }
            for (slot, _, _, file) in &attachments {
                if attachment_identity
                    .get(slot.as_str())
                    .is_some_and(|old| old.as_slice() != file.id.0)
                {
                    changed = true;
                }
            }
        }
        if capture
            .field_states
            .get("attachments")
            .is_some_and(|s| s == "known")
        {
            let previously_complete = previous
                .as_ref()
                .map(|p| serde_json::from_str::<BTreeMap<String, String>>(&p.4))
                .transpose()?
                .is_some_and(|s| s.get("attachments").is_some_and(|v| v == "known"));
            if previously_complete
                && (attachment_identity
                    .keys()
                    .any(|slot| !attachment_slots.contains(slot.as_str()))
                    || attachment_slots
                        .iter()
                        .any(|slot| !attachment_identity.contains_key(*slot)))
            {
                changed = true;
            }
            attachment_identity.retain(|slot, _| attachment_slots.contains(slot.as_str()));
        }
        for (slot, _, _, file) in &attachments {
            attachment_identity.insert(slot.to_string(), file.id.0.to_vec());
        }
        let legacy_uncertain = legacy_kind.is_some_and(|kind| match kind {
            "repair" => changed,
            "source" => !changed,
            _ => true,
        });
        if legacy_kind.is_some_and(|kind| kind != "repair") {
            changed = true;
        }
        if changed && previous_body.is_some() && body.is_none() {
            return Err(Error::Integrity(
                "取得元の変更を確定する前に本文の再取得が必要です".into(),
            ));
        }
        for file in &prepared {
            Self::register_file(&tx, file)?;
        }
        // 保存状態に差がなければ、取得記録を残して候補の構築だけを巻き戻す。
        tx.execute_batch("SAVEPOINT snapshot_publication")?;
        let revision = if changed {
            let id = LibraryId::now_v7();
            let ordinal: i64 = tx.query_row(
                "SELECT coalesce(max(ordinal),0)+1 FROM source_revisions WHERE entity_id=?1",
                [entity.as_bytes().as_slice()],
                |r| r.get(0),
            )?;
            // 旧棚の「修復」の版は、同じ版を取り直して欠けを埋めたもの。中身が違えば版は分けるが、
            // 作者の改稿ではないことを残す（旧棚の改稿の絞り込みに数えない）。
            let repair = legacy_kind == Some("repair");
            tx.execute(
                "INSERT INTO source_revisions(id,entity_id,ordinal,classification,discovered_at,repair) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    id.as_bytes().as_slice(),
                    entity.as_bytes().as_slice(),
                    ordinal,
                    if legacy_uncertain {
                        "legacy_unclassified"
                    } else {
                        "source"
                    },
                    acquired_at,
                    repair
                ],
            )?;
            id
        } else {
            uuid(previous.as_ref().unwrap().1.clone())?
        };
        let semantic = Fingerprint::of(b"unpublished");
        // 仮の値は公開前に、継承したものも含む実際の全参照から作り直す。
        let manifest = Fingerprint::of(b"unpublished");
        let snapshot = LibraryId::now_v7();
        let sequence: i64 = tx.query_row(
            "SELECT coalesce(max(sequence),0)+1 FROM revision_snapshots WHERE entity_id=?1",
            [entity.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        tx.execute("INSERT INTO revision_snapshots(id,entity_id,revision_id,metadata_json,field_states_json,body_file_id,interpretation_version,semantic_rule,semantic_hash,manifest_hash,created_at,sequence,body_format,body_semantic_hash) VALUES(?1,?2,?3,?4,?5,?6,1,1,?7,?8,?9,?10,?11,?12)",params![snapshot.as_bytes().as_slice(),entity.as_bytes().as_slice(),revision.as_bytes().as_slice(),serde_json::to_string(&merged)?,serde_json::to_string(&states)?,body_id,semantic.0.as_slice(),manifest.0.as_slice(),acquired_at,sequence,body_format,body_semantic])?;
        let observation = LibraryId::now_v7();
        let insert_observation = |result_snapshot: LibraryId| -> Result<()> {
            tx.execute("INSERT INTO acquisition_observations(id,entity_id,request_id,request_hash,acquired_at,recorded_at,route,status,raw_file_id,result_snapshot_id,created_revision,body_file_id,body_format) VALUES(?1,?2,?3,?4,?5,?6,?7,'available',?8,?9,?10,?11,?12)",params![observation.as_bytes().as_slice(),entity.as_bytes().as_slice(),capture.request_id.as_bytes().as_slice(),request_hash.0.as_slice(),acquired_at,now,capture.route,raw.id.0.as_slice(),result_snapshot.as_bytes().as_slice(),changed,input_body.as_ref().map(|b|b.file.id.0.to_vec()),input_body.as_ref().map(|b|b.format.name())])?;
            Ok(())
        };
        if let Some(previous) = &previous {
            let old = &previous.0;
            tx.execute("INSERT INTO snapshot_media SELECT ?1,slot,role,media_id,rendition_id,original_name FROM snapshot_media WHERE snapshot_id=?2",params![snapshot.as_bytes().as_slice(),old])?;
            let mut statement=tx.prepare("SELECT slot,file_id,original_name,mime_type,role FROM snapshot_files WHERE snapshot_id=?1 ORDER BY slot")?;
            let rows = statement
                .query_map([old], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, Vec<u8>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                    ))
                })?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(statement);
            for (slot, file, name, mime, role) in rows {
                tx.execute(
                    "INSERT INTO snapshot_files VALUES(?1,?2,?3,?4,?5,?6,?7)",
                    params![
                        LibraryId::now_v7().as_bytes().as_slice(),
                        snapshot.as_bytes().as_slice(),
                        slot,
                        file,
                        name,
                        mime,
                        role
                    ],
                )?;
            }
        }
        if capture
            .field_states
            .get("attachments")
            .is_some_and(|s| s == "known")
        {
            let mut stmt = tx.prepare("SELECT slot FROM snapshot_files WHERE snapshot_id=?1")?;
            let slots = stmt
                .query_map([snapshot.as_bytes().as_slice()], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            for slot in slots {
                if !attachment_slots.contains(slot.as_str()) {
                    tx.execute(
                        "DELETE FROM snapshot_files WHERE snapshot_id=?1 AND slot=?2",
                        params![snapshot.as_bytes().as_slice(), slot],
                    )?;
                }
            }
        }
        for (slot, name, mime, file) in &attachments {
            tx.execute("INSERT INTO snapshot_files VALUES(?1,?2,?3,?4,?5,?6,'attachment') ON CONFLICT(snapshot_id,slot) DO UPDATE SET file_id=excluded.file_id,original_name=excluded.original_name,mime_type=excluded.mime_type",params![LibraryId::now_v7().as_bytes().as_slice(),snapshot.as_bytes().as_slice(),slot,file.id.0.as_slice(),name,mime])?;
        }
        Self::snapshot_relations(&tx, snapshot, &capture.provider, &merged, now)?;
        if capture
            .field_states
            .get("media")
            .is_some_and(|s| s == "known")
        {
            let mut stmt = tx.prepare("SELECT slot FROM snapshot_media WHERE snapshot_id=?1")?;
            let old_slots = stmt
                .query_map([snapshot.as_bytes().as_slice()], |r| r.get::<_, String>(0))?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            drop(stmt);
            for slot in old_slots {
                if !slots.contains(slot.as_str()) {
                    tx.execute(
                        "DELETE FROM snapshot_media WHERE snapshot_id=?1 AND slot=?2",
                        params![snapshot.as_bytes().as_slice(), slot],
                    )?;
                }
            }
        }
        for next in &media {
            if legacy_kind.is_none() && next.source_key.is_none() && next.file.is_none() {
                let saved:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM snapshot_media WHERE snapshot_id=?1 AND slot=?2 AND role=?3 AND rendition_id IS NOT NULL)",params![snapshot.as_bytes().as_slice(),next.slot,next.role],|r|r.get(0))?;
                if saved {
                    continue;
                }
            }
            let known = match next.source_key {
                Some(key)=>tx.query_row("SELECT id FROM source_media WHERE provider=?1 AND kind=?2 AND source_key=?3 AND identity_rule=1",params![capture.provider,next.role,key],|r|r.get::<_,Vec<u8>>(0)).optional()?,
                None=>None,
            };
            let media_id = known
                .map(uuid)
                .transpose()?
                .unwrap_or_else(LibraryId::now_v7);
            tx.execute(
                "INSERT OR IGNORE INTO source_media VALUES(?1,?2,?3,?4,1)",
                params![
                    media_id.as_bytes().as_slice(),
                    capture.provider,
                    next.role,
                    next.source_key
                ],
            )?;
            let rendition = if let Some(file) = &next.file {
                let existing: Option<Vec<u8>> = tx.query_row(
                    "SELECT id FROM media_renditions WHERE media_id=?1 AND file_id=?2 AND mime_type=?3 AND quality=?4 AND width IS NULL AND height IS NULL",
                    params![media_id.as_bytes().as_slice(),file.id.0.as_slice(),next.mime_type,next.quality], |r| r.get(0),
                ).optional()?;
                let rendition = match existing {
                    Some(id) => uuid(id)?,
                    None => {
                        let id = LibraryId::now_v7();
                        tx.execute("INSERT INTO media_renditions(id,media_id,file_id,mime_type,quality) VALUES(?1,?2,?3,?4,?5)",params![id.as_bytes().as_slice(),media_id.as_bytes().as_slice(),file.id.0.as_slice(),next.mime_type,next.quality])?;
                        id
                    }
                };
                tx.execute(
                    "INSERT INTO media_observations VALUES(?1,?2,?3,?4,?5)",
                    params![
                        LibraryId::now_v7().as_bytes().as_slice(),
                        rendition.as_bytes().as_slice(),
                        next.source_url,
                        acquired_at,
                        capture.route
                    ],
                )?;
                Some(rendition.as_bytes().to_vec())
            } else {
                tx.query_row("SELECT rendition_id FROM snapshot_media WHERE snapshot_id=?1 AND slot=?2 AND media_id=?3",params![snapshot.as_bytes().as_slice(),next.slot,media_id.as_bytes().as_slice()],|r|r.get::<_,Option<Vec<u8>>>(0)).optional()?.flatten()
            };
            // 過去の保存状態の移行では、その時に選ばれていた画像を正確に復元する。
            // 通常取得では、同じ画像の確認済み原寸を縮小版で置き換えない。
            let preferred = if legacy_kind.is_none() {
                super::renditions::prefer_saved_quality(&tx, media_id, rendition.as_deref())?
            } else {
                None
            };
            let (rendition, name) = match preferred {
                Some((id, name)) => (Some(id), name),
                None => (rendition, next.original_name.to_owned()),
            };
            tx.execute("INSERT INTO snapshot_media VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(snapshot_id,slot) DO UPDATE SET role=excluded.role,media_id=excluded.media_id,rendition_id=excluded.rendition_id,original_name=excluded.original_name",params![snapshot.as_bytes().as_slice(),next.slot,next.role,media_id.as_bytes().as_slice(),rendition,name])?;
        }
        if context.is_some() && !changed {
            if let Some(previous) = &previous {
                let old = uuid(previous.0.clone())?;
                if super::saved_state::fingerprint(&tx, snapshot)?
                    == super::saved_state::fingerprint(&tx, old)?
                {
                    tx.execute_batch(
                        "ROLLBACK TO snapshot_publication; RELEASE snapshot_publication;",
                    )?;
                    insert_observation(old)?;
                    let result = CaptureResult {
                        entity_id: entity,
                        snapshot_id: old,
                        created_revision: false,
                    };
                    self.prepare_capture_reader(&tx, &result, context.as_ref())?;
                    Self::finish_capture(
                        &tx,
                        &result,
                        capture.request_id,
                        observation,
                        request_hash,
                        now,
                        context.as_ref(),
                    )?;
                    tx.commit()?;
                    return Ok(result);
                }
            }
        }
        tx.execute_batch("RELEASE snapshot_publication")?;
        insert_observation(snapshot)?;
        tx.execute(
            "INSERT INTO snapshot_observations VALUES(?1,?2,?3)",
            params![
                entity.as_bytes().as_slice(),
                snapshot.as_bytes().as_slice(),
                observation.as_bytes().as_slice()
            ],
        )?;
        if let Some(previous) = &previous {
            tx.execute("INSERT OR IGNORE INTO snapshot_observations SELECT entity_id,?1,observation_id FROM snapshot_observations WHERE snapshot_id=?2",params![snapshot.as_bytes().as_slice(),previous.0])?;
        }
        let (semantic, manifest) = super::integrity::snapshot_fingerprints(&tx, snapshot)?;
        tx.execute(
            "UPDATE revision_snapshots SET semantic_hash=?2,manifest_hash=?3,published=1 WHERE id=?1",
            params![snapshot.as_bytes().as_slice(),semantic.0.as_slice(), manifest.0.as_slice()],
        )?;
        tx.execute("INSERT INTO entity_current(entity_id,snapshot_id,generation) VALUES(?1,?2,1) ON CONFLICT(entity_id) DO UPDATE SET snapshot_id=excluded.snapshot_id,generation=generation+1",params![entity.as_bytes().as_slice(),snapshot.as_bytes().as_slice()])?;
        if capture.kind == EntityKind::Work {
            if let Some(content_type) = merged
                .get("content_type")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            {
                tx.execute(
                    "UPDATE works SET content_type=?2 WHERE id=?1",
                    params![entity.as_bytes().as_slice(), content_type],
                )?;
            }
            tx.execute("INSERT INTO library_items(work_id,added_at,generation) VALUES(?1,?2,1) ON CONFLICT(work_id) DO NOTHING",params![entity.as_bytes().as_slice(),acquired_at])?;
            // 本文の文字数は本文ファイルだけで決まる。引き継いだ本文は前に数えた値を使う。
            if let Some(body) = body {
                tx.execute(
                    "INSERT OR IGNORE INTO body_text_stats(file_id,text_length) VALUES(?1,?2)",
                    params![body.file.id.0.as_slice(), sql_size(body.text_length)?],
                )?;
            }
            self.refresh_work_list(&tx, entity)?;
        }
        let result = CaptureResult {
            entity_id: entity,
            snapshot_id: snapshot,
            created_revision: changed,
        };
        self.prepare_capture_reader(&tx, &result, context.as_ref())?;
        Self::finish_capture(
            &tx,
            &result,
            capture.request_id,
            observation,
            request_hash,
            now,
            context.as_ref(),
        )?;
        tx.commit()?;
        Ok(result)
    }

    fn prepare_capture_reader(
        &self,
        tx: &Transaction<'_>,
        result: &CaptureResult,
        context: Option<&super::acquisitions::CompletionContext>,
    ) -> Result<()> {
        let Some(context) = context else {
            return Ok(());
        };
        let Some(bindings) = &context.reader_bindings else {
            return Ok(());
        };
        let plan = self.publish_reader_bindings_in_tx(
            tx,
            result.entity_id,
            result.snapshot_id,
            bindings.clone(),
        )?;
        tx.execute("INSERT INTO acquisition_reader_outputs(attempt_id,entity_id,snapshot_id,recipe,bindings_hash) VALUES(?1,?2,?3,1,?4)",params![context.ticket.id.as_bytes().as_slice(),result.entity_id.as_bytes().as_slice(),result.snapshot_id.as_bytes().as_slice(),plan.bindings_hash.0.as_slice()])?;
        Ok(())
    }

    fn finish_capture(
        tx: &Transaction<'_>,
        result: &CaptureResult,
        request: LibraryId,
        observation: LibraryId,
        request_hash: Fingerprint,
        now: i64,
        context: Option<&super::acquisitions::CompletionContext>,
    ) -> Result<()> {
        tx.execute(
            "UPDATE library_meta SET generation=generation+1 WHERE singleton=1",
            [],
        )?;
        if let Some(context) = context {
            if result.entity_id != context.ticket.entity {
                return Err(Error::Integrity(
                    "取得開始の対象と保存対象が一致しません".into(),
                ));
            }
            tx.execute("UPDATE acquisition_attempts SET state='complete',input_manifest_file_id=?2,observation_id=?3,result_snapshot_id=?4,finished_at=?5 WHERE id=?1",params![context.ticket.id.as_bytes().as_slice(),request_hash.0.as_slice(),observation.as_bytes().as_slice(),result.snapshot_id.as_bytes().as_slice(),now])?;
        }
        tx.execute("INSERT INTO outbox(request_id,entity_id,operation,payload_json,created_at) VALUES(?1,?2,'capture',?3,?4)",params![request.as_bytes().as_slice(),result.entity_id.as_bytes().as_slice(),serde_json::to_string(result)?,now])?;
        Ok(())
    }

    pub(crate) fn relation_entity(
        tx: &Transaction<'_>,
        provider: &str,
        kind: EntityKind,
        key: &str,
        now: i64,
    ) -> Result<LibraryId> {
        if let Some(found)=tx.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind=?2 AND external_key=?3",params![provider,kind.name(),key],|r|r.get::<_,Vec<u8>>(0)).optional()? {
            return uuid(found);
        }
        let id = LibraryId::now_v7();
        tx.execute(
            "INSERT INTO entities(id,kind,created_at) VALUES(?1,?2,?3)",
            params![id.as_bytes().as_slice(), kind.name(), now],
        )?;
        tx.execute(
            "INSERT INTO external_identities VALUES(?1,?2,?3,?4)",
            params![provider, kind.name(), key, id.as_bytes().as_slice()],
        )?;
        match kind {
            EntityKind::Work => {
                tx.execute(
                    "INSERT INTO works(id,content_type) VALUES(?1,'unknown')",
                    [id.as_bytes().as_slice()],
                )?;
            }
            EntityKind::Person => {
                tx.execute(
                    "INSERT INTO people(id) VALUES(?1)",
                    [id.as_bytes().as_slice()],
                )?;
            }
            EntityKind::Series => {
                tx.execute(
                    "INSERT INTO series(id) VALUES(?1)",
                    [id.as_bytes().as_slice()],
                )?;
            }
            _ => return Err(Error::Integrity("関係先の種類が不正です".into())),
        }
        Ok(id)
    }

    fn snapshot_relations(
        tx: &Transaction<'_>,
        snapshot: LibraryId,
        provider: &str,
        metadata: &BTreeMap<String, Value>,
        now: i64,
    ) -> Result<()> {
        let key = |name: &str| {
            metadata
                .get(name)
                .and_then(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .or_else(|| v.as_u64().map(|n| n.to_string()))
                })
                .filter(|s| !s.is_empty() && s != "0")
        };
        if let Some(key) = key("author_key") {
            let person = Self::relation_entity(tx, provider, EntityKind::Person, &key, now)?;
            let name = metadata
                .get("author_name")
                .and_then(Value::as_str)
                .unwrap_or("");
            let role = metadata
                .get("author_role")
                .and_then(Value::as_str)
                .unwrap_or(if provider == "fanbox" {
                    "creator"
                } else {
                    "author"
                });
            tx.execute(
                "INSERT INTO snapshot_people VALUES(?1,?2,?3,?4)",
                params![
                    snapshot.as_bytes().as_slice(),
                    person.as_bytes().as_slice(),
                    role,
                    name
                ],
            )?;
        }
        if let Some(key) = key("series_id") {
            let series = Self::relation_entity(tx, provider, EntityKind::Series, &key, now)?;
            let title = metadata
                .get("series_title")
                .and_then(Value::as_str)
                .unwrap_or("");
            tx.execute(
                "INSERT INTO snapshot_series VALUES(?1,?2,?3,?4)",
                params![
                    snapshot.as_bytes().as_slice(),
                    series.as_bytes().as_slice(),
                    title,
                    metadata.get("series_position").and_then(Value::as_i64)
                ],
            )?;
        }
        if let Some(tags) = metadata.get("tags").and_then(Value::as_array) {
            for name in tags.iter().filter_map(Value::as_str) {
                tx.execute(
                    "INSERT INTO tags VALUES(?1,?2,?3) ON CONFLICT(provider,name) DO NOTHING",
                    params![LibraryId::now_v7().as_bytes().as_slice(), provider, name],
                )?;
                let tag: Vec<u8> = tx.query_row(
                    "SELECT id FROM tags WHERE provider=?1 AND name=?2",
                    params![provider, name],
                    |r| r.get(0),
                )?;
                tx.execute(
                    "INSERT OR IGNORE INTO snapshot_tags VALUES(?1,?2)",
                    params![snapshot.as_bytes().as_slice(), tag],
                )?;
            }
        }
        Ok(())
    }

    pub fn works(&self, limit: usize) -> Result<Vec<WorkSummary>> {
        let conn = self.reader()?;
        let mut stmt = conn.prepare("SELECT work_id,snapshot_id,title,author_name,text_length,logical_size,published_at FROM work_current_projection ORDER BY title,work_id LIMIT ?1")?;
        let rows = stmt.query_map([limit.min(1000) as i64], |r| {
            Ok((
                r.get::<_, Vec<u8>>(0)?,
                r.get::<_, Vec<u8>>(1)?,
                r.get(2)?,
                r.get(3)?,
                read_size(r, 4)?,
                read_size(r, 5)?,
                r.get(6)?,
            ))
        })?;
        rows.map(|row| {
            let (id, snapshot_id, title, author_name, text_length, logical_size, published_at) =
                row?;
            Ok(WorkSummary {
                id: uuid(id)?,
                snapshot_id: uuid(snapshot_id)?,
                title,
                author_name,
                text_length,
                logical_size,
                published_at,
            })
        })
        .collect()
    }

    /// 全ファイルを復元して確かめる。棚の大きさに比例して時間がかかるので、
    /// 書き込み用の接続をふさがず、一ファイルごとに読み取りを手放す。
    pub fn verify(&self) -> Result<u64> {
        let conn = self.reader()?;
        let unsealed: i64 = conn.query_row(
            "SELECT (SELECT count(*) FROM content_files WHERE sealed=0)+(SELECT count(*) FROM revision_snapshots WHERE published=0)+(SELECT count(*) FROM edit_revisions WHERE sealed=0)",
            [],
            |r| r.get(0),
        )?;
        if unsealed != 0 {
            return Err(Error::Integrity("未確定の保存記録があります".into()));
        }
        let fk: Option<String> = conn
            .query_row("PRAGMA foreign_key_check", [], |r| r.get(0))
            .optional()?;
        if let Some(table) = fk {
            return Err(Error::Integrity(format!("外部キー違反: {table}")));
        }
        let check: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
        if check != "ok" {
            return Err(Error::Integrity(check));
        }
        let ids = conn
            .prepare("SELECT id FROM content_files")?
            .query_map([], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(conn);
        let mut count = 0;
        for id in ids {
            let id: [u8; 32] = id
                .try_into()
                .map_err(|_| Error::Integrity("ファイル ID が不正です".into()))?;
            // 確定済みのファイルの構成は変わらない。復元の間は DB を読み続けない。
            let file = Self::file_from_conn(&*self.reader()?, Fingerprint(id))?;
            self.objects.verify(&file)?;
            count += 1;
        }
        let conn = self.reader()?;
        self.verify_edit_documents(&conn)?;
        self.verify_source_bodies(&conn)?;
        self.verify_work_list(&conn)?;
        self.verify_reading_positions(&conn)?;
        self.verify_acquisitions(&conn)?;
        super::source_state::verify_source_state(&conn)?;
        super::searches::verify_searches(&conn)?;
        self.verify_reader_bindings(&conn)?;
        self.verify_job_history(&conn)?;
        Self::verify_maintenance_markers(&conn)?;
        super::integrity::verify_snapshots(&conn)?;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reading_and_verifying_do_not_wait_for_a_save_that_is_being_committed() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let saved = library.capture(capture("work")).unwrap();
        // 保存の確定の途中を模す。書き込み用の接続を持ったまま、未確定の変更を置く。
        let writer = library.conn.lock().unwrap();
        writer
            .execute_batch("BEGIN IMMEDIATE; UPDATE library_meta SET generation=generation+100")
            .unwrap();
        // 接続が一本だった頃は、ここで永久に待った。今は確定済みの状態を読んで返る。
        assert_eq!(
            library.current_snapshot(saved.entity_id).unwrap(),
            saved.snapshot_id
        );
        assert_eq!(library.snapshots(saved.entity_id).unwrap().len(), 1);
        assert_eq!(library.works(10).unwrap().len(), 1);
        let files = library.verify().unwrap();
        assert!(files > 0);
        // 借りた接続は書き込めない。
        assert!(library
            .reader()
            .unwrap()
            .execute("UPDATE library_meta SET generation=0", [])
            .is_err());
        writer.execute_batch("ROLLBACK").unwrap();
        drop(writer);
        // 確定した保存は、次に借りた接続から見える。
        let mut another = capture("another work");
        another.external_key = "456".into();
        library.capture(another).unwrap();
        assert_eq!(library.works(10).unwrap().len(), 2);
        assert!(library.verify().unwrap() > files);
    }

    #[test]
    fn profile_preview_urls_and_object_key_order_do_not_change_source_content() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let profile = |items: Value| {
            let mut input = capture("profile");
            input.kind = EntityKind::Person;
            input.body = None;
            input.metadata.insert("profile_items".into(), items);
            input
        };
        let first=library.capture(profile(serde_json::from_str(r#"[{"id":"one","type":"image","imageUrl":"https://example.test/original","thumbnailUrl":"https://example.test/small"}]"#).unwrap())).unwrap();
        let next=library.capture(profile(serde_json::from_str(r#"[{"imageUrl":"https://example.test/original","id":"one","thumbnailUrl":"https://example.test/large","type":"image"}]"#).unwrap())).unwrap();
        assert!(!next.created_revision);
        let conn = library.conn.lock().unwrap();
        let hash = |id: LibraryId| {
            conn.query_row(
                "SELECT semantic_hash FROM revision_snapshots WHERE id=?1",
                [id.as_bytes().as_slice()],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .unwrap()
        };
        assert_eq!(hash(first.snapshot_id), hash(next.snapshot_id));
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn structured_body_order_changes_are_revisions_and_preview_changes_only_add_a_snapshot() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let original = json!({"blocks":[{"type":"p","text":"one"},{"type":"image","imageId":"picture"},{"type":"p","text":"two"}],"imageMap":{"picture":{"originalUrl":"https://example.test/original.png","thumbnailUrl":"https://example.test/small.png"}}});
        let article = |data: Value| {
            let mut input = capture("article");
            input.provider = "fanbox".into();
            input.body = Some(SourceBody::FanboxBody(data));
            input
        };
        let first = library.capture(article(original.clone())).unwrap();
        let mut preview = original.clone();
        preview["imageMap"]["picture"]["thumbnailUrl"] = json!("https://example.test/large.png");
        let repaired = library.capture(article(preview.clone())).unwrap();
        assert!(!repaired.created_revision);
        let mut reordered = preview.clone();
        reordered["blocks"].as_array_mut().unwrap().swap(1, 2);
        let revised = library.capture(article(reordered.clone())).unwrap();
        assert!(revised.created_revision);
        let snapshots = library.snapshots(first.entity_id).unwrap();
        assert_eq!(
            snapshots.iter().map(|s| s.revision).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        for (snapshot, expected) in [
            (first.snapshot_id, original),
            (repaired.snapshot_id, preview),
            (revised.snapshot_id, reordered),
        ] {
            let source = library
                .source_body(first.entity_id, snapshot)
                .unwrap()
                .unwrap();
            let SourceBody::FanboxBody(actual) = source else {
                panic!("本文形式が変わりました")
            };
            assert_eq!(actual, expected);
        }
        assert_eq!(snapshots[0].body_format, Some(BodyFormat::FanboxBody));
        assert_ne!(snapshots[0].body_file, snapshots[1].body_file);
        library.verify().unwrap();
    }

    #[test]
    fn incomplete_body_inputs_are_preserved_as_observations_and_do_not_replace_known_body() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let partial = |text: &str| {
            let mut input = capture("title");
            input.body = Some(text.into());
            input
                .field_states
                .insert("body".into(), "restricted".into());
            input
        };
        let first = library.capture(partial("partial input")).unwrap();
        assert!(library
            .source_body(first.entity_id, first.snapshot_id)
            .unwrap()
            .is_none());
        let mut full = capture("title");
        full.body = Some("full body".into());
        let full = library.capture(full).unwrap();
        assert!(!full.created_revision);
        let ignored = library.capture(partial("different partial input")).unwrap();
        assert!(!ignored.created_revision);
        assert_eq!(
            library
                .source_body(first.entity_id, ignored.snapshot_id)
                .unwrap()
                .unwrap()
                .text_content(),
            "full body"
        );
        assert_eq!(
            library
                .snapshots(first.entity_id)
                .unwrap()
                .last()
                .unwrap()
                .field_states["body"],
            "known"
        );
        let mut missing = partial("partial of changed work");
        missing
            .metadata
            .insert("title".into(), json!("changed title"));
        assert!(library.capture(missing).is_err());
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            ignored.snapshot_id
        );
        let conn = library.conn.lock().unwrap();
        let file: Vec<u8> = conn
            .query_row(
                "SELECT body_file_id FROM acquisition_observations WHERE result_snapshot_id=?1",
                [first.snapshot_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(file, Fingerprint::of(b"partial input").0);
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn equivalent_publication_date_formats_share_a_revision_and_keep_each_raw_response() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let milliseconds = 1735754645123_i64;
        for (n, date) in [
            json!("2025-01-02T03:04:05.123+09:00"),
            json!("2025-01-01T18:04:05.123Z"),
            json!(milliseconds),
        ]
        .into_iter()
        .enumerate()
        {
            let raw = serde_json::to_vec(&json!({"create_date":date})).unwrap();
            let mut input = capture("title");
            input.metadata.insert("create_date".into(), date);
            input.raw = raw.clone().into();
            let saved = library.capture(input).unwrap();
            assert_eq!(saved.created_revision, n == 0);
            assert_eq!(
                library.works(1).unwrap()[0].published_at,
                Some(milliseconds)
            );
            let mut restored = Vec::new();
            library
                .objects
                .export(&library.file(Fingerprint::of(&raw)).unwrap(), &mut restored)
                .unwrap();
            assert_eq!(restored, raw);
            let snapshots = library.snapshots(saved.entity_id).unwrap();
            assert_eq!(snapshots.len(), n + 1);
            assert!(snapshots
                .iter()
                .all(|s| s.revision_id == snapshots[0].revision_id
                    && s.metadata["create_date"] == json!(milliseconds)));
        }
        library.verify().unwrap();
    }

    #[test]
    fn acquisition_time_is_preserved_without_using_the_clock_to_order_or_select_history() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let mut first = capture("title");
        first.acquired_at = Some(20);
        let request = first.request_id;
        let first = library.capture(first).unwrap();
        let mut next = capture("title");
        next.acquired_at = Some(10);
        let next = library.capture(next).unwrap();
        assert!(!next.created_revision);
        let snapshots = library.snapshots(first.entity_id).unwrap();
        assert_eq!(
            snapshots
                .iter()
                .map(|s| (s.sequence, s.saved_at))
                .collect::<Vec<_>>(),
            vec![(1, 20), (2, 10)]
        );
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            next.snapshot_id
        );
        let mut retry = capture("title");
        retry.acquired_at = Some(20);
        retry.request_id = request;
        assert_eq!(
            library.capture(retry).unwrap().snapshot_id,
            first.snapshot_id
        );
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            next.snapshot_id
        );
        let mut changed = capture("title");
        changed.acquired_at = Some(21);
        changed.request_id = request;
        assert!(library.capture(changed).is_err());
        let mut invalid = capture("title");
        invalid.acquired_at = Some(i64::MAX);
        assert!(library.capture(invalid).is_err());
        let conn = library.conn.lock().unwrap();
        let (acquired, recorded): (i64, i64) = conn
            .query_row(
                "SELECT acquired_at,recorded_at FROM acquisition_observations WHERE request_id=?1",
                [request.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(acquired, 20);
        assert!(recorded > acquired);
        assert_eq!(
            conn.query_row("SELECT added_at FROM library_items", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            20
        );
        drop(conn);
        library.verify().unwrap();
    }

    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("piep-library-{}", LibraryId::now_v7())))
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn capture(title: &str) -> Capture {
        Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "pixiv".into(),
            external_key: "123".into(),
            kind: EntityKind::Work,
            route: "test".into(),
            raw: b"{ \"text\": \"original\" }\r\n".to_vec().into(),
            metadata: BTreeMap::from([
                ("title".into(), json!(title)),
                ("author_name".into(), json!("作者")),
            ]),
            field_states: BTreeMap::from([("caption".into(), "unknown".into())]),
            body: Some("本文\n\n原本".into()),
            media: vec![],
            attachments: vec![],
        }
    }

    fn image(quality: &str, bytes: &[u8]) -> CapturedMedia {
        CapturedMedia {
            slot: "cover".into(),
            role: "cover".into(),
            source_key: Some("same-source-image".into()),
            source_url: Some(format!("https://example.invalid/{quality}")),
            quality: quality.into(),
            original_name: format!("{quality}.png"),
            mime_type: "image/png".into(),
            bytes: Some(bytes.to_vec().into()),
        }
    }

    #[test]
    fn renditions_are_reused_and_runtime_capture_never_downgrades_a_known_original() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let input = |quality, bytes: &[u8]| {
            let mut c = capture("title");
            c.media.push(image(quality, bytes));
            c
        };
        let first = library.capture(input("original", b"original")).unwrap();
        let second = library.capture(input("original", b"original")).unwrap();
        assert!(!second.created_revision);
        {
            let conn = library.conn.lock().unwrap();
            assert_eq!(
                conn.query_row("SELECT count(*) FROM media_renditions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            assert_eq!(
                conn.query_row("SELECT count(*) FROM media_observations", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                2
            );
        }
        let old = library
            .capture_with_legacy_classification(input("thumbnail", b"thumbnail"), Some("source"))
            .unwrap();
        assert_eq!(
            library
                .media_file(first.entity_id, old.snapshot_id, "cover")
                .unwrap()
                .id,
            Fingerprint::of(b"thumbnail")
        );
        let current = library.capture(input("thumbnail", b"thumbnail")).unwrap();
        assert!(!current.created_revision);
        assert_eq!(
            library
                .media_file(first.entity_id, current.snapshot_id, "cover")
                .unwrap()
                .id,
            Fingerprint::of(b"original")
        );
        {
            let conn = library.conn.lock().unwrap();
            let name: String = conn
                .query_row(
                    "SELECT original_name FROM snapshot_media WHERE snapshot_id=?1",
                    [current.snapshot_id.as_bytes().as_slice()],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(name, "original.png");
        }
        let mut changed = input("thumbnail", b"different picture");
        changed.media[0].source_key = Some("different-source-image".into());
        let changed = library.capture(changed).unwrap();
        assert!(changed.created_revision);
        assert_eq!(
            library
                .media_file(first.entity_id, changed.snapshot_id, "cover")
                .unwrap()
                .id,
            Fingerprint::of(b"different picture")
        );
        library.verify().unwrap();
    }

    #[test]
    fn unknown_quality_is_not_assumed_to_be_a_thumbnail() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        for (quality, bytes) in [
            ("master", b"master".as_slice()),
            ("unknown", b"unknown".as_slice()),
            ("thumbnail", b"thumb".as_slice()),
        ] {
            let mut input = capture("title");
            input.media.push(image(quality, bytes));
            let saved = library.capture(input).unwrap();
            let expected = if quality == "thumbnail" {
                b"master".as_slice()
            } else {
                bytes
            };
            assert_eq!(
                library
                    .media_file(saved.entity_id, saved.snapshot_id, "cover")
                    .unwrap()
                    .id,
                Fingerprint::of(expected)
            );
        }
        library.verify().unwrap();
    }

    #[test]
    fn supplementary_images_and_metadata_do_not_create_author_revisions() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let mut initial = capture("題名");
        initial.media.push(image("thumbnail", b"thumbnail bytes"));
        let first = library.capture(initial).unwrap();
        assert!(first.created_revision);
        let mut next = capture("題名");
        next.media
            .push(image("original", b"original bytes with ICC and EXIF"));
        next.metadata
            .insert("caption".into(), json!("補完した説明"));
        next.field_states.insert("caption".into(), "known".into());
        let supplemented = library.capture(next).unwrap();
        assert_eq!(first.entity_id, supplemented.entity_id);
        assert_ne!(first.snapshot_id, supplemented.snapshot_id);
        assert!(!supplemented.created_revision);
        {
            let conn = library.conn.lock().unwrap();
            assert_eq!(
                conn.query_row("SELECT count(*) FROM source_revisions", [], |r| r
                    .get::<_, i64>(0))
                    .unwrap(),
                1
            );
            let hashes:Vec<Vec<u8>>=conn.prepare("SELECT f.id FROM snapshot_media s JOIN media_renditions r ON r.id=s.rendition_id JOIN content_files f ON f.id=r.file_id ORDER BY s.snapshot_id").unwrap().query_map([],|r|r.get(0)).unwrap().collect::<std::result::Result<_,_>>().unwrap();
            assert_eq!(hashes.len(), 2);
            assert_ne!(hashes[0], hashes[1]);
        }
        let edit = library.capture(capture("変更した題名")).unwrap();
        assert!(edit.created_revision);
        let reverted = library.capture(capture("題名")).unwrap();
        assert!(reverted.created_revision);
        assert_eq!(
            library
                .conn
                .lock()
                .unwrap()
                .query_row("SELECT count(*) FROM source_revisions", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            3
        );
        assert_eq!(library.works(10).unwrap()[0].title, "題名");
        library.verify().unwrap();
        assert!(matches!(Library::open(&temp.0), Err(Error::Busy)));
        drop(library);
        Library::open(&temp.0).unwrap().verify().unwrap();
    }

    #[test]
    fn retried_requests_are_idempotent_and_conflicting_retries_are_rejected() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let request = LibraryId::now_v7();
        let mut first = capture("同じ要求");
        first.request_id = request;
        let result = library.capture(first).unwrap();
        let mut retry = capture("同じ要求");
        retry.request_id = request;
        assert_eq!(
            library.capture(retry).unwrap().snapshot_id,
            result.snapshot_id
        );
        let mut conflicting = capture("別の内容");
        conflicting.request_id = request;
        assert!(library.capture(conflicting).is_err());
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM revision_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row("SELECT count(*) FROM outbox", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn a_failed_capture_rolls_back_its_catalogue_and_does_not_overwrite_a_snapshot() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let first = library.capture(capture("元の題名")).unwrap();
        let mut invalid = capture("失敗した変更");
        invalid.media.push(image("invalid-quality", b"invalid"));
        assert!(library.capture(invalid).is_err());
        assert_eq!(library.works(10).unwrap()[0].snapshot_id, first.snapshot_id);
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM source_revisions", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert!(conn
            .execute(
                "UPDATE revision_snapshots SET metadata_json='{}' WHERE id=?1",
                [first.snapshot_id.as_bytes().as_slice()]
            )
            .is_err());
        let foreign = LibraryId::now_v7();
        conn.execute(
            "INSERT INTO entities VALUES(?1,'work',0,NULL)",
            [foreign.as_bytes().as_slice()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO works(id,content_type) VALUES(?1,'novel')",
            [foreign.as_bytes().as_slice()],
        )
        .unwrap();
        assert!(conn
            .execute(
                "INSERT INTO entity_current(entity_id,snapshot_id,generation) VALUES(?1,?2,1)",
                params![
                    foreign.as_bytes().as_slice(),
                    first.snapshot_id.as_bytes().as_slice()
                ]
            )
            .is_err());
        assert!(conn
            .execute(
                "INSERT INTO edit_revisions(id,work_id,base_snapshot_id,content_file_id,created_at) VALUES(?1,?2,?3,?4,0)",
                params![
                    LibraryId::now_v7().as_bytes().as_slice(),
                    foreign.as_bytes().as_slice(),
                    first.snapshot_id.as_bytes().as_slice(),
                    Fingerprint::of(b"missing").0.as_slice()
                ]
            )
            .is_err());
    }

    #[test]
    fn creation_restarts_after_the_database_is_committed_but_the_marker_is_not_published() {
        let temp = Temp::new();
        let id = {
            let library = Library::create(&temp.0).unwrap();
            library.id
        };
        fs::rename(temp.0.join("library.json"), temp.0.join(".creation.json")).unwrap();
        assert!(Library::open(&temp.0).is_err());
        let recovered = Library::create(&temp.0).unwrap();
        assert_eq!(recovered.id, id);
        recovered.verify().unwrap();
        drop(recovered);
        assert!(Library::create(&temp.0).is_err());
        assert_eq!(Library::open(&temp.0).unwrap().id, id);
    }

    #[test]
    fn author_display_names_and_tag_order_do_not_create_a_source_revision() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let mut first = capture("題名");
        first
            .metadata
            .insert("author_key".into(), serde_json::json!(123));
        first
            .metadata
            .insert("tags".into(), serde_json::json!(["乙", "甲"]));
        let initial = library.capture(first).unwrap();
        let mut next = capture("題名");
        next.metadata
            .insert("author_key".into(), serde_json::json!(123));
        next.metadata
            .insert("author_name".into(), serde_json::json!("新しい作者名"));
        next.metadata
            .insert("tags".into(), serde_json::json!(["甲", "乙", "甲"]));
        let saved = library.capture(next).unwrap();
        assert!(!saved.created_revision);
        let conn = library.conn.lock().unwrap();
        let first: Vec<u8> = conn
            .query_row(
                "SELECT semantic_hash FROM revision_snapshots WHERE id=?1",
                [initial.snapshot_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        let second: Vec<u8> = conn
            .query_row(
                "SELECT semantic_hash FROM revision_snapshots WHERE id=?1",
                [saved.snapshot_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(first, second);
        assert!(conn
            .execute(
                "DELETE FROM snapshot_tags WHERE snapshot_id=?1",
                [saved.snapshot_id.as_bytes().as_slice()]
            )
            .is_err());
        assert!(conn
            .execute("UPDATE source_revisions SET ordinal=50", [])
            .is_err());
        assert_eq!(
            conn.query_row("SELECT count(*) FROM people", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
    }

    #[test]
    fn current_logical_size_counts_raw_attachments_and_media_once_per_file_without_the_derived_body(
    ) {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let mut input = capture("題名");
        input.raw = b"raw provenance".to_vec().into();
        input.body = Some("本文".into());
        input.media = vec![image("original", b"shared picture")];
        input.attachments = vec![
            CapturedAttachment {
                slot: "same".into(),
                original_name: "picture.png".into(),
                mime_type: None,
                bytes: b"shared picture".to_vec().into(),
            },
            CapturedAttachment {
                slot: "other".into(),
                original_name: "other.zip".into(),
                mime_type: None,
                bytes: b"attachment".to_vec().into(),
            },
        ];
        library.capture(input).unwrap();
        assert_eq!(
            library.works(1).unwrap()[0].logical_size,
            (b"raw provenance".len() + b"shared picture".len() + b"attachment".len()) as u64
        );
    }

    #[test]
    fn profile_identity_and_current_state_are_explicit_and_quality_is_not_a_revision() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let profile = |name: &str, quality: &str, bytes: &[u8]| Capture {
            acquired_at: None,
            request_id: LibraryId::now_v7(),
            provider: "local".into(),
            external_key: "author".into(),
            kind: EntityKind::Person,
            route: "test".into(),
            raw: b"{}".to_vec().into(),
            metadata: BTreeMap::from([("display_name".into(), json!(name))]),
            field_states: BTreeMap::from([("media".into(), "known".into())]),
            body: None,
            media: vec![CapturedMedia {
                slot: "icon".into(),
                role: "icon".into(),
                source_key: Some("same-author-picture".into()),
                source_url: None,
                quality: quality.into(),
                original_name: "icon.png".into(),
                mime_type: "image/png".into(),
                bytes: Some(bytes.to_vec().into()),
            }],
            attachments: vec![],
        };
        let first = library
            .capture(profile("Old name", "thumbnail", b"thumbnail"))
            .unwrap();
        let repaired = library
            .capture(profile("Old name", "original", b"original"))
            .unwrap();
        assert!(!repaired.created_revision);
        let renamed = library
            .capture(profile("New name", "original", b"original"))
            .unwrap();
        assert!(renamed.created_revision);
        assert_eq!(
            library.current_snapshot(first.entity_id).unwrap(),
            renamed.snapshot_id
        );
        let states = library.snapshots(first.entity_id).unwrap();
        assert_eq!(
            states.iter().map(|s| s.revision).collect::<Vec<_>>(),
            vec![1, 1, 2]
        );
        assert_eq!(
            library
                .media_file(first.entity_id, first.snapshot_id, "icon")
                .unwrap()
                .id,
            Fingerprint::of(b"thumbnail")
        );
        assert!(library.works(10).unwrap().is_empty());
        library.verify().unwrap();
    }

    #[test]
    fn known_media_can_be_missing_then_filled_without_changing_the_source_revision() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let mut first = capture("題名");
        let mut missing = image("unknown", b"unused");
        missing.bytes = None;
        first.media.push(missing);
        first.field_states.insert("media".into(), "known".into());
        let initial = library.capture(first).unwrap();
        assert!(library
            .media_file(initial.entity_id, initial.snapshot_id, "cover")
            .is_err());
        let mut repaired = capture("題名");
        repaired.media.push(image("original", b"repaired"));
        repaired.field_states.insert("media".into(), "known".into());
        let repaired = library.capture(repaired).unwrap();
        assert!(!repaired.created_revision);
        assert!(library
            .media_file(initial.entity_id, initial.snapshot_id, "cover")
            .is_err());
        library
            .media_file(repaired.entity_id, repaired.snapshot_id, "cover")
            .unwrap();
    }

    #[test]
    fn attachment_replacement_is_a_revision_and_a_complete_empty_list_removes_current_references() {
        let temp = Temp::new();
        let library = Library::create(&temp.0).unwrap();
        let attachment = |bytes: &[u8]| CapturedAttachment {
            slot: "file:one".into(),
            original_name: "one.pdf".into(),
            mime_type: Some("application/pdf".into()),
            bytes: bytes.to_vec().into(),
        };
        let mut first = capture("題名");
        first.attachments.push(attachment(b"original document"));
        first
            .field_states
            .insert("attachments".into(), "known".into());
        let first = library.capture(first).unwrap();
        let mut second = capture("題名");
        second.attachments.push(attachment(b"changed document"));
        second
            .field_states
            .insert("attachments".into(), "known".into());
        assert!(library.capture(second).unwrap().created_revision);
        let mut third = capture("題名");
        third
            .field_states
            .insert("attachments".into(), "known".into());
        let third = library.capture(third).unwrap();
        assert!(third.created_revision);
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM snapshot_files WHERE snapshot_id=?1",
                [first.snapshot_id.as_bytes().as_slice()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            1
        );
        assert_eq!(
            conn.query_row(
                "SELECT count(*) FROM snapshot_files WHERE snapshot_id=?1",
                [third.snapshot_id.as_bytes().as_slice()],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
    }
}
