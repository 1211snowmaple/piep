use super::tests::{key, summary_json};
use super::*;

/// 棚から受け取ったつもりで、作業領域に一式を組む。`pages` は各ページの本文。
fn fill(staging: &Path, pages: &[&str]) -> Result<Manifest, ClientError> {
    fs::create_dir(staging.join("pages")).map_err(|_| storage())?;
    fs::create_dir(staging.join("assets")).map_err(|_| storage())?;
    let mut records = Vec::new();
    for (index, text) in pages.iter().enumerate() {
        let page = MobilePage {
            page: index,
            page_count: pages.len(),
            html: format!("<p>{text}</p>"),
            total_plain_text_chars: 2,
            source_page_starts: vec![0],
            revision: None,
        };
        let bytes = write_json(&staging.join("pages").join(format!("{index}.json")), &page)?;
        records.push(PageRecord {
            page: index,
            sha256: digest(&bytes),
        });
    }
    let manifest = Manifest {
        summary: serde_json::from_value(summary_json(&key(3), "3", "作品", pages.len())).unwrap(),
        pages: records,
        assets: Vec::new(),
        cover: None,
        author_cover: None,
        series_cover: None,
        collection_covers: Vec::new(),
        outline_sha256: None,
    };
    write_json(&staging.join("manifest.json"), &manifest)?;
    Ok(manifest)
}

async fn save_pages(state: &OfflineState, pages: &[&str]) -> Result<OfflineWork, ClientError> {
    save_snapshot(
        state,
        &key(3),
        |staging| async move { fill(&staging, pages) },
    )
    .await
}

fn assert_original_is_readable(state: &OfflineState, original: &Path) {
    assert_eq!(read_manifest(state, &key(3)).unwrap().1, original);
    for index in 0..2 {
        assert!(
            read_offline_page(state, &key(3), index)
                .unwrap()
                .page
                .html
                .contains(&format!("page-{index}"))
        );
    }
}

#[test]
fn failed_updates_keep_the_published_pages_readable() {
    tauri::async_runtime::block_on(async {
        let root = std::env::temp_dir().join(format!("piep-offline-revisions-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        save_pages(&state, &["page-0", "page-1"]).await.unwrap();
        let original = read_manifest(&state, &key(3)).unwrap().1;
        // 途中まで組んでから失敗した保存は、公開中の版を置き換えず、作業領域も残さない。
        let failed = save_snapshot(&state, &key(3), |staging| async move {
            fill(&staging, &["changed-0", "changed-1"])?;
            Err::<Manifest, _>(ClientError::remote())
        })
        .await;
        assert!(failed.is_err());
        assert_original_is_readable(&state, &original);
        assert_eq!(
            fs::read_dir(state.work_dir(&key(3)).unwrap())
                .unwrap()
                .count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn reading_during_a_save_and_aborting_it_keep_the_published_snapshot() {
    tauri::async_runtime::block_on(async {
        let root = std::env::temp_dir().join(format!("piep-offline-abort-{}", Uuid::new_v4()));
        let state = Arc::new(OfflineState::new(root.clone()));
        save_pages(&state, &["page-0", "page-1"]).await.unwrap();
        let original = read_manifest(&state, &key(3)).unwrap().1;
        let (sent, received) = std::sync::mpsc::channel();
        let saving = {
            let state = state.clone();
            tauri::async_runtime::spawn(async move {
                save_snapshot(&state, &key(3), |staging| async move {
                    fill(&staging, &["changed-0", "changed-1"])?;
                    // 受け取りの途中で止まったまま中止される。
                    sent.send(()).unwrap();
                    std::future::pending::<Result<Manifest, ClientError>>().await
                })
                .await
            })
        };
        received.recv_timeout(Duration::from_secs(5)).unwrap();
        {
            let _read = state.lock.lock().await;
            assert_original_is_readable(&state, &original);
        }
        saving.abort();
        assert!(saving.await.is_err());
        assert_original_is_readable(&state, &original);
        drop(state);
        // 中止で残った作業領域は、次に開いたときに回収する。
        let reopened = OfflineState::new(root.clone());
        assert_original_is_readable(&reopened, &original);
        assert_eq!(
            fs::read_dir(reopened.work_dir(&key(3)).unwrap())
                .unwrap()
                .count(),
            2
        );
        fs::remove_dir_all(root).unwrap();
    });
}

#[test]
fn saved_bodies_are_searchable_offline_and_follow_saves_and_deletes() {
    tauri::async_runtime::block_on(async {
        let root = std::env::temp_dir().join(format!("piep-offline-search-{}", Uuid::new_v4()));
        let state = OfflineState::new(root.clone());
        save_pages(&state, &["page-0", "page-1"]).await.unwrap();

        // 保存した時点で索引に入っている。3 文字以上は索引で、短い語は順に探す。
        let hits = super::index::search(&state, "page-1").unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(serde_json::to_value(&hits[0]).unwrap()["page"], 1);
        let both = super::index::search(&state, "pa").unwrap();
        assert_eq!(serde_json::to_value(&both[0]).unwrap()["pages"], 2);
        assert!(
            super::index::search(&state, "page-1 missing")
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            super::index::search(&state, "PAGE-0 ge-1").unwrap().len(),
            1
        );

        // 索引が失われても、検索の前に保存版から作り直す。
        state
            .db
            .transaction(|tx| {
                tx.execute_batch("DELETE FROM offline_text; DELETE FROM offline_catalog;")
                    .map_err(super::super::device_db::records_error)
            })
            .unwrap();
        assert!(super::index::search(&state, "page-1").unwrap().is_empty());
        super::index::reconcile(&state).unwrap();
        assert_eq!(super::index::search(&state, "page-1").unwrap().len(), 1);

        delete_saved_work(&state, &key(3), "pixiv", "3").unwrap();
        assert!(super::index::search(&state, "page").unwrap().is_empty());
        fs::remove_dir_all(root).unwrap();
    });
}
