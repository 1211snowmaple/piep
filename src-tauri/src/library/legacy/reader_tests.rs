use super::*;
use crate::library::{ReaderBody, ResourceScope};
use rusqlite::{params, Connection};
use serde_json::json;

struct Fixture(PathBuf);
impl Fixture {
    fn new(ambiguous: bool) -> Self {
        let root = std::env::temp_dir().join(format!("piep-legacy-reader-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(root.join("old")).unwrap();
        let db = Connection::open(root.join("old/piep.db")).unwrap();
        db.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
INSERT INTO downloads VALUES(1,'pixiv','one','title','author',0);").unwrap();
        let mut asset_id = 0;
        for version in 1..=2 {
            let dir = root.join(format!("old/downloads/pixiv/one/v{version}"));
            std::fs::create_dir_all(dir.join("data_assets/illustrations")).unwrap();
            let json = dir.join("original.json");
            std::fs::write(&json,json!({"detail":{"title":format!("version {version}")},"text":"[uploadedimage:123][pixivimage:456-2][uploadedimage:999]"}).to_string()).unwrap();
            db.execute(
                "INSERT INTO download_versions VALUES(?1,1,?1,?2,?2,'source')",
                params![version, json.to_str().unwrap()],
            )
            .unwrap();
            for name in if ambiguous {
                vec!["inline_123.png", "illust_inline_123.png", "456_p1.png"]
            } else {
                vec!["inline_123.png", "456_p1.png"]
            } {
                asset_id += 1;
                let image = dir.join("data_assets/illustrations").join(name);
                std::fs::write(&image, [version as u8]).unwrap();
                db.execute(
                    "INSERT INTO assets VALUES(?1,1,'illustration',?2,?3,NULL,'image/png',1)",
                    params![asset_id, name, image.to_str().unwrap()],
                )
                .unwrap();
            }
        }
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn imported_pixiv_notation_keeps_each_version_and_declares_unsaved_images_without_falling_back() {
    let fixture = Fixture::new(false);
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
    let work = library.works(1).unwrap().remove(0);
    let snapshots = library.snapshots(work.id).unwrap();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let document = library
            .reader_document(work.id, ResourceScope::Snapshot(snapshot.id))
            .unwrap();
        assert_eq!(document.bindings.len(), 3);
        assert!(matches!(document.body, ReaderBody::PixivText(_)));
        for binding in document.bindings {
            if binding.reference
                == (BodyAssetReference::PixivUploaded {
                    image_id: "999".into(),
                })
            {
                assert!(binding.resource.is_none());
            } else {
                assert_eq!(
                    binding.resource.unwrap().file.unwrap().id,
                    crate::library::Fingerprint::of(&[(index + 1) as u8])
                );
            }
        }
    }
    let before = library.reader_bindings(work.id, snapshots[0].id).unwrap();
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(
        library.reader_bindings(work.id, snapshots[0].id).unwrap(),
        before
    );
    library.backup(&fixture.0.join("backup")).unwrap();
    drop(library);
    drop(source);
    std::fs::rename(fixture.0.join("old"), fixture.0.join("unavailable")).unwrap();
    let backup = Library::open(&fixture.0.join("backup")).unwrap();
    assert_eq!(
        backup.reader_bindings(work.id, snapshots[0].id).unwrap(),
        before
    );
    backup.verify().unwrap();
}

#[test]
#[cfg(windows)]
fn windows_path_spelling_does_not_change_the_owned_reader_assets() {
    for article in [false, true] {
        let fixture = Fixture::new(false);
        {
            let db = Connection::open(fixture.0.join("old/piep.db")).unwrap();
            db.execute_batch("UPDATE download_versions SET json_path=lower(json_path),original_json_path=lower(original_json_path); UPDATE assets SET local_path=lower(local_path);").unwrap();
            if article {
                db.execute("UPDATE downloads SET source='fanbox'", [])
                    .unwrap();
                for version in 1..=2 {
                    let dir = fixture
                        .0
                        .join(format!("old/downloads/pixiv/one/v{version}"));
                    let absolute = dir
                        .join("data_assets/illustrations/inline_123.png")
                        .to_str()
                        .unwrap()
                        .to_ascii_lowercase();
                    std::fs::write(dir.join("original.json"),json!({"detail":{"title":format!("version {version}")},"body":{"images":[{"localPath":absolute},{"localPath":"./data_assets/illustrations/456_p1.png"}]}}).to_string()).unwrap();
                }
            }
        }
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let work = library.works(1).unwrap().remove(0);
        let document = library
            .reader_document(work.id, ResourceScope::Snapshot(work.snapshot_id))
            .unwrap();
        assert_eq!(
            document
                .bindings
                .iter()
                .filter(|b| b.resource.is_some())
                .count(),
            2
        );
        if article {
            assert_eq!(
                document.bindings[0].resource.as_ref().unwrap().reference,
                ResourceReference::Media {
                    slot: "inline_123".into()
                }
            );
            assert_eq!(
                document.bindings[1].resource.as_ref().unwrap().reference,
                ResourceReference::Media {
                    slot: "456_p1".into()
                }
            );
        }
        library.verify().unwrap();
    }
}

#[test]
fn ambiguous_old_image_names_remain_unresolved_without_publishing_a_guess() {
    let fixture = Fixture::new(true);
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let report = source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap();
    assert!(report.unresolved.iter().any(|p| p.contains("対応")));
    assert!(library
        .activation_problems()
        .unwrap()
        .iter()
        .any(|p| p == "__normalization__: reader_bindings"));
    let work = library.works(1).unwrap().remove(0);
    for snapshot in library.snapshots(work.id).unwrap() {
        assert!(library.reader_bindings(work.id, snapshot.id).is_err());
    }
    library.verify().unwrap();
}

#[test]
fn cancellation_before_binding_publication_keeps_migration_incomplete_and_resume_keeps_source_history(
) {
    let fixture = Fixture::new(false);
    let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
    let library = Library::create(&fixture.0.join("new")).unwrap();
    let cancel = AtomicBool::new(false);
    let stopped = source.import(&library, &cancel, |report| {
        if report.works == 1 {
            cancel.store(true, Ordering::Release);
        }
    });
    assert!(matches!(stopped, Err(Error::Canceled)));
    assert!(library
        .activation_problems()
        .unwrap()
        .iter()
        .any(|p| p == "__normalization__: migration"));
    let work = library.works(1).unwrap().remove(0);
    assert_eq!(library.snapshots(work.id).unwrap().len(), 2);
    assert!(source
        .import(&library, &AtomicBool::new(false), |_| {})
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(library.snapshots(work.id).unwrap().len(), 2);
    assert_eq!(
        library
            .reader_document(work.id, ResourceScope::Snapshot(work.snapshot_id))
            .unwrap()
            .bindings
            .len(),
        3
    );
    library.verify().unwrap();
}

#[test]
fn explicit_article_urls_and_paths_are_checked_and_different_known_sources_are_not_substituted() {
    let fixture = Fixture::new(false);
    let root = fixture.0.join("old").canonicalize().unwrap();
    let path = root.join("downloads/pixiv/one/v1/original.json");
    let assets = vec![Asset {
        reference: ResourceReference::Media {
            slot: "opaque".into(),
        },
        name: "image.png".into(),
        paths: vec![spelling(
            &root,
            "downloads/pixiv/one/v1/data_assets/illustrations/inline_123.png",
        )
        .unwrap()],
        url: Some("https://example.test/original".into()),
    }];
    let body = SourceBody::FanboxBody(
        json!({"images":[{"id":"different-name","originalUrl":"https://example.test/original"}]}),
    );
    assert_eq!(
        choose(
            &root,
            &path,
            &body,
            &BodyAssetReference::ArticleImageAt { index: 0 },
            &assets
        )
        .unwrap(),
        Some(assets[0].reference.clone())
    );
    let wrong = SourceBody::FanboxBody(
        json!({"images":[{"id":"image","originalUrl":"https://example.test/other"}]}),
    );
    assert!(choose(
        &root,
        &path,
        &wrong,
        &BodyAssetReference::ArticleImageAt { index: 0 },
        &assets
    )
    .unwrap()
    .is_none());
    let escape = SourceBody::FanboxBody(json!({"images":[{"localPath":"../foreign.png"}]}));
    assert!(matches!(
        choose(
            &root,
            &path,
            &escape,
            &BodyAssetReference::ArticleImageAt { index: 0 },
            &assets
        ),
        Err(Error::UnsafePath(_))
    ));
    let conflict = SourceBody::FanboxBody(
        json!({"images":[{"localPath":"./data_assets/illustrations/inline_123.png","originalUrl":"https://example.test/other"}]}),
    );
    assert!(matches!(
        choose(
            &root,
            &path,
            &conflict,
            &BodyAssetReference::ArticleImageAt { index: 0 },
            &assets
        ),
        Err(Error::Integrity(_))
    ));
}
