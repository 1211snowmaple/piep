use super::*;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        crate::database::tantivy_index::release_runtime(&self.0.join("derived"));
        crate::database::tantivy_index::release_runtime(&self.0.join("derived").join("history"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-v2-app-{name}-{}", LibraryId::now_v7())))
}

#[test]
fn a_missing_library_is_absent_and_serves_nothing() {
    let dir = temp("absent");
    let state = LibraryV2::default();
    assert_eq!(state.status(), LibraryV2Status::Starting);
    state.open(&dir.0);
    assert_eq!(
        state.status(),
        LibraryV2Status::Absent {
            path: dir.0.clone()
        }
    );
    assert!(state.library().is_err());
    assert_eq!(respond(&state, &"0".repeat(64)).status(), 503);
    assert_eq!(respond(&state, "/not-a-file").status(), 400);
}

#[test]
fn an_existing_library_opens_indexes_and_serves_files_by_their_fingerprint() {
    let dir = temp("open");
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
    let stored = {
        let library = Library::create(&dir.0).unwrap();
        let file = library
            .store_original(&crate::library::CaptureSource::Bytes(png.to_vec()))
            .unwrap();
        file.id
    };
    let state = LibraryV2::default();
    state.open(&dir.0);
    let LibraryV2Status::Ready { unresolved, .. } = state.status() else {
        panic!("開いて同期する: {:?}", state.status());
    };
    assert!(unresolved.is_empty());
    let found = respond(&state, &format!("/{stored}"));
    assert_eq!(found.status(), 200);
    assert_eq!(found.body(), &png);
    assert_eq!(found.headers()["Content-Type"], "image/png");
    assert_eq!(respond(&state, &"0".repeat(64)).status(), 404);
    // 開いている棚は、ほかから開けない（移行器も止まる）。
    assert!(Library::open(&dir.0).is_err());
}

#[test]
fn image_kinds_come_from_the_bytes() {
    assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
    assert_eq!(sniff(b"GIF89a"), Some("image/gif"));
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
    assert_eq!(sniff(b"<html>"), None);
}
