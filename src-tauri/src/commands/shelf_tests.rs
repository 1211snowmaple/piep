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
    Temp(std::env::temp_dir().join(format!("piep-shelf-app-{name}-{}", LibraryId::now_v7())))
}

#[test]
fn a_missing_library_is_created_switched_over_and_serves_nothing_until_asked() {
    let dir = temp("absent");
    let state = Shelf::default();
    assert_eq!(state.status(), ShelfState::Starting);
    assert!(state.library().is_err());
    assert_eq!(respond(&state, &"0".repeat(64)).status(), 503);
    assert_eq!(respond(&state, "/not-a-file").status(), 400);
    state.open(&dir.0).unwrap();
    assert_eq!(
        state.status(),
        ShelfState::Indexing {
            path: dir.0.clone()
        }
    );
    let library = state.library().unwrap();
    assert!(library.switched_at().unwrap().is_some());
    assert_eq!(respond(&state, &"0".repeat(64)).status(), 404);
}

#[test]
fn an_existing_library_opens_indexes_and_serves_files_by_their_fingerprint() {
    let dir = temp("open");
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3];
    let stored = {
        let library = Library::create(&dir.0).unwrap();
        library.switch_over().unwrap();
        let file = library
            .store_original(&crate::library::CaptureSource::Bytes(png.to_vec()))
            .unwrap();
        file.id
    };
    let state = Shelf::default();
    state.open(&dir.0).unwrap();
    state.sync_index(&dir.0);
    assert_eq!(
        state.status(),
        ShelfState::Ready {
            path: dir.0.clone()
        }
    );
    let found = respond(&state, &format!("/{stored}"));
    assert_eq!(found.status(), 200);
    assert_eq!(found.body(), &png);
    assert_eq!(found.headers()["Content-Type"], "image/png");
    assert_eq!(respond(&state, &"0".repeat(64)).status(), 404);
    // 開いている棚は、ほかから開けない（移行器も止まる）。
    assert!(Library::open(&dir.0).is_err());
}

#[test]
fn a_shelf_that_was_never_switched_over_is_not_opened() {
    let dir = temp("switch");
    {
        let library = Library::create(&dir.0).unwrap();
        assert_eq!(library.switched_at().unwrap(), None);
    }
    let state = Shelf::default();
    assert!(state.open(&dir.0).is_err());
    assert!(matches!(state.status(), ShelfState::Failed { .. }));
    assert!(state.library().is_err());
}

#[test]
fn image_kinds_come_from_the_bytes() {
    assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
    assert_eq!(sniff(b"GIF89a"), Some("image/gif"));
    assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
    assert_eq!(sniff(b"<html>"), None);
}

#[test]
fn the_shelf_lives_inside_the_app_folder() {
    let app = Path::new("C:/Users/u/AppData/Roaming/io.github.example.piep");
    if std::env::var_os(DIR_ENV).is_none() {
        assert_eq!(library_dir(app), app.join(INSIDE_DIR));
    }
}
