use super::*;
use std::fs;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn pixiv(text: &str) -> Value {
    json!({
        "detail": {
            "title": "題",
            "caption": "説明",
            "create_date": "2026-01-01T00:00:00+09:00",
            "user": { "id": 77, "name": "作者" },
            "tags": [{ "name": "雨" }, { "name": "雨" }, "晴れ"],
            "text": text,
            "cover_url": "https://i.pximg.net/c/600x600/novel-cover-master/img/2026/01/01/00/00/00/ci1_abc_master1200.jpg"
        }
    })
}

#[test]
fn fields_follow_the_saved_response_shape() {
    let fields = work_fields("pixiv", &pixiv("本文[uploadedimage:1]"));
    assert_eq!(fields.metadata["content_type"], json!("novel"));
    assert_eq!(fields.metadata["author_key"], json!(77));
    assert_eq!(fields.metadata["tags"], json!(["晴れ", "雨"]));
    assert!(matches!(fields.body, Some(SourceBody::PixivText(_))));
    assert_eq!(fields.field_states["media"], "known");

    let fanbox = json!({ "title": "投稿", "type": "article", "creatorId": "c", "publishedDatetime": "2026-01-01T00:00:00+09:00", "body": null });
    let fields = work_fields("fanbox", &fanbox);
    assert_eq!(fields.metadata["content_type"], json!("article"));
    assert_eq!(fields.metadata["author_key"], json!("c"));
    assert!(fields.body.is_none());
}

#[test]
fn a_save_keeps_the_response_and_files_and_does_not_repeat_the_same_content() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let staged = temp.0.join("staged");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("cover.jpg"), b"cover").unwrap();
    fs::write(staged.join("1.png"), b"image").unwrap();
    fs::write(staged.join("a.pdf"), b"%PDF").unwrap();
    let assets = || {
        vec![
            StagedAsset {
                role: "cover".into(),
                name: "cover.jpg".into(),
                path: staged.join("cover.jpg"),
                url: None,
                mime_type: Some("image/jpeg".into()),
            },
            StagedAsset {
                role: "illustration".into(),
                name: "1.png".into(),
                path: staged.join("1.png"),
                url: Some(
                    "https://i.pximg.net/novel-cover-original/img/2026/01/01/00/00/00/1.png".into(),
                ),
                mime_type: Some("image/png".into()),
            },
            StagedAsset {
                role: "file".into(),
                name: "a.pdf".into(),
                path: staged.join("a.pdf"),
                url: None,
                mime_type: Some("application/pdf".into()),
            },
        ]
    };
    let data = pixiv("本文");
    let save = |data: &Value| {
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "123",
                route: "save",
                data,
                assets: assets(),
            })
            .unwrap()
    };
    let first = save(&data);
    assert!(first.created_revision);
    let card = library.work_card(first.entity_id).unwrap();
    assert_eq!(card.title, "題");
    // 同じ中身をもう一度保存しても、版は増えない。
    let again = save(&data);
    assert_eq!(again.entity_id, first.entity_id);
    assert_eq!(again.snapshot_id, first.snapshot_id);
    assert!(!again.created_revision);
    // 本文が変われば改稿になる。
    let changed = save(&pixiv("直した本文"));
    assert!(changed.created_revision);
    assert_ne!(changed.snapshot_id, first.snapshot_id);
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 2);
    library.verify().unwrap();
}

#[test]
fn a_failed_save_closes_its_acquisition_and_keeps_the_current_version() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = pixiv("本文");
    let first = library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key: "123",
            route: "save",
            data: &data,
            assets: Vec::new(),
        })
        .unwrap();
    // 保存の前に取得を始めた記録を作っておいてもよい（同じ要求なら二度目は同じ記録を返す）。
    let request = LibraryId::now_v7();
    library
        .begin_acquisition(request, "pixiv", EntityKind::Work, "123", "save")
        .unwrap();
    let missing = library.save_work(WorkSave {
        request,
        provider: "pixiv",
        key: "123",
        route: "save",
        data: &pixiv("直した本文"),
        assets: vec![StagedAsset {
            role: "cover".into(),
            name: "cover.jpg".into(),
            path: temp.0.join("ない.jpg"),
            url: None,
            mime_type: None,
        }],
    });
    assert!(missing.is_err());
    assert_eq!(
        library.acquisition_status(request).unwrap().state,
        super::super::AcquisitionState::Cancelled
    );
    assert_eq!(library.snapshots(first.entity_id).unwrap().len(), 1);
    library.verify().unwrap();
}

#[test]
fn saving_a_removed_work_again_puts_it_back_even_when_nothing_changed() {
    let temp = Temp(std::env::temp_dir().join(format!("piep-work-save-{}", LibraryId::now_v7())));
    let library = Library::create(&temp.0.join("shelf")).unwrap();
    let data = pixiv("本文");
    let save = || {
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "pixiv",
                key: "123",
                route: "save",
                data: &data,
                assets: Vec::new(),
            })
            .unwrap()
    };
    let first = save();
    let generation = library
        .library_item_generation(first.entity_id)
        .unwrap()
        .unwrap();
    library
        .remove_work(LibraryId::now_v7(), first.entity_id, generation)
        .unwrap();
    assert!(library.work_card(first.entity_id).is_err());
    let again = save();
    assert!(!again.created_revision);
    assert_eq!(library.work_card(first.entity_id).unwrap().title, "題");
    library.verify().unwrap();
}

#[test]
fn image_quality_is_read_from_the_url_for_pixiv_and_fanbox() {
    use super::quality;
    for (url, expected) in [
        ("https://i.pximg.net/c/240x480_80/novel-cover-master/img/2024/11/15/19/39/13/ci1_master1200.jpg", "thumbnail"),
        ("https://i.pximg.net/novel-cover-original/img/2024/11/15/19/39/13/ci1.png", "original"),
        ("https://i.pximg.net/img-original/img/2018/09/17/00/00/11/1_p0.png", "original"),
        ("https://i.pximg.net/img-master/img/2018/09/17/00/00/11/1_p0_master1200.jpg", "master"),
        ("https://pixiv.pximg.net/c/1200x630_90_a2_g5/fanbox/public/images/post/1/cover/a.jpeg", "thumbnail"),
        ("https://pixiv.pximg.net/fanbox/public/images/post/1/cover/a.jpeg", "original"),
        ("https://downloads.fanbox.cc/images/post/1/a.jpeg", "original"),
        ("https://example.com/a.png", "unknown"),
    ] {
        assert_eq!(quality(Some(url)), expected, "{url}");
    }
    assert_eq!(quality(None), "unknown");
}

/// 旧棚から移した記録（schema 29 の形）を作り、030 で今の取得と同じ形にそろうことを確かめる。
#[test]
fn migrated_acquisition_routes_and_image_quality_are_settled_and_refingerprinted() {
    settle_migrated_acquisitions(false);
    // 28 は見出しだけを移し、対応表の指紋を移し忘れた。その形の対応表も直る。
    settle_migrated_acquisitions(true);
}

fn settle_migrated_acquisitions(stale_bindings: bool) {
    let temp = Temp(std::env::temp_dir().join(format!("piep-routes-{}", LibraryId::now_v7())));
    let root = temp.0.join("shelf");
    let staged = temp.0.join("staged");
    fs::create_dir_all(&staged).unwrap();
    fs::write(staged.join("a.jpeg"), b"image").unwrap();
    let post = json!({"title":"投稿","type":"article","creatorId":"c","publishedDatetime":"2026-01-01T00:00:00+09:00",
        "body":{"blocks":[{"type":"image","imageId":"i"}],"imageMap":{"i":{"id":"i","originalUrl":"https://downloads.fanbox.cc/images/post/1/a.jpeg"}}}});
    let saved = {
        let library = Library::create(&root).unwrap();
        // アプリと同じく取得を始めてから保存し、取得の読む形の記録も作る。
        let request = LibraryId::now_v7();
        library
            .begin_acquisition(request, "fanbox", EntityKind::Work, "1", "save")
            .unwrap();
        library
            .save_readable_work(
                WorkSave {
                    request,
                    provider: "fanbox",
                    key: "1",
                    route: "save",
                    data: &post,
                    assets: vec![StagedAsset {
                        role: "illustration".into(),
                        name: "a.jpeg".into(),
                        path: staged.join("a.jpeg"),
                        url: Some("https://downloads.fanbox.cc/images/post/1/a.jpeg".into()),
                        mime_type: Some("image/jpeg".into()),
                    }],
                },
                None,
            )
            .unwrap()
    };
    let snapshot = saved.snapshot_id.as_bytes().to_vec();
    {
        // 移した直後の形へ戻す（経路は legacy-*、画質は不明、行を写した記録つき）。指紋もその形で。
        let conn = rusqlite::Connection::open(root.join("library.db")).unwrap();
        conn.execute_batch(
            "DROP TRIGGER acquisition_observations_immutable; DROP TRIGGER media_observations_immutable;
             DROP TRIGGER media_renditions_immutable; DROP TRIGGER published_snapshot_immutable;
             DROP TRIGGER reader_binding_header_update; DROP TRIGGER acquisition_reader_update;
             UPDATE acquisition_observations SET route='legacy-import';
             UPDATE media_observations SET route='legacy-import';
             UPDATE media_renditions SET quality='unknown';",
        )
        .unwrap();
        let copy = LibraryId::now_v7();
        conn.execute("INSERT INTO acquisition_observations(id,entity_id,request_id,request_hash,acquired_at,recorded_at,route,status,raw_file_id,body_file_id,body_format,result_snapshot_id,created_revision)
            SELECT ?1,entity_id,?2,request_hash,acquired_at,recorded_at,'legacy-current-row',status,raw_file_id,body_file_id,body_format,result_snapshot_id,0 FROM acquisition_observations LIMIT 1",
            rusqlite::params![copy.as_bytes().as_slice(), LibraryId::now_v7().as_bytes().as_slice()]).unwrap();
        conn.execute("INSERT INTO snapshot_observations(entity_id,snapshot_id,observation_id) SELECT entity_id,?1,?2 FROM revision_snapshots WHERE id=?1",
            rusqlite::params![snapshot.as_slice(), copy.as_bytes().as_slice()]).unwrap();
        let (_, manifest) =
            super::super::integrity::snapshot_fingerprints(&conn, saved.snapshot_id).unwrap();
        if stale_bindings {
            conn.execute(
                "UPDATE reader_binding_sets SET source_manifest=?2 WHERE snapshot_id=?1",
                rusqlite::params![snapshot.as_slice(), manifest.0.as_slice()],
            )
            .unwrap();
        } else {
            assert!(!super::super::reader::move_to_manifest(
                &conn,
                saved.entity_id,
                saved.snapshot_id,
                manifest
            )
            .unwrap());
        }
        conn.execute(
            "UPDATE revision_snapshots SET manifest_hash=?2 WHERE id=?1",
            rusqlite::params![snapshot.as_slice(), manifest.0.as_slice()],
        )
        .unwrap();
        let held: i64 = conn
            .query_row("SELECT count(*) FROM reader_binding_sets", [], |r| r.get(0))
            .unwrap();
        assert_eq!(held, 1, "読む形の対応表も指紋の移し替えの対象になる");
        let outputs: i64 = conn
            .query_row("SELECT count(*) FROM acquisition_reader_outputs", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(outputs, 1, "取得の読む形の記録も指紋の移し替えの対象になる");
        conn.execute_batch(&super::super::schema::guards_for_tests())
            .unwrap();
        conn.execute_batch(
            "DROP TABLE library_tasks; DELETE FROM schema_migrations WHERE version>=30; PRAGMA user_version=29;",
        )
        .unwrap();
    }
    let before: Vec<u8> = rusqlite::Connection::open(root.join("library.db"))
        .unwrap()
        .query_row(
            "SELECT manifest_hash FROM revision_snapshots WHERE id=?1",
            [snapshot.as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    let library = Library::open(&root).unwrap();
    library.verify().unwrap();
    let after: Vec<u8> = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT manifest_hash FROM revision_snapshots WHERE id=?1",
            [snapshot.as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    assert_ne!(before, after, "移した形の版の指紋は計算し直される");
    library
        .reader_document(
            saved.entity_id,
            crate::library::ResourceScope::Snapshot(saved.snapshot_id),
        )
        .unwrap();
    let conn = library.reader().unwrap();
    let routes: Vec<String> = conn
        .prepare("SELECT route FROM acquisition_observations UNION ALL SELECT route FROM media_observations")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(routes, ["save", "save"]);
    let quality: String = conn
        .query_row("SELECT quality FROM media_renditions", [], |r| r.get(0))
        .unwrap();
    assert_eq!(quality, "original");
    let linked: i64 = conn
        .query_row(
            "SELECT count(*) FROM snapshot_observations WHERE snapshot_id=?1",
            [snapshot.as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(linked, 1);
}
