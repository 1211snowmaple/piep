use crate::{Error, Fingerprint, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

const MAGIC: &[u8; 8] = b"PIEPOB01";
pub const OBJECT_HEADER_BYTES: u64 = 57;
pub const MAX_CHUNK_ORIGINAL_BYTES: u64 = 8 * 1024 * 1024;
// v1 の既存デコーダーが受け入れる上限。新規エンコードは原寸以下に収める。
pub const MAX_CHUNK_STORED_BYTES: u64 = 16 * 1024 * 1024;
pub const MAX_ENCODED_CHUNK_BYTES: u64 = OBJECT_HEADER_BYTES + MAX_CHUNK_STORED_BYTES;
const HEADER_BYTES: u64 = OBJECT_HEADER_BYTES;
const CHUNK_BYTES: usize = 4 * 1024 * 1024;
const SINGLE_FILE_BYTES: usize = 8 * 1024 * 1024;

/// 元ファイルを復元するための、順序付きの部分実体。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileChunk {
    pub id: Fingerprint,
    pub length: u64,
}

/// 同じ元バイト列は、ファイル名・作品・保存場所によらず同じ識別子になる。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContentFile {
    pub id: Fingerprint,
    pub length: u64,
    pub chunks: Vec<FileChunk>,
}

impl ContentFile {
    /// v1 の長さ・個数・分割位置を検査する。実体の存在やバイト列の検証とは別。
    pub fn validate_layout(&self) -> Result<()> {
        ObjectStore::validate_manifest(self)
    }
}

/// 保存済みの部分実体の状態。未受信と破損を区別する。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChunkState {
    /// ヘッダー・長さ・展開結果・元ハッシュがすべて一致した。
    Verified,
    /// まだ保存していない。
    Missing,
    /// 保存したものが読めないか、中身が一致しない。正しい中身を受け取れば置き換えられる。
    Damaged,
}

/// 壊れた実体とみなす失敗。権限や安全でないパスは修復の対象にしない。
fn damaged(error: &Error) -> bool {
    match error {
        Error::Integrity(_) => true,
        Error::Io(e) => matches!(
            e.kind(),
            std::io::ErrorKind::InvalidData
                | std::io::ErrorKind::UnexpectedEof
                | std::io::ErrorKind::Other
        ),
        Error::UnsafePath(_) => false,
    }
}

fn missing(error: &Error) -> bool {
    matches!(error, Error::Io(e) if e.kind() == std::io::ErrorKind::NotFound)
}

/// 一つの実体の保存表現。容量の集計と、呼び手の台帳の記録に使う。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkEncoding {
    /// `raw` か `zstd`。
    pub codec: &'static str,
    /// ヘッダーを含む、保存したファイルの長さ。
    pub stored_bytes: u64,
}

/// 保管して検証を終えたファイル。`put_stored` と `inspect` だけが作る。
/// 全部分の展開結果と元ハッシュを、受け取った時点でディスクから読んで確かめてある。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredFile {
    pub file: ContentFile,
    /// `file.chunks` と同じ順番。
    pub encodings: Vec<ChunkEncoding>,
}

#[derive(Clone, Copy, Default, Debug, Serialize)]
pub struct StorageStats {
    pub chunks: u64,
    pub original_bytes: u64,
    pub stored_bytes: u64,
    pub compressed_chunks: u64,
}

/// 実体は不変。既存の内容を上書きせず、検証を通ったものだけを公開する。
#[derive(Clone)]
pub struct ObjectStore {
    root: PathBuf,
}

struct Pending(PathBuf);
impl Drop for Pending {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub fn regular_directory(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Error::UnsafePath(path.into()));
        }
    }
    if meta.file_type().is_symlink() || !meta.is_dir() {
        return Err(Error::UnsafePath(path.into()));
    }
    Ok(())
}

pub fn regular_file(path: &Path) -> Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x400 != 0 {
            return Err(Error::UnsafePath(path.into()));
        }
    }
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(Error::UnsafePath(path.into()));
    }
    Ok(meta)
}

/// 原本の実体を DB から参照する前に、ディレクトリの公開も同期する。
/// 使い回した実体の更新時刻を今にする。回収は、未参照でも最後に使われてから猶予が
/// 過ぎていない実体を退けない。保存は実体を置いた（使い回した）あとで台帳に確定するので、
/// その間の実体を未参照と見て退けないための印になる。印を付けられなくても保存は続ける
/// （回収の猶予が短くなるだけで、退けた実体は台帳が参照すれば戻せる）。
fn freshen(path: &Path) {
    let _ = OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(std::time::SystemTime::now()));
}

/// 実体領域にあるファイルの種類。回収が一覧に使う。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoredEntry {
    /// 公開済みの実体 `<hash>.obj`。
    Object(Fingerprint),
    /// 書き込み途中で残った一時ファイル `<hash>.<UUID>.part`。
    Part,
    /// 壊れて退けた実体 `<hash>.<UUID>.damaged`。
    Damaged,
    /// 回収で隔離した実体 `<hash>.<隔離した時刻 ms>.quarantined`。
    Quarantined { id: Fingerprint, at_ms: i64 },
}

/// 実体領域の一つのファイル。
#[derive(Clone, Debug)]
pub struct StoredListing {
    pub entry: StoredEntry,
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub bytes: u64,
}

pub fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .open(path)?
    };
    #[cfg(not(windows))]
    let file = File::open(path)?;
    match file.sync_all() {
        Ok(()) => Ok(()),
        // Windows の一部のファイルシステムはディレクトリの flush を提供しない。
        // ファイルの sync_all は常に必要で、ここで無視できるのは未対応の場合だけ。
        #[cfg(windows)]
        Err(error) if matches!(error.raw_os_error(), Some(1 | 5 | 6 | 50)) => Ok(()),
        Err(error) => Err(error.into()),
    }
}

// Android では hard link の利用を前提にしない。
// https://man7.org/linux/man-pages/man2/rename.2.html
#[cfg(target_os = "android")]
fn publish_new(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};
    let source = CString::new(source.as_os_str().as_bytes())?;
    let destination = CString::new(destination.as_os_str().as_bytes())?;
    // SAFETY: 両 CString は呼出し中有効な NUL 終端パス。fd と flags は Linux ABI の値。
    // libc wrapper は API 30 以降なので minSdk 24 では syscall を使う。
    // NOREPLACE が非対応ならエラーにし、上書きする rename へは切り替えない。
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            libc::AT_FDCWD,
            source.as_ptr(),
            libc::AT_FDCWD,
            destination.as_ptr(),
            1u32,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// Windows の API へ直に渡すパスを、260 文字を超えても通る形（`\\?\` を付けた完全なパス）に
/// する。std の関数は自分でこうするが、直に呼ぶ API には渡す側で付ける必要がある。
/// https://learn.microsoft.com/windows/win32/fileio/maximum-file-path-limitation
#[cfg(windows)]
fn verbatim(path: &Path) -> std::io::Result<std::path::PathBuf> {
    let full = std::path::absolute(path)?;
    let text = full.as_os_str().to_string_lossy();
    if text.starts_with(r"\\?\") || text.starts_with(r"\\.\") {
        return Ok(full);
    }
    let mut prefixed = std::ffi::OsString::new();
    if let Some(share) = text.strip_prefix(r"\\") {
        // UNC の共有（\\server\share）は \\?\UNC\server\share にする。
        if text.len() != full.as_os_str().len() {
            return Ok(full);
        }
        prefixed.push(r"\\?\UNC\");
        prefixed.push(share);
    } else {
        prefixed.push(r"\\?\");
        prefixed.push(full.as_os_str());
    }
    Ok(prefixed.into())
}

// Windows は置き換えない名前変更で公開する。hard link は exFAT・FAT32 の外付けドライブや
// 一部のネットワーク共有で使えず、棚をそこへ置くと保存がすべて失敗していた。
// MOVEFILE_REPLACE_EXISTING を付けないので、既存の実体があれば ERROR_ALREADY_EXISTS になる。
// https://learn.microsoft.com/windows/win32/api/winbase/nf-winbase-movefileexw
#[cfg(windows)]
fn publish_new(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
    }
    const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
    let wide = |path: &Path| -> std::io::Result<Vec<u16>> {
        let mut units: Vec<u16> = verbatim(path)?.as_os_str().encode_wide().collect();
        if units.contains(&0) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "パスに NUL が含まれています",
            ));
        }
        units.push(0);
        Ok(units)
    };
    let source = wide(source)?;
    let destination = wide(destination)?;
    // SAFETY: 両方とも呼出し中有効な NUL 終端の UTF-16 パス。
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved != 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(any(target_os = "android", windows)))]
fn publish_new(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::hard_link(source, destination)
}

fn validate_header(
    header: &[u8; HEADER_BYTES as usize],
    expected: &FileChunk,
) -> Result<(u64, u64)> {
    let original = u64::from_le_bytes(header[9..17].try_into().unwrap());
    let stored = u64::from_le_bytes(header[17..25].try_into().unwrap());
    if &header[..8] != MAGIC
        || header[8] > 1
        || original != expected.length
        || original > MAX_CHUNK_ORIGINAL_BYTES
        || stored > MAX_CHUNK_STORED_BYTES
        || header[25..57] != expected.id.0
        || (header[8] == 0 && stored != original)
    {
        return Err(Error::Integrity(format!(
            "部分実体のヘッダーが不正です: {}",
            expected.id
        )));
    }
    Ok((original, stored))
}

fn decode_object(source: impl Read, encoded_length: u64, expected: &FileChunk) -> Result<Vec<u8>> {
    decode_encoded(source, encoded_length, expected).map(|(bytes, _)| bytes)
}

/// 展開結果と一緒に、読んだ実体の保存表現も返す。同じ実体を二度読まずに済ませる。
/// 書き終えた一時ファイルを、開き直さずに頭から読み戻して復元する。
///
/// 書いた直後のファイルを開き直すと、Windows では開くたびに数ミリ秒待たされる
/// （常駐の検査が新しいファイルを開くところで走る）。棚全体（実体 13 万個）を保管したときの
/// 実測では、保管の時間の 6 割がこの開き直しだった。
/// 確定（`sync_all`）の後に同じ手で読むので、確かめている中身は開き直した場合と同じ。
fn read_back(mut file: File, expected: &FileChunk) -> Result<Vec<u8>> {
    let length = file.metadata()?.len();
    file.rewind()?;
    decode_encoded(file, length, expected).map(|(bytes, _)| bytes)
}

fn decode_encoded(
    mut source: impl Read,
    encoded_length: u64,
    expected: &FileChunk,
) -> Result<(Vec<u8>, ChunkEncoding)> {
    if !(HEADER_BYTES..=MAX_ENCODED_CHUNK_BYTES).contains(&encoded_length) {
        return Err(Error::Integrity("部分実体の保存サイズが不正です".into()));
    }
    let mut header = [0; HEADER_BYTES as usize];
    source.read_exact(&mut header)?;
    let (original, stored) = validate_header(&header, expected)?;
    if encoded_length != HEADER_BYTES + stored {
        return Err(Error::Integrity(
            "部分実体の保存サイズが一致しません".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(original as usize);
    if header[8] == 0 {
        source.take(original + 1).read_to_end(&mut bytes)?;
    } else {
        let mut decoder = zstd::stream::read::Decoder::new(source)?;
        decoder.window_log_max(23)?;
        decoder.take(original + 1).read_to_end(&mut bytes)?;
    }
    if bytes.len() as u64 != original || Fingerprint::of(&bytes) != expected.id {
        return Err(Error::Integrity(format!(
            "部分実体の元データが一致しません: {}",
            expected.id
        )));
    }
    let encoding = ChunkEncoding {
        codec: if header[8] == 1 { "zstd" } else { "raw" },
        stored_bytes: encoded_length,
    };
    Ok((bytes, encoding))
}

/// 固定した v1 のデコーダー。出力・圧縮ウィンドウ・入力サイズを制限する。
/// 圧縮して十分に小さくなれば圧縮した中身を、ならなければ `None` を返す（保管の判断と同じ）。
fn compress(original: &[u8]) -> Result<Option<Vec<u8>>> {
    let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3)?;
    encoder.include_checksum(true)?;
    encoder.set_pledged_src_size(Some(original.len() as u64))?;
    encoder.write_all(original)?;
    let compressed = encoder.finish()?;
    let saving = 1024usize.max(original.len().div_ceil(100));
    let use_compressed = compressed
        .len()
        .checked_add(saving)
        .is_some_and(|required| required <= original.len());
    Ok(use_compressed.then_some(compressed))
}

/// 元ファイルを保管の分け方で区切り、部分ごとに `each` を呼ぶ。全体の識別子を求める。
fn split(
    mut source: impl Read,
    mut each: impl FnMut(&[u8]) -> Result<FileChunk>,
) -> Result<ContentFile> {
    let mut initial = Vec::new();
    source
        .by_ref()
        .take((SINGLE_FILE_BYTES + 1) as u64)
        .read_to_end(&mut initial)?;
    let mut whole = Sha256::new();
    let mut length = 0u64;
    let mut chunks = Vec::new();
    if initial.len() <= SINGLE_FILE_BYTES {
        whole.update(&initial);
        length = initial.len() as u64;
        chunks.push(each(&initial)?);
    } else {
        let mut source = Cursor::new(initial).chain(source);
        loop {
            let mut block = Vec::new();
            source
                .by_ref()
                .take(CHUNK_BYTES as u64)
                .read_to_end(&mut block)?;
            if block.is_empty() {
                break;
            }
            whole.update(&block);
            length = length
                .checked_add(block.len() as u64)
                .ok_or_else(|| Error::Integrity("元サイズが上限を超えました".into()))?;
            chunks.push(each(&block)?);
        }
    }
    Ok(ContentFile {
        id: Fingerprint(whole.finalize().into()),
        length,
        chunks,
    })
}

/// 元のバイト列を、保管と同じ形（ヘッダーと中身）に符号化する。`export_chunk` が返す形と
/// 同じなので、受け手の `import_chunk` へそのまま渡せる。置き場には書かない。
pub fn encode_chunk(original: &[u8]) -> Result<(FileChunk, Vec<u8>)> {
    if original.len() > SINGLE_FILE_BYTES {
        return Err(Error::Integrity("部分実体が上限を超えています".into()));
    }
    let chunk = FileChunk {
        id: Fingerprint::of(original),
        length: original.len() as u64,
    };
    let compressed = compress(original)?;
    let payload = compressed.as_deref().unwrap_or(original);
    let mut encoded = Vec::with_capacity(HEADER_BYTES as usize + payload.len());
    encoded.extend_from_slice(MAGIC);
    encoded.push(u8::from(compressed.is_some()));
    encoded.extend_from_slice(&(original.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    encoded.extend_from_slice(&chunk.id.0);
    encoded.extend_from_slice(payload);
    // 符号化したものが元に戻ることを確かめてから渡す。
    if decode_chunk(&encoded, &chunk)? != original {
        return Err(Error::Integrity("符号化した部分が元に戻りません".into()));
    }
    Ok((chunk, encoded))
}

pub fn decode_chunk(encoded: &[u8], expected: &FileChunk) -> Result<Vec<u8>> {
    decode_object(Cursor::new(encoded), encoded.len() as u64, expected)
}

impl ObjectStore {
    /// 空の管理領域か既存の実体領域を開く。外部のパスを実体名に使わない。
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        regular_directory(root)?;
        Ok(Self {
            root: root.canonicalize()?,
        })
    }

    fn chunk_path(&self, id: Fingerprint, create: bool) -> Result<PathBuf> {
        regular_directory(&self.root)?;
        let hex = id.to_string();
        let dir = self.root.join(&hex[..2]);
        if create {
            match fs::create_dir(&dir) {
                Ok(()) => sync_directory(&self.root)?,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
                Err(e) => return Err(e.into()),
            }
        }
        regular_directory(&dir)?;
        Ok(dir.join(format!("{hex}.obj")))
    }

    /// 任意の読取元を、最大約 8 MiB の入力バッファで保管する。
    pub fn put(&self, source: impl Read) -> Result<ContentFile> {
        self.put_stored(source).map(|stored| stored.file)
    }

    /// `put` と同じ保管をして、各部分の保存表現も返す。
    /// 保管の中で全部分を読み戻して確かめるので、直後に `verify` を重ねる必要はない。
    pub fn put_stored(&self, source: impl Read) -> Result<StoredFile> {
        let mut encodings = Vec::new();
        let file = split(source, |block| {
            let (chunk, encoding) = self.put_chunk(block)?;
            encodings.push(encoding);
            Ok(chunk)
        })?;
        Ok(StoredFile { file, encodings })
    }

    /// 保管と同じ分け方で、元ファイルの構成（識別子と部分の並び）だけを求める。何も書かない。
    /// 送り手が、受け手の持っていない部分だけを `encode_chunk` で送るのに使う。
    pub fn plan(source: impl Read) -> Result<ContentFile> {
        split(source, |block| {
            Ok(FileChunk {
                id: Fingerprint::of(block),
                length: block.len() as u64,
            })
        })
    }

    pub fn put_path(&self, path: &Path) -> Result<ContentFile> {
        self.put_path_stored(path).map(|stored| stored.file)
    }

    pub fn put_path_stored(&self, path: &Path) -> Result<StoredFile> {
        regular_file(path)?;
        self.put_stored(File::open(path)?)
    }

    fn put_chunk(&self, original: &[u8]) -> Result<(FileChunk, ChunkEncoding)> {
        if original.len() > SINGLE_FILE_BYTES {
            return Err(Error::Integrity("部分実体が上限を超えています".into()));
        }
        let id = Fingerprint::of(original);
        let result = FileChunk {
            id,
            length: original.len() as u64,
        };
        let dest = self.chunk_path(id, true)?;
        // 壊れた既存の実体は再利用しない。下で検証した中身と置き換える。
        match self.load(&dest, &result) {
            Ok((_, encoding)) => {
                freshen(&dest);
                return Ok((result, encoding));
            }
            Err(error) if missing(&error) || damaged(&error) => (),
            Err(error) => return Err(error),
        }
        let compressed = compress(original)?;
        let use_compressed = compressed.is_some();
        let payload = compressed.as_deref().unwrap_or(original);
        let temporary = dest.with_extension(format!("{}.part", uuid::Uuid::now_v7()));
        let guard = Pending(temporary.clone());
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(MAGIC)?;
        file.write_all(&[u8::from(use_compressed)])?;
        file.write_all(&(original.len() as u64).to_le_bytes())?;
        file.write_all(&(payload.len() as u64).to_le_bytes())?;
        file.write_all(&id.0)?;
        file.write_all(payload)?;
        file.sync_all()?;
        // 一時ファイルも公開前に読み戻す。既存実体との競合時にも上書きしない。
        if read_back(file, &result)? != original {
            return Err(Error::Integrity("保存直後の復元結果が一致しません".into()));
        }
        let published = self.publish_verified(&temporary, &dest, &result)?;
        drop(guard);
        let encoding = if published {
            ChunkEncoding {
                codec: if use_compressed { "zstd" } else { "raw" },
                stored_bytes: HEADER_BYTES + payload.len() as u64,
            }
        } else {
            // 同時に保管した別の書き手の実体が残った。表現が違うことがあるので読んで確かめる。
            self.load(&dest, &result)?.1
        };
        Ok((result, encoding))
    }

    /// 検証済みの一時ファイルを公開する。同じ実体がすでにあれば、それを検証して使う。
    /// 既存の実体が壊れていたときだけ、それを消さずに隔離してから置き換える。
    /// 渡した一時ファイルが実体になったときに true を返す。
    fn publish_verified(
        &self,
        temporary: &Path,
        dest: &Path,
        expected: &FileChunk,
    ) -> Result<bool> {
        let published = match publish_new(temporary, dest) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                match self.read_chunk(expected) {
                    Ok(_) => {
                        freshen(dest);
                        false
                    }
                    Err(error) if damaged(&error) || missing(&error) => {
                        self.replace_damaged(temporary, dest, expected)?
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(e) => return Err(e.into()),
        };
        sync_directory(dest.parent().unwrap())?;
        Ok(published)
    }

    /// 壊れた実体を `<hash>.<UUID>.damaged` へ退けて、検証済みの中身を公開する。
    /// 退けたファイルは調査と利用者の判断のために残し、この crate では消さない。
    fn replace_damaged(&self, temporary: &Path, dest: &Path, expected: &FileChunk) -> Result<bool> {
        // 同じ実体を同時に直す書き手が、互いの修復結果を壊れたものとして退けないようにする。
        // 棚は一つの処理だけが開くので、処理内で順番にすれば足りる。
        static REPAIR: Mutex<()> = Mutex::new(());
        let _turn = REPAIR.lock().unwrap_or_else(PoisonError::into_inner);
        match self.read_chunk(expected) {
            // 待っているあいだに別の書き手が直した。
            Ok(_) => return Ok(false),
            Err(error) if missing(&error) => (),
            Err(error) if damaged(&error) => {
                let kept = dest.with_extension(format!("{}.damaged", uuid::Uuid::now_v7()));
                fs::rename(dest, kept)?;
            }
            Err(error) => return Err(error),
        }
        publish_new(temporary, dest)?;
        self.read_chunk(expected).map(|_| true)
    }

    fn decode(&self, path: &Path, expected: &FileChunk) -> Result<Vec<u8>> {
        self.load(path, expected).map(|(bytes, _)| bytes)
    }

    fn load(&self, path: &Path, expected: &FileChunk) -> Result<(Vec<u8>, ChunkEncoding)> {
        let meta = regular_file(path)?;
        decode_encoded(File::open(path)?, meta.len(), expected)
    }

    /// 保存済みの実体を検証して状態を返す。壊れた実体は `put`・`import_chunk` で
    /// 正しい中身を渡すと置き換わるので、受信し直す対象として扱える。
    pub fn chunk_state(&self, expected: &FileChunk) -> Result<ChunkState> {
        match self.read_chunk(expected) {
            Ok(_) => {
                // 受信し直さずに使い回す判断に使われる。回収の猶予を今から数え直す。
                freshen(&self.chunk_path(expected.id, false)?);
                Ok(ChunkState::Verified)
            }
            Err(error) if missing(&error) => Ok(ChunkState::Missing),
            Err(error) if damaged(&error) => Ok(ChunkState::Damaged),
            Err(error) => Err(error),
        }
    }

    /// 未受信だけを false とする。破損はエラーで、再利用可能とは扱わない。
    /// 受信し直す部分を決めるときは、破損も区別できる `chunk_state` を使う。
    pub fn has_verified_chunk(&self, expected: &FileChunk) -> Result<bool> {
        match self.read_chunk(expected) {
            Ok(_) => {
                freshen(&self.chunk_path(expected.id, false)?);
                Ok(true)
            }
            Err(Error::Io(e)) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// 圧縮済みの受信実体を検証して公開する。認可やファイル全体の完成は別途必要。
    /// 入力は一つの実体で終わるストリームとし、ヘッダーの長さを超える入力も拒否する。
    pub fn import_chunk(&self, expected: &FileChunk, mut source: impl Read) -> Result<()> {
        let mut header = [0; HEADER_BYTES as usize];
        source.read_exact(&mut header)?;
        let (_, stored) = validate_header(&header, expected)?;
        let dest = self.chunk_path(expected.id, true)?;
        let pending = Pending(dest.with_extension(format!("{}.part", uuid::Uuid::now_v7())));
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&pending.0)?;
        file.write_all(&header)?;
        let copied = std::io::copy(&mut source.take(stored + 1), &mut file)?;
        if copied != stored {
            return Err(Error::Integrity(
                "受信した部分実体の長さが一致しません".into(),
            ));
        }
        file.sync_all()?;
        // キャッシュ済みでも受信内容を検証し、破損した入力を成功扱いしない。
        read_back(file, expected)?;
        // 既存の実体が壊れていれば、受信して検証した中身と置き換える。
        self.publish_verified(&pending.0, &dest, expected)?;
        Ok(())
    }

    /// 検証した圧縮済みバイト列をそのまま送る。元ファイルを再圧縮しない。
    /// 所有権・現在の接続の認可は呼び手が検査し、裸のハッシュを認可に使わない。
    pub fn export_chunk(&self, expected: &FileChunk, mut destination: impl Write) -> Result<()> {
        let path = self.chunk_path(expected.id, false)?;
        regular_file(&path)?;
        let mut encoded = Vec::new();
        File::open(&path)?
            .take(MAX_ENCODED_CHUNK_BYTES + 1)
            .read_to_end(&mut encoded)?;
        decode_chunk(&encoded, expected)?;
        destination.write_all(&encoded)?;
        Ok(())
    }

    fn read_chunk(&self, chunk: &FileChunk) -> Result<Vec<u8>> {
        self.decode(&self.chunk_path(chunk.id, false)?, chunk)
    }

    /// 実体を検証して保存表現を返す。読むのは一度だけ。
    pub fn chunk_info(&self, chunk: &FileChunk) -> Result<(&'static str, u64)> {
        let (_, encoding) = self.load(&self.chunk_path(chunk.id, false)?, chunk)?;
        Ok((encoding.codec, encoding.stored_bytes))
    }

    fn validate_manifest(file: &ContentFile) -> Result<()> {
        let mut length = 0u64;
        if file.chunks.is_empty() {
            return Err(Error::Integrity("部分実体一覧が空です".into()));
        }
        let count = if file.length <= SINGLE_FILE_BYTES as u64 {
            1
        } else {
            file.length.div_ceil(CHUNK_BYTES as u64)
        };
        if file.chunks.len() as u64 != count {
            return Err(Error::Integrity(
                "部分実体の個数がファイル長と一致しません".into(),
            ));
        }
        for (index, chunk) in file.chunks.iter().enumerate() {
            let expected = if file.length <= SINGLE_FILE_BYTES as u64 {
                file.length
            } else {
                (file.length - index as u64 * CHUNK_BYTES as u64).min(CHUNK_BYTES as u64)
            };
            if chunk.length != expected {
                return Err(Error::Integrity("部分実体の分割位置が不正です".into()));
            }
        }
        for chunk in &file.chunks {
            length = length
                .checked_add(chunk.length)
                .ok_or_else(|| Error::Integrity("元サイズが上限を超えています".into()))?;
            if chunk.length > SINGLE_FILE_BYTES as u64 || (file.length > 0 && chunk.length == 0) {
                return Err(Error::Integrity("部分実体の長さが不正です".into()));
            }
        }
        if length != file.length {
            return Err(Error::Integrity("元ファイルの長さが一致しません".into()));
        }
        Ok(())
    }

    /// 呼び手は成功するまで出力を公開しない。途中失敗時に成功扱いしない。
    pub fn export(&self, file: &ContentFile, destination: impl Write) -> Result<()> {
        self.restore(file, destination).map(|_| ())
    }

    /// 全部分を一度ずつ読み、元ファイル全体のハッシュまで確かめながら書き出す。
    fn restore(
        &self,
        file: &ContentFile,
        mut destination: impl Write,
    ) -> Result<Vec<ChunkEncoding>> {
        Self::validate_manifest(file)?;
        let mut hash = Sha256::new();
        let mut encodings = Vec::with_capacity(file.chunks.len());
        for chunk in &file.chunks {
            let (bytes, encoding) = self.load(&self.chunk_path(chunk.id, false)?, chunk)?;
            hash.update(&bytes);
            destination.write_all(&bytes)?;
            encodings.push(encoding);
        }
        if Fingerprint(hash.finalize().into()) != file.id {
            return Err(Error::Integrity(
                "元ファイル全体のハッシュが一致しません".into(),
            ));
        }
        Ok(encodings)
    }

    pub fn verify(&self, file: &ContentFile) -> Result<()> {
        self.export(file, std::io::sink())
    }

    /// `verify` と同じ検査を一度の読み取りで行い、各部分の保存表現も返す。
    /// 既存のファイルを新しい参照に使う前に呼ばれるので、各部分の回収の猶予を数え直す。
    pub fn inspect(&self, file: &ContentFile) -> Result<StoredFile> {
        let encodings = self.restore(file, std::io::sink())?;
        for chunk in &file.chunks {
            freshen(&self.chunk_path(chunk.id, false)?);
        }
        Ok(StoredFile {
            encodings,
            file: file.clone(),
        })
    }

    /// 別の棚へ検証済み実体だけを複製する。全ファイルを一度に展開しない。
    pub fn replicate(&self, file: &ContentFile, destination: &ObjectStore) -> Result<()> {
        Self::validate_manifest(file)?;
        for chunk in &file.chunks {
            let bytes = self.read_chunk(chunk)?;
            if destination.put_chunk(&bytes)?.0 != *chunk {
                return Err(Error::Integrity("複製した実体の識別が変わりました".into()));
            }
        }
        destination.verify(file)
    }

    /// 指定した範囲に必要な部分実体だけを検査して返す。全体の検査とは区別する。
    pub fn read_range(
        &self,
        file: &ContentFile,
        offset: u64,
        length: u64,
        mut out: impl Write,
    ) -> Result<()> {
        Self::validate_manifest(file)?;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= file.length)
            .ok_or_else(|| Error::Integrity("読取範囲が元ファイルを超えています".into()))?;
        let mut start = 0u64;
        for chunk in &file.chunks {
            let chunk_end = start + chunk.length;
            if start < end && chunk_end > offset {
                let bytes = self.read_chunk(chunk)?;
                out.write_all(
                    &bytes[offset.saturating_sub(start) as usize
                        ..(end.min(chunk_end) - start) as usize],
                )?;
            }
            start = chunk_end;
            if start >= end {
                break;
            }
        }
        Ok(())
    }

    /// キャッシュではない復元先へ、新規作成して公開する。既存ファイルは上書きしない。
    pub fn export_new(&self, file: &ContentFile, destination: &Path) -> Result<()> {
        let parent = destination
            .parent()
            .ok_or_else(|| Error::UnsafePath(destination.into()))?;
        regular_directory(parent)?;
        let pending = Pending(parent.join(format!(".{}.part", uuid::Uuid::now_v7())));
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&pending.0)?;
        self.export(file, &mut output)?;
        output.sync_all()?;
        drop(output);
        publish_new(&pending.0, destination)?;
        sync_directory(parent)?;
        Ok(())
    }

    /// 実体領域のファイルを全部並べる。中身は読まない。知らない名前のファイルは返さない。
    pub fn listing(&self) -> Result<Vec<StoredListing>> {
        let mut found = Vec::new();
        for dir in fs::read_dir(&self.root)? {
            let dir = dir?;
            let name = dir.file_name();
            let name = name.to_string_lossy();
            if name.len() != 2 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            regular_directory(&dir.path())?;
            for entry in fs::read_dir(dir.path())? {
                let path = entry?.path();
                let Some(file_name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };
                let parts: Vec<&str> = file_name.split('.').collect();
                let entry = match parts.as_slice() {
                    [hash, "obj"] => StoredEntry::Object(Fingerprint::parse(hash)?),
                    [_, _, "part"] => StoredEntry::Part,
                    [_, _, "damaged"] => StoredEntry::Damaged,
                    [hash, at, "quarantined"] => StoredEntry::Quarantined {
                        id: Fingerprint::parse(hash)?,
                        at_ms: at
                            .parse()
                            .map_err(|_| Error::Integrity("隔離した実体の名前が不正です".into()))?,
                    },
                    _ => continue,
                };
                let meta = match regular_file(&path) {
                    Ok(meta) => meta,
                    // 走査中に保存が一時ファイルを公開したり、壊れた実体を退けたりする。
                    Err(error) if missing(&error) => continue,
                    Err(error) => return Err(error),
                };
                found.push(StoredListing {
                    entry,
                    modified: meta.modified()?,
                    bytes: meta.len(),
                    path,
                });
            }
        }
        Ok(found)
    }

    /// 別の置き場にある実体を、この置き場にも置く。実体は不変なので、同じボリュームなら
    /// ハードリンクで共有して容量を増やさず、できなければ圧縮したまま写す（写すときは受信と同じく
    /// 検証してから公開する）。すでに置いてあれば何もしない。中身の検証は呼び手が棚ごと行う。
    pub fn adopt_chunk(&self, source: &ObjectStore, chunk: &FileChunk) -> Result<()> {
        if self.has_object(chunk.id)? {
            return Ok(());
        }
        let from = source.chunk_path(chunk.id, false)?;
        regular_file(&from)?;
        let dest = self.chunk_path(chunk.id, true)?;
        if fs::hard_link(&from, &dest).is_ok() {
            if let Some(parent) = dest.parent() {
                sync_directory(parent)?;
            }
            return Ok(());
        }
        let mut encoded = Vec::new();
        source.export_chunk(chunk, &mut encoded)?;
        self.import_chunk(chunk, Cursor::new(encoded))
    }

    /// 公開済みの実体ファイルがあるか（中身は読まない）。
    pub fn has_object(&self, id: Fingerprint) -> Result<bool> {
        // 先頭 2 桁のフォルダーがまだ無いのも「無い」（一つ目の実体を置く前の置き場）。
        let path = match self.chunk_path(id, false) {
            Ok(path) => path,
            Err(error) if missing(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        match regular_file(&path) {
            Ok(_) => Ok(true),
            Err(error) if missing(&error) => Ok(false),
            Err(error) => Err(error),
        }
    }

    /// 実体を隔離する。最後に使われたのが `unused_since` より後なら退けず false を返す
    /// （一覧を作ったあとに保存が使い回した実体）。実体がなければ false。
    pub fn quarantine(
        &self,
        id: Fingerprint,
        unused_since: std::time::SystemTime,
        at_ms: i64,
    ) -> Result<bool> {
        let path = self.chunk_path(id, false)?;
        let meta = match regular_file(&path) {
            Ok(meta) => meta,
            Err(error) if missing(&error) => return Ok(false),
            Err(error) => return Err(error),
        };
        if meta.modified()? > unused_since {
            return Ok(false);
        }
        fs::rename(&path, path.with_extension(format!("{at_ms}.quarantined")))?;
        sync_directory(path.parent().unwrap())?;
        Ok(true)
    }

    /// 隔離した実体を戻す。同じ実体がもう置き直されていれば、隔離した方を消す。
    /// 戻したら true。
    pub fn restore_quarantined(&self, listing: &StoredListing) -> Result<bool> {
        let StoredEntry::Quarantined { id, .. } = listing.entry else {
            return Err(Error::Integrity("隔離した実体ではありません".into()));
        };
        let dest = self.chunk_path(id, false)?;
        let restored = match fs::hard_link(&listing.path, &dest) {
            Ok(()) => true,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => false,
            Err(e) => return Err(e.into()),
        };
        fs::remove_file(&listing.path)?;
        sync_directory(dest.parent().unwrap())?;
        Ok(restored)
    }

    /// 一覧で見つけた、実体でないファイル（隔離・壊れ・一時）を消す。消したら true。
    pub fn remove_listed(&self, listing: &StoredListing) -> Result<bool> {
        if matches!(listing.entry, StoredEntry::Object(_)) {
            return Err(Error::Integrity(
                "公開済みの実体は隔離してから消します".into(),
            ));
        }
        if !listing.path.starts_with(&self.root) {
            return Err(Error::UnsafePath(listing.path.clone()));
        }
        match regular_file(&listing.path) {
            Ok(_) => {}
            Err(error) if missing(&error) => return Ok(false),
            Err(error) => return Err(error),
        }
        fs::remove_file(&listing.path)?;
        Ok(true)
    }

    /// 実体の個数と容量を、各実体のヘッダーだけを読んで数える。
    /// 中身は確かめない。確かめるのは `verify` の役目で、同じ実体を二度読まない。
    pub fn stats(&self) -> Result<StorageStats> {
        let mut stats = StorageStats::default();
        for dir in fs::read_dir(&self.root)? {
            let dir = dir?;
            if !dir
                .file_name()
                .to_string_lossy()
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
            {
                continue;
            }
            regular_directory(&dir.path())?;
            for entry in fs::read_dir(dir.path())? {
                let path = entry?.path();
                if path.extension().and_then(|s| s.to_str()) != Some("obj") {
                    continue;
                }
                let id =
                    Fingerprint::parse(path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))?;
                let stored_bytes = regular_file(&path)?.len();
                let mut header = [0u8; HEADER_BYTES as usize];
                File::open(&path)?.read_exact(&mut header)?;
                let len = u64::from_le_bytes(header[9..17].try_into().unwrap());
                let (_, stored) = validate_header(&header, &FileChunk { id, length: len })?;
                if stored_bytes != HEADER_BYTES + stored {
                    return Err(Error::Integrity(
                        "部分実体の保存サイズが一致しません".into(),
                    ));
                }
                stats.chunks += 1;
                stats.original_bytes += len;
                stats.stored_bytes += stored_bytes;
                stats.compressed_chunks += u64::from(header[8] == 1);
            }
        }
        Ok(stats)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(PathBuf);
    impl Temp {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("piep-objects-{}", uuid::Uuid::now_v7()));
            fs::create_dir(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn removing_a_listed_part_succeeds_if_its_writer_has_finished() {
        let temp = Temp::new();
        let root = temp.0.join("objects");
        let store = ObjectStore::open(&root).unwrap();
        let dir = root.join("00");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("chunk.write.part");
        fs::write(&path, b"part").unwrap();
        let listing = store.listing().unwrap();
        assert_eq!(listing.len(), 1);
        assert!(matches!(listing[0].entry, StoredEntry::Part));
        fs::remove_file(&path).unwrap();
        assert!(!store.remove_listed(&listing[0]).unwrap());
    }

    #[test]
    fn originals_roundtrip_deduplicate_and_restore_without_overwriting() {
        let temp = Temp::new();
        let store = ObjectStore::open(&temp.0.join("objects")).unwrap();
        for input in [
            Vec::new(),
            b"original\r\n\0EXIF".to_vec(),
            vec![b'x'; 30_000],
            (0..30_000).map(|_| rand::random::<u8>()).collect(),
        ] {
            let file = store.put(Cursor::new(&input)).unwrap();
            assert_eq!(store.put(Cursor::new(&input)).unwrap(), file);
            let mut restored = Vec::new();
            store.export(&file, &mut restored).unwrap();
            assert_eq!(restored, input);
            let dest = temp.0.join(uuid::Uuid::now_v7().to_string());
            store.export_new(&file, &dest).unwrap();
            assert!(store.export_new(&file, &dest).is_err());
            assert_eq!(fs::read(dest).unwrap(), input);
        }
        let stats = store.stats().unwrap();
        assert_eq!(stats.chunks, 4);
        assert_eq!(stats.compressed_chunks, 1);
    }

    #[test]
    fn stored_encodings_match_the_objects_on_disk_for_new_reused_and_split_files() {
        let temp = Temp::new();
        let store = ObjectStore::open(&temp.0).unwrap();
        let split: Vec<_> = (0..SINGLE_FILE_BYTES + 7123)
            .map(|i| (i % 251) as u8)
            .collect();
        for input in [
            b"short raw".to_vec(),
            vec![b'x'; 30_000],
            (0..30_000).map(|_| rand::random::<u8>()).collect(),
            split,
        ] {
            let stored = store.put_stored(Cursor::new(&input)).unwrap();
            assert_eq!(stored.encodings.len(), stored.file.chunks.len());
            // 保存表現は、実体のヘッダーとファイルの長さから読んだものと同じ。
            for (chunk, encoding) in stored.file.chunks.iter().zip(&stored.encodings) {
                let path = store.chunk_path(chunk.id, false).unwrap();
                assert_eq!(encoding.stored_bytes, fs::metadata(&path).unwrap().len());
                assert_eq!(
                    encoding.codec,
                    if fs::read(&path).unwrap()[8] == 1 {
                        "zstd"
                    } else {
                        "raw"
                    }
                );
                assert_eq!(
                    store.chunk_info(chunk).unwrap(),
                    (encoding.codec, encoding.stored_bytes)
                );
            }
            // すでにある実体を使った二度目の保管も、検査だけの読み取りも同じ結果を返す。
            assert_eq!(store.put_stored(Cursor::new(&input)).unwrap(), stored);
            assert_eq!(store.inspect(&stored.file).unwrap(), stored);
        }
        // 壊れた実体は、検査でも保管済みの扱いにしない。
        let stored = store.put_stored(Cursor::new(b"to be damaged")).unwrap();
        let path = store.chunk_path(stored.file.chunks[0].id, false).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(store.inspect(&stored.file).is_err());
    }

    #[test]
    fn large_files_support_ranges_across_chunk_boundaries() {
        let temp = Temp::new();
        let store = ObjectStore::open(&temp.0).unwrap();
        let input: Vec<_> = (0..SINGLE_FILE_BYTES + 7123)
            .map(|i| (i % 251) as u8)
            .collect();
        let file = store.put(Cursor::new(&input)).unwrap();
        assert_eq!(file.chunks.len(), 3);
        let mut restored = Vec::new();
        store.export(&file, &mut restored).unwrap();
        assert_eq!(restored, input);
        let mut range = Vec::new();
        store
            .read_range(&file, (CHUNK_BYTES - 3) as u64, 11, &mut range)
            .unwrap();
        assert_eq!(range, input[CHUNK_BYTES - 3..CHUNK_BYTES + 8]);
        assert!(store
            .read_range(&file, file.length, 1, std::io::sink())
            .is_err());
    }

    #[test]
    fn corruption_never_becomes_success_or_a_reusable_object() {
        let temp = Temp::new();
        let store = ObjectStore::open(&temp.0).unwrap();
        let file = store.put(Cursor::new(b"content")).unwrap();
        let path = store.chunk_path(file.chunks[0].id, false).unwrap();
        let mut bytes = fs::read(&path).unwrap();
        *bytes.last_mut().unwrap() ^= 1;
        fs::write(&path, bytes).unwrap();
        assert!(store.verify(&file).is_err());
        assert_eq!(
            store.chunk_state(&file.chunks[0]).unwrap(),
            ChunkState::Damaged
        );
        assert!(store.has_verified_chunk(&file.chunks[0]).is_err());
        let dest = temp.0.join("not-published");
        assert!(store.export_new(&file, &dest).is_err());
        assert!(!dest.exists());
    }

    fn files_with_extension(root: &Path, extension: &str) -> Vec<PathBuf> {
        fs::read_dir(root)
            .unwrap()
            .flat_map(|dir| fs::read_dir(dir.unwrap().path()).unwrap())
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.extension().is_some_and(|ext| ext == extension))
            .collect()
    }

    #[test]
    fn stats_read_headers_only_and_reject_a_wrong_length() {
        let root = Temp::new();
        let store = ObjectStore::open(&root.0).unwrap();
        store.put(&b"counted ".repeat(4096)[..]).unwrap();
        let stats = store.stats().unwrap();
        assert_eq!((stats.chunks, stats.original_bytes), (1, 8 * 4096));
        let path = files_with_extension(&root.0, "obj").remove(0);
        assert_eq!(stats.stored_bytes, fs::metadata(&path).unwrap().len());
        let mut bytes = fs::read(&path).unwrap();
        bytes.push(0);
        fs::write(&path, &bytes).unwrap();
        assert!(store.stats().is_err());
    }

    #[test]
    fn saving_the_same_content_again_replaces_a_damaged_object_and_keeps_it_aside() {
        let temp = Temp::new();
        let store = ObjectStore::open(&temp.0).unwrap();
        // raw は SHA-256 の不一致、zstd は展開の失敗として壊れ方が違う。
        for (index, input) in [b"content".to_vec(), vec![b'z'; 30_000]].iter().enumerate() {
            let file = store.put(Cursor::new(input)).unwrap();
            let path = store.chunk_path(file.chunks[0].id, false).unwrap();
            let mut bytes = fs::read(&path).unwrap();
            let middle = bytes.len() - 2;
            bytes[middle] ^= 0x55;
            fs::write(&path, &bytes).unwrap();
            assert_eq!(
                store.chunk_state(&file.chunks[0]).unwrap(),
                ChunkState::Damaged
            );
            assert_eq!(store.put(Cursor::new(input)).unwrap(), file);
            store.verify(&file).unwrap();
            let kept = files_with_extension(&temp.0, "damaged");
            assert_eq!(kept.len(), index + 1);
            assert!(kept.iter().any(|kept| fs::read(kept).unwrap() == bytes));
        }
        assert!(files_with_extension(&temp.0, "part").is_empty());
        assert_eq!(store.stats().unwrap().chunks, 2);
    }
}
