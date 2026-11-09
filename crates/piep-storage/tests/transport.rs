use piep_storage::{
    decode_chunk, ContentFile, FileChunk, Fingerprint, ObjectStore, MAX_CHUNK_ORIGINAL_BYTES,
    MAX_CHUNK_STORED_BYTES, OBJECT_HEADER_BYTES,
};
use std::{
    fs,
    io::{Cursor, Read},
    path::PathBuf,
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("piep-transport-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn store(&self, name: &str) -> ObjectStore {
        ObjectStore::open(&self.0.join(name)).unwrap()
    }
    fn assert_no_parts(&self) {
        fn visit(path: &std::path::Path) {
            for entry in fs::read_dir(path).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    visit(&path);
                } else {
                    assert_ne!(path.extension().unwrap(), "part");
                }
            }
        }
        visit(&self.0);
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn descriptor(original: &[u8]) -> FileChunk {
    FileChunk {
        id: Fingerprint::of(original),
        length: original.len() as u64,
    }
}
fn encoded(store: &ObjectStore, chunk: &FileChunk) -> Vec<u8> {
    let mut out = Vec::new();
    store.export_chunk(chunk, &mut out).unwrap();
    out
}
fn frame_object(payload: &[u8], original: &[u8]) -> Vec<u8> {
    let mut bytes = b"PIEPOB01\x01".to_vec();
    bytes.extend_from_slice(&(original.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&Fingerprint::of(original).0);
    bytes.extend_from_slice(payload);
    bytes
}

#[test]
fn frozen_objects_from_pre_extraction_desktop_are_readable_without_reencoding() {
    let manifest: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/manifest.json")).unwrap();
    let temp = Temp::new();
    let store = temp.store("objects");
    for (index, (raw, original)) in [
        (
            include_bytes!("fixtures/raw-control.obj").as_slice(),
            b"original\r\n\0EXIF".to_vec(),
        ),
        (
            include_bytes!("fixtures/zstd-repeat.obj").as_slice(),
            vec![b'x'; 30_000],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let vector = &manifest["vectors"][index];
        let expected = descriptor(&original);
        assert_eq!(expected.id.to_string(), vector["file_sha256"]);
        assert_eq!(Fingerprint::of(raw).to_string(), vector["encoded_sha256"]);
        assert_eq!(raw.len() as u64, vector["encoded_length"].as_u64().unwrap());
        assert_eq!(decode_chunk(raw, &expected).unwrap(), original);
        store.import_chunk(&expected, Cursor::new(raw)).unwrap();
        assert_eq!(encoded(&store, &expected), raw);
        // 永続化済み manifest と raw evidence の fingerprint 表現を変えない。
        assert_eq!(
            serde_json::to_value(expected.id).unwrap(),
            serde_json::to_value(expected.id.0).unwrap()
        );
        assert_eq!(
            Fingerprint::parse(&expected.id.to_string().to_uppercase()).unwrap(),
            expected.id
        );
    }
}

#[test]
fn resumed_transfer_preserves_compressed_bytes_and_requires_all_chunks_and_whole_hash() {
    let temp = Temp::new();
    let source = temp.store("source");
    let target = temp.store("target");
    let input: Vec<u8> = (0..MAX_CHUNK_ORIGINAL_BYTES as usize + 17)
        .map(|n| (n % 239) as u8)
        .collect();
    let file = source.put(Cursor::new(&input)).unwrap();
    assert_eq!(file.chunks.len(), 3);
    for (index, chunk) in file.chunks.iter().enumerate() {
        assert!(!target.has_verified_chunk(chunk).unwrap());
        let wire = encoded(&source, chunk);
        target.import_chunk(chunk, Cursor::new(&wire)).unwrap();
        assert!(target.has_verified_chunk(chunk).unwrap());
        assert_eq!(encoded(&target, chunk), wire);
        if index < file.chunks.len() - 1 {
            assert!(target.verify(&file).is_err());
        }
    }
    target.verify(&file).unwrap();
    let mut restored = Vec::new();
    target.export(&file, &mut restored).unwrap();
    assert_eq!(restored, input);
    let mut wrong: ContentFile = file.clone();
    wrong.id = Fingerprint::of(b"another file");
    assert!(target.verify(&wrong).is_err());
    temp.assert_no_parts();
}

#[test]
fn malformed_inputs_never_publish_and_cannot_replace_a_verified_cached_object() {
    let temp = Temp::new();
    let good = include_bytes!("fixtures/raw-control.obj").to_vec();
    let expected = descriptor(b"original\r\n\0EXIF");
    let mut cases = vec![
        Vec::new(),
        good[..56].to_vec(),
        good[..good.len() - 1].to_vec(),
    ];
    let mut trailing = good.clone();
    trailing.push(1);
    cases.push(trailing);
    for (offset, value) in [(0, 0), (8, 2), (25, 1), (57, 1)] {
        let mut bad = good.clone();
        bad[offset] = value;
        cases.push(bad);
    }
    for (offset, length) in [
        (9, MAX_CHUNK_ORIGINAL_BYTES + 1),
        (17, MAX_CHUNK_STORED_BYTES + 1),
        (17, 2),
    ] {
        let mut bad = good.clone();
        bad[offset..offset + 8].copy_from_slice(&length.to_le_bytes());
        cases.push(bad);
    }
    for (index, bad) in cases.into_iter().enumerate() {
        let target = temp.store(&format!("bad-{index}"));
        assert!(target.import_chunk(&expected, Cursor::new(&bad)).is_err());
        assert!(!target.has_verified_chunk(&expected).unwrap());
        assert!(decode_chunk(&bad, &expected).is_err());
        target.import_chunk(&expected, Cursor::new(&good)).unwrap();
        assert!(target.import_chunk(&expected, Cursor::new(&bad)).is_err());
        assert_eq!(encoded(&target, &expected), good);
    }
    let target = temp.store("wrong-descriptor");
    let wrong = FileChunk {
        length: expected.length + 1,
        ..expected.clone()
    };
    assert!(target.import_chunk(&wrong, Cursor::new(&good)).is_err());
    let wrong = FileChunk {
        id: Fingerprint::of(b"wrong"),
        ..expected.clone()
    };
    assert!(target.import_chunk(&wrong, Cursor::new(&good)).is_err());
    temp.assert_no_parts();
}

#[test]
fn corrupt_existing_objects_are_reported_instead_of_reused_or_overwritten() {
    let temp = Temp::new();
    let target = temp.store("objects");
    let good = include_bytes!("fixtures/raw-control.obj");
    let chunk = descriptor(b"original\r\n\0EXIF");
    target.import_chunk(&chunk, Cursor::new(good)).unwrap();
    let hash = chunk.id.to_string();
    let path = temp
        .0
        .join("objects")
        .join(&hash[..2])
        .join(format!("{hash}.obj"));
    fs::write(&path, b"corruption").unwrap();
    assert!(target.has_verified_chunk(&chunk).is_err());
    assert!(target.import_chunk(&chunk, Cursor::new(good)).is_err());
    let mut out = Vec::new();
    assert!(target.export_chunk(&chunk, &mut out).is_err());
    assert!(out.is_empty());
    assert_eq!(fs::read(path).unwrap(), b"corruption");
    temp.assert_no_parts();
}

#[test]
fn receiver_stops_reading_at_declared_payload_plus_one_byte() {
    struct Endless {
        header: Cursor<Vec<u8>>,
        read: u64,
    }
    impl Read for Endless {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            let count = self.header.read(out)?;
            if count > 0 {
                self.read += count as u64;
                return Ok(count);
            }
            out.fill(0);
            self.read += out.len() as u64;
            Ok(out.len())
        }
    }
    let temp = Temp::new();
    let target = temp.store("objects");
    let good = include_bytes!("fixtures/raw-control.obj");
    let mut input = Endless {
        header: Cursor::new(good[..OBJECT_HEADER_BYTES as usize].to_vec()),
        read: 0,
    };
    assert!(target
        .import_chunk(&descriptor(b"original\r\n\0EXIF"), &mut input)
        .is_err());
    assert_eq!(input.read, good.len() as u64 + 1);
    temp.assert_no_parts();
}

#[test]
fn decompression_limits_output_and_window_even_when_declared_original_is_small() {
    // https://github.com/facebook/zstd/blob/dev/doc/zstd_compression_format.md
    // FHD=0, windowLog=10+(WD>>3), last raw block=(3<<3)|1, payload=abc.
    let mut frame = vec![
        0x28, 0xb5, 0x2f, 0xfd, 0, 0x68, 0x19, 0, 0, b'a', b'b', b'c',
    ];
    let expected = descriptor(b"abc");
    assert_eq!(
        decode_chunk(&frame_object(&frame, b"abc"), &expected).unwrap(),
        b"abc"
    );
    frame[5] = 0x70; // 16 MiB window; content itself remains valid.
    assert!(zstd::stream::decode_all(Cursor::new(&frame)).is_ok());
    assert!(decode_chunk(&frame_object(&frame, b"abc"), &expected).is_err());
    let oversized = zstd::stream::encode_all(Cursor::new(vec![0; 100_000]), 3).unwrap();
    assert!(decode_chunk(&frame_object(&oversized, b"abc"), &expected).is_err());
}

#[test]
fn concurrent_receivers_publish_one_verified_object_without_overwriting() {
    let temp = Temp::new();
    let store = temp.store("objects");
    let expected = descriptor(&vec![b'x'; 30_000]);
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| {
                barrier.wait();
                store
                    .import_chunk(
                        &expected,
                        Cursor::new(include_bytes!("fixtures/zstd-repeat.obj")),
                    )
                    .unwrap();
            });
        }
    });
    assert_eq!(
        encoded(&store, &expected),
        include_bytes!("fixtures/zstd-repeat.obj")
    );
    assert_eq!(store.stats().unwrap().chunks, 1);
    temp.assert_no_parts();
}
