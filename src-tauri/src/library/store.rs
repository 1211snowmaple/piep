use super::{Error, Fingerprint, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 8] = b"PIEPOB01";
const HEADER_BYTES: u64 = 57;
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

pub(crate) fn regular_directory(path: &Path) -> Result<()> {
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

pub(crate) fn regular_file(path: &Path) -> Result<fs::Metadata> {
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
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
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
    pub fn put(&self, mut source: impl Read) -> Result<ContentFile> {
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
            chunks.push(self.put_chunk(&initial)?);
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
                chunks.push(self.put_chunk(&block)?);
            }
        }
        Ok(ContentFile {
            id: Fingerprint(whole.finalize().into()),
            length,
            chunks,
        })
    }

    pub fn put_path(&self, path: &Path) -> Result<ContentFile> {
        regular_file(path)?;
        self.put(File::open(path)?)
    }

    fn put_chunk(&self, original: &[u8]) -> Result<FileChunk> {
        if original.len() > SINGLE_FILE_BYTES {
            return Err(Error::Integrity("部分実体が上限を超えています".into()));
        }
        let id = Fingerprint::of(original);
        let result = FileChunk {
            id,
            length: original.len() as u64,
        };
        let dest = self.chunk_path(id, true)?;
        if dest.try_exists()? {
            self.read_chunk(&result)?;
            return Ok(result);
        }
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
        let payload = if use_compressed {
            compressed.as_slice()
        } else {
            original
        };
        let temporary = dest.with_extension(format!("{}.part", uuid::Uuid::now_v7()));
        let guard = Pending(temporary.clone());
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        file.write_all(MAGIC)?;
        file.write_all(&[u8::from(use_compressed)])?;
        file.write_all(&(original.len() as u64).to_le_bytes())?;
        file.write_all(&(payload.len() as u64).to_le_bytes())?;
        file.write_all(&id.0)?;
        file.write_all(payload)?;
        file.sync_all()?;
        drop(file);
        // 一時ファイルも公開前に読み戻す。既存実体との競合時にも上書きしない。
        if self.decode(&temporary, &result)? != original {
            return Err(Error::Integrity("保存直後の復元結果が一致しません".into()));
        }
        match fs::hard_link(&temporary, &dest) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                self.read_chunk(&result)?;
            }
            Err(e) => return Err(e.into()),
        }
        sync_directory(dest.parent().unwrap())?;
        drop(guard);
        Ok(result)
    }

    fn decode(&self, path: &Path, expected: &FileChunk) -> Result<Vec<u8>> {
        let meta = regular_file(path)?;
        let mut file = File::open(path)?;
        let mut header = [0u8; HEADER_BYTES as usize];
        file.read_exact(&mut header)?;
        let original = u64::from_le_bytes(header[9..17].try_into().unwrap());
        let stored = u64::from_le_bytes(header[17..25].try_into().unwrap());
        if &header[..8] != MAGIC
            || header[8] > 1
            || original != expected.length
            || original > SINGLE_FILE_BYTES as u64
            || stored > (SINGLE_FILE_BYTES * 2) as u64
            || meta.len() != HEADER_BYTES + stored
            || header[25..57] != expected.id.0
        {
            return Err(Error::Integrity(format!(
                "部分実体のヘッダーが不正です: {}",
                expected.id
            )));
        }
        let mut bytes = Vec::with_capacity(original as usize);
        if header[8] == 0 {
            if stored != original {
                return Err(Error::Integrity("無圧縮実体の長さが不正です".into()));
            }
            file.take(original + 1).read_to_end(&mut bytes)?;
        } else {
            let mut decoder = zstd::stream::read::Decoder::new(file)?;
            decoder.window_log_max(23)?;
            decoder.take(original + 1).read_to_end(&mut bytes)?;
        }
        if bytes.len() as u64 != original || Fingerprint::of(&bytes) != expected.id {
            return Err(Error::Integrity(format!(
                "部分実体の元データが一致しません: {}",
                expected.id
            )));
        }
        Ok(bytes)
    }

    fn read_chunk(&self, chunk: &FileChunk) -> Result<Vec<u8>> {
        self.decode(&self.chunk_path(chunk.id, false)?, chunk)
    }

    pub(crate) fn chunk_info(&self, chunk: &FileChunk) -> Result<(&'static str, u64)> {
        let path = self.chunk_path(chunk.id, false)?;
        self.decode(&path, chunk)?;
        let mut input = File::open(&path)?;
        input.seek(SeekFrom::Start(8))?;
        let mut codec = [0];
        input.read_exact(&mut codec)?;
        Ok((
            if codec[0] == 1 { "zstd" } else { "raw" },
            fs::metadata(path)?.len(),
        ))
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
    pub fn export(&self, file: &ContentFile, mut destination: impl Write) -> Result<()> {
        Self::validate_manifest(file)?;
        let mut hash = Sha256::new();
        for chunk in &file.chunks {
            let bytes = self.read_chunk(chunk)?;
            hash.update(&bytes);
            destination.write_all(&bytes)?;
        }
        if Fingerprint(hash.finalize().into()) != file.id {
            return Err(Error::Integrity(
                "元ファイル全体のハッシュが一致しません".into(),
            ));
        }
        Ok(())
    }

    pub fn verify(&self, file: &ContentFile) -> Result<()> {
        self.export(file, std::io::sink())
    }

    /// 別の棚へ検証済み実体だけを複製する。全ファイルを一度に展開しない。
    pub fn replicate(&self, file: &ContentFile, destination: &ObjectStore) -> Result<()> {
        Self::validate_manifest(file)?;
        for chunk in &file.chunks {
            let bytes = self.read_chunk(chunk)?;
            if destination.put_chunk(&bytes)? != *chunk {
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
        fs::hard_link(&pending.0, destination)?;
        sync_directory(parent)?;
        Ok(())
    }

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
                let mut input = File::open(&path)?;
                input.seek(SeekFrom::Start(8))?;
                let mut fields = [0u8; 9];
                input.read_exact(&mut fields)?;
                let len = u64::from_le_bytes(fields[1..].try_into().unwrap());
                self.decode(&path, &FileChunk { id, length: len })?;
                stats.chunks += 1;
                stats.original_bytes += len;
                stats.stored_bytes += fs::metadata(path)?.len();
                stats.compressed_chunks += u64::from(fields[0] == 1);
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
        assert!(store.put(Cursor::new(b"content")).is_err());
        let dest = temp.0.join("not-published");
        assert!(store.export_new(&file, &dest).is_err());
        assert!(!dest.exists());
    }
}
