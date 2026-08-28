use super::*;
use std::collections::HashSet;
use std::fs;
use std::time::{Duration, Instant};

#[test]
fn malformed_json_projection_is_reported_instead_of_becoming_empty_data() {
    let conn = Connection::open_in_memory().unwrap();
    let error = conn
        .query_row("SELECT 'not-json'", [], |row| {
            json_column_or_default::<Vec<String>>(row, 0)
        })
        .unwrap_err();

    assert!(
        matches!(error, rusqlite::Error::FromSqlConversionFailure(..)),
        "unexpected error: {error}"
    );
}

/// Builds prose whose vocabulary keeps growing, the way a real library does.
fn synthetic_body(seed: u64, chars: usize) -> String {
    let nouns = [
        "教室",
        "図書館",
        "海岸",
        "旋律",
        "記憶",
        "季節",
        "手紙",
        "灯台",
        "回廊",
        "約束",
        "硝子",
        "残響",
        "標本",
        "封筒",
        "螺旋",
        "夜明",
        "輪郭",
        "潮騒",
        "書架",
        "遠雷",
    ];
    let verbs = [
        "見つめていた",
        "思い出していた",
        "書き留めた",
        "数えていた",
        "聞いていた",
        "受け止めた",
    ];
    let adjectives = ["静かな", "薄い", "淡い", "遠い", "冷たい", "眩しい"];
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as usize
    };
    let mut text = String::with_capacity(chars * 3);
    while text.chars().count() < chars {
        text.push_str(adjectives[next() % adjectives.len()]);
        text.push_str(nouns[next() % nouns.len()]);
        text.push('と');
        text.push_str(nouns[next() % nouns.len()]);
        let number = next() % 100_000;
        text.push_str(&format!("{number}番"));
        text.push('を');
        text.push_str(verbs[next() % verbs.len()]);
        text.push_str("。\n");
    }
    text.chars().take(chars).collect()
}

#[test]
#[ignore = "measurement harness, run with --ignored --nocapture"]
fn measure_full_index_rebuild() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let works: usize = std::env::var("PIEP_BENCH_WORKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300);
    let body_chars: usize = std::env::var("PIEP_BENCH_CHARS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12_000);

    let seeding = Instant::now();
    for index in 0..works {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("bench-{index}"),
            &format!("計測用作品 {index}"),
            &format!("計測作者 {}", index % 40),
            &["計測", "長編"],
            &synthetic_body(index as u64 + 1, body_chars),
        );
    }
    println!(
        "seeded {works} works of {body_chars} chars in {:.1} s",
        seeding.elapsed().as_secs_f64()
    );

    let started = Instant::now();
    let outcome = db
        .rebuild_search_index(
            SearchIndexRebuildOptions::default(),
            &|| false,
            |_progress| {},
        )
        .unwrap();
    let elapsed = started.elapsed().as_secs_f64();
    let status = db.get_search_index_status().unwrap();
    let index_bytes = recursive_file_size(&storage.join("search-index"));
    println!(
            "rebuilt {} works ({} failed) in {elapsed:.2} s = {:.1} works/s | pending {} | index {:.1} MB",
            outcome.processed,
            outcome.failed,
            outcome.processed as f64 / elapsed,
            status.pending_downloads,
            index_bytes as f64 / 1_048_576.0,
        );
    assert_eq!(status.pending_downloads, 0);
}

#[test]
fn reader_transport_pages_keep_complete_source_blocks() {
    let large = "あ".repeat(READER_PAGE_TARGET_BYTES / 3);
    let html = format!(
        "<p>{large}</p><!-- content-block --><p>{large}</p><!-- content-block --><p>{large}</p>"
    );
    let pages = paginate_reader_html(&html, "fanbox");
    assert!(pages.len() >= 2);
    assert!(pages.iter().all(|page| !page.contains("content-block")));
    assert!(pages
        .iter()
        .all(|page| page.starts_with("<p>") && page.ends_with("</p>")));

    let pixiv = paginate_reader_html("first<!-- newpage -->second", "pixiv");
    assert_eq!(pixiv, vec!["first", "second"]);

    // 明示的な1ページが巨大でも、1回の IPC に丸ごと載せない。
    let line = format!("{}<br />\n", "あ".repeat(READER_PAGE_TARGET_BYTES / 6));
    let oversized_pixiv = paginate_reader_html(&line.repeat(4), "pixiv");
    assert!(oversized_pixiv.len() >= 2);
    assert_eq!(oversized_pixiv.concat(), line.repeat(4).trim());
    assert!(oversized_pixiv
        .iter()
        .all(|page| page.len() <= READER_PAGE_TARGET_BYTES));
}

#[test]
fn diagnostic_percentiles_are_stable() {
    let mut samples = [9.0, 1.0, 7.0, 3.0, 5.0];
    assert_eq!(benchmark_percentiles(&mut samples), (5.0, 9.0));
}

#[test]
fn diagnostic_scan_aggregates_each_tree_in_one_bounded_walk() {
    let (_temp, root, storage) = temp_paths();
    let version = storage.join("pixiv/work/v1");
    let assets = version.join("data_assets");
    let lexical = storage.join("search-index");
    let semantic = root.join("search");
    fs::create_dir_all(&assets).unwrap();
    fs::create_dir_all(&lexical).unwrap();
    fs::create_dir_all(&semantic).unwrap();
    fs::write(version.join("original.json"), b"12345").unwrap();
    let known = assets.join("known.png");
    fs::write(&known, b"123").unwrap();
    fs::write(assets.join("orphan.png"), b"1234").unwrap();
    fs::write(lexical.join("segment.store"), b"123456").unwrap();
    fs::write(lexical.join("meta.json"), b"12").unwrap();
    fs::write(semantic.join("vectors.bin"), b"1234567").unwrap();
    let interrupted = storage.join("pixiv/work/.v2.0123abcd.stage");
    fs::create_dir_all(&interrupted).unwrap();
    fs::write(interrupted.join("data.json"), b"123").unwrap();
    let known_paths = HashSet::from([normalized_diagnostic_path(&known)]);
    let mut is_known_asset =
        |path: &Path| Ok(known_paths.contains(&normalized_diagnostic_path(path)));

    let stats = collect_diagnostic_file_stats(&storage, &semantic, &mut is_known_asset).unwrap();

    assert_eq!(stats.storage_size_bytes, 23);
    assert_eq!(stats.lexical_index_size_bytes, 8);
    assert_eq!(stats.lexical_index_file_count, 2);
    assert_eq!(stats.lexical_index_segment_count, 1);
    assert_eq!(stats.semantic_index_size_bytes, 7);
    assert_eq!(stats.orphan_asset_files, 1);
    assert_eq!(stats.orphan_asset_file_bytes, 4);
    assert_eq!(stats.transient_files, 1);
    assert_eq!(stats.transient_file_bytes, 3);
    assert_eq!(
        stats.visited_entries, 13,
        "each directory entry is visited once, including disjoint semantic storage"
    );
}

#[test]
fn file_integrity_check_streams_and_classifies_manual_changes() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let missing_download = storage.join("pixiv/missing/v1/original.json");
    let missing_version = storage.join("pixiv/missing/v2/original.json");
    let asset = storage.join("pixiv/missing/v1/data_assets/image.png");
    let empty_profile = root.join("profiles/pixiv/empty/icon.png");
    let entity_json = root.join("series/pixiv/series-1/v1/data.json");
    let outside_profile = root.join("outside.png");
    fs::create_dir_all(asset.parent().unwrap()).unwrap();
    fs::create_dir_all(empty_profile.parent().unwrap()).unwrap();
    fs::create_dir_all(entity_json.parent().unwrap()).unwrap();
    fs::write(&asset, b"123").unwrap();
    fs::write(&empty_profile, b"").unwrap();
    fs::write(&entity_json, b"{}").unwrap();
    fs::write(&outside_profile, b"outside").unwrap();

    let conn = db.conn.lock().unwrap();
    conn.execute(
        "INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                json_path, downloaded_at
             ) VALUES ('pixiv', 'missing', '作品', '作者', 'author', 'novel', ?1, ?2)",
        params![
            missing_download.to_string_lossy(),
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .unwrap();
    let download_id = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO download_versions (
                download_id, version, json_path, created_at
             ) VALUES (?1, 2, ?2, ?3)",
        params![
            download_id,
            missing_version.to_string_lossy(),
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO assets (
                download_id, asset_type, filename, local_path, file_size_bytes
             ) VALUES (?1, 'image', 'image.png', ?2, 9)",
        params![download_id, asset.to_string_lossy()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO people (source, source_key, display_name, icon_path)
             VALUES ('pixiv', 'outside', '外部', ?1)",
        params![outside_profile.to_string_lossy()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO series (source, source_key, title, cover_path)
             VALUES ('pixiv', 'empty', '空', ?1)",
        params![empty_profile.to_string_lossy()],
    )
    .unwrap();
    // シリーズの表紙は、最初に保存した作品の表紙をそのまま指すことがある。
    // downloads/ の下にあっても、これはアプリが置いたものである。
    conn.execute(
        "INSERT INTO series (source, source_key, title, cover_path)
             VALUES ('pixiv', 'borrowed', '借りた表紙', ?1)",
        params![asset.to_string_lossy()],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO entity_versions (
                entity_type, source, source_key, version, json_path, created_at
             ) VALUES ('series', 'pixiv', 'series-1', 1, ?1, ?2)",
        params![
            entity_json.to_string_lossy(),
            chrono::Utc::now().to_rfc3339()
        ],
    )
    .unwrap();

    let integrity = check_library_file_integrity(&conn, &storage, &root).unwrap();
    assert_eq!(integrity.checked_file_references, 7);
    assert_eq!(integrity.missing_json_files, 2);
    assert_eq!(integrity.missing_asset_files, 0);
    assert_eq!(integrity.missing_profile_files, 0);
    assert_eq!(
        integrity.unsafe_referenced_files, 1,
        "作品の表紙を借りたシリーズは、許可領域外ではない"
    );
    assert_eq!(integrity.unreadable_referenced_files, 0);
    assert_eq!(integrity.empty_referenced_files, 1);
    assert_eq!(integrity.mismatched_asset_files, 1);
    assert_eq!(integrity.issue_samples.len(), 5);
    assert!(integrity
        .issue_samples
        .iter()
        .any(|issue| issue.issue_type == "unsafe"
            && issue.path.ends_with("outside.png")
            && issue.label.as_deref() == Some("外部")));
    assert!(integrity
        .issue_samples
        .iter()
        .any(|issue| issue.issue_type == "missing" && issue.label.as_deref() == Some("作品")));
    assert!(integrity
        .issue_samples
        .iter()
        .any(|issue| issue.path.ends_with("icon.png")
            && issue.category == "profile"
            && issue.issue_type == "empty"));
    assert!(!integrity
        .issue_samples
        .iter()
        .any(|issue| issue.path.ends_with("data.json")));
    let lookup_plan = conn
        .prepare("EXPLAIN QUERY PLAN SELECT 1 FROM assets WHERE local_path = ?1 LIMIT 1")
        .unwrap()
        .query_map(params![asset.to_string_lossy()], |row| {
            row.get::<_, String>(3)
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
        .join(" ");
    assert!(
        lookup_plan.contains("idx_assets_local_path"),
        "{lookup_plan}"
    );
    drop(conn);
    drop(db);
}

#[test]
fn diagnostic_scan_skips_links_and_enforces_depth_and_entry_limits() {
    let (_temp, root, storage) = temp_paths();
    let outside = root.join("outside");
    fs::create_dir_all(outside.join("data_assets")).unwrap();
    fs::write(outside.join("data_assets/escape.png"), b"outside").unwrap();
    fs::write(storage.join("inside.bin"), b"i").unwrap();
    let link = storage.join("linked");
    #[cfg(unix)]
    let link_result = std::os::unix::fs::symlink(&outside, &link);
    #[cfg(windows)]
    let link_result = std::os::windows::fs::symlink_dir(&outside, &link);
    #[cfg(not(any(unix, windows)))]
    let link_result: std::io::Result<()> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "links unavailable",
    ));
    if link_result.is_ok() {
        let mut no_known_assets = |_path: &Path| Ok(false);
        let stats =
            collect_diagnostic_file_stats(&storage, &root.join("search"), &mut no_known_assets)
                .unwrap();
        assert_eq!(stats.storage_size_bytes, 1);
        assert_eq!(stats.orphan_asset_files, 0);
    }

    let mut no_known_assets = |_path: &Path| Ok(false);
    fs::create_dir_all(storage.join("a/b")).unwrap();
    let depth_error = collect_diagnostic_file_stats_with_limits(
        &storage,
        &root.join("search"),
        &mut no_known_assets,
        DiagnosticScanLimits {
            max_depth: 0,
            max_entries: 100,
        },
    )
    .unwrap_err();
    assert!(depth_error.contains("depth-0"));

    let entry_error = collect_diagnostic_file_stats_with_limits(
        &storage,
        &root.join("search"),
        &mut no_known_assets,
        DiagnosticScanLimits {
            max_depth: 64,
            max_entries: 1,
        },
    )
    .unwrap_err();
    assert!(entry_error.contains("1-entry"));
}

#[test]
fn atomic_restore_transaction_rolls_back_database_rows() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download_unindexed(
        &db,
        &storage,
        "restore-rollback",
        "残る作品",
        "作者",
        &[],
        "本文",
    );
    db.begin_atomic_restore().unwrap();
    db.delete_download_record_for_restore(id).unwrap();
    std::thread::scope(|scope| {
        let concurrent = scope
            .spawn(|| db.delete_download_record_for_restore(id))
            .join()
            .unwrap();
        assert!(concurrent.unwrap_err().contains("restore is in progress"));
    });
    db.rollback_atomic_restore();
    assert_eq!(db.get_download(id).unwrap().title, "残る作品");
}

#[test]
fn injected_save_failure_rolls_back_download_tags_assets_and_version() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let json_path = storage.join("pixiv/atomic-save/v1/original.json");
    let download = NewDownload {
        source: "pixiv".to_string(),
        source_id: "atomic-save".to_string(),
        title: "Atomic save".to_string(),
        author_name: "Author".to_string(),
        author_id: "author".to_string(),
        content_type: "novel".to_string(),
        tags: vec!["atomic".to_string()],
        excerpt: None,
        cover_path: None,
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(json_path.to_string_lossy().to_string()),
        asset_count: 1,
        file_size_bytes: 10,
        downloaded_at: "2026-08-12T00:00:00Z".to_string(),
        source_created_at: None,
        content_hash: Some("hash".to_string()),
        text_length: 4,
        source_updated_at: None,
        watch_updates: false,
        current_version: 1,
        favorite: false,
    };
    let assets = vec![NewAsset {
        download_id: 0,
        asset_type: "illustration".to_string(),
        filename: "image.png".to_string(),
        local_path: storage
            .join("pixiv/atomic-save/v1/data_assets/image.png")
            .to_string_lossy()
            .to_string(),
        original_url: None,
        mime_type: Some("image/png".to_string()),
        file_size_bytes: 6,
    }];
    let versions = vec![NewVersion {
        download_id: 0,
        version: 1,
        content_hash: Some("hash".to_string()),
        text_length: 4,
        json_path: download.json_path.clone(),
        original_json_path: download.original_json_path.clone(),
        asset_count: 1,
        file_size_bytes: 10,
        created_at: download.downloaded_at.clone(),
        change_summary: None,
    }];

    assert!(db
        .commit_download_save_with_injected_failure(&download, &assets, &versions)
        .is_err());

    let duplicate_versions = vec![versions[0].clone(), versions[0].clone()];
    assert!(db
        .commit_reimported_download(&download, &assets, &duplicate_versions)
        .is_err());

    assert!(db
        .get_download_by_source("pixiv", "atomic-save")
        .unwrap()
        .is_none());
    let conn = db.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM download_tags", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM assets", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM download_versions", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    drop(conn);
}

#[test]
fn startup_removes_only_journaled_uncommitted_versions_and_reserved_stages() {
    let (_temp, root, storage) = temp_paths();
    let db_path = root.join("piep.db");
    let db = Database::open(&db_path, &storage).unwrap();
    insert_download_unindexed(
        &db,
        &storage,
        "crash-recovery",
        "Committed work",
        "Author",
        &[],
        "body",
    );
    let work = storage.join("pixiv/crash-recovery");
    let committed = work.join("v1");
    let unjournaled = work.join("v2");
    let stage = work.join(".v2.0123456789abcdef.stage");
    let published_stage = work.join(".v3.fedcba9876543210.stage");
    let published = work.join("v3");
    let unrelated = work.join("notes");
    fs::create_dir_all(&unjournaled).unwrap();
    fs::write(unjournaled.join("original.json"), b"user data").unwrap();
    fs::create_dir_all(&stage).unwrap();
    fs::write(stage.join("original.json"), b"staged").unwrap();
    fs::create_dir_all(&published_stage).unwrap();
    fs::write(published_stage.join("original.json"), b"published").unwrap();
    db.create_download_save_journal("pixiv", "crash-recovery", 3, &published_stage, &published)
        .unwrap();
    fs::rename(&published_stage, &published).unwrap();
    fs::create_dir_all(&unrelated).unwrap();
    drop(db);

    let reopened = Database::open(&db_path, &storage).unwrap();

    assert!(committed.exists());
    assert!(
        unjournaled.exists(),
        "an unregistered version may be user data awaiting import"
    );
    assert!(!stage.exists());
    assert!(!published.exists(), "journal proves v3 was never committed");
    assert!(unrelated.exists(), "unknown directories are user data");
    let conn = reopened.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM download_save_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    drop(conn);
    drop(reopened);
}

#[test]
fn startup_preserves_a_version_committed_with_its_journal_marker() {
    let (_temp, root, storage) = temp_paths();
    let db_path = root.join("piep.db");
    let db = Database::open(&db_path, &storage).unwrap();
    let work = storage.join("pixiv/committed-journal");
    let stage = work.join(".v1.0123456789abcdef.stage");
    let final_path = work.join("v1");
    fs::create_dir_all(&stage).unwrap();
    let json_path = final_path.join("original.json");
    fs::write(stage.join("original.json"), b"committed body").unwrap();
    let journal_id = db
        .create_download_save_journal("pixiv", "committed-journal", 1, &stage, &final_path)
        .unwrap();
    fs::rename(&stage, &final_path).unwrap();
    let download = NewDownload {
        source: "pixiv".to_string(),
        source_id: "committed-journal".to_string(),
        title: "Committed work".to_string(),
        author_name: "Author".to_string(),
        author_id: "author".to_string(),
        content_type: "novel".to_string(),
        tags: Vec::new(),
        excerpt: None,
        cover_path: None,
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(json_path.to_string_lossy().to_string()),
        asset_count: 0,
        file_size_bytes: 14,
        downloaded_at: "2026-08-12T00:00:00Z".to_string(),
        source_created_at: None,
        content_hash: Some("committed-hash".to_string()),
        text_length: 14,
        source_updated_at: None,
        watch_updates: false,
        current_version: 1,
        favorite: false,
    };
    let version = NewVersion {
        download_id: 0,
        version: 1,
        content_hash: download.content_hash.clone(),
        text_length: download.text_length,
        json_path: download.json_path.clone(),
        original_json_path: download.original_json_path.clone(),
        asset_count: 0,
        file_size_bytes: download.file_size_bytes,
        created_at: download.downloaded_at.clone(),
        change_summary: None,
    };
    db.commit_download_save_with_journal(&download, &[], &[version], &journal_id)
        .unwrap();
    drop(db);

    let reopened = Database::open(&db_path, &storage).unwrap();

    assert!(final_path.join("original.json").exists());
    assert!(reopened
        .get_download_by_source("pixiv", "committed-journal")
        .unwrap()
        .is_some());
    let conn = reopened.conn.lock().unwrap();
    assert_eq!(
        conn.query_row("SELECT COUNT(*) FROM download_save_journal", [], |row| {
            row.get::<_, i64>(0)
        })
        .unwrap(),
        0
    );
    drop(conn);
    drop(reopened);
}

#[test]
fn reimport_record_delete_preserves_the_scanned_work_tree() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download_unindexed(
        &db,
        &storage,
        "reimport-preserve",
        "再取り込み作品",
        "作者",
        &[],
        "本文",
    );
    let json_path = PathBuf::from(db.get_download(id).unwrap().json_path);

    db.delete_download_record_for_reimport(id).unwrap();

    assert!(
        json_path.exists(),
        "the scanner still needs to read this file"
    );
    assert!(db
        .get_download_by_source("pixiv", "reimport-preserve")
        .unwrap()
        .is_none());
}

#[test]
fn deleting_a_legacy_work_never_removes_its_source_siblings() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let insert_legacy = |source_id: &str, title: &str| {
        let work_dir = storage.join("pixiv").join(source_id);
        fs::create_dir_all(&work_dir).unwrap();
        let json_path = work_dir.join("data.json");
        fs::write(&json_path, serde_json::json!({ "text": title }).to_string()).unwrap();
        let id = db
            .upsert_download(&NewDownload {
                source: "pixiv".to_string(),
                source_id: source_id.to_string(),
                title: title.to_string(),
                author_name: "作者".to_string(),
                author_id: "legacy-author".to_string(),
                content_type: "novel".to_string(),
                tags: Vec::new(),
                excerpt: None,
                cover_path: None,
                json_path: json_path.to_string_lossy().to_string(),
                original_json_path: None,
                asset_count: 0,
                file_size_bytes: 0,
                downloaded_at: "2026-01-01T00:00:00Z".to_string(),
                source_created_at: None,
                content_hash: None,
                text_length: 0,
                source_updated_at: None,
                watch_updates: false,
                current_version: 1,
                favorite: false,
            })
            .unwrap();
        (id, work_dir, json_path)
    };
    let (deleted_id, deleted_dir, _) = insert_legacy("legacy-one", "削除対象");
    let (_, sibling_dir, sibling_json) = insert_legacy("legacy-two", "残る作品");

    db.delete_download(deleted_id).unwrap();

    assert!(!deleted_dir.exists());
    assert!(sibling_dir.exists());
    assert!(sibling_json.exists(), "another work in pixiv must survive");
}

#[test]
fn entity_reconstruction_preserves_fetched_profiles_and_versions() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download_unindexed(
        &db,
        &storage,
        "profile-work",
        "プロフィール付き作品",
        "作者",
        &[],
        "本文",
    );
    let author_key = "author-profile-work";
    db.upsert_download_person(id, "pixiv", author_key, "author", "作者")
        .unwrap();
    db.upsert_download_relation(id, "series", "pixiv", "series-1", "連作")
        .unwrap();
    db.upsert_download_series(id, "pixiv", "series-1", "連作", Some(1))
        .unwrap();
    db.upsert_person_profile(
        "pixiv",
        author_key,
        "作者",
        Some("profiles/author/icon.png"),
        Some("profiles/author/cover.png"),
        Some("保存済みプロフィール"),
        Some("{\"web\":\"https://example.com\"}"),
        "person-hash",
        "profiles/author/v1/data.json",
        2,
        128,
        EntityProfileFreshness::RemoteChecked,
    )
    .unwrap();
    db.upsert_series_profile(
        "pixiv",
        "series-1",
        "連作",
        Some("保存済みシリーズ説明"),
        Some("series/series-1/cover.png"),
        "series-hash",
        "series/series-1/v1/data.json",
        1,
        64,
        EntityProfileFreshness::RemoteChecked,
        None,
        None,
    )
    .unwrap();

    db.reconstruct_entities_after_import().unwrap();

    let person = db.get_person("pixiv", author_key).unwrap();
    assert_eq!(person.description.as_deref(), Some("保存済みプロフィール"));
    assert_eq!(person.current_version, 1);
    assert_eq!(
        db.list_entity_versions("person", "pixiv", author_key)
            .unwrap()
            .len(),
        1
    );
    let series = db.get_series("pixiv", "series-1").unwrap();
    assert_eq!(series.description.as_deref(), Some("保存済みシリーズ説明"));
    assert_eq!(series.current_version, 1);
    assert_eq!(
        db.list_entity_versions("series", "pixiv", "series-1")
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn reader_cache_pages_and_full_document_search_share_one_index() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download_unindexed(
        &db,
        &storage,
        "reader-search",
        "検索できる作品",
        "作者",
        &[],
        "最初のページ [newpage] 次のページにNeedleとneedle",
    );
    let first = db.get_reader_content_page(id, None, 0).unwrap();
    let second = db.get_reader_content_page(id, None, 1).unwrap();
    assert_eq!(first.page_count, 2);
    assert_eq!(second.page_count, 2);
    assert!(second.html.contains("Needle"));

    let hits = db.search_reader_content(id, None, "needle", 20).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].page, 2);
    assert_eq!(hits[0].count, 2);
    assert!(hits[0].snippet.to_lowercase().contains("needle"));
}

#[test]
fn editor_blocks_preserve_rich_content_order() {
    let assets = vec![AssetEntry {
        id: 42,
        download_id: 7,
        asset_type: "image".to_string(),
        filename: "scene.webp".to_string(),
        local_path: "assets/scene.webp".to_string(),
        original_url: None,
        mime_type: Some("image/webp".to_string()),
        file_size_bytes: 128,
    }];
    let html = concat!(
        "<p>導入<br>二行目</p>",
        "<!-- newpage -->",
        "<h2>章題</h2>",
        "<img data-local-path=\"assets/scene.webp\" alt=\"挿絵\">",
        "<a class=\"novel-link-card\" href=\"https://example.com/story\">続きはこちら</a>",
        "<hr>",
        "<p>結び</p>"
    );

    let blocks = html_to_editor_blocks(html, &assets);

    assert_eq!(
        blocks
            .iter()
            .map(|block| block.block_type.as_str())
            .collect::<Vec<_>>(),
        vec![
            "paragraph",
            "page_break",
            "heading",
            "image",
            "link",
            "separator",
            "paragraph"
        ]
    );
    assert_eq!(blocks[0].text.as_deref(), Some("導入\n二行目"));
    assert_eq!(blocks[3].asset_id, Some(42));
    assert_eq!(blocks[3].text.as_deref(), Some("挿絵"));
    assert_eq!(blocks[4].text.as_deref(), Some("https://example.com/story"));
    assert!(blocks[4]
        .attrs_json
        .as_deref()
        .is_some_and(|attrs| attrs.contains("続きはこちら")));
    assert!(blocks_to_html(&blocks, &assets).contains("<!-- newpage -->"));
}

/// 前の実行が残した一時ディレクトリを一度だけ掃除する。
///
/// 取りこぼしを掃除するための保険。通常の後始末は `TempRoot` が行うが、
/// それが無かった頃の残骸が temp に積み上がっている。実行中の別プロセスを
/// 巻き込まないよう、1 時間より古いものだけを対象にする。
fn sweep_stale_test_dirs() {
    static SWEEP: std::sync::Once = std::sync::Once::new();
    SWEEP.call_once(|| {
        let Ok(entries) = fs::read_dir(std::env::temp_dir()) else {
            return;
        };
        let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(3_600);
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().starts_with("piep_search_test_") {
                continue;
            }
            let stale = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified < cutoff);
            if stale {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    });
}

/// テストの一時ツリー。テスト終了時に削除する。
///
/// 後始末は `Database` より後に走らなければならない。SQLite は削除を許す
/// 共有モードでファイルを開かないので、`db` が生きている間は Windows の
/// 削除が共有違反で失敗する。旧コードは `db` が生きたまま `remove_dir_all`
/// を呼んでいたため、一時ディレクトリを数千個積み残していた。`Database`
/// より前に束縛すればローカルは宣言と逆順に drop されるので、この順序が
/// 自然に得られる。末尾の `remove_dir_all` と違って panic や早期 return
/// でも走る。
///
/// それでも削除は best-effort のままにしてある。接続の後始末が drop の
/// 直後まで尾を引くことがあり、失敗を致命扱いにすると後始末そのものが
/// テストを落とす要因になるからである。
struct TempRoot {
    root: PathBuf,
    storage: PathBuf,
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        super::super::tantivy_index::release_runtime(&self.storage);
        // Windows unlinks a file that still has an open handle by marking it
        // delete-pending: the contents go, but the name survives until the
        // last handle closes, and a directory that still holds names cannot
        // be removed. SQLite's connections finish closing just after
        // `Database` drops, so the first attempt clears the tree and a
        // retry a few milliseconds later clears the directory itself.
        for attempt in 0..5 {
            if fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2 << attempt));
        }
    }
}

fn temp_paths() -> (TempRoot, PathBuf, PathBuf) {
    sweep_stale_test_dirs();
    let rand_val: u32 = rand::random();
    let root = std::env::temp_dir().join(format!("piep_search_test_{}", rand_val));
    let storage = root.join("downloads");
    fs::create_dir_all(&storage).unwrap();
    let guard = TempRoot {
        root: root.clone(),
        storage: storage.clone(),
    };
    (guard, root, storage)
}

fn params(query: &str) -> SearchV2Params {
    SearchV2Params {
        text: None,
        query: Some(query.to_string()),
        source: None,
        content_type: None,
        sort_by: Some("relevance".to_string()),
        sort_order: Some("desc".to_string()),
        limit: Some(20),
        cursor: None,
        favorite: None,
        tags_include: None,
        tags_exclude: None,
        tag_filter_mode: None,
        authors_include: None,
        authors_exclude: None,
        min_char_count: None,
        max_char_count: None,
        asset_filter: None,
        watch_filter: None,
        person_source: None,
        person_key: None,
        series_source: None,
        series_key: None,
        offset: None,
        ids_include: None,
        view_mode: None,
        projection: None,
        search_mode: None,
    }
}

fn v2_params(query: Option<&str>, limit: i64, cursor: Option<String>) -> SearchV2Params {
    SearchV2Params {
        text: None,
        query: query.map(str::to_string),
        source: None,
        content_type: None,
        sort_by: Some(if query.is_some() { "relevance" } else { "date" }.to_string()),
        sort_order: Some("desc".to_string()),
        limit: Some(limit),
        cursor,
        favorite: None,
        tags_include: None,
        tags_exclude: None,
        tag_filter_mode: None,
        authors_include: None,
        authors_exclude: None,
        min_char_count: None,
        max_char_count: None,
        asset_filter: None,
        watch_filter: None,
        person_source: None,
        person_key: None,
        series_source: None,
        series_key: None,
        offset: None,
        ids_include: None,
        view_mode: None,
        projection: None,
        search_mode: None,
    }
}

fn insert_download(
    db: &Database,
    storage: &Path,
    source_id: &str,
    title: &str,
    author: &str,
    tags: &[&str],
    body: &str,
) -> i64 {
    insert_download_with_reindex(
        db,
        storage,
        TestDownloadInput {
            source_id,
            title,
            author,
            tags,
            body,
            reindex: true,
        },
    )
}

fn insert_download_unindexed(
    db: &Database,
    storage: &Path,
    source_id: &str,
    title: &str,
    author: &str,
    tags: &[&str],
    body: &str,
) -> i64 {
    insert_download_with_reindex(
        db,
        storage,
        TestDownloadInput {
            source_id,
            title,
            author,
            tags,
            body,
            reindex: false,
        },
    )
}

/// Seed a metadata-only browsing fixture in one transaction. Creating one
/// directory and JSON file per work made a million-row acceptance test
/// measure NTFS setup/cleanup for hours instead of the library queries we
/// care about. The generated rows still exercise production indexes,
/// author aggregation, tags, shelves, dates, and cursor tie-breaks.
fn seed_metadata_only_library(db: &Database, works: usize) {
    assert!((1..=1_000_000).contains(&works));
    let mut conn = db.conn.lock().unwrap();
    let tx = conn.transaction().unwrap();
    tx.execute(
        "WITH digits(d) AS (
                 VALUES (0),(1),(2),(3),(4),(5),(6),(7),(8),(9)
             ), numbers(n) AS (
                 SELECT a.d + 10*b.d + 100*c.d + 1000*d.d + 10000*e.d + 100000*f.d
                 FROM digits a
                 CROSS JOIN digits b
                 CROSS JOIN digits c
                 CROSS JOIN digits d
                 CROSS JOIN digits e
                 CROSS JOIN digits f
             )
             INSERT INTO downloads (
                 source, source_id, title, author_name, author_id, content_type,
                 excerpt, json_path, asset_count, file_size_bytes, downloaded_at,
                 source_created_at, content_hash, text_length, watch_updates,
                 current_version, favorite
             )
             SELECT
                 CASE WHEN n % 5 = 0 THEN 'fanbox' ELSE 'pixiv' END,
                 printf('scale-%07d', n),
                 printf('蔵書 %07d', n),
                 printf('作者 %03d', n % 400),
                 printf('author-%03d', n % 400),
                 CASE WHEN n % 5 = 0 THEN 'article' ELSE 'novel' END,
                 '大規模ライブラリ受入試験',
                 printf('synthetic/scale-%07d.json', n),
                 n % 6,
                 4096 + (n % 1048576),
                 printf('2026-%02d-%02dT%02d:00:00Z', 1 + n % 12, 1 + n % 27, n % 24),
                 printf('2025-%02d-%02dT00:00:00Z', 1 + n % 12, 1 + n % 27),
                 printf('fixture-hash-%07d', n),
                 1000 + (n % 200000),
                 CASE WHEN n % 13 = 0 THEN 1 ELSE 0 END,
                 1,
                 CASE WHEN n % 11 = 0 THEN 1 ELSE 0 END
             FROM numbers
             WHERE n < ?1",
        params![works as i64],
    )
    .unwrap();
    for tag in 0..30 {
        tx.execute(
            "INSERT INTO tags (name) VALUES (?1)",
            params![format!("tag{tag}")],
        )
        .unwrap();
    }
    tx.execute(
        "INSERT INTO download_tags (download_id, tag_id)
             SELECT id, 1 + ((id - 1) % 30) FROM downloads",
        [],
    )
    .unwrap();
    tx.commit().unwrap();
    conn.execute_batch("ANALYZE; PRAGMA optimize;").unwrap();
}

struct TestDownloadInput<'a> {
    source_id: &'a str,
    title: &'a str,
    author: &'a str,
    tags: &'a [&'a str],
    body: &'a str,
    reindex: bool,
}

fn insert_download_with_reindex(
    db: &Database,
    storage: &Path,
    input: TestDownloadInput<'_>,
) -> i64 {
    let TestDownloadInput {
        source_id,
        title,
        author,
        tags,
        body,
        reindex,
    } = input;
    let dir = storage.join("pixiv").join(source_id).join("v1");
    fs::create_dir_all(&dir).unwrap();
    let json_path = dir.join("original.json");
    fs::write(
        &json_path,
        serde_json::json!({ "text": body }).to_string().as_bytes(),
    )
    .unwrap();
    let dl = NewDownload {
        source: "pixiv".to_string(),
        source_id: source_id.to_string(),
        title: title.to_string(),
        author_name: author.to_string(),
        author_id: format!("author-{}", source_id),
        content_type: "novel".to_string(),
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        excerpt: Some("短い概要".to_string()),
        cover_path: None,
        json_path: json_path.to_string_lossy().to_string(),
        original_json_path: Some(json_path.to_string_lossy().to_string()),
        asset_count: 0,
        file_size_bytes: 0,
        downloaded_at: "2026-01-01T00:00:00Z".to_string(),
        source_created_at: Some("2026-01-01T00:00:00Z".to_string()),
        content_hash: Some(format!("hash-{}", source_id)),
        text_length: body.chars().count() as i64,
        source_updated_at: None,
        watch_updates: false,
        current_version: 1,
        favorite: false,
    };
    let id = db.upsert_download(&dl).unwrap();
    if reindex {
        db.reindex_download(id).unwrap();
    }
    id
}

/// 実ライブラリで見つかった並び順の崩れを固定する。番号のない初回が
/// 末尾へ流れ、単なる言及リンクで加点された作品が話数を追い越していた。
#[test]
fn unnumbered_opening_leads_and_mentions_do_not_reorder_episodes() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let opening = insert_download_unindexed(
        &db,
        &storage,
        "suggest-open",
        "灯台の話",
        "連載作者",
        &[],
        "初回本文",
    );
    let second = insert_download_unindexed(
        &db,
        &storage,
        "suggest-2",
        "灯台の話#2",
        "連載作者",
        &[],
        "二話本文",
    );
    let third = insert_download_unindexed(
        &db,
        &storage,
        "suggest-3",
        "灯台の話#3",
        "連載作者",
        &[],
        "三話本文",
    );

    // 第2話を種にしても、無印の初回が先頭に来る。
    let suggestion = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![second],
            limit: Some(20),
        })
        .unwrap();
    let order = suggestion
        .members
        .iter()
        .map(|member| member.title.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order, vec!["灯台の話", "灯台の話#2", "灯台の話#3"]);
    assert_eq!(suggestion.collection_kind, "ordered");
    let _ = (opening, third);
}

/// 公式シリーズに同居しているだけの短編集は、束にならない。
///
/// 以前は「候補には出すが既定では選ばない」だった。しかしチェックの
/// 付かない候補が並ぶことに価値は無く、151作のシリーズを種にすると
/// 60件がそうやって画面を埋めた。**同じ棚に載っていることは、同じ束に
/// 属する証拠ではない。**話数が隣り合っていて初めて証拠になる。
#[test]
fn sharing_an_anthology_shelf_yields_no_bundle() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let seed = insert_download_unindexed(
        &db,
        &storage,
        "anth-seed",
        "海の便り",
        "短編作者",
        &[],
        "本文",
    );
    let mut others = Vec::new();
    for index in 0..4 {
        others.push(insert_download_unindexed(
            &db,
            &storage,
            &format!("anth-{index}"),
            &format!("まったく別の話{index}"),
            "短編作者",
            &[],
            "本文",
        ));
    }
    for id in std::iter::once(seed).chain(others.iter().copied()) {
        db.upsert_download_series(id, "pixiv", "anthology", "短編集", None)
            .unwrap();
    }
    let result = db.generate_collection_suggestion(&CollectionSuggestionRequest {
        seed_download_ids: vec![seed],
        limit: Some(20),
    });
    assert_eq!(
        result.unwrap_err(),
        "関連作品は見つかりませんでした",
        "同じ短編集に載っているだけの作品を束にしてはいけない"
    );

    // 話数が入れば話は別。隣り合う2作だけが束になる。
    for (id, order) in std::iter::once(seed)
        .chain(others.iter().copied())
        .zip([1_i64, 2, 40, 41, 42])
    {
        db.upsert_download_series(id, "pixiv", "anthology", "短編集", Some(order))
            .unwrap();
    }
    let suggestion = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![seed],
            limit: Some(20),
        })
        .unwrap();
    assert_eq!(
        suggestion.members.len(),
        2,
        "隣接する1作だけが加わる: {:?}",
        suggestion
            .members
            .iter()
            .map(|member| member.title.as_str())
            .collect::<Vec<_>>()
    );
    assert!(
        suggestion.members.iter().all(|member| member.selected),
        "証拠のあるものだけが残るので、全部が既定で選ばれる"
    );
}

#[test]
fn work_collections_keep_order_and_reconnect_deleted_works() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download_unindexed(
        &db,
        &storage,
        "collection-first",
        "夜の手紙 前編",
        "同じ作者",
        &[],
        "前編本文",
    );
    let second = insert_download_unindexed(
        &db,
        &storage,
        "collection-second",
        "夜の手紙 後編",
        "同じ作者",
        &[],
        "後編本文",
    );
    let collection = db
        .upsert_work_collection(&WorkCollectionInput {
            name: "夜の手紙".to_string(),
            description: Some("前後編".to_string()),
            collection_kind: "ordered".to_string(),
            cover_download_id: Some(first),
            ..Default::default()
        })
        .unwrap();
    let collection = db
        .add_work_collection_members(
            &collection.summary.id,
            &[
                WorkCollectionMemberInput {
                    source: "pixiv".to_string(),
                    source_id: "collection-first".to_string(),
                    title_snapshot: None,
                    author_snapshot: None,
                    position: None,
                    member_role: None,
                    added_by: None,
                    pinned: None,
                    note: None,
                },
                WorkCollectionMemberInput {
                    source: "pixiv".to_string(),
                    source_id: "collection-second".to_string(),
                    title_snapshot: None,
                    author_snapshot: None,
                    position: None,
                    member_role: None,
                    added_by: None,
                    pinned: None,
                    note: None,
                },
            ],
        )
        .unwrap();
    assert_eq!(collection.summary.member_count, 2);
    assert_eq!(collection.summary.available_count, 2);

    let reordered = db
        .reorder_work_collection_members(
            &collection.summary.id,
            &[
                WorkKey {
                    source: "pixiv".to_string(),
                    source_id: "collection-second".to_string(),
                },
                WorkKey {
                    source: "pixiv".to_string(),
                    source_id: "collection-first".to_string(),
                },
            ],
        )
        .unwrap();
    assert_eq!(reordered.members[0].download_id, Some(second));

    db.delete_download(first).unwrap();
    let missing = db.get_work_collection(&collection.summary.id).unwrap();
    assert_eq!(missing.summary.member_count, 2);
    assert_eq!(missing.summary.available_count, 1);
    assert!(missing
        .members
        .iter()
        .any(|member| member.source_id == "collection-first" && member.missing));

    let restored = insert_download_unindexed(
        &db,
        &storage,
        "collection-first",
        "夜の手紙 前編・改訂",
        "同じ作者",
        &[],
        "再取得本文",
    );
    assert_ne!(restored, first);
    let reconnected = db.get_work_collection(&collection.summary.id).unwrap();
    let first_member = reconnected
        .members
        .iter()
        .find(|member| member.source_id == "collection-first")
        .unwrap();
    assert_eq!(first_member.download_id, Some(restored));
    assert_eq!(first_member.title, "夜の手紙 前編・改訂");
    assert!(!first_member.missing);
}

#[test]
fn content_links_recognize_pixiv_and_fanbox_with_direction() {
    let links = extract_work_link_evidence(
            "前編はこちら https://www.pixiv.net/novel/show.php?id=123 。続きは https://creator.fanbox.cc/posts/456",
            "pixiv",
            "999",
        );
    assert_eq!(links.len(), 2);
    assert!(links
        .iter()
        .any(|link| link.to_source == "pixiv" && link.to_source_id == "123"));
    assert!(links.iter().any(|link| {
        link.to_source == "fanbox"
            && link.to_source_id == "456"
            && link.relation_type == "continues_to"
    }));
}

/// 同じシリーズに同居しているだけの作品を、候補にしない。
///
/// 以前は公式シリーズ 0.58 と同一作者 0.12 を足して必ず 0.70 になり、
/// 採用閾値 0.44 を越えて候補に並んでいた。既定では選ばれないので、
/// 「チェックの付かない候補」が151作のシリーズから60件出ていた。
#[test]
fn sharing_a_series_shelf_is_not_evidence_of_a_bundle() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let seed = insert_download_unindexed(
        &db,
        &storage,
        "shelf-seed",
        "夜の灯",
        "受注作者",
        &[],
        "本文",
    );
    let far = insert_download_unindexed(
        &db,
        &storage,
        "shelf-far",
        "まったく別の話",
        "受注作者",
        &[],
        "本文",
    );
    let neighbour = insert_download_unindexed(
        &db,
        &storage,
        "shelf-neighbour",
        "続きもの その2",
        "受注作者",
        &[],
        "本文",
    );
    {
        let conn = db.conn.lock().unwrap();
        for (id, order) in [(seed, 1), (far, 90), (neighbour, 2)] {
            conn.execute(
                "INSERT INTO download_series
                       (download_id, series_source, series_key, title, content_order)
                     VALUES (?1, 'pixiv', 'req-1', '有償依頼', ?2)",
                params![id, order],
            )
            .unwrap();
        }
    }

    let suggestion = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![seed],
            limit: Some(20),
        })
        .unwrap();
    let ids = suggestion
        .members
        .iter()
        .map(|member| member.source_id.as_str())
        .collect::<Vec<_>>();
    // 90話離れた同居作は、もう候補に入らない。
    assert!(
        !ids.contains(&"shelf-far"),
        "同じ棚に載っているだけの作品が候補に残っている: {ids:?}"
    );
    // 話数が隣り合っているものは束の証拠として残る。
    assert!(
        ids.contains(&"shelf-neighbour"),
        "話数が隣接する作品が落ちている: {ids:?}"
    );

    // 名前が管理用ラベル「有償依頼」にならない。
    assert_ne!(suggestion.proposed_name, "有償依頼");
    assert!(!suggestion.name_options.is_empty());
    // 確度%ではなく、言葉で根拠が出る。
    assert!(!suggestion.evidence_summary.is_empty());
}

/// 棚全体の走査。1作ずつ種を選ばなくても束が出てくる。
#[test]
fn sweeping_the_shelf_finds_bundles_without_a_seed() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    // 語幹は9文字以上ないと束ねない。短い語幹は無関係な作品どうしを
    // 結びつけるので、実データに近い長さで確かめる。
    for (source_id, title) in [
        ("sweep-1", "岬の灯台守が季節はずれの手紙を受け取る話 第1話"),
        ("sweep-2", "岬の灯台守が季節はずれの手紙を受け取る話 第2話"),
        ("sweep-3", "岬の灯台守が季節はずれの手紙を受け取る話 第3話"),
    ] {
        insert_download_unindexed(&db, &storage, source_id, title, "連載作者", &[], "本文");
    }
    {
        // 試験の下ごしらえは作品ごとに違う author_id を振る。同じ作者の
        // 連載であることを、束ねる側が見ている形にそろえる。
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = 'writer-1' WHERE author_name = '連載作者'",
            [],
        )
        .unwrap();
    }
    // 無関係な単発。束にはならない。
    insert_download_unindexed(&db, &storage, "sweep-solo", "別の話", "別作者", &[], "本文");
    // 告知は走査の対象外。
    let notice = insert_download_unindexed(
        &db,
        &storage,
        "sweep-notice",
        "2023年5月進捗のご報告と6月の展望",
        "連載作者",
        &[],
        "本文",
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET content_type = 'article', text_length = 800 WHERE id = ?1",
            params![notice],
        )
        .unwrap();
    }

    let swept = db.sweep_collection_candidates().unwrap().bundles;
    assert_eq!(swept.len(), 1, "束がひとつだけ出るはず: {swept:?}");
    let bundle = &swept[0];
    assert_eq!(bundle.origin, "sweep");
    assert_eq!(bundle.track, "sequence");
    assert_eq!(bundle.members.len(), 3);
    // 話数の順に並ぶ。
    let order = bundle
        .members
        .iter()
        .map(|member| member.source_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(order, vec!["sweep-1", "sweep-2", "sweep-3"]);
    // 確度%ではなく、言葉で根拠が出る。
    assert!(
        bundle.evidence_summary.contains("連番"),
        "{}",
        bundle.evidence_summary
    );
    // 名前が検索用の正規化キーになっていない。
    assert!(
        bundle.proposed_name.contains("灯台守"),
        "{}",
        bundle.proposed_name
    );
    assert!(
        !bundle.proposed_name.contains("第1話"),
        "{}",
        bundle.proposed_name
    );

    // 二度走らせても、同じ束が1つだけ残る。走査は積み上げない。
    let again = db.sweep_collection_candidates().unwrap().bundles;
    assert_eq!(again.len(), 1);
    assert_eq!(
        db.list_collection_suggestions(Some("pending"))
            .unwrap()
            .len(),
        1
    );
}

/// タグの出どころを混ぜない。
///
/// 取得元が付けたタグとモデルが足したタグは確からしさが違う。**どちらか
/// 分からなくなった時点で、両方が信用できなくなる。** 取り直しで消えるのは
/// 取得元のぶんだけで、利用者が採った案は残る。
#[test]
fn assisted_tags_stay_distinguishable_and_survive_a_refetch() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download_unindexed(
        &db,
        &storage,
        "tagged-1",
        "催眠アプリで人生を染められる話",
        "作者",
        &["催眠", "R-18"],
        "本文",
    );

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
                "INSERT INTO search_index_state (download_id, current_version, content_hash, indexed_at)
                 SELECT id, current_version, content_hash, 'before-tag-change' FROM downloads WHERE id = ?1",
                params![id],
            )
            .unwrap();
        conn.execute(
                "INSERT INTO semantic_index_state (download_id, current_version, content_hash, model_id, indexed_at)
                 SELECT id, current_version, content_hash, 'test-model', 'before-tag-change' FROM downloads WHERE id = ?1",
                params![id],
            )
            .unwrap();
    }

    let added = db
        .add_assisted_tags(id, &["洗脳".to_string(), "催眠".to_string()])
        .unwrap();
    // すでに取得元が付けている「催眠」は触らない。
    assert_eq!(added, vec!["洗脳".to_string()]);
    {
        let conn = db.conn.lock().unwrap();
        let lexical: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM search_index_state WHERE download_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        let semantic: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM semantic_index_state WHERE download_id = ?1",
                params![id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((lexical, semantic), (0, 0));
    }

    let tags = db.work_tags_with_source(id).unwrap();
    let by_name = tags
        .iter()
        .map(|value| (value.name.as_str(), value.source.as_str()))
        .collect::<HashMap<_, _>>();
    assert_eq!(by_name.get("催眠"), Some(&"origin"));
    assert_eq!(by_name.get("洗脳"), Some(&"llm"));

    // 取り直しても、モデルの案から採ったタグは残る。
    insert_download_unindexed(
        &db,
        &storage,
        "tagged-1",
        "催眠アプリで人生を染められる話",
        "作者",
        &["催眠", "R-18", "常識改変"],
        "本文",
    );
    let after = db.work_tags_with_source(id).unwrap();
    let names = after
        .iter()
        .map(|v| v.name.as_str())
        .collect::<HashSet<_>>();
    assert!(
        names.contains("洗脳"),
        "モデルのタグが消えている: {names:?}"
    );
    assert!(
        names.contains("常識改変"),
        "取得元の新しいタグが入っていない"
    );

    // 外せるのはモデルのぶんだけ。取得元のタグは外せない。
    assert!(!db.remove_assisted_tag(id, "催眠").unwrap());
    assert!(db.remove_assisted_tag(id, "洗脳").unwrap());
    let finally = db.work_tags_with_source(id).unwrap();
    assert!(finally.iter().all(|value| value.name != "洗脳"));
    assert!(finally.iter().any(|value| value.name == "催眠"));
}

/// 覚え書きは、書いたモデルと一緒に残す。
#[test]
fn ai_notes_remember_which_model_wrote_them() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    assert!(db.load_ai_note("work", "1", "synopsis").unwrap().is_none());

    db.save_ai_note("work", "1", "synopsis", "最初の文", "model-a")
        .unwrap();
    let note = db.load_ai_note("work", "1", "synopsis").unwrap().unwrap();
    assert_eq!(note.text, "最初の文");
    assert_eq!(note.model_id, "model-a");

    // 書き直すと、モデルの名前も一緒に入れ替わる。古い文が別のモデルの
    // ものとして残らない。
    db.save_ai_note("work", "1", "synopsis", "書き直した文", "model-b")
        .unwrap();
    let note = db.load_ai_note("work", "1", "synopsis").unwrap().unwrap();
    assert_eq!(note.text, "書き直した文");
    assert_eq!(note.model_id, "model-b");

    assert!(db.delete_ai_note("work", "1", "synopsis").unwrap());
    assert!(db.load_ai_note("work", "1", "synopsis").unwrap().is_none());
}

/// 走査で出た候補は、まとめて閉じられる。
///
/// 300件を1件ずつ閉じる人はいない。**一括で消せないなら、出さないほうが
/// まし**になってしまう。閉じるのは下書きだけで、否定は記録しない。
#[test]
fn swept_candidates_can_be_closed_in_one_go() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for (source_id, title) in [
        ("bulk-1", "岬の灯台守が季節はずれの手紙を受け取る話 第1話"),
        ("bulk-2", "岬の灯台守が季節はずれの手紙を受け取る話 第2話"),
    ] {
        insert_download_unindexed(&db, &storage, source_id, title, "連載作者", &[], "本文");
    }
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE downloads SET author_id = 'writer-1'", [])
            .unwrap();
    }
    assert_eq!(db.sweep_collection_candidates().unwrap().bundles.len(), 1);

    assert_eq!(
        db.dismiss_swept_suggestions(Some("theme")).unwrap(),
        0,
        "系統が違えば消えない"
    );
    assert_eq!(db.dismiss_swept_suggestions(None).unwrap(), 1);
    assert!(db
        .list_collection_suggestions(Some("pending"))
        .unwrap()
        .is_empty());

    // 否定は記録していないので、もう一度走査すれば同じものが出てくる。
    assert_eq!(db.sweep_collection_candidates().unwrap().bundles.len(), 1);
}

/// 合本は、分冊がそろっているときだけ畳む。
#[test]
fn a_combined_volume_folds_only_when_its_parts_are_there() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let base = "財閥系お嬢様に貸し切られた電車の中で搾り尽くされる話";
    for (source_id, title) in [
        ("omni-1", format!("【前編】{base}")),
        ("omni-2", format!("【中編】{base}")),
        ("omni-3", format!("【後編】{base}")),
        ("omni-all", format!("【前編＋中編】{base}")),
    ] {
        insert_download_unindexed(&db, &storage, source_id, &title, "連載作者", &[], "本文");
    }
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("UPDATE downloads SET author_id = 'writer-2'", [])
            .unwrap();
    }
    let swept = db.sweep_collection_candidates().unwrap().bundles;
    let ids = swept
        .iter()
        .flat_map(|bundle| bundle.members.iter())
        .map(|member| member.source_id.as_str())
        .collect::<Vec<_>>();
    assert!(
        !ids.contains(&"omni-all"),
        "分冊がそろっているのに合本が残っている: {ids:?}"
    );
    assert!(
        ids.contains(&"omni-1") && ids.contains(&"omni-3"),
        "{ids:?}"
    );
}

/// 束にできないほど大きなタグは、黙って捨てずに絞り込みとして勧める。
#[test]
fn a_tag_too_large_to_bundle_is_offered_as_a_saved_search() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..70 {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("wide-{index}"),
            &format!("それぞれ無関係な話 {index}"),
            &format!("作者{index}"),
            &["催眠"],
            "本文",
        );
    }
    let swept = db.sweep_collection_candidates().unwrap();
    let idea = swept
        .saved_search_suggestions
        .iter()
        .find(|value| value.tag == "催眠")
        .expect("大きすぎるタグが勧められていない");
    assert_eq!(idea.work_count, 70);
    assert!(idea.reason.contains("大きい"), "{}", idea.reason);
    // 束としては出さない。まとまりではなく絞り込みの結果だからである。
    assert!(swept.bundles.iter().all(|bundle| bundle.track != "theme"));
}

/// 取得元をまたいだ同じ作品を、続きとして二度並べない。
///
/// `author_id` は取得元ごとに違う（同じ人が pixiv では数字、FANBOX では
/// 英字を名乗る）。ID で突き合わせていたころは、pixiv 版と FANBOX 版が
/// 別の作品として同じ束に二度出ていた。
#[test]
fn the_same_work_from_two_sources_is_one_member_not_two() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let title = "ギアスにかけられた純血派の活動資金援助を条件にする話";
    let pixiv = insert_download_unindexed(
        &db,
        &storage,
        "cross-pixiv",
        title,
        "キモデブ君",
        &[],
        "本文",
    );
    let fanbox = insert_download_unindexed(
        &db,
        &storage,
        "cross-fanbox",
        title,
        "キモデブ君",
        &[],
        "本文がもっと長い",
    );
    let other = insert_download_unindexed(
        &db,
        &storage,
        "cross-other",
        "ギアスにかけられた純血派の活動資金援助を条件にする話 第2話",
        "キモデブ君",
        &[],
        "続き",
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET source = 'fanbox', author_id = 'kimodebu-kun' WHERE id = ?1",
            params![fanbox],
        )
        .unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = '3259258' WHERE id IN (?1, ?2)",
            params![pixiv, other],
        )
        .unwrap();
    }

    let swept = db.sweep_collection_candidates().unwrap().bundles;
    let bundle = swept
        .iter()
        .find(|value| value.members.len() >= 2)
        .expect("題名の連番で束になるはず");
    let sources = bundle
        .members
        .iter()
        .map(|member| member.source_id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(sources.len(), 2, "同じ作品が二度並んでいる: {sources:?}");
    // 代表は本文の長いほう。サンプルは導入だけのことが多い。
    assert!(sources.contains(&"cross-fanbox"), "{sources:?}");
    assert!(!sources.contains(&"cross-pixiv"), "{sources:?}");
}

/// 告知記事は束ねない。目次を渡って先へも行かない。
#[test]
fn notices_neither_join_a_bundle_nor_bridge_one() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let seed = insert_download_unindexed(
        &db,
        &storage,
        "notice-seed",
        "灯台 前編",
        "作者",
        &[],
        "本文",
    );
    let sequel = insert_download_unindexed(
        &db,
        &storage,
        "notice-next",
        "灯台 後編",
        "作者",
        &[],
        "本文",
    );
    let notice = insert_download_unindexed(
        &db,
        &storage,
        "notice-post",
        "【重要なお知らせ】公開方針を変更いたします",
        "作者",
        &[],
        "本文",
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET content_type = 'article', text_length = 900 WHERE id = ?1",
            params![notice],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO work_links
                   (from_source, from_source_id, from_download_id,
                    to_source, to_source_id, to_download_id,
                    relation_type, evidence_type, confidence, status)
                 VALUES ('pixiv', 'notice-seed', ?1, 'pixiv', 'notice-post', ?2,
                         'mentions', 'content_link', 0.9, 'observed')",
            params![seed, notice],
        )
        .unwrap();
    }

    let suggestion = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![seed],
            limit: Some(20),
        })
        .unwrap();
    let ids = suggestion
        .members
        .iter()
        .map(|member| member.source_id.as_str())
        .collect::<Vec<_>>();
    assert!(
        !ids.contains(&"notice-post"),
        "告知記事が束に入っている: {ids:?}"
    );
    assert!(ids.contains(&"notice-next"), "続きが落ちている: {ids:?}");
    assert_eq!(sequel, sequel);
}

#[test]
fn collection_suggestions_use_title_parts_and_learn_rejection() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download_unindexed(
        &db,
        &storage,
        "suggest-first",
        "星を待つ 前編",
        "連載作者",
        &[],
        "導入",
    );
    let _second = insert_download_unindexed(
        &db,
        &storage,
        "suggest-second",
        "星を待つ 後編",
        "連載作者",
        &[],
        "結末",
    );
    let suggestion = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![first],
            limit: Some(20),
        })
        .unwrap();
    assert_eq!(suggestion.members.len(), 2);
    assert!(suggestion.members.iter().any(|member| {
        member.source_id == "suggest-second"
            && member
                .evidence
                .iter()
                .any(|evidence| evidence.kind == "title_similarity")
    }));
    assert!(db
        .reject_collection_suggestion(&suggestion.id, None)
        .unwrap());

    let after_rejection = db.generate_collection_suggestion(&CollectionSuggestionRequest {
        seed_download_ids: vec![first],
        limit: Some(20),
    });
    assert_eq!(
        after_rejection.unwrap_err(),
        "関連作品は見つかりませんでした"
    );

    // Feedback belongs to the rule that produced it. A future rule can
    // evaluate the pair again instead of leaving an irreversible dead end.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE collection_pair_feedback SET rule_version = 'collection-suggest-v1'",
            [],
        )
        .unwrap();
    let reconsidered = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![first],
            limit: Some(20),
        })
        .unwrap();
    assert_eq!(reconsidered.members.len(), 2);
}

#[test]
fn collection_suggestions_walk_the_whole_link_component_from_either_end() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download(
            &db,
            &storage,
            "410001",
            "倉本エリカを手に入れたあなたが、棚ぼたで鈴原美紗も寝取って発情種付け立ちバック交尾を堪能しちゃうお話",
            "連載作者",
            &[],
            "第一作",
        );
    let second = insert_download(
            &db,
            &storage,
            "410002",
            "倉本エリカと鈴原美紗を堕としたあなたが、温泉旅館で極上美女二人から迫られるハーレム3P交尾を好き放題に堪能しちゃうお話",
            "連載作者",
            &[],
            "第二作",
        );
    let third = insert_download(
            &db,
            &storage,
            "410003",
            "超人気アイドル兼魔法少女の倉本エリカが、あなたを引き留める為にマットプレイでアナル舐め＆スパイダー騎乗位膣内射精をさせてくれる話",
            "連載作者",
            &[],
            "前作はこちら https://www.pixiv.net/novel/show.php?id=410002",
        );
    let fourth = insert_download(
            &db,
            &storage,
            "410004",
            "超人気アイドル兼魔法少女の倉本エリカを庇ったあなたが、生殖本能むき出しの世界一気持ちいい膣内射精立ちバック交尾を出来ちゃう話",
            "連載作者",
            &[],
            "前作はこちら https://www.pixiv.net/novel/show.php?id=410003",
        );

    // The first edge exists only in the caption/excerpt; the others are in
    // the body. Both surfaces must feed the same normalized link graph.
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE downloads SET excerpt = ?1 WHERE id = ?2",
            params![
                "前作はこちら https://www.pixiv.net/novel/show.php?id=410001",
                second
            ],
        )
        .unwrap();
    db.reindex_download(second).unwrap();

    // Simulate an existing library whose full-text index predates the
    // normalized work-link graph. Starting at the first work must discover
    // incoming references and keep walking from every newly found work.
    db.conn
        .lock()
        .unwrap()
        .execute("DELETE FROM work_links", [])
        .unwrap();
    let from_first = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![first],
            limit: Some(20),
        })
        .unwrap();
    assert_eq!(
        from_first
            .members
            .iter()
            .map(|member| member.download_id.unwrap())
            .collect::<Vec<_>>(),
        vec![first, second, third, fourth]
    );
    assert!(from_first.members.iter().skip(1).all(|member| member
        .evidence
        .iter()
        .any(|evidence| evidence.kind == "content_link")));
    assert!(from_first.members.iter().any(|member| member
        .evidence
        .iter()
        .any(|evidence| evidence.label.contains("3段追跡"))));

    // The opposite endpoint works too: its outgoing "previous work" links
    // lead back to the first chapter and are ordered by their direction.
    db.conn
        .lock()
        .unwrap()
        .execute("DELETE FROM work_links", [])
        .unwrap();
    let from_last = db
        .generate_collection_suggestion(&CollectionSuggestionRequest {
            seed_download_ids: vec![fourth],
            limit: Some(20),
        })
        .unwrap();
    assert_eq!(
        from_last
            .members
            .iter()
            .map(|member| member.download_id.unwrap())
            .collect::<Vec<_>>(),
        vec![first, second, third, fourth]
    );
}

#[test]
fn bulk_delete_clears_lexical_and_semantic_sidecars() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download(
        &db,
        &storage,
        "delete-index-one",
        "quartzdeletemarker alpha",
        "作者",
        &[],
        "意味検索からも消える本文 alpha",
    );
    let second = insert_download(
        &db,
        &storage,
        "delete-index-two",
        "quartzdeletemarker beta",
        "作者",
        &[],
        "意味検索からも消える本文 beta",
    );
    db.save_ai_note("work", &first.to_string(), "synopsis", "要約", "model")
        .unwrap();
    db.save_ai_note(
        "work",
        &format!("{second}:{first}"),
        "recap",
        "前回まで",
        "model",
    )
    .unwrap();

    let before =
        crate::database::tantivy_index::matching_download_ids(&storage, "quartzdeletemarker")
            .unwrap();
    assert!(before.contains(&first));
    assert!(before.contains(&second));
    assert!(crate::database::semantic_index::status(&storage).indexed_chunks > 0);

    let result = db.delete_downloads(&[first, second, first]).unwrap();
    assert_eq!(result.matched_count, 2);
    assert_eq!(result.changed_count, 2);

    let after =
        crate::database::tantivy_index::matching_download_ids(&storage, "quartzdeletemarker")
            .unwrap();
    assert!(!after.contains(&first));
    assert!(!after.contains(&second));
    assert_eq!(
        crate::database::semantic_index::status(&storage).indexed_chunks,
        0
    );
    assert!(!storage.join("pixiv").join("delete-index-one").exists());
    assert!(!storage.join("pixiv").join("delete-index-two").exists());
    assert!(db
        .load_ai_note("work", &first.to_string(), "synopsis")
        .unwrap()
        .is_none());
    assert!(db
        .load_ai_note("work", &format!("{second}:{first}"), "recap")
        .unwrap()
        .is_none());
}

#[test]
fn deleting_a_series_cover_selects_a_surviving_work_or_clears_it() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download_unindexed(
        &db,
        &storage,
        "series-cover-first",
        "連作 第一話",
        "作者",
        &[],
        "第一話",
    );
    let second = insert_download_unindexed(
        &db,
        &storage,
        "series-cover-second",
        "連作 第二話",
        "作者",
        &[],
        "第二話",
    );
    let first_cover = storage
        .join("pixiv")
        .join("series-cover-first")
        .join("v1")
        .join("cover.jpg");
    let second_cover = storage
        .join("pixiv")
        .join("series-cover-second")
        .join("v1")
        .join("cover.jpg");
    fs::write(&first_cover, b"first cover").unwrap();
    fs::write(&second_cover, b"second cover").unwrap();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET cover_path = ?1 WHERE id = ?2",
            params![first_cover.to_string_lossy(), first],
        )
        .unwrap();
        conn.execute(
            "UPDATE downloads SET cover_path = ?1 WHERE id = ?2",
            params![second_cover.to_string_lossy(), second],
        )
        .unwrap();
    }
    db.upsert_download_series(first, "pixiv", "series-cover", "連作", Some(1))
        .unwrap();
    db.upsert_download_series(second, "pixiv", "series-cover", "連作", Some(2))
        .unwrap();
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE series SET cover_path = ?1
                 WHERE source = 'pixiv' AND source_key = 'series-cover'",
            params![first_cover.to_string_lossy()],
        )
        .unwrap();

    db.delete_downloads(&[first]).unwrap();
    assert_eq!(
        db.get_series("pixiv", "series-cover")
            .unwrap()
            .cover_path
            .as_deref(),
        Some(second_cover.to_string_lossy().as_ref())
    );

    db.delete_downloads(&[second]).unwrap();
    assert!(db
        .get_series("pixiv", "series-cover")
        .unwrap()
        .cover_path
        .is_none());
}

#[test]
fn semantic_prune_clears_orphans_when_library_is_empty() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download(
        &db,
        &storage,
        "last-semantic-work",
        "最後の作品",
        "作者",
        &[],
        "索引へ残してからDB行だけ消す本文",
    );
    assert!(crate::database::semantic_index::status(&storage).indexed_chunks > 0);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .unwrap();
    }

    assert!(db.prune_semantic_index().unwrap() > 0);
    assert_eq!(
        crate::database::semantic_index::status(&storage).indexed_chunks,
        0
    );
}

#[test]
fn opening_database_recovers_both_sides_of_an_interrupted_delete() {
    let (_temp, root, storage) = temp_paths();
    let db_path = root.join("piep.db");
    let db = Database::open(&db_path, &storage).unwrap();
    let id = insert_download(
        &db,
        &storage,
        "delete-recovery",
        "復旧対象",
        "作者",
        &[],
        "削除処理の途中で停止する本文",
    );
    let original = storage.join("pixiv").join("delete-recovery");
    let operation = root.join("delete-staging").join("before-commit");
    fs::create_dir_all(&operation).unwrap();
    let manifest = vec![StagedDeleteEntry {
        download_id: id,
        source: "pixiv".to_string(),
        source_id: "delete-recovery".to_string(),
        staged_name: "0".to_string(),
    }];
    fs::write(
        operation.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::rename(&original, operation.join("0")).unwrap();
    drop(db);

    // SQLite still has the row, so startup rolls the file move back.
    let db = Database::open(&db_path, &storage).unwrap();
    assert!(original.exists());
    assert!(db.get_download(id).is_ok());

    // Simulate a crash just after SQLite committed but before staged files
    // and sidecars were removed. Startup completes the deletion instead.
    let operation = root.join("delete-staging").join("after-commit");
    fs::create_dir_all(&operation).unwrap();
    fs::write(
        operation.join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    fs::rename(&original, operation.join("0")).unwrap();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute("DELETE FROM downloads WHERE id = ?1", params![id])
            .unwrap();
    }
    drop(db);

    let reopened = Database::open(&db_path, &storage).unwrap();
    assert!(!original.exists());
    assert!(!operation.exists());
    assert!(reopened.get_download(id).is_err());
    assert_eq!(
        crate::database::semantic_index::status(&storage).indexed_chunks,
        0
    );
}

#[test]
fn opening_database_prunes_only_unreferenced_managed_collection_covers() {
    let (_temp, root, storage) = temp_paths();
    let db_path = root.join("piep.db");
    let cover_root = root.join("collection-covers");
    let referenced = cover_root.join("referenced.png");
    let orphaned = cover_root.join("orphaned.png");
    let nested_orphan = cover_root.join("old-collection").join("cover.webp");

    let db = Database::open(&db_path, &storage).unwrap();
    db.upsert_work_collection(&WorkCollectionInput {
        name: "表紙を残す束".to_string(),
        collection_kind: "ordered".to_string(),
        cover_mode: Some("file".to_string()),
        cover_image_path: Some(referenced.to_string_lossy().to_string()),
        ..Default::default()
    })
    .unwrap();
    fs::create_dir_all(nested_orphan.parent().unwrap()).unwrap();
    fs::write(&referenced, b"referenced").unwrap();
    fs::write(&orphaned, b"orphaned").unwrap();
    fs::write(&nested_orphan, b"nested orphan").unwrap();
    drop(db);

    let reopened = Database::open(&db_path, &storage).unwrap();
    assert!(referenced.exists());
    assert!(!orphaned.exists());
    assert!(!nested_orphan.exists());
    assert!(reopened.list_work_collections().unwrap()[0]
        .cover_image_path
        .as_deref()
        .is_some_and(|path| path == referenced.to_string_lossy()));
}

#[test]
fn sort_aliases_map_to_the_expected_safe_sql_columns() {
    let cases = [
        ("date", "date", "d.downloaded_at"),
        ("downloaded_at", "date", "d.downloaded_at"),
        ("author_name", "author", "d.author_name COLLATE NOCASE"),
        (
            "source_created_at",
            "published",
            "COALESCE(d.source_created_at, d.downloaded_at)",
        ),
        (
            "source_updated_at",
            "updated",
            "COALESCE(d.source_updated_at, d.source_created_at, d.downloaded_at)",
        ),
        ("text_length", "length", "d.text_length"),
        ("file_size_bytes", "size", "d.file_size_bytes"),
    ];

    for (requested, normalized, expected_sql) in cases {
        let mut search = params("");
        search.sort_by = Some(requested.to_string());
        assert_eq!(effective_sort_by(&search).as_deref(), Some(normalized));
        assert!(sort_clause(&search).contains(expected_sql));
        assert!(sort_compare_expr(&search).contains(expected_sql));
    }

    let mut malicious = params("");
    malicious.sort_by = Some("downloaded_at; DROP TABLE downloads".to_string());
    assert_eq!(effective_sort_by(&malicious).as_deref(), Some("date"));
    assert!(!sort_clause(&malicious).contains("DROP TABLE"));
}

#[test]
fn optional_update_target_lookup_returns_exact_match_or_none() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    db.upsert_update_target(&UpdateTargetInput {
        target_type: "person".to_string(),
        source: "pixiv".to_string(),
        source_key: "author-1".to_string(),
        display_name: "作者1".to_string(),
        enabled: true,
        metadata_json: None,
    })
    .unwrap();

    let found = db
        .find_update_target("person", "pixiv", "author-1")
        .unwrap()
        .unwrap();
    assert_eq!(found.display_name, "作者1");
    assert!(found.enabled);
    assert!(db
        .find_update_target("person", "pixiv", "missing")
        .unwrap()
        .is_none());
}

#[test]
fn update_target_keyset_pages_have_no_gaps_or_duplicates() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for (target_type, source, source_key) in [
        ("work", "pixiv", "w2"),
        ("person", "pixiv", "p2"),
        ("person", "fanbox", "p1"),
        ("person", "pixiv", "p1"),
        ("series", "pixiv", "s1"),
    ] {
        db.upsert_update_target(&UpdateTargetInput {
            target_type: target_type.to_string(),
            source: source.to_string(),
            source_key: source_key.to_string(),
            display_name: source_key.to_string(),
            enabled: true,
            metadata_json: None,
        })
        .unwrap();
    }

    let mut all = Vec::new();
    let mut cursor: Option<(String, String, String)> = None;
    loop {
        let page = db
            .list_update_targets_after(
                cursor
                    .as_ref()
                    .map(|(kind, source, key)| (kind.as_str(), source.as_str(), key.as_str())),
                2,
            )
            .unwrap();
        if page.is_empty() {
            break;
        }
        let last = page.last().unwrap();
        cursor = Some((
            last.target_type.clone(),
            last.source.clone(),
            last.source_key.clone(),
        ));
        all.extend(
            page.into_iter()
                .map(|target| (target.target_type, target.source, target.source_key)),
        );
    }
    assert_eq!(all.len(), 5);
    assert!(all.windows(2).all(|pair| pair[0] < pair[1]));
    let unique = all.iter().collect::<HashSet<_>>();
    assert_eq!(unique.len(), all.len());
}

#[test]
fn bulk_search_collection_crosses_the_former_single_page_boundary() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const SIMULATED_LEGACY_CAP: usize = 5;
    const MATCHES: usize = SIMULATED_LEGACY_CAP * 2 + 3;
    for index in 0..MATCHES {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("bulk-page-{index:02}"),
            &format!("一括対象 {index:02}"),
            "作者",
            &["一括"],
            "本文",
        );
    }

    let mut selection = v2_params(None, 1, None);
    selection.offset = Some(9);
    let snapshot = db
        .collect_search_match_snapshot(&selection, SIMULATED_LEGACY_CAP as i64)
        .unwrap();
    assert_eq!(snapshot.row_count, MATCHES as i64);
    let guard = snapshot.connection.lock().unwrap();
    let conn = guard.as_ref().unwrap();
    let (rows, distinct): (i64, i64) = conn
        .query_row(
            "SELECT COUNT(*), COUNT(DISTINCT id) FROM bulk_matches",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(rows, MATCHES as i64);
    assert_eq!(distinct, MATCHES as i64);
    drop(guard);
}

#[test]
fn bulk_search_snapshot_updates_and_deletes_without_retaining_all_ids() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const MATCHES: usize = 13;
    for index in 0..MATCHES {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("bulk-mutate-{index:02}"),
            &format!("一括変更 {index:02}"),
            "作者",
            &["一括変更"],
            "本文",
        );
    }

    let selection = v2_params(None, 3, None);
    let watched = db.set_watch_updates_for_search(&selection, true).unwrap();
    assert_eq!(watched.matched_count, MATCHES as i64);
    assert_eq!(watched.changed_count, MATCHES as i64);
    {
        let conn = db.conn.lock().unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM downloads WHERE watch_updates = 1",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            MATCHES as i64
        );
        // 作品の監視は downloads.watch_updates だけで表す。監視対象の一覧は
        // 「自分で選んだ作者・シリーズ」のためのもので、作品名を混ぜない。
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM update_targets WHERE target_type = 'work'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );
    }

    let deleted = db.delete_downloads_for_search(&selection).unwrap();
    assert_eq!(deleted.matched_count, MATCHES as i64);
    assert_eq!(deleted.changed_count, MATCHES as i64);
    assert_eq!(
        db.conn
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM downloads", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
    let snapshot_files = fs::read_dir(search_snapshot_dir(&storage))
        .unwrap()
        .filter_map(Result::ok)
        .count();
    assert_eq!(snapshot_files, 0, "bulk snapshot must be cleaned up");
}

#[test]
fn facet_search_limits_in_sql_and_keeps_direct_rare_matches() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download_unindexed(
        &db,
        &storage,
        "facet-1",
        "作品1",
        "人気作者",
        &["人気タグ"],
        "本文",
    );
    insert_download_unindexed(
        &db,
        &storage,
        "facet-2",
        "作品2",
        "作者%特別",
        &["希少タグ"],
        "本文",
    );

    assert_eq!(
        db.search_filter_facets("authors", None, 1).unwrap().len(),
        1
    );
    let escaped = db
        .search_filter_facets("authors", Some("作者%"), 10)
        .unwrap();
    assert_eq!(
        escaped.first().map(|facet| facet.name.as_str()),
        Some("作者%特別")
    );
    let rare = db.search_filter_facets("tags", Some("希少"), 10).unwrap();
    assert_eq!(
        rare.first().map(|facet| facet.name.as_str()),
        Some("希少タグ")
    );
}

#[test]
fn facet_and_suggestion_caches_hit_then_invalidate_on_commit() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download(
        &db,
        &storage,
        "cache-generation-1",
        "世代候補 一冊目",
        "世代作者",
        &["世代タグ"],
        "本文",
    );

    let first_facets = db.search_filter_facets("tags", None, 30).unwrap();
    assert_eq!(
        first_facets
            .iter()
            .find(|facet| facet.name == "世代タグ")
            .map(|facet| facet.count),
        Some(1)
    );
    let _ = db.search_filter_facets("tags", None, 30).unwrap();

    let suggest_params = SearchSuggestParams {
        text: Some("世代候補".to_string()),
        limit: Some(12),
    };
    assert_eq!(db.search_suggest(&suggest_params).unwrap().items.len(), 1);
    let _ = db.search_suggest(&suggest_params).unwrap();
    let (facet_before, suggest_before, _) = db.query_cache_stats();
    assert!(facet_before.0 >= 1, "facet cache should have a hit");
    assert!(suggest_before.0 >= 1, "suggest cache should have a hit");

    let second_id = insert_download(
        &db,
        &storage,
        "cache-generation-2",
        "世代候補 二冊目",
        "世代作者",
        &["世代タグ"],
        "本文",
    );
    let updated_facets = db.search_filter_facets("tags", None, 30).unwrap();
    assert_eq!(
        updated_facets
            .iter()
            .find(|facet| facet.name == "世代タグ")
            .map(|facet| facet.count),
        Some(2),
        "a committed write must invalidate cached aggregates"
    );
    assert_eq!(db.search_suggest(&suggest_params).unwrap().items.len(), 2);
    let (facet_after, suggest_after, _) = db.query_cache_stats();
    assert!(facet_after.1 > facet_before.1);
    assert!(suggest_after.1 > suggest_before.1);

    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET title = '別の候補' WHERE id = ?1",
            params![second_id],
        )
        .unwrap();
    }
    assert_eq!(
        db.search_suggest(&suggest_params).unwrap().items.len(),
        1,
        "a direct committed update must invalidate cached suggestions"
    );

    db.delete_download(second_id).unwrap();
    let after_delete = db.search_filter_facets("tags", None, 30).unwrap();
    assert_eq!(
        after_delete
            .iter()
            .find(|facet| facet.name == "世代タグ")
            .map(|facet| facet.count),
        Some(1),
        "a committed delete must invalidate cached aggregates"
    );
}

#[test]
fn lexical_search_reports_full_hits_beyond_the_candidate_page() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..3 {
        insert_download(
            &db,
            &storage,
            &format!("count-{index}"),
            &format!("共通検索語 作品{index}"),
            "作者",
            &["検索"],
            "本文",
        );
    }

    let result = super::super::tantivy_index::search_with_total(&storage, "共通検索語", 1).unwrap();
    assert_eq!(result.hits.len(), 1);
    assert_eq!(result.total_hits, 3);

    let mut deep = params("共通検索語");
    deep.limit = Some(600);
    assert!(search_candidate_limit(&deep, 600) > 1_000);
    let mut another = params("別の検索");
    another.limit = deep.limit;
    assert_ne!(search_cursor_scope(&deep), search_cursor_scope(&another));
}

#[test]
fn lexical_cursor_reaches_every_match_beyond_one_thousand_results() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const ITEM_COUNT: usize = 1_025;

    for index in 0..ITEM_COUNT {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("deep-{index:04}"),
            &format!("全件到達テスト {index:04}"),
            "同一作者",
            &["ページング"],
            "全件到達テストの共通本文",
        );
    }
    loop {
        let status = db.rebuild_search_index_batch(200).unwrap();
        if status.pending_downloads == 0 {
            break;
        }
    }
    let mut request = params("全件到達テスト");
    request.limit = Some(137);
    let mut seen = HashSet::new();
    let mut checked_native_cursor = false;
    loop {
        let result = db.search_downloads_v2(&request).unwrap();
        assert_eq!(result.total_estimate, Some(ITEM_COUNT as i64));
        for item in result.items {
            assert!(
                seen.insert(item.id),
                "cursor returned duplicate id {}",
                item.id
            );
        }
        match result.next_cursor {
            Some(cursor) => {
                if !checked_native_cursor {
                    let decoded = decode_cursor(Some(&cursor)).unwrap();
                    assert_eq!(decoded.kind, "ranked-search");
                    assert!(decoded.snapshot_id.is_some());
                    assert!(decoded.tantivy_score.is_some());
                    checked_native_cursor = true;
                }
                request.cursor = Some(cursor);
            }
            None => break,
        }
    }

    assert!(checked_native_cursor);
    assert_eq!(seen.len(), ITEM_COUNT);
}

#[test]
fn lexical_search_after_does_not_skip_filtered_hits_inside_a_batch() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const ITEM_COUNT: usize = 450;
    const EXPECTED: usize = 45;
    for index in 0..ITEM_COUNT {
        let tags: &[&str] = if index % 10 == 0 {
            &["対象"]
        } else {
            &["対象外"]
        };
        insert_download_unindexed(
            &db,
            &storage,
            &format!("filtered-cursor-{index:04}"),
            &format!("絞り込みカーソル {index:04}"),
            "カーソル作者",
            tags,
            "絞り込みカーソルの共通本文",
        );
    }
    loop {
        let status = db.rebuild_search_index_batch(200).unwrap();
        if status.pending_downloads == 0 {
            break;
        }
    }

    let mut request = params("絞り込みカーソル");
    request.limit = Some(17);
    request.tags_include = Some(vec!["対象".to_string()]);
    let mut seen = HashSet::new();
    let mut checked_internal_total = false;
    loop {
        let result = db.search_downloads_v2(&request).unwrap();
        for item in result.items {
            assert!(seen.insert(item.id));
            assert!(item.tags.iter().any(|tag| tag == "対象"));
        }
        match result.next_cursor {
            Some(cursor) => {
                if !checked_internal_total {
                    let decoded = decode_cursor(Some(&cursor)).unwrap();
                    assert_eq!(decoded.total_estimate, Some(EXPECTED as i64));
                    assert_eq!(decoded.tantivy_total_hits, None);
                    assert!(decoded.snapshot_id.is_some());
                    checked_internal_total = true;
                }
                request.cursor = Some(cursor);
            }
            None => break,
        }
    }
    assert!(checked_internal_total);
    assert_eq!(seen.len(), EXPECTED);
}

#[test]
fn lexical_cursor_carries_the_first_page_total() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..7 {
        insert_download(
            &db,
            &storage,
            &format!("count-cursor-{index}"),
            &format!("総件数カーソル {index}"),
            "作者",
            &["件数"],
            "総件数カーソル本文",
        );
    }

    let mut request = params("総件数カーソル");
    request.limit = Some(2);
    let first = db.search_downloads_v2(&request).unwrap();
    assert_eq!(first.total_estimate, Some(7));
    let decoded = decode_cursor(first.next_cursor.as_deref()).unwrap();
    assert_eq!(decoded.total_estimate, Some(7));
    assert_eq!(decoded.tantivy_total_hits, None);
    assert!(decoded.snapshot_id.is_some());

    request.cursor = first.next_cursor;
    let second = db.search_downloads_v2(&request).unwrap();
    assert_eq!(second.total_estimate, Some(7));
}

#[test]
fn lexical_cursor_survives_equal_score_segment_merge() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const ITEM_COUNT: usize = 137;
    for index in 0..ITEM_COUNT {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("merge-cursor-{index:04}"),
            &format!("同点マージ境界 {index:04}"),
            "同一作者",
            &["マージ"],
            "全作品で同じ語数の同点マージ境界本文",
        );
    }
    while db.get_search_index_status().unwrap().pending_downloads > 0 {
        db.rebuild_search_index_batch(19).unwrap();
    }
    assert!(
        super::super::tantivy_index::searchable_segment_count(&storage).unwrap() > 1,
        "fixture must start with multiple segments"
    );

    let mut request = params("同点マージ境界");
    request.limit = Some(17);
    let first = db.search_downloads_v2(&request).unwrap();
    let mut seen = first
        .items
        .iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    request.cursor = first.next_cursor;

    // DocAddress cursors become invalid here because every segment/doc id
    // can change. The cursor's score + stable download id must not.
    let (_, after) = super::super::tantivy_index::optimize_segments(&storage).unwrap();
    assert_eq!(after, 1);
    while request.cursor.is_some() {
        let page = db.search_downloads_v2(&request).unwrap();
        for item in &page.items {
            assert!(seen.insert(item.id), "duplicate id {}", item.id);
        }
        request.cursor = page.next_cursor;
    }
    assert_eq!(seen.len(), ITEM_COUNT);
}

#[test]
fn ranked_snapshot_failure_rolls_back_and_removes_partial_file() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const MATCHES: usize = 519;
    for index in 0..MATCHES {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("snapshot-fail-{index:04}"),
            &format!("失敗注入検索 {index:04}"),
            "作者",
            &["失敗注入"],
            "失敗注入検索の本文",
        );
    }
    loop {
        let status = db.rebuild_search_index_batch(200).unwrap();
        if status.pending_downloads == 0 {
            break;
        }
    }

    let request = params("失敗注入検索");
    let error = db
        .ranked_search_snapshot_inner(&request, "失敗注入検索", None, Some(1), None)
        .err()
        .expect("the second streamed score batch must fail");
    assert!(error.contains("Injected ranked snapshot stream failure"));
    assert_eq!(
        fs::read_dir(search_snapshot_dir(&storage))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        0,
        "a failed snapshot must leave neither the database nor sidecars"
    );
    assert!(db.search_snapshot_cache.lock().unwrap().entries.is_empty());

    let quota_error = db
        .ranked_search_snapshot_inner(&request, "失敗注入検索", None, None, Some(4 * 1024))
        .err()
        .expect("the injected disk quota must reject the snapshot");
    assert!(quota_error.contains("shared 4096-byte disk budget"));
    assert_eq!(
        fs::read_dir(search_snapshot_dir(&storage))
            .unwrap()
            .filter_map(Result::ok)
            .count(),
        0,
        "a quota failure must clean its partial snapshot"
    );
}

#[test]
fn ranked_snapshot_cursor_expires_after_library_or_index_generation_changes() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..5 {
        insert_download(
            &db,
            &storage,
            &format!("snapshot-generation-{index}"),
            &format!("世代固定検索 {index}"),
            "作者",
            &["世代"],
            "世代固定検索の本文",
        );
    }
    let mut request = params("世代固定検索");
    request.limit = Some(2);
    let first = db.search_downloads_v2(&request).unwrap();
    let cursor = first.next_cursor.expect("first page cursor");

    insert_download(
        &db,
        &storage,
        "snapshot-generation-new",
        "世代固定検索 新着",
        "作者",
        &["世代"],
        "世代固定検索の本文",
    );
    request.cursor = Some(cursor);
    let error = db.search_downloads_v2(&request).unwrap_err();
    assert!(
        error.contains("expired") || error.contains("invalidated"),
        "a cursor must never continue against a newly ranked generation: {error}"
    );

    request.cursor = None;
    let restarted = db.search_downloads_v2(&request).unwrap();
    assert_eq!(restarted.total_estimate, Some(6));
}

#[test]
fn search_index_optimization_reduces_segments_without_changing_results() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..24 {
        insert_download(
            &db,
            &storage,
            &format!("merge-{index:02}"),
            &format!("索引統合検証 {index:02}"),
            "統合テスト作者",
            &["索引"],
            "索引統合検証の共通本文",
        );
    }
    let before_segments = super::super::tantivy_index::searchable_segment_count(&storage).unwrap();
    assert!(
        before_segments > 1,
        "test setup must create fragmented segments"
    );
    let mut search_params = params("索引統合検証");
    search_params.limit = Some(30);
    let before = db.search_downloads_v2(&search_params).unwrap();
    let before_ids = before
        .items
        .iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    assert_eq!(before_ids.len(), 24);

    let (reported_before, reported_after) =
        super::super::tantivy_index::optimize_segments(&storage).unwrap();
    assert_eq!(reported_before, before_segments);
    assert_eq!(reported_after, 1);
    let after = db.search_downloads_v2(&search_params).unwrap();
    let after_ids = after
        .items
        .iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    assert_eq!(after.total_estimate, before.total_estimate);
    assert_eq!(after_ids, before_ids);
}

#[test]
fn ordinary_tantivy_writer_is_reused_and_yields_to_bulk_writer() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download(
        &db,
        &storage,
        "writer-cache-1",
        "writer cache first",
        "author",
        &["writer"],
        "body",
    );
    assert!(
        super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap(),
        "a normal save should retain its writer arena"
    );

    let bulk = super::super::tantivy_index::bulk_writer(&storage).unwrap();
    assert!(
        !super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap(),
        "the cached writer must release Tantivy's directory lock for a rebuild"
    );
    drop(bulk);

    insert_download(
        &db,
        &storage,
        "writer-cache-2",
        "writer cache second",
        "author",
        &["writer"],
        "body",
    );
    assert!(super::super::tantivy_index::ordinary_writer_is_cached(&storage).unwrap());
}

/// 段に分けて統合しても、最後は1つに収まる。
///
/// 一度に全部を混ぜていたころは、そのあいだ保存も削除も待たされた。段に
/// 分けたことで途中で場所を空けられるようになったが、**分けたせいで
/// 途中で止まってはいけない**。
///
/// なお 1 段の上限（16）を超えるセグメントは、この試験では作れない。
/// ふつうの保存は tantivy の既定の方針で自動的に統合されるので、80件
/// 入れても3つにしかならない。段をまたぐ経路そのものは、ここでは通らない。
#[test]
fn optimizing_a_multi_segment_index_still_ends_with_one() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    // 1件ずつ確定するので、作品の数だけセグメントができる。
    for index in 0..30 {
        insert_download(
            &db,
            &storage,
            &format!("segment-{index}"),
            &format!("段 {index} 番目"),
            "author",
            &["segment"],
            "本文",
        );
    }
    let before = super::super::tantivy_index::searchable_segment_count(&storage).unwrap();
    assert!(before > 1, "統合するものが要る: {before}");

    let (reported_before, after) =
        super::super::tantivy_index::optimize_segments(&storage).unwrap();

    assert_eq!(reported_before, before);
    assert_eq!(after, 1, "統合が途中で止まっている");
    assert_eq!(
        super::super::tantivy_index::searchable_segment_count(&storage).unwrap(),
        1
    );
}

/// 再構築の最中でも、保存や削除は待たされ続けない。
///
/// tantivy は 1 ディレクトリに 1 つしか書き手を許さないので、再構築が場所を
/// 占めているあいだ、ふつうの書き手は待つしかない。譲らなかったころは、
/// その待ちが**再構築の全期間**だった。1万件なら数分、しかも止まっている
/// ことが画面のどこにも出ない。区切りのいいところで場所を空ければ、待ちは
/// 一区切りぶんで済む。
#[test]
fn a_rebuild_hands_the_index_back_to_a_waiting_save() {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download(
        &db,
        &storage,
        "yield-1",
        "yield first",
        "author",
        &["yield"],
        "body",
    );

    let mut bulk = super::super::tantivy_index::bulk_writer(&storage).unwrap();
    assert!(!bulk.has_waiting_writers(), "まだ誰も待っていない");

    let (done_tx, done_rx) = mpsc::channel();
    let storage_for_waiter = storage.clone();
    let waiter = std::thread::spawn(move || {
        let result = super::super::tantivy_index::delete_documents(&storage_for_waiter, &[9_999]);
        let _ = done_tx.send(result);
    });

    // 待ちに入ったことを、名乗りで確かめる。名乗らない実装では、再構築は
    // 「誰も待っていない」と判断して最後まで場所を占め続ける。
    let deadline = Instant::now() + Duration::from_secs(10);
    while !bulk.has_waiting_writers() {
        assert!(Instant::now() < deadline, "削除が待ちに入らなかった");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "場所を空ける前に通ってしまった"
    );

    bulk.yield_now().unwrap();

    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("場所を空けたのに、待っていた削除が通らなかった")
        .expect("削除そのものは成功する");
    waiter.join().unwrap();

    // 譲ったあとも再構築は続けられる。取り直せていなければここで落ちる。
    assert!(!bulk.has_waiting_writers());
    bulk.commit().unwrap();
}

#[test]
fn entity_facets_search_and_page_beyond_the_dashboard_cap() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    // get_filter_facets only ever returns the top 60 authors, so the
    // library tab needs a query that can reach past that.
    for index in 0..70 {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("{}", 100 + index),
            &format!("作品{}", index),
            &format!("作者{:02}", index),
            &["日常"],
            "本文",
        );
    }

    let capped = db.get_filter_facets().unwrap();
    assert_eq!(capped.author_entities.len(), 60);

    let second_page = db
        .search_entity_facets("person", None, 60, 60, None, None, None, None)
        .unwrap();
    assert_eq!(second_page.len(), 10, "authors past the cap stay reachable");

    let filtered = db
        .search_entity_facets("person", Some("作者69"), 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].display_name, "作者69");
    assert_eq!(filtered[0].count, 1);

    let missing = db
        .search_entity_facets("person", Some("存在しない"), 60, 0, None, None, None, None)
        .unwrap();
    assert!(missing.is_empty());

    assert!(db
        .search_entity_facets("unknown", None, 10, 0, None, None, None, None)
        .is_err());

    // The pager cannot name a last page without this, and a total that does
    // not agree with the rows it is counting is worse than none at all.
    assert_eq!(
        db.count_entity_facets("person", None, None, None).unwrap(),
        70
    );
    assert_eq!(
        db.count_entity_facets("person", Some("作者69"), None, None)
            .unwrap(),
        1
    );
    assert_eq!(
        db.count_entity_facets("person", Some("存在しない"), None, None)
            .unwrap(),
        0
    );
    assert!(db.count_entity_facets("unknown", None, None, None).is_err());

    // Walked page by page, the rows add up to exactly what was counted.
    let total = db.count_entity_facets("person", None, None, None).unwrap();
    let mut walked = 0usize;
    for page in 0..(total as usize).div_ceil(20) {
        walked += db
            .search_entity_facets(
                "person",
                None,
                20,
                (page * 20) as i64,
                None,
                None,
                None,
                None,
            )
            .unwrap()
            .len();
    }
    assert_eq!(walked as i64, total);
}

/// 取得元が黙ったからといって、知っていたことを忘れない。
///
/// 完結の有無は web からしか来ない。保存のついでに作る控え（アプリAPI
/// だけを見る道）が None を書き戻して消していたら、カードの印が保存の
/// たびに消えたり点いたりする。
#[test]
fn a_silent_source_does_not_erase_what_a_series_already_said() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    db.upsert_series_profile(
        "pixiv",
        "9001",
        "連作",
        Some("説明"),
        Some("series/9001/v1/assets/cover.jpg"),
        "hash-1",
        "series/9001/v1/original.json",
        1,
        64,
        EntityProfileFreshness::RemoteChecked,
        Some(true),
        Some(89),
    )
    .unwrap();
    let stored = db.get_series("pixiv", "9001").unwrap();
    assert_eq!(stored.is_concluded, Some(true));
    assert_eq!(stored.published_content_count, Some(89));

    // 何も聞けなかった更新。表紙も完結も、手元の値が残る。
    db.upsert_series_profile(
        "pixiv",
        "9001",
        "連作",
        Some("説明"),
        None,
        "hash-2",
        "series/9001/v2/original.json",
        0,
        0,
        EntityProfileFreshness::SnapshotOnly,
        None,
        None,
    )
    .unwrap();
    let after = db.get_series("pixiv", "9001").unwrap();
    assert_eq!(after.is_concluded, Some(true), "黙りは「連載中」ではない");
    assert_eq!(after.published_content_count, Some(89));
    assert_eq!(
        after.cover_path.as_deref(),
        Some("series/9001/v1/assets/cover.jpg"),
        "表紙も同じ扱い"
    );

    // 連載が終われば、そう言われたときに変わる。
    db.upsert_series_profile(
        "pixiv",
        "9001",
        "連作",
        Some("説明"),
        None,
        "hash-3",
        "series/9001/v3/original.json",
        0,
        0,
        EntityProfileFreshness::RemoteChecked,
        Some(false),
        Some(90),
    )
    .unwrap();
    let latest = db.get_series("pixiv", "9001").unwrap();
    assert_eq!(latest.is_concluded, Some(false));
    assert_eq!(latest.published_content_count, Some(90));
}

/// 並べ替えの決まりそのもの。知らない指定でSQLへ文字列が漏れないことも
/// ここで固定する。
#[test]
fn entity_ordering_only_speaks_words_it_knows() {
    let clause = |by: Option<&str>, order: Option<&str>| {
        Database::entity_facet_order_clause(by, order, "display_name")
    };
    assert_eq!(
        clause(None, None),
        "ORDER BY count DESC, display_name ASC",
        "既定は作品が多い順"
    );
    assert!(clause(Some("downloaded_at"), None).starts_with("ORDER BY latest_downloaded_at DESC"));
    assert!(clause(Some("source_updated_at"), None)
        .starts_with("ORDER BY latest_source_updated_at DESC"));
    assert!(clause(Some("name"), None).starts_with("ORDER BY display_name COLLATE NOCASE ASC"));
    assert!(
        clause(Some("name"), Some("desc")).starts_with("ORDER BY display_name COLLATE NOCASE DESC")
    );
    // 知らない指定は既定へ落ちる。文字列はそのままSQLへ入らない。
    let injected = clause(Some("1; DROP TABLE downloads"), Some("desc; --"));
    assert_eq!(injected, "ORDER BY count DESC, display_name ASC");
    assert!(!injected.contains("DROP"));
    // 同じ値が並んだときの順番まで決めておく。ページの境目が揺れる。
    assert!(clause(Some("downloaded_at"), None).contains("count DESC, display_name ASC"));
}

/// 一覧そのものにかける条件（監視・作品数・完結）。
///
/// 「配下の作品の条件」と混ざっていないこと、件数（総数）も同じ条件で
/// 数えられていることを固定する。数が合わないページ送りは、最後の
/// ページが存在しないという嘘になる。
#[test]
fn entity_facets_narrow_by_what_the_listing_itself_is() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download_unindexed(&db, &storage, "sc-1", "追う人の一", "追う人", &[], "本文");
    insert_download_unindexed(&db, &storage, "sc-2", "追う人の二", "追う人", &[], "本文");
    insert_download_unindexed(
        &db,
        &storage,
        "sc-3",
        "止めた人の一",
        "止めた人",
        &[],
        "本文",
    );
    insert_download_unindexed(
        &db,
        &storage,
        "sc-4",
        "未登録の人の一",
        "未登録の人",
        &[],
        "本文",
    );
    {
        let conn = db.conn.lock().unwrap();
        for (id, author) in [("sc-1", "追う人"), ("sc-2", "追う人")] {
            conn.execute(
                "UPDATE downloads SET author_id = ?1, author_name = ?2 WHERE source_id = ?3",
                params!["author-watched", author, id],
            )
            .unwrap();
        }
        conn.execute(
            "UPDATE downloads SET author_id = 'author-paused' WHERE source_id = 'sc-3'",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = 'author-none' WHERE source_id = 'sc-4'",
            [],
        )
        .unwrap();
    }
    db.upsert_update_target(&UpdateTargetInput {
        target_type: "author".to_string(),
        source: "pixiv".to_string(),
        source_key: "author-watched".to_string(),
        display_name: "追う人".to_string(),
        enabled: true,
        metadata_json: None,
    })
    .unwrap();
    db.upsert_update_target(&UpdateTargetInput {
        target_type: "author".to_string(),
        source: "pixiv".to_string(),
        source_key: "author-paused".to_string(),
        display_name: "止めた人".to_string(),
        enabled: false,
        metadata_json: None,
    })
    .unwrap();

    let names = |scope: EntityFacetScope| {
        let listed = db
            .search_entity_facets("person", None, 60, 0, None, None, None, Some(&scope))
            .unwrap()
            .into_iter()
            .map(|facet| facet.display_name)
            .collect::<Vec<_>>();
        let counted = db
            .count_entity_facets("person", None, None, Some(&scope))
            .unwrap();
        assert_eq!(
            listed.len() as i64,
            counted,
            "総数は、数えている行と一致していなければならない"
        );
        listed
    };

    assert_eq!(
        names(EntityFacetScope {
            watch: Some("watched".into()),
            ..Default::default()
        }),
        vec!["追う人"]
    );
    assert_eq!(
        names(EntityFacetScope {
            watch: Some("paused".into()),
            ..Default::default()
        }),
        vec!["止めた人"]
    );
    let mut unwatched = names(EntityFacetScope {
        watch: Some("unwatched".into()),
        ..Default::default()
    });
    unwatched.sort();
    assert_eq!(unwatched, vec!["未登録の人"]);
    // 知らない言葉は条件なしとして扱う。
    assert_eq!(
        names(EntityFacetScope {
            watch: Some("いつか".into()),
            ..Default::default()
        })
        .len(),
        3
    );
    // 作品数の下限。1以下は条件なしと同じ。
    assert_eq!(
        names(EntityFacetScope {
            min_work_count: Some(2),
            ..Default::default()
        }),
        vec!["追う人"]
    );
    assert_eq!(
        names(EntityFacetScope {
            min_work_count: Some(1),
            ..Default::default()
        })
        .len(),
        3
    );
    // 監視と作品数は同時に効く。
    assert_eq!(
        names(EntityFacetScope {
            watch: Some("unwatched".into()),
            min_work_count: Some(2),
            ..Default::default()
        })
        .len(),
        0
    );
}

/// 完結の有無で絞る。まだ聞いていないシリーズは、どちらにも入らない。
#[test]
fn series_facets_narrow_by_whether_the_source_called_it_finished() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for (index, (source_id, series_key, title)) in [
        ("cs-1", "done", "完結した連作"),
        ("cs-2", "running", "続いている連作"),
        ("cs-3", "unknown", "まだ聞いていない連作"),
    ]
    .into_iter()
    .enumerate()
    {
        let id = insert_download_unindexed(
            &db,
            &storage,
            source_id,
            &format!("{title}#{index}"),
            "連載作者",
            &[],
            "本文",
        );
        db.upsert_download_series(id, "pixiv", series_key, title, Some(1))
            .unwrap();
    }
    for (key, title, concluded) in [
        ("done", "完結した連作", Some(true)),
        ("running", "続いている連作", Some(false)),
        ("unknown", "まだ聞いていない連作", None),
    ] {
        db.upsert_series_profile(
            "pixiv",
            key,
            title,
            None,
            None,
            &format!("hash-{key}"),
            &format!("series/{key}/v1/original.json"),
            0,
            0,
            EntityProfileFreshness::RemoteChecked,
            concluded,
            None,
        )
        .unwrap();
    }
    let names = |concluded: Option<bool>| {
        let scope = EntityFacetScope {
            concluded,
            ..Default::default()
        };
        let listed = db
            .search_entity_facets(
                "series",
                None,
                60,
                0,
                None,
                Some("name"),
                None,
                Some(&scope),
            )
            .unwrap()
            .into_iter()
            .map(|facet| facet.display_name)
            .collect::<Vec<_>>();
        assert_eq!(
            listed.len() as i64,
            db.count_entity_facets("series", None, None, Some(&scope))
                .unwrap()
        );
        listed
    };
    assert_eq!(names(Some(true)), vec!["完結した連作"]);
    assert_eq!(names(Some(false)), vec!["続いている連作"]);
    assert_eq!(names(None).len(), 3, "指定が無ければ全部");

    // 完結の印は一覧まで届く。
    let all = db
        .search_entity_facets("series", None, 60, 0, None, Some("name"), None, None)
        .unwrap();
    let finished = all.iter().find(|f| f.source_key == "done").unwrap();
    let unknown = all.iter().find(|f| f.source_key == "unknown").unwrap();
    assert_eq!(finished.is_concluded, Some(true));
    assert_eq!(unknown.is_concluded, None);
}

/// 作者・シリーズの並びは、その中にある作品を見て決まる。
#[test]
fn entity_facets_sort_by_the_works_underneath() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let a1 = insert_download_unindexed(&db, &storage, "sort-a1", "Aの一", "作者A", &[], "本文");
    let a2 = insert_download_unindexed(&db, &storage, "sort-a2", "Aの二", "作者A", &[], "本文");
    let b1 = insert_download_unindexed(&db, &storage, "sort-b1", "Bの一", "作者B", &[], "本文");
    {
        // 作者Aは作品が多く、保存は古く、取得元での更新は新しい。
        // 作者Bはその逆。どの鍵で並べたかが順番に出る。
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = 'author-A', author_name = '作者A',
                    downloaded_at = '2026-01-01T00:00:00Z',
                    source_updated_at = '2026-08-01T00:00:00Z'
                 WHERE id IN (?1, ?2)",
            params![a1, a2],
        )
        .unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = 'author-B', author_name = '作者B',
                    downloaded_at = '2026-08-20T00:00:00Z',
                    source_updated_at = '2026-01-05T00:00:00Z'
                 WHERE id = ?1",
            params![b1],
        )
        .unwrap();
    }
    let names = |by: Option<&str>| {
        db.search_entity_facets("person", None, 60, 0, None, by, None, None)
            .unwrap()
            .into_iter()
            .map(|facet| facet.display_name)
            .collect::<Vec<_>>()
    };
    assert_eq!(names(None), vec!["作者A", "作者B"], "作品が多い順");
    assert_eq!(
        names(Some("downloaded_at")),
        vec!["作者B", "作者A"],
        "保存が新しい順"
    );
    assert_eq!(
        names(Some("source_updated_at")),
        vec!["作者A", "作者B"],
        "取得元での更新が新しい順"
    );
    assert_eq!(names(Some("name")), vec!["作者A", "作者B"], "名前順");
}

#[test]
fn entity_facets_narrow_to_the_library_filters() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    // 作者Aは絞り込みに合う作品と合わない作品を持ち、作者Bは合う作品を
    // 持たない。絞り込みが効いていれば、Aだけが残り、その件数も1になる。
    let favourite =
        insert_download_unindexed(&db, &storage, "301", "日常の話", "作者A", &["日常"], "本文");
    let other =
        insert_download_unindexed(&db, &storage, "302", "冒険の話", "作者A", &["冒険"], "本文");
    insert_download_unindexed(
        &db,
        &storage,
        "303",
        "冒険の続き",
        "作者B",
        &["冒険"],
        "本文",
    );
    // 作品ごとに別の作者IDを振る補助関数なので、この2件を同じ人物にまとめる。
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = 'author-A' WHERE id IN (?1, ?2)",
            rusqlite::params![favourite, other],
        )
        .unwrap();
    }
    db.set_favorite(favourite, true).unwrap();

    let unfiltered = db
        .search_entity_facets("person", None, 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(unfiltered.len(), 2);
    assert_eq!(
        db.count_entity_facets("person", None, None, None).unwrap(),
        2
    );

    let by_tag = SearchV2Params {
        tags_include: Some(vec!["日常".to_string()]),
        ..params("")
    };
    let tagged = db
        .search_entity_facets("person", None, 60, 0, Some(&by_tag), None, None, None)
        .unwrap();
    assert_eq!(tagged.len(), 1, "作者Bはこのタグの作品を持たない");
    assert_eq!(tagged[0].display_name, "作者A");
    // 件数は「条件に合う作品の数」。作者Aの2件すべてではない。
    assert_eq!(tagged[0].count, 1);
    assert_eq!(
        db.count_entity_facets("person", None, Some(&by_tag), None)
            .unwrap(),
        1,
        "総数は数えている行と一致する"
    );

    // 名前での絞り込みと同時に効く。作者Aは条件に合うが、名前が違う。
    let named = db
        .search_entity_facets(
            "person",
            Some("作者B"),
            60,
            0,
            Some(&by_tag),
            None,
            None,
            None,
        )
        .unwrap();
    assert!(named.is_empty());

    let favourites_only = SearchV2Params {
        favorite: Some(true),
        ..params("")
    };
    let favoured = db
        .search_entity_facets(
            "person",
            None,
            60,
            0,
            Some(&favourites_only),
            None,
            None,
            None,
        )
        .unwrap();
    assert_eq!(favoured.len(), 1);
    assert_eq!(favoured[0].display_name, "作者A");

    // 同じキャッシュを別の条件で引かない。
    let unfiltered_again = db
        .search_entity_facets("person", None, 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(unfiltered_again.len(), 2);
}

#[test]
fn entity_key_pages_include_orphans_without_gaps() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    {
        let conn = db.conn.lock().unwrap();
        for (source, key) in [("fanbox", "p3"), ("pixiv", "p1"), ("pixiv", "p2")] {
            conn.execute(
                "INSERT INTO people (source, source_key, display_name)
                     VALUES (?1, ?2, ?2)",
                params![source, key],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO series (source, source_key, title)
                     VALUES (?1, ?2, ?2)",
                params![source, key],
            )
            .unwrap();
        }
    }

    let people_first = db.list_people_keys_after(None, 2).unwrap();
    assert_eq!(
        people_first,
        vec![
            ("fanbox".to_string(), "p3".to_string()),
            ("pixiv".to_string(), "p1".to_string())
        ]
    );
    let people_second = db
        .list_people_keys_after(Some((&people_first[1].0, &people_first[1].1)), 2)
        .unwrap();
    assert_eq!(people_second, vec![("pixiv".to_string(), "p2".to_string())]);

    let series_first = db.list_series_keys_after(None, 2).unwrap();
    let series_second = db
        .list_series_keys_after(Some((&series_first[1].0, &series_first[1].1)), 2)
        .unwrap();
    assert_eq!(series_first.len() + series_second.len(), 3);
}

#[test]
fn smart_search_ranks_metadata_over_body_and_supports_body_search() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let body_only = insert_download(
        &db,
        &storage,
        "1",
        "静かな本文一致",
        "作者A",
        &["日常"],
        "ここだけに秘密キーワードが出てくる長い本文です。",
    );
    let title_hit = insert_download(
        &db,
        &storage,
        "2",
        "秘密キーワードのタイトル",
        "作者B",
        &["冒険"],
        "本文には別の内容を書いておく。",
    );

    let results = db
        .search_downloads_v2(&params("秘密キーワード"))
        .unwrap()
        .items;
    assert_eq!(results.first().map(|dl| dl.id), Some(title_hit));
    assert!(results.iter().any(|dl| dl.id == body_only));
    assert!(results
        .iter()
        .find(|dl| dl.id == body_only)
        .map(|dl| !dl.match_highlights.is_empty())
        .unwrap_or(false));

    let partial = db.search_downloads_v2(&params("秘密キ")).unwrap().items;
    assert!(partial.iter().any(|dl| dl.id == body_only));

    let excluded = db
        .search_downloads_v2(&params("秘密 -タイトル"))
        .unwrap()
        .items;
    assert!(excluded.iter().any(|dl| dl.id == body_only));
    assert!(!excluded.iter().any(|dl| dl.id == title_hit));
}

#[test]
fn search_v2_uses_cursor_without_duplicate_pages() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let first = insert_download(&db, &storage, "1", "一番目", "作者A", &["日常"], "本文A");
    let second = insert_download(&db, &storage, "2", "二番目", "作者B", &["日常"], "本文B");
    let third = insert_download(&db, &storage, "3", "三番目", "作者C", &["日常"], "本文C");

    let page1 = db.search_downloads_v2(&v2_params(None, 2, None)).unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next_cursor.is_some());
    assert!(page1.next_cursor.as_deref().unwrap().starts_with("k:"));

    let page2 = db
        .search_downloads_v2(&v2_params(None, 2, page1.next_cursor.clone()))
        .unwrap();
    let mut seen = page1.items.iter().map(|dl| dl.id).collect::<Vec<_>>();
    seen.extend(page2.items.iter().map(|dl| dl.id));
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), 3);
    assert!(seen.contains(&first));
    assert!(seen.contains(&second));
    assert!(seen.contains(&third));
    assert!(page2.next_cursor.is_none());

    let query = db
        .search_downloads_v2(&v2_params(Some("番目"), 10, None))
        .unwrap();
    assert_eq!(query.search_meta.engine, "hybrid-local");
    assert_eq!(query.items.len(), 3);
}

#[test]
fn search_suggest_returns_metadata_candidates() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    insert_download(
        &db,
        &storage,
        "suggest-1",
        "候補タイトル",
        "候補作者",
        &["候補タグ"],
        "本文",
    );

    let suggestions = db
        .search_suggest(&SearchSuggestParams {
            text: Some("候補".to_string()),
            limit: Some(10),
        })
        .unwrap();
    assert!(suggestions
        .items
        .iter()
        .any(|item| item.kind == "tag" && item.label == "候補タグ"));
    assert!(suggestions
        .items
        .iter()
        .any(|item| item.kind == "author" && item.label == "候補作者"));
    assert!(suggestions
        .items
        .iter()
        .any(|item| item.kind == "title" && item.label == "候補タイトル"));

    let exact = db
        .search_suggest(&SearchSuggestParams {
            text: Some("候補作者".to_string()),
            limit: Some(2),
        })
        .unwrap();
    assert!(exact.items.len() <= 2);
    assert_eq!(
        exact.items.first().map(|item| item.kind.as_str()),
        Some("author")
    );
    assert!(exact.items.first().is_some_and(|item| item.exact_match));
}

#[test]
fn exact_author_intent_excludes_other_authors_that_only_mention_the_name() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download(
        &db,
        &storage,
        "exact-author-1",
        "作者本人の一作目",
        "明確な作者名",
        &["創作"],
        "本文",
    );
    let second = insert_download(
        &db,
        &storage,
        "exact-author-2",
        "作者本人の二作目",
        "明確な作者名",
        &["創作"],
        "本文",
    );
    insert_download(
        &db,
        &storage,
        "other-author",
        "言及だけの作品",
        "別の作者",
        &["評論"],
        "本文で明確な作者名について触れている",
    );

    let result = db.search_downloads_v2(&params("明確な作者名")).unwrap();
    let ids = result
        .items
        .iter()
        .map(|item| item.id)
        .collect::<HashSet<_>>();
    assert_eq!(ids, HashSet::from([first, second]));
    assert_eq!(result.search_meta.engine, "sqlite-exact-entity");
    let intent = result.search_meta.exact_entity.unwrap();
    assert_eq!(intent.kind, "author");
    assert!(intent.strict);
    assert!(result
        .search_meta
        .explanations
        .iter()
        .any(|line| line.contains("関係する作品だけ")));
}

#[test]
fn series_token_filters_by_series_relation() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let in_series = insert_download(
        &db,
        &storage,
        "series-1",
        "シリーズ内作品",
        "作者A",
        &["連載"],
        "本文",
    );
    let outside = insert_download(
        &db,
        &storage,
        "series-2",
        "シリーズ外作品",
        "作者B",
        &["読切"],
        "本文",
    );
    db.upsert_download_series(in_series, "pixiv", "s-100", "構造化シリーズ", Some(1))
        .unwrap();

    let result = db
        .search_downloads_v2(&params("series:pixiv:s-100"))
        .unwrap();
    assert_eq!(
        result.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
        vec![in_series]
    );
    assert!(!result.items.iter().any(|dl| dl.id == outside));

    let by_title = db
        .search_downloads_v2(&params("series:\"構造化シリーズ\""))
        .unwrap();
    assert_eq!(
        by_title.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
        vec![in_series]
    );

    let legacy_suggestion_value = db.search_downloads_v2(&params("pixiv:s-100")).unwrap();
    assert_eq!(
        legacy_suggestion_value
            .items
            .iter()
            .map(|dl| dl.id)
            .collect::<Vec<_>>(),
        vec![in_series]
    );

    let exact_title = db.search_downloads_v2(&params("構造化シリーズ")).unwrap();
    assert_eq!(
        exact_title.items.iter().map(|dl| dl.id).collect::<Vec<_>>(),
        vec![in_series]
    );
    assert_eq!(
        exact_title
            .search_meta
            .exact_entity
            .as_ref()
            .map(|intent| intent.kind.as_str()),
        Some("series")
    );

    let suggestions = db
        .search_suggest(&SearchSuggestParams {
            text: Some("構造化".to_string()),
            limit: Some(10),
        })
        .unwrap();
    assert!(suggestions.items.iter().any(|item| {
        item.kind == "series"
            && item.label == "構造化シリーズ"
            && item.value == "pixiv:s-100"
            && item.source.as_deref() == Some("pixiv")
            && item.source_key.as_deref() == Some("s-100")
    }));
}

#[test]
fn japanese_reading_kana_and_romaji_match_same_work() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let target = insert_download(
        &db,
        &storage,
        "jp-reading-1",
        "小説テスト作品",
        "作者A",
        &["物語"],
        "本文にも小説という語を含みます。",
    );

    for query in ["てすと", "テスト", "tesuto", "しょうせつ", "shousetsu"] {
        let result = db.search_downloads_v2(&params(query)).unwrap();
        assert!(
            result.items.iter().any(|dl| dl.id == target),
            "query {query} should match the target"
        );
    }

    let romaji = db.search_downloads_v2(&params("shousetsu")).unwrap();
    let target_row = romaji.items.iter().find(|dl| dl.id == target).unwrap();
    assert!(target_row.score_reasons.iter().any(|reason| {
        matches!(
            reason.match_type.as_str(),
            "exact" | "reading" | "romaji" | "synonym"
        )
    }));
    assert!(!target_row
        .score_reasons
        .iter()
        .any(|reason| reason.match_type == "semantic"));
    assert!(!target_row.match_highlights.is_empty());
}

#[test]
fn smart_search_does_not_add_semantic_reasons() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let target = insert_download(
        &db,
        &storage,
        "smart-no-semantic-1",
        "小説テスト作品",
        "作者A",
        &["物語"],
        "本文にも小説という語を含みます。",
    );
    let result = db.search_downloads_v2(&params("novel")).unwrap();
    let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();

    assert_eq!(result.search_meta.engine, "hybrid-local");
    assert!(!target_row
        .score_reasons
        .iter()
        .any(|reason| reason.match_type == "semantic"));
    assert!(!target_row
        .match_highlights
        .iter()
        .any(|highlight| highlight.match_type.as_deref() == Some("semantic")));
}

/// Opens a copy of a real library and reports what survived.
///
/// Schema recognition decides between "migrate in place" and "archive and
/// start empty", and the tests around it necessarily use databases this
/// code just created. This one can be pointed at a library saved by an
/// earlier release, which is the case that actually goes wrong.
#[test]
#[ignore = "set PIEP_VERIFY_DB to a real library file"]
fn opens_a_real_library_without_resetting_it() {
    let Ok(source) = std::env::var("PIEP_VERIFY_DB") else {
        panic!("set PIEP_VERIFY_DB to the database to check");
    };
    let source = PathBuf::from(source);
    let before: i64 = {
        let conn = Connection::open_with_flags(&source, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open source read-only");
        conn.query_row("SELECT COUNT(*) FROM downloads", [], |row| row.get(0))
            .unwrap()
    };

    // Never open the real file for writing: work on a copy.
    let (_temp, root, storage) = temp_paths();
    let copy = root.join("piep.db");
    fs::copy(&source, &copy).expect("copy the library");

    let db = Database::open(&copy, &storage).expect("open the copied library");
    let after = db.get_search_index_status().unwrap();
    println!(
        "works before: {before} | after opening: {} | pending index: {}",
        after.total_downloads, after.pending_downloads
    );
    assert_eq!(
        after.total_downloads, before,
        "opening a saved library must never discard its works"
    );
    assert!(before > 0, "the source library should not be empty");
}

#[test]
fn index_status_is_cached_without_going_stale_across_changes() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    assert_eq!(db.get_search_index_status().unwrap().total_downloads, 0);

    // Reading it again immediately is served from the cache, and adding a
    // work has to drop that cache: a count the screens keep showing after
    // the library changed is worse than measuring it again.
    insert_download_unindexed(
        &db,
        &storage,
        "cache-1",
        "追加された作品",
        "作者",
        &["cache"],
        "本文",
    );
    let after_insert = db.get_search_index_status().unwrap();
    assert_eq!(after_insert.total_downloads, 1);
    assert_eq!(after_insert.pending_downloads, 1);

    db.reindex_download(
        db.get_download_by_source("pixiv", "cache-1")
            .unwrap()
            .unwrap()
            .id,
    )
    .unwrap();
    let after_index = db.get_search_index_status().unwrap();
    assert_eq!(after_index.pending_downloads, 0);
    assert!(after_index.is_complete);
}

#[test]
fn semantic_completion_requires_every_current_document() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download_unindexed(
        &db,
        &storage,
        "coverage-1",
        "索引済み作品",
        "作者",
        &[],
        "最初の本文",
    );
    let second = insert_download_unindexed(
        &db,
        &storage,
        "coverage-2",
        "未索引作品",
        "作者",
        &[],
        "次の本文",
    );

    db.reindex_download(first).unwrap();
    let partial = db.get_search_index_status().unwrap();
    assert_eq!(partial.semantic_indexed_downloads, 1);
    assert_eq!(partial.semantic_pending_downloads, 1);

    db.reindex_download(second).unwrap();
    let complete = db.get_search_index_status().unwrap();
    assert_eq!(complete.semantic_indexed_downloads, 2);
    assert_eq!(complete.semantic_pending_downloads, 0);
}

#[test]
fn an_author_page_can_show_their_series_and_tags_and_drill_into_both() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let first = insert_download(
        &db,
        &storage,
        "auth-1",
        "序章",
        "青葉しおり",
        &["ファンタジー", "長編"],
        "本文",
    );
    let second = insert_download(
        &db,
        &storage,
        "auth-2",
        "第二話",
        "青葉しおり",
        &["ファンタジー"],
        "本文",
    );
    let standalone = insert_download(
        &db,
        &storage,
        "auth-3",
        "読み切り",
        "青葉しおり",
        &["短編"],
        "本文",
    );
    let other = insert_download(
        &db,
        &storage,
        "auth-4",
        "別作者の話",
        "別の人",
        &["ファンタジー"],
        "本文",
    );
    db.upsert_download_person(first, "pixiv", "aoba", "author", "青葉しおり")
        .unwrap();
    db.upsert_download_person(second, "pixiv", "aoba", "author", "青葉しおり")
        .unwrap();
    db.upsert_download_person(standalone, "pixiv", "aoba", "author", "青葉しおり")
        .unwrap();
    db.upsert_download_person(other, "pixiv", "hoka", "author", "別の人")
        .unwrap();
    db.upsert_download_series(first, "pixiv", "s1", "季節の栞", Some(1))
        .unwrap();
    db.upsert_download_series(second, "pixiv", "s1", "季節の栞", Some(2))
        .unwrap();

    let series = db.list_entity_series("pixiv", "aoba", 20).unwrap();
    assert_eq!(series.len(), 1, "only the series this author appears in");
    assert_eq!(series[0].display_name, "季節の栞");
    assert_eq!(series[0].count, 2);

    let tags = db.list_entity_tags("person", "pixiv", "aoba", 20).unwrap();
    let named = tags
        .iter()
        .map(|t| (t.name.as_str(), t.count))
        .collect::<Vec<_>>();
    assert_eq!(
        named,
        vec![("ファンタジー", 2), ("短編", 1), ("長編", 1)],
        "the author's own tags, most used first, without the other author's"
    );

    // Drilling in: the author's works carrying one of their tags. Author and
    // tag filters have to compose, or a tag on this page cannot be followed.
    let mut params = v2_params(None, 20, None);
    params.person_source = Some("pixiv".to_string());
    params.person_key = Some("aoba".to_string());
    params.tags_include = Some(vec!["ファンタジー".to_string()]);
    let page = db.search_downloads_v2(&params).unwrap();
    let ids = page.items.iter().map(|item| item.id).collect::<Vec<_>>();
    assert_eq!(ids.len(), 2);
    assert!(ids.contains(&first) && ids.contains(&second));
    assert!(
        !ids.contains(&other),
        "another author's work with the same tag must not appear"
    );

    // An author with nothing recorded is an empty page, not an error.
    assert!(db
        .list_entity_series("pixiv", "unknown", 20)
        .unwrap()
        .is_empty());
    assert!(db
        .list_entity_tags("person", "pixiv", "unknown", 20)
        .unwrap()
        .is_empty());
}

#[test]
fn entity_series_keyset_has_no_gaps_duplicates_or_same_name_instability() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    assert!(db
        .list_entity_series_paged("", "author", None, 20, None)
        .is_err());
    assert!(db
        .list_entity_series_paged(&"s".repeat(65), "author", None, 20, None)
        .is_err());
    assert!(db
        .list_entity_series_paged("pixiv", &"k".repeat(1_025), None, 20, None)
        .is_err());
    assert!(db
        .list_entity_series_paged("pixiv", "author", Some(&"検".repeat(201)), 20, None,)
        .is_err());
    assert!(db
        .list_entity_series_paged(
            "pixiv",
            "author",
            None,
            20,
            Some(&"c".repeat(64 * 1024 + 1)),
        )
        .is_err());
    assert!(db
        .list_entity_series_paged("pixiv", "author", None, 20, Some("malformed"))
        .is_err());

    let add =
        |source_id: &str, series_source: &str, series_key: &str, title: &str, created_at: &str| {
            let id = insert_download_unindexed(
                &db,
                &storage,
                source_id,
                source_id,
                "ページ作者",
                &[],
                "本文",
            );
            db.upsert_download_person(id, "pixiv", "paged-author", "author", "ページ作者")
                .unwrap();
            db.upsert_download_series(id, series_source, series_key, title, Some(1))
                .unwrap();
            db.conn
                .lock()
                .unwrap()
                .execute(
                    "UPDATE downloads
                     SET downloaded_at = ?1, source_created_at = ?1
                     WHERE id = ?2",
                    params![created_at, id],
                )
                .unwrap();
            id
        };

    add(
        "count-a",
        "pixiv",
        "counted",
        "最多",
        "2026-05-01T00:00:00Z",
    );
    add(
        "count-b",
        "pixiv",
        "counted",
        "最多",
        "2026-05-01T00:00:00Z",
    );
    add("alpha", "pixiv", "alpha", "Alpha", "2026-05-01T00:00:00Z");
    add("tie-f", "fanbox", "same", "同名", "2026-05-01T00:00:00Z");
    add("tie-p1", "pixiv", "same-1", "同名", "2026-05-01T00:00:00Z");
    add("tie-p2", "pixiv", "same-2", "同名", "2026-05-01T00:00:00Z");
    add("older", "pixiv", "older", "旧作", "2025-01-01T00:00:00Z");

    let mut cursor = None;
    let mut identities = Vec::new();
    let mut first_cursor = None;
    loop {
        let page = db
            .list_entity_series_paged("pixiv", "paged-author", None, 2, cursor.as_deref())
            .unwrap();
        assert_eq!(page.total, 6);
        identities.extend(
            page.items
                .iter()
                .map(|item| (item.source.clone(), item.source_key.clone())),
        );
        if first_cursor.is_none() {
            first_cursor = page.next_cursor.clone();
        }
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        identities,
        vec![
            ("pixiv".to_string(), "counted".to_string()),
            ("pixiv".to_string(), "alpha".to_string()),
            ("fanbox".to_string(), "same".to_string()),
            ("pixiv".to_string(), "same-1".to_string()),
            ("pixiv".to_string(), "same-2".to_string()),
            ("pixiv".to_string(), "older".to_string()),
        ]
    );
    assert_eq!(identities.iter().collect::<HashSet<_>>().len(), 6);

    let filtered = db
        .list_entity_series_paged("pixiv", "paged-author", Some("同名"), 2, None)
        .unwrap();
    assert_eq!(filtered.total, 3);
    assert_eq!(filtered.items.len(), 2);
    assert!(db
        .list_entity_series_paged(
            "pixiv",
            "paged-author",
            Some("別query"),
            2,
            first_cursor.as_deref(),
        )
        .is_err());

    add("mutation", "pixiv", "new", "追加", "2027-01-01T00:00:00Z");
    assert!(db
        .list_entity_series_paged("pixiv", "paged-author", None, 2, first_cursor.as_deref(),)
        .is_err());
}

#[test]
#[ignore = "performance smoke test for a prolific author's series"]
fn entity_series_paging_stays_fast_at_twenty_thousand() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let seeded = std::env::var("PIEP_BENCH_ENTITY_SERIES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000);
    {
        let mut conn = db.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut download = tx
                .prepare(
                    "INSERT INTO downloads (
                            source, source_id, title, author_name, author_id,
                            content_type, json_path, downloaded_at, source_created_at
                         ) VALUES ('pixiv', ?1, ?2, '大量作者', 'large-author',
                                   'novel', 'unused.json', ?3, ?3)",
                )
                .unwrap();
            let mut person = tx
                .prepare(
                    "INSERT INTO download_people (
                            download_id, person_source, person_key, role, display_name
                         ) VALUES (?1, 'pixiv', 'large-author', 'author', '大量作者')",
                )
                .unwrap();
            let mut series = tx
                .prepare(
                    "INSERT INTO series (source, source_key, title)
                         VALUES ('pixiv', ?1, ?2)",
                )
                .unwrap();
            let mut relation = tx
                .prepare(
                    "INSERT INTO download_series (
                            download_id, series_source, series_key, title, content_order
                         ) VALUES (?1, 'pixiv', ?2, ?3, 1)",
                )
                .unwrap();
            for index in 0..seeded {
                let key = format!("series-{index:05}");
                let title = format!("シリーズ {index:05}");
                let timestamp =
                    format!("2026-{:02}-{:02}T00:00:00Z", index % 12 + 1, index % 27 + 1);
                download.execute(params![key, title, timestamp]).unwrap();
                let id = tx.last_insert_rowid();
                person.execute(params![id]).unwrap();
                series.execute(params![key, title]).unwrap();
                relation.execute(params![id, key, title]).unwrap();
            }
        }
        tx.commit().unwrap();
    }

    let started = Instant::now();
    let first = db
        .list_entity_series_paged("pixiv", "large-author", None, 200, None)
        .unwrap();
    let first_elapsed = started.elapsed();
    assert_eq!(first.total, seeded as i64);
    assert_eq!(first.items.len(), seeded.min(200));

    let mut cursor = first.next_cursor;
    let mut deepest = Duration::ZERO;
    for _ in 0..50 {
        let Some(current) = cursor else { break };
        let started = Instant::now();
        let page = db
            .list_entity_series_paged("pixiv", "large-author", None, 200, Some(&current))
            .unwrap();
        deepest = deepest.max(started.elapsed());
        cursor = page.next_cursor;
    }
    eprintln!(
        "{seeded} entity series: first {:?}, deepest {:?}",
        first_elapsed, deepest
    );
    assert!(first_elapsed < Duration::from_secs(1));
    assert!(deepest < Duration::from_secs(1));
}

#[test]
fn numbered_pages_work_for_an_ordering_and_are_refused_for_relevance() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..7 {
        insert_download(
            &db,
            &storage,
            &format!("page-{index}"),
            &format!("作品{index}"),
            "作者",
            &["頁"],
            "本文",
        );
    }

    let page_of = |offset: i64| {
        let mut params = v2_params(None, 3, None);
        params.sort_by = Some("title".to_string());
        params.sort_order = Some("asc".to_string());
        params.offset = Some(offset);
        db.search_downloads_v2(&params)
            .unwrap()
            .items
            .iter()
            .map(|item| item.title.clone())
            .collect::<Vec<_>>()
    };
    assert_eq!(page_of(0), vec!["作品0", "作品1", "作品2"]);
    assert_eq!(
        page_of(3),
        vec!["作品3", "作品4", "作品5"],
        "the third page, without walking to it"
    );
    assert_eq!(page_of(6), vec!["作品6"]);
    assert!(
        page_of(99).is_empty(),
        "past the end is empty, not an error"
    );

    // Relevance has no nth page: results are walked with a score cursor, so
    // an offset into them would not be the page that was asked for.
    let mut relevance = params("作品");
    relevance.offset = Some(3);
    let first = relevance.clone();
    let mut without = first.clone();
    without.offset = None;
    assert_eq!(
        db.search_downloads_v2(&relevance).unwrap().items.len(),
        db.search_downloads_v2(&without).unwrap().items.len(),
        "an offset must be ignored rather than silently shifting a relevance page"
    );
}

#[test]
fn shelf_counts_ignore_reading_positions_for_works_that_no_longer_exist() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let first = insert_download(&db, &storage, "shelf-1", "一作目", "作者", &["棚"], "本文");
    let second = insert_download(&db, &storage, "shelf-2", "二作目", "作者", &["棚"], "本文");
    insert_download(&db, &storage, "shelf-3", "三作目", "作者", &["棚"], "本文");
    db.set_favorite(first, true).unwrap();
    db.set_watch_updates(second, true).unwrap();

    let empty = db.get_library_shelf_counts(&[]).unwrap();
    assert_eq!(
        (empty.total, empty.favorite, empty.watched, empty.reading),
        (3, 1, 1, 0)
    );

    // Reading positions are per device and nothing prunes them, so a shelf
    // must count works that exist, not entries that were left behind.
    let counts = db
        .get_library_shelf_counts(&[first, second, 999_999, first])
        .unwrap();
    assert_eq!(
        counts.reading, 2,
        "a deleted work and a duplicate must not be counted"
    );

    // The same list used as a filter returns exactly those works.
    let mut params = v2_params(None, 20, None);
    params.ids_include = Some(vec![first, 999_999]);
    let page = db.search_downloads_v2(&params).unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, first);

    // An empty membership list means an empty shelf, never the whole library.
    let mut none = v2_params(None, 20, None);
    none.ids_include = Some(Vec::new());
    assert_eq!(db.search_downloads_v2(&none).unwrap().items.len(), 0);
}

#[test]
fn saved_searches_survive_reuse_of_a_name_and_reject_nonsense() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let created = db
        .upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "  長編ファンタジー  ".to_string(),
            query: Some("tag:ファンタジー".to_string()),
            params_json: "{\"tagsInclude\":[\"ファンタジー\"]}".to_string(),
        })
        .unwrap();
    assert_eq!(
        created.name, "長編ファンタジー",
        "the name is stored trimmed"
    );

    // Saving again under the same name replaces that search. The unique
    // constraint must not surface as an error the reader has to decode.
    let replaced = db
        .upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "長編ファンタジー".to_string(),
            query: Some("tag:ファンタジー -短編".to_string()),
            params_json: "{\"tagsExclude\":[\"短編\"]}".to_string(),
        })
        .unwrap();
    assert_eq!(replaced.id, created.id);
    assert_eq!(db.list_saved_searches().unwrap().len(), 1);
    assert_eq!(replaced.query.as_deref(), Some("tag:ファンタジー -短編"));

    assert!(
        db.upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "   ".to_string(),
            query: None,
            params_json: "{}".to_string(),
        })
        .is_err(),
        "a blank name is not a search anyone can find again"
    );
    assert!(
        db.upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "壊れた条件".to_string(),
            query: None,
            params_json: "{not json".to_string(),
        })
        .is_err(),
        "conditions that cannot be read back must not be stored"
    );
    assert!(
        db.upsert_saved_search(&SavedSearchInput {
            id: Some(4_242),
            name: "存在しない".to_string(),
            query: None,
            params_json: "{}".to_string(),
        })
        .is_err(),
        "updating a search that was deleted elsewhere must say so"
    );

    assert!(db.delete_saved_search(created.id).unwrap());
    assert!(
        !db.delete_saved_search(created.id).unwrap(),
        "a second delete is not an error"
    );
    assert!(db.list_saved_searches().unwrap().is_empty());
}

#[test]
fn saved_searches_keep_their_order_and_stop_at_the_limit() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    for index in 0..MAX_SAVED_SEARCHES {
        db.upsert_saved_search(&SavedSearchInput {
            id: None,
            name: format!("検索{index:03}"),
            query: None,
            params_json: "{}".to_string(),
        })
        .unwrap();
    }
    let listed = db.list_saved_searches().unwrap();
    assert_eq!(listed.len() as i64, MAX_SAVED_SEARCHES);
    assert_eq!(
        listed[0].name, "検索000",
        "the sidebar order must be stable"
    );
    assert_eq!(listed[listed.len() - 1].name, "検索099");

    let overflow = db.upsert_saved_search(&SavedSearchInput {
        id: None,
        name: "一件多い".to_string(),
        query: None,
        params_json: "{}".to_string(),
    });
    assert!(
        overflow.is_err(),
        "the limit must be refused, not silently applied"
    );
    // Replacing an existing one still works when the list is full.
    assert!(db
        .upsert_saved_search(&SavedSearchInput {
            id: None,
            name: "検索000".to_string(),
            query: Some("更新".to_string()),
            params_json: "{}".to_string(),
        })
        .is_ok());
}

#[test]
fn synonyms_do_not_match_every_work_through_its_source_url() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    // Every pixiv URL is .../novel/show.php?id=N, and the synonym table
    // expands 物語 to "novel". Neither of these works mentions 物語.
    insert_download(
        &db,
        &storage,
        "111",
        "海辺の記録",
        "作者A",
        &["日常"],
        "静かな本文です",
    );
    insert_download(
        &db,
        &storage,
        "222",
        "灯台の手紙",
        "作者B",
        &["日常"],
        "別の本文です",
    );
    let about = insert_download(
        &db,
        &storage,
        "333",
        "夜明けの物語",
        "作者C",
        &["創作"],
        "本文",
    );

    let hits = db.search_downloads_v2(&params("物語")).unwrap();
    let ids = hits.items.iter().map(|item| item.id).collect::<Vec<_>>();
    assert_eq!(
        ids,
        vec![about],
        "only the work that actually says 物語 should match"
    );

    // The URL itself is still findable: pasting one has to reach its work.
    let pasted = db
        .search_downloads_v2(&params("https://www.pixiv.net/novel/show.php?id=111"))
        .unwrap();
    assert!(
        pasted.items.iter().any(|item| item.source_id == "111"),
        "a pasted source URL must still find its work"
    );
    // As must the bare id, which is what the URL actually identifies.
    let by_id = db.search_downloads_v2(&params("222")).unwrap();
    assert!(by_id.items.iter().any(|item| item.source_id == "222"));
}

#[test]
fn changing_the_index_format_requeues_the_whole_library() {
    let (_temp, root, storage) = temp_paths();
    let db_path = root.join("piep.db");
    let db = Database::open(&db_path, &storage).unwrap();
    insert_download(
        &db,
        &storage,
        "fmt-1",
        "書式変更の作品",
        "作者",
        &["書式"],
        "本文",
    );
    assert_eq!(db.get_search_index_status().unwrap().pending_downloads, 0);
    drop(db);

    // Stand in for a release that changes the on-disk index layout: the
    // bookkeeping now describes an index the app no longer reads.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO search_index_meta (id, index_version, updated_at)
                 VALUES (1, 'v-previous', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    }

    let db = Database::open(&db_path, &storage).unwrap();
    assert_eq!(
        db.get_search_index_status().unwrap().pending_downloads,
        1,
        "a format change must queue the library for reindexing, not report it as complete"
    );

    // What the app does at launch: notice the backlog and clear it without
    // being asked. Nothing here should need a person to press anything.
    let outcome = db
        .rebuild_search_index(SearchIndexRebuildOptions::default(), &|| false, |_| {})
        .unwrap();
    assert_eq!(outcome.processed, 1);
    assert!(!outcome.canceled);
    let status = db.get_search_index_status().unwrap();
    assert_eq!(status.pending_downloads, 0);
    assert!(status.is_complete);
    // And the work is genuinely searchable again, not merely marked done.
    let found = db.search_downloads_v2(&params("書式変更")).unwrap();
    assert_eq!(found.items.len(), 1);
}

#[test]
fn sorted_search_orders_matches_by_column_and_pages_without_gaps() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    // Titles are deliberately out of insertion order so a title sort cannot
    // pass by accident. The shared token avoids the synonym table, which
    // would otherwise pull in unrelated works through their source URL.
    for (source_id, title) in [
        ("sorted-1", "さくら花暦"),
        ("sorted-2", "あおぞら花暦"),
        ("sorted-3", "なのはな花暦"),
        ("sorted-4", "かえで花暦"),
        ("sorted-5", "たんぽぽ花暦"),
    ] {
        insert_download(
            &db,
            &storage,
            source_id,
            title,
            "並び替え作者",
            &["並替"],
            "共通の本文です",
        );
    }
    // A work that must never appear: it does not match the query.
    insert_download(
        &db,
        &storage,
        "sorted-x",
        "無関係な作品",
        "別作者",
        &["他"],
        "別の本文",
    );

    let mut sorted = params("花暦");
    sorted.sort_by = Some("title".to_string());
    sorted.sort_order = Some("asc".to_string());
    sorted.limit = Some(2);

    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..5 {
        let mut page_params = sorted.clone();
        page_params.cursor = cursor.clone();
        let page = db.search_downloads_v2(&page_params).unwrap();
        assert_eq!(page.search_meta.engine, "tantivy-sorted");
        assert_eq!(page.total_estimate, Some(5));
        seen.extend(page.items.iter().map(|item| item.title.clone()));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(
        seen,
        vec![
            "あおぞら花暦",
            "かえで花暦",
            "さくら花暦",
            "たんぽぽ花暦",
            "なのはな花暦",
        ],
        "every match should appear exactly once, in title order"
    );

    // Without an explicit sort the same query still ranks by relevance.
    let relevance = db.search_downloads_v2(&params("花暦")).unwrap();
    assert_ne!(relevance.search_meta.engine, "tantivy-sorted");
}

#[test]
fn sorted_search_respects_library_filters() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let favorite = insert_download(
        &db,
        &storage,
        "filter-1",
        "対象作品 花暦",
        "作者",
        &["絞込"],
        "本文",
    );
    insert_download(
        &db,
        &storage,
        "filter-2",
        "対象外作品 花暦",
        "作者",
        &["絞込"],
        "本文",
    );
    db.set_favorite(favorite, true).unwrap();

    let mut sorted = params("花暦");
    sorted.sort_by = Some("downloaded_at".to_string());
    sorted.favorite = Some(true);
    let page = db.search_downloads_v2(&sorted).unwrap();
    assert_eq!(page.total_estimate, Some(1));
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, favorite);
}

#[test]
fn sorted_search_uses_bounded_temp_table_batches_and_carries_total() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    const MATCHES: usize = SORTED_SEARCH_ID_BATCH_SIZE * 2 + 17;
    for index in 0..MATCHES {
        insert_download_unindexed(
            &db,
            &storage,
            &format!("streamed-{index:04}"),
            &format!("流し込み検索 {index:04}"),
            "作者",
            &["一時表"],
            "流し込み検索の本文",
        );
    }
    loop {
        let status = db.rebuild_search_index_batch(200).unwrap();
        if status.pending_downloads == 0 {
            break;
        }
    }
    super::super::tantivy_index::optimize_segments(&storage).unwrap();

    let mut request = params("流し込み検索");
    request.sort_by = Some("title".to_string());
    request.sort_order = Some("asc".to_string());
    request.limit = Some(31);
    let first = db.search_downloads_v2(&request).unwrap();
    assert_eq!(first.total_estimate, Some(MATCHES as i64));
    assert_eq!(first.items.len(), 31);
    let (_, _, cache_after_first) = db.query_cache_stats();

    let cursor = decode_cursor(first.next_cursor.as_deref()).unwrap();
    assert_eq!(cursor.total_estimate, Some(MATCHES as i64));
    request.cursor = first.next_cursor;
    let second = db.search_downloads_v2(&request).unwrap();
    assert_eq!(second.total_estimate, Some(MATCHES as i64));
    assert!(first
        .items
        .iter()
        .all(|left| second.items.iter().all(|right| left.id != right.id)));
    let (_, _, cache_after_second) = db.query_cache_stats();
    assert!(cache_after_second.0 > cache_after_first.0);

    insert_download(
        &db,
        &storage,
        "streamed-new",
        "流し込み検索 新着",
        "作者",
        &["一時表"],
        "流し込み検索の本文",
    );
    request.cursor = None;
    let after_insert = db.search_downloads_v2(&request).unwrap();
    assert_eq!(after_insert.total_estimate, Some(MATCHES as i64 + 1));
    let (_, _, cache_after_insert) = db.query_cache_stats();
    assert!(
        cache_after_insert.1 > cache_after_second.1,
        "library/index generation change must rebuild the snapshot"
    );
}

#[test]
fn multi_term_search_requires_each_term() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let target = insert_download(
        &db,
        &storage,
        "multi-term-1",
        "alpha beta target",
        "作者A",
        &["mixed"],
        "本文",
    );
    let alpha_only = insert_download(
        &db,
        &storage,
        "multi-term-2",
        "alpha only",
        "作者B",
        &["mixed"],
        "本文",
    );
    let beta_only = insert_download(
        &db,
        &storage,
        "multi-term-3",
        "beta only",
        "作者C",
        &["mixed"],
        "本文",
    );

    let result = db.search_downloads_v2(&params("alpha beta")).unwrap();
    assert!(result.items.iter().any(|item| item.id == target));
    assert!(!result.items.iter().any(|item| item.id == alpha_only));
    assert!(!result.items.iter().any(|item| item.id == beta_only));
}

#[test]
fn semantic_mode_returns_body_chunk_highlight_and_reason() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let target = insert_download(
        &db,
        &storage,
        "semantic-1",
        "静かな作品",
        "作者A",
        &["日常"],
        "これは長い小説本文です。読者が物語を探すときに本文チャンクで見つかります。",
    );
    let mut semantic_params = params("novel");
    semantic_params.search_mode = Some("semantic".to_string());

    let result = db.search_downloads_v2(&semantic_params).unwrap();
    let target_row = result.items.iter().find(|dl| dl.id == target).unwrap();
    assert!(target_row
        .score_reasons
        .iter()
        .any(|reason| reason.match_type == "semantic"));
    assert!(target_row.match_highlights.iter().any(|highlight| {
        highlight.match_type.as_deref() == Some("semantic")
            && highlight
                .segments
                .iter()
                .any(|segment| segment.matched && segment.text.contains("小説本文"))
    }));
}

#[test]
#[ignore = "performance smoke test for large-library browsing"]
fn library_browsing_stays_fast_on_a_large_library() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    let seeded: usize = std::env::var("PIEP_BENCH_WORKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000);
    let metadata_only =
        seeded >= 250_000 || std::env::var("PIEP_BENCH_METADATA_ONLY").as_deref() == Ok("1");
    let seed_started = Instant::now();
    if metadata_only {
        seed_metadata_only_library(&db, seeded);
    } else {
        for index in 0..seeded {
            insert_download_unindexed(
                &db,
                &storage,
                &format!("scale-{}", index),
                &format!("蔵書 {:05}", index),
                &format!("作者 {:03}", index % 400),
                &[&format!("tag{}", index % 30)],
                "大規模ライブラリの一覧性能を確認するための本文です。",
            );
        }
    }
    let seed_elapsed = seed_started.elapsed();
    let database_bytes = fs::metadata(root.join("piep.db"))
        .map(|metadata| metadata.len())
        .unwrap_or(0);

    // The library browses with an empty query, which is the keyset path.
    let mut first = v2_params(None, 60, None);
    first.projection = Some("libraryGallery".to_string());
    let started = Instant::now();
    let page1 = db.search_downloads_v2(&first).unwrap();
    let first_elapsed = started.elapsed();
    assert_eq!(page1.items.len(), 60);
    assert_eq!(page1.total_estimate, Some(seeded as i64));
    assert!(
        page1.next_cursor.is_some(),
        "a large library must expose more pages"
    );
    let started = Instant::now();
    let warm_page1 = db.search_downloads_v2(&first).unwrap();
    let warm_first_elapsed = started.elapsed();
    assert_eq!(warm_page1.items.len(), page1.items.len());

    // Walk deep into the list: keyset paging must not degrade with depth.
    let mut cursor = page1.next_cursor.clone();
    let mut deepest = Duration::ZERO;
    for _ in 0..40 {
        let mut page_params = v2_params(None, 60, cursor.clone());
        page_params.projection = Some("libraryGallery".to_string());
        let started = Instant::now();
        let page = db.search_downloads_v2(&page_params).unwrap();
        deepest = deepest.max(started.elapsed());
        cursor = page.next_cursor.clone();
        assert!(cursor.is_some(), "paging ended earlier than expected");
    }

    let started = Instant::now();
    let authors = db
        .search_entity_facets("person", None, 60, 300, None, None, None, None)
        .unwrap();
    let entity_elapsed = started.elapsed();
    assert_eq!(authors.len(), 60);
    let started = Instant::now();
    let warm_authors = db
        .search_entity_facets("person", None, 60, 300, None, None, None, None)
        .unwrap();
    let warm_entity_elapsed = started.elapsed();
    assert_eq!(warm_authors.len(), authors.len());

    let started = Instant::now();
    let facets = db.get_filter_facets_with(false).unwrap();
    let facet_elapsed = started.elapsed();
    assert!(!facets.tags.is_empty());
    assert!(
        facets.author_entities.is_empty(),
        "the light variant must skip the entity aggregates"
    );

    let started = Instant::now();
    let cached_facets = db.get_filter_facets_with(false).unwrap();
    let cached_facet_elapsed = started.elapsed();
    assert_eq!(
        cached_facets
            .tags
            .iter()
            .map(|facet| (&facet.name, facet.count))
            .collect::<Vec<_>>(),
        facets
            .tags
            .iter()
            .map(|facet| (&facet.name, facet.count))
            .collect::<Vec<_>>()
    );

    let suggest_params = SearchSuggestParams {
        text: Some("蔵書".to_string()),
        limit: Some(12),
    };
    let started = Instant::now();
    let suggestions = db.search_suggest(&suggest_params).unwrap();
    let suggest_elapsed = started.elapsed();
    assert_eq!(suggestions.items.len(), 12);
    let started = Instant::now();
    let cached_suggestions = db.search_suggest(&suggest_params).unwrap();
    let cached_suggest_elapsed = started.elapsed();
    assert_eq!(cached_suggestions.items.len(), suggestions.items.len());

    eprintln!(
            "{} works (metadata_only={}, seed {:?}, db {} MiB): first page cold {:?}/warm {:?}, deepest page {:?}, authors cold {:?}/warm {:?}, filter options cold {:?}/warm {:?}, suggestions cold {:?}/warm {:?}",
            seeded,
            metadata_only,
            seed_elapsed,
            database_bytes / (1024 * 1024),
            first_elapsed,
            warm_first_elapsed,
            deepest,
            entity_elapsed,
            warm_entity_elapsed,
            facet_elapsed,
            cached_facet_elapsed,
            suggest_elapsed,
            cached_suggest_elapsed
        );
    let first_page_budget = if seeded >= 500_000 {
        Duration::from_secs(1)
    } else {
        Duration::from_millis(400)
    };
    assert!(
        first_elapsed < first_page_budget,
        "first page took {:?}",
        first_elapsed
    );
    assert!(
        warm_first_elapsed < Duration::from_millis(400),
        "warm first page took {:?}",
        warm_first_elapsed
    );
    assert!(
        deepest < Duration::from_millis(400),
        "deep page took {:?}",
        deepest
    );
    let aggregate_budget = if seeded >= 500_000 {
        Duration::from_secs(3)
    } else {
        Duration::from_millis(500)
    };
    // Without idx_downloads_author_recent this listing takes ~1.9s here.
    assert!(
        entity_elapsed < aggregate_budget,
        "author listing took {:?}",
        entity_elapsed
    );
    assert!(
        warm_entity_elapsed < Duration::from_millis(100),
        "cached author listing took {:?}",
        warm_entity_elapsed
    );
    assert!(
        facet_elapsed < aggregate_budget,
        "filter options took {:?}",
        facet_elapsed
    );
    assert!(
        suggest_elapsed < aggregate_budget,
        "cold suggestions took {:?}",
        suggest_elapsed
    );
    assert!(
        cached_facet_elapsed < Duration::from_millis(50),
        "cached filter options took {:?}",
        cached_facet_elapsed
    );
    assert!(
        cached_suggest_elapsed < Duration::from_millis(50),
        "cached suggestions took {:?}",
        cached_suggest_elapsed
    );

    drop(db);
}

#[test]
#[ignore = "measurement harness for disk-backed lexical snapshots"]
fn lexical_snapshots_scale_with_fixed_memory_batches() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let works: usize = std::env::var("PIEP_BENCH_WORKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000);
    assert!((1_000..=1_000_000).contains(&works));

    let seed_started = Instant::now();
    seed_metadata_only_library(&db, works);
    let seed_elapsed = seed_started.elapsed();
    let index_started = Instant::now();
    let mut writer = super::super::tantivy_index::bulk_writer(&storage).unwrap();
    for index in 0..works {
        let prepared = super::super::tantivy_index::prepare_document(
            &storage,
            &super::super::tantivy_index::TantivyIndexDocument {
                download_id: index as i64 + 1,
                source: "pixiv".to_string(),
                source_id: format!("scale-{index:07}"),
                source_url: format!("https://example.invalid/{index}"),
                title: format!("snapshotterm work {index:07}"),
                author_name: format!("author {:03}", index % 400),
                author_id: format!("author-{:03}", index % 400),
                tags: format!("tag{}", index % 30),
                series_title: String::new(),
                excerpt: "snapshotterm benchmark".to_string(),
                body: "snapshotterm bounded disk ranking".to_string(),
                published_at: "2026-01-01T00:00:00Z".to_string(),
                downloaded_at: "2026-01-01T00:00:00Z".to_string(),
                favorite: false,
                watch_updates: false,
                asset_kinds: String::new(),
                text_length: 36,
            },
        )
        .unwrap();
        writer.upsert(prepared).unwrap();
        if writer.uncommitted() >= 20_000 {
            writer.commit().unwrap();
        }
    }
    writer.commit().unwrap();
    drop(writer);
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO search_index_state (
                    download_id, current_version, content_hash, indexed_at
                 )
                 SELECT id, current_version, content_hash, CURRENT_TIMESTAMP FROM downloads",
            [],
        )
        .unwrap();
    }
    let index_elapsed = index_started.elapsed();

    let mut ranked = params("snapshotterm");
    ranked.limit = Some(60);
    ranked.projection = Some("bulk".to_string());
    let started = Instant::now();
    let first = db.search_downloads_v2(&ranked).unwrap();
    let ranked_cold = started.elapsed();
    assert_eq!(first.total_estimate, Some(works as i64));
    ranked.cursor = first.next_cursor;
    let started = Instant::now();
    let second = db.search_downloads_v2(&ranked).unwrap();
    let ranked_warm = started.elapsed();
    assert_eq!(second.items.len(), 60);

    let mut sorted = params("snapshotterm");
    sorted.limit = Some(60);
    sorted.projection = Some("bulk".to_string());
    sorted.sort_by = Some("title".to_string());
    sorted.sort_order = Some("asc".to_string());
    let started = Instant::now();
    let first_sorted = db.search_downloads_v2(&sorted).unwrap();
    let sorted_cold = started.elapsed();
    assert_eq!(first_sorted.total_estimate, Some(works as i64));
    sorted.cursor = first_sorted.next_cursor;
    let started = Instant::now();
    let second_sorted = db.search_downloads_v2(&sorted).unwrap();
    let sorted_warm = started.elapsed();
    assert_eq!(second_sorted.items.len(), 60);

    let cache = db.search_snapshot_cache.lock().unwrap();
    let snapshot_bytes = cache.disk_bytes;
    let snapshot_count = cache.entries.len();
    drop(cache);
    eprintln!(
            "{works} lexical matches: seed {seed_elapsed:?}, index {index_elapsed:?}, ranked cold/warm {ranked_cold:?}/{ranked_warm:?}, sorted cold/warm {sorted_cold:?}/{sorted_warm:?}, {snapshot_count} snapshots {} MiB",
            snapshot_bytes / (1024 * 1024)
        );
    assert!(snapshot_count >= 2);
    assert!(snapshot_bytes <= super::super::resource_budget::search_snapshot_disk_bytes());
    assert!(ranked_warm < Duration::from_millis(250));
    assert!(sorted_warm < Duration::from_millis(250));

    drop(db);
}

#[test]
fn entity_facet_cache_is_invalidated_by_a_library_commit() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    insert_download_unindexed(
        &db,
        &storage,
        "facet-cache-one",
        "一冊目",
        "同じ作者",
        &[],
        "本文",
    );
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE downloads SET author_id = 'same-author' WHERE source_id = 'facet-cache-one'",
            [],
        )
        .unwrap();
    let first = db
        .search_entity_facets("person", None, 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(first[0].count, 1);
    let cached = db
        .search_entity_facets("person", None, 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(cached[0].count, 1);

    insert_download_unindexed(
        &db,
        &storage,
        "facet-cache-two",
        "二冊目",
        "同じ作者",
        &[],
        "本文",
    );
    db.conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE downloads SET author_id = 'same-author' WHERE source_id = 'facet-cache-two'",
            [],
        )
        .unwrap();
    let refreshed = db
        .search_entity_facets("person", None, 60, 0, None, None, None, None)
        .unwrap();
    assert_eq!(refreshed[0].count, 2);
    drop(db);
}

#[test]
#[ignore = "performance smoke test for local search tuning"]
fn smart_search_handles_5000_seed_items_under_target() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();

    for index in 0..5_000 {
        insert_download_unindexed(
                &db,
                &storage,
                &format!("perf-{}", index),
                &format!("Seed Work {:04}", index),
                &format!("Seed Author {:02}", index % 50),
                &[&format!("seed{}", index % 25)],
                &format!(
                    "検索性能検証用の本文です。番号 {}、タグ seed{}。日本語とASCII mixed text for search.",
                    index,
                    index % 25
                ),
            );
    }
    loop {
        let status = db.rebuild_search_index_batch(200).unwrap();
        if status.pending_downloads == 0 {
            break;
        }
    }

    let mut search_params = params("検索性能 seed7");
    search_params.limit = Some(120);
    let started = Instant::now();
    let result = db.search_downloads_v2(&search_params).unwrap();
    let elapsed = started.elapsed();
    eprintln!("5000-item smart search: {elapsed:?}");

    assert!(!result.items.is_empty());
    assert!(
        elapsed < Duration::from_secs(1),
        "smart search took {:?}",
        elapsed
    );
}

#[test]
fn projection_selects_do_not_include_unneeded_subqueries() {
    let bulk = download_select_sql_for_projection(Some("bulk"), "NULL", "NULL");
    assert!(!bulk.contains("download_tags"));
    assert!(!bulk.contains("download_people"));
    assert!(!bulk.contains("download_series"));

    let entity = download_select_sql_for_projection(Some("entityFacet"), "NULL", "NULL");
    assert!(!entity.contains("download_tags"));
    assert!(!entity.contains("download_people"));
    assert!(!entity.contains("download_series"));
    assert!(entity.contains("d.cover_path"));

    // 一覧もタグとあらすじを出すようになったので、読むものはギャラリーと同じである。
    // 別の射影を残しておくと、片方に列を足したときにもう片方が取り残される。
    let compact = download_select_sql_for_projection(Some("libraryCompact"), "NULL", "NULL");
    assert!(compact.contains("download_tags"));
    assert!(compact.contains("download_people"));
    assert!(compact.contains("download_series"));
    assert!(compact.contains("d.excerpt"));

    let gallery = download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL");
    assert!(gallery.contains("download_tags"));
    assert!(gallery.contains("download_people"));
    assert!(gallery.contains("download_series"));
}

#[test]
fn active_edit_revision_drives_reader_and_search_body() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let download_id = insert_download(
        &db,
        &storage,
        "edit-1",
        "編集対象",
        "作者A",
        &["編集"],
        "原本だけにある本文です。",
    );

    let initial_reader = db.get_reader_document(download_id, None).unwrap();
    assert!(!initial_reader.is_edited);
    assert!(initial_reader.plain_text.contains("原本だけ"));

    let draft = db
        .save_work_draft(
            download_id,
            1,
            &[
                WorkBlockInput {
                    block_type: "heading".to_string(),
                    text: Some("編集版見出し".to_string()),
                    asset_id: None,
                    attrs_json: None,
                },
                WorkBlockInput {
                    block_type: "paragraph".to_string(),
                    text: Some("編集固有キーワードを含む本文です。".to_string()),
                    asset_id: None,
                    attrs_json: None,
                },
            ],
        )
        .unwrap();
    db.activate_work_edit(draft.id).unwrap();

    let edited_reader = db.get_reader_document(download_id, None).unwrap();
    assert!(edited_reader.is_edited);
    assert!(edited_reader.html.contains("編集版見出し"));
    assert!(edited_reader.plain_text.contains("編集固有キーワード"));

    let search = db
        .search_downloads_v2(&v2_params(Some("編集固有キーワード"), 10, None))
        .unwrap();
    assert_eq!(search.items.first().map(|item| item.id), Some(download_id));
}

#[test]
fn update_job_schema_recovers_interrupted_jobs() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "work".to_string(),
        mode: "auto_save".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job(
        "job-test",
        &request,
        &[UpdateJobItemInput {
            item_type: "work".to_string(),
            source: Some("pixiv".to_string()),
            source_id: Some("1".to_string()),
            target_type: Some("work".to_string()),
            title: "Test".to_string(),
            payload_json: "{}".to_string(),
            status: "queued".to_string(),
        }],
    )
    .unwrap();
    db.set_update_job_status("job-test", "running", Some("running"))
        .unwrap();
    db.recover_update_jobs_on_startup().unwrap();
    let snapshot = db.update_job_snapshot("job-test").unwrap();
    assert_eq!(snapshot.status, "paused");
}

#[test]
fn stopped_update_jobs_leave_their_next_item_queued() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "work".to_string(),
        mode: "auto_save".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };

    for status in ["paused", "canceling", "canceled"] {
        let job_id = format!("job-{status}");
        db.create_update_job(
            &job_id,
            &request,
            &[UpdateJobItemInput {
                item_type: "work".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some(status.to_string()),
                target_type: Some("work".to_string()),
                title: format!("{status} item"),
                payload_json: "{}".to_string(),
                status: "queued".to_string(),
            }],
        )
        .unwrap();
        db.set_update_job_status(&job_id, status, Some(status))
            .unwrap();

        assert!(db.next_update_job_item(&job_id).unwrap().is_none());
        let (job_status, item_status): (String, String) = db
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT j.status, i.status
                     FROM update_jobs j
                     JOIN update_job_items i ON i.job_id = j.id
                     WHERE j.id = ?1",
                params![job_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(job_status, status);
        assert_eq!(item_status, "queued");
    }
}

#[test]
fn active_update_job_claims_its_next_item_atomically() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "work".to_string(),
        mode: "auto_save".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job(
        "job-claim",
        &request,
        &[UpdateJobItemInput {
            item_type: "work".to_string(),
            source: Some("pixiv".to_string()),
            source_id: Some("claim".to_string()),
            target_type: Some("work".to_string()),
            title: "Claim me".to_string(),
            payload_json: "{}".to_string(),
            status: "queued".to_string(),
        }],
    )
    .unwrap();

    let item = db.next_update_job_item("job-claim").unwrap().unwrap();
    assert_eq!(item.status, "running");
    let snapshot = db.update_job_snapshot("job-claim").unwrap();
    assert_eq!(snapshot.status, "running");
    assert_eq!(snapshot.active_label.as_deref(), Some("Claim me"));
}

/// 履歴は放っておくと溜まり続ける。整理は「古くて終わったもの」だけに効き、
/// 走っているジョブと直近の履歴には触れない。
#[test]
fn old_finished_jobs_are_pruned_but_recent_and_live_ones_stay() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "all".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    for id in ["job-old", "job-recent", "job-live"] {
        db.create_update_job(id, &request, &[]).unwrap();
    }
    let long_ago = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE update_jobs SET status = 'completed', finished_at = ?1, updated_at = ?1
                 WHERE id = 'job-old'",
            params![long_ago],
        )
        .unwrap();
        conn.execute(
            "UPDATE update_jobs SET status = 'completed', finished_at = ?1, updated_at = ?1
                 WHERE id = 'job-recent'",
            params![chrono::Utc::now().to_rfc3339()],
        )
        .unwrap();
        // job-live は running のまま。日付が古くても消してはいけない。
        conn.execute(
            "UPDATE update_jobs SET status = 'running', updated_at = ?1 WHERE id = 'job-live'",
            params![long_ago],
        )
        .unwrap();
    }

    let removed = db.prune_update_jobs(1, 30).unwrap();
    assert_eq!(removed, 1);
    assert!(db.update_job_snapshot("job-old").is_err());
    assert!(db.update_job_snapshot("job-recent").is_ok());
    assert!(db.update_job_snapshot("job-live").is_ok());
}

#[test]
fn job_logs_are_capped_from_the_oldest_end() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "all".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job("job-logs", &request, &[]).unwrap();
    for index in 0..20 {
        db.append_update_job_log("job-logs", "info", &format!("行 {index}"))
            .unwrap();
    }
    // ジョブ作成時にも1行積まれるので、消えた数はそれ以上になる。
    assert!(db.trim_update_job_logs("job-logs", 5).unwrap() >= 15);
    let snapshot = db.update_job_snapshot("job-logs").unwrap();
    let messages: Vec<&str> = snapshot
        .logs
        .iter()
        .map(|log| log.message.as_str())
        .collect();
    assert_eq!(messages, vec!["行 15", "行 16", "行 17", "行 18", "行 19"]);
}

/// シリーズの作者が手元のライブラリから分かること。
///
/// 作者を監視しているならシリーズの新作もその一覧に出るので、両方を走査
/// しないための判断材料になる（同じものを二度取りに行かない）。
#[test]
fn a_series_can_name_the_author_behind_it() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let id = insert_download(
        &db,
        &storage,
        "novel-1",
        "夜明けの糸",
        "青葉しおり",
        &[],
        "本文",
    );
    {
        let conn = db.conn.lock().unwrap();
        conn.execute(
            "UPDATE downloads SET author_id = '7' WHERE id = ?1",
            params![id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO download_series (download_id, series_source, series_key, title)
                 VALUES (?1, 'pixiv', '778120', '星を編む人')",
            params![id],
        )
        .unwrap();
    }

    assert_eq!(
        db.series_author_keys("pixiv", "778120").unwrap(),
        vec!["7".to_string()]
    );
    // 手元に作品が無いシリーズは判断できない。そのときは走査する側に倒す。
    assert!(db.series_author_keys("pixiv", "999999").unwrap().is_empty());
}

/// 終わったジョブは操作履歴からまとめて消せる。走っているものは残る。
#[test]
fn finished_jobs_can_be_cleared_from_the_history() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "all".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job("job-done", &request, &[]).unwrap();
    db.create_update_job("job-running", &request, &[]).unwrap();
    db.set_update_job_status("job-done", "completed", None)
        .unwrap();
    db.set_update_job_status("job-running", "running", None)
        .unwrap();

    assert_eq!(db.clear_finished_update_jobs().unwrap(), 1);
    assert!(db.update_job_snapshot("job-done").is_err());
    assert!(db.update_job_snapshot("job-running").is_ok());
}

/// 見つけた候補は、ジョブが変わっても残る。
///
/// 取得元の一覧は「前回見た位置」から先しか返さないので、候補をジョブ限りに
/// すると、保存しなかった作品が二度と現れない。実際にそれで作品が消えた。
#[test]
fn a_candidate_nobody_answered_survives_into_the_next_job() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let candidate = UpdateCandidateInput {
        source: "pixiv".to_string(),
        source_id: "12345".to_string(),
        kind: "new".to_string(),
        title: "モリゾーの最新作".to_string(),
        payload_json: "{}".to_string(),
        target_type: Some("author".to_string()),
    };
    db.upsert_update_candidate(&candidate).unwrap();

    // 同じ作品を何度見つけても増えない。
    db.upsert_update_candidate(&candidate).unwrap();
    let pending = db.list_pending_update_candidates(100).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].title, "モリゾーの最新作");

    // 「今後は出さない」と決めたら候補から外れ、見つけ直しても戻らない。
    db.set_update_candidate_status("pixiv", "12345", "dismissed")
        .unwrap();
    db.upsert_update_candidate(&candidate).unwrap();
    assert!(db.list_pending_update_candidates(100).unwrap().is_empty());
    assert_eq!(
        db.update_candidate_status("pixiv", "12345")
            .unwrap()
            .as_deref(),
        Some("dismissed")
    );
    assert_eq!(db.count_dismissed_update_candidates().unwrap(), 1);

    // 決定は取り消せる。
    assert_eq!(db.restore_dismissed_update_candidates().unwrap(), 1);
    assert_eq!(db.list_pending_update_candidates(100).unwrap().len(), 1);

    // 保存できたら、もう決めていないものではない。
    db.clear_update_candidate("pixiv", "12345").unwrap();
    assert!(db.list_pending_update_candidates(100).unwrap().is_empty());
    assert!(db
        .update_candidate_status("pixiv", "12345")
        .unwrap()
        .is_none());
}

#[test]
fn restored_candidates_reject_untrusted_provider_fields() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let mut row = UpdateCandidateRow {
        source: "pixiv".to_string(),
        source_id: "12345".to_string(),
        kind: "new".to_string(),
        title: "候補".to_string(),
        payload_json: "{}".to_string(),
        target_type: Some("author".to_string()),
        status: "pending".to_string(),
        first_seen_at: "2026-08-28T00:00:00Z".to_string(),
        updated_at: "2026-08-28T00:00:00Z".to_string(),
    };
    db.restore_update_candidate(&row).unwrap();

    row.source = "https://evil.example".to_string();
    assert!(db.restore_update_candidate(&row).is_err());
    row.source = "pixiv".to_string();
    row.kind = "script".to_string();
    assert!(db.restore_update_candidate(&row).is_err());
    row.kind = "new".to_string();
    row.payload_json = "not json".to_string();
    assert!(db.restore_update_candidate(&row).is_err());
}

/// 進捗は「これから何件やるか」を表す。保存のために待機列へ入れた候補は
/// 作業なので数え、見つけただけの候補は数えない。
#[test]
fn queued_candidates_count_towards_progress_but_unanswered_ones_do_not() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "author".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job(
        "job-progress",
        &request,
        &[UpdateJobItemInput {
            item_type: "target".to_string(),
            source: Some("pixiv".to_string()),
            source_id: Some("7".to_string()),
            target_type: Some("author".to_string()),
            title: "作者".to_string(),
            payload_json: "{}".to_string(),
            status: "queued".to_string(),
        }],
    )
    .unwrap();
    for index in 0..3 {
        db.insert_update_job_candidate(
            "job-progress",
            &UpdateJobItemInput {
                item_type: "candidate".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some(format!("novel-{index}")),
                target_type: Some("author".to_string()),
                title: format!("候補 {index}"),
                payload_json: "{}".to_string(),
                status: "candidate".to_string(),
            },
        )
        .unwrap();
    }

    // 見つけただけの段階: 作業は対象1件のみ。
    let snapshot = db.update_job_snapshot("job-progress").unwrap();
    assert_eq!(snapshot.totals, 1);
    assert_eq!(snapshot.candidate_count, 3);

    // 保存を頼んだ2件は作業に加わる。ここで進捗が 1/1 のままだと、
    // 保存の最中に「完了」と見えてしまう。
    let ids: Vec<i64> = snapshot.candidates.iter().take(2).map(|c| c.id).collect();
    db.queue_update_job_candidates("job-progress", &ids)
        .unwrap();
    let snapshot = db.update_job_snapshot("job-progress").unwrap();
    assert_eq!(snapshot.totals, 3);
    assert_eq!(snapshot.processed, 0);
}

/// 監視対象の健康状態。「確認した」と「見つかった」は別で、失敗は積み上がる。
#[test]
fn a_target_records_when_it_last_found_something_and_when_it_keeps_failing() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    db.upsert_update_target(&UpdateTargetInput {
        target_type: "author".to_string(),
        source: "pixiv".to_string(),
        source_key: "7".to_string(),
        display_name: "青葉しおり".to_string(),
        enabled: true,
        metadata_json: None,
    })
    .unwrap();

    // 何も見つからなかった確認では、最後に見つけた時刻は空のまま。
    db.mark_update_target_checked("author", "pixiv", "7", None, None, 0)
        .unwrap();
    let target = db.list_update_targets(None, false).unwrap().remove(0);
    assert!(target.last_checked_at.is_some());
    assert!(target.last_hit_at.is_none());
    assert_eq!(target.consecutive_errors, 0);

    // 失敗は積み上がる。
    db.mark_update_target_failed("author", "pixiv", "7")
        .unwrap();
    db.mark_update_target_failed("author", "pixiv", "7")
        .unwrap();
    let target = db.list_update_targets(None, false).unwrap().remove(0);
    assert_eq!(target.consecutive_errors, 2);

    // 見つかった確認で、時刻が進み、連続失敗は 0 に戻る。
    db.mark_update_target_checked("author", "pixiv", "7", Some("12002"), None, 3)
        .unwrap();
    let target = db.list_update_targets(None, false).unwrap().remove(0);
    assert!(target.last_hit_at.is_some());
    assert_eq!(target.consecutive_errors, 0);
    assert_eq!(target.last_seen_source_id.as_deref(), Some("12002"));
}

#[test]
fn update_job_candidates_can_be_queued_for_saving() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "author".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job(
        "job-candidates",
        &request,
        &[UpdateJobItemInput {
            item_type: "target".to_string(),
            source: Some("pixiv".to_string()),
            source_id: Some("user-1".to_string()),
            target_type: Some("author".to_string()),
            title: "Author".to_string(),
            payload_json: "{}".to_string(),
            status: "done".to_string(),
        }],
    )
    .unwrap();
    let candidate = UpdateJobItemInput {
        item_type: "candidate".to_string(),
        source: Some("pixiv".to_string()),
        source_id: Some("novel-1".to_string()),
        target_type: Some("author".to_string()),
        title: "Novel".to_string(),
        payload_json: serde_json::json!({
            "targetLabel": "Author",
            "subtitle": "Author / now"
        })
        .to_string(),
        status: "candidate".to_string(),
    };
    assert!(db
        .insert_update_job_candidate("job-candidates", &candidate)
        .unwrap());
    assert!(!db
        .insert_update_job_candidate("job-candidates", &candidate)
        .unwrap());
    let snapshot = db.update_job_snapshot("job-candidates").unwrap();
    assert_eq!(snapshot.candidate_count, 1);
    assert_eq!(snapshot.candidates.len(), 1);
    let candidate_id = snapshot.candidates[0].id;
    let changed = db
        .queue_update_job_candidates("job-candidates", &[candidate_id])
        .unwrap();
    assert_eq!(changed, 1);
    let snapshot = db.update_job_snapshot("job-candidates").unwrap();
    assert_eq!(snapshot.candidates[0].status, "queued");
}

/// v0.11.0 のまとめ保存ジョブは target_type を NULL で記録していた。
/// アップデート後も、そのジョブを一覧表示して再開できる。
#[test]
fn update_job_snapshot_reads_legacy_save_candidates_without_target_type() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "save".to_string(),
        mode: "save".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job(
        "job-legacy-save",
        &request,
        &[UpdateJobItemInput {
            item_type: "candidate".to_string(),
            source: Some("pixiv".to_string()),
            source_id: Some("novel-1".to_string()),
            target_type: None,
            title: "Legacy save".to_string(),
            payload_json: serde_json::json!({ "kind": "save" }).to_string(),
            status: "queued".to_string(),
        }],
    )
    .unwrap();

    let snapshot = db.update_job_snapshot("job-legacy-save").unwrap();
    assert_eq!(snapshot.candidates.len(), 1);
    assert_eq!(snapshot.candidates[0].target_type, "work");
}

#[test]
fn update_job_snapshot_pages_candidate_payloads() {
    let (_temp, root, storage) = temp_paths();
    let db = Database::open(&root.join("piep.db"), &storage).unwrap();
    let request = StartUpdateJobRequest {
        scope: "author".to_string(),
        mode: "check_only".to_string(),
        work_ids: None,
        target_ids: None,
        credentials: None,
        watch_saved: None,
        adhoc_targets: None,
    };
    db.create_update_job("job-paged", &request, &[]).unwrap();
    for index in 0..205 {
        db.insert_update_job_candidate(
            "job-paged",
            &UpdateJobItemInput {
                item_type: "candidate".to_string(),
                source: Some("pixiv".to_string()),
                source_id: Some(format!("novel-{index}")),
                target_type: Some("author".to_string()),
                title: format!("Novel {index}"),
                payload_json: "{}".to_string(),
                status: "candidate".to_string(),
            },
        )
        .unwrap();
    }

    let first = db.update_job_snapshot("job-paged").unwrap();
    assert_eq!(first.candidates.len(), 200);
    let cursor = first.next_candidate_cursor.expect("next page");
    let second = db
        .update_job_snapshot_page("job-paged", Some(cursor), None)
        .unwrap();
    assert_eq!(second.candidates.len(), 5);
    assert!(second.next_candidate_cursor.is_none());
    assert!(second.candidates[0].id > cursor);
}
