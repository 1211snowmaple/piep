use super::*;
use crate::library::{LibraryId, UserStateChange, WorkSave};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shelf() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-health-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Temp(root), library)
}

fn save(library: &Library, key: &str, author: &str, tags: &[&str], text: &str) -> LibraryId {
    let tags: Vec<_> = tags
        .iter()
        .map(|t| serde_json::json!({ "name": t }))
        .collect();
    library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key,
            route: "test",
            data: &serde_json::json!({
                "detail": {"title": format!("作品{key}"), "user": {"id": 7, "name": author},
                           "tags": tags},
                "text": text,
            }),
            assets: vec![],
        })
        .unwrap()
        .entity_id
}

fn favorite(library: &Library, work: LibraryId) {
    library
        .set_user_state(
            LibraryId::now_v7(),
            work,
            library.id,
            0,
            UserStateChange {
                favorite: Some(true),
                watching: None,
            },
        )
        .unwrap();
}

#[test]
fn counts_and_the_dashboard_follow_the_current_versions() {
    let (_temp, library) = shelf();
    let a = save(&library, "1", "甲", &["雨", "夏"], "一行目");
    save(&library, "2", "甲", &["雨"], "二行目です");
    save(&library, "3", "乙", &[], "三");
    favorite(&library, a);

    let counts = library.shelf_counts().unwrap();
    assert_eq!(
        (
            counts.total,
            counts.favorite,
            counts.watched,
            counts.reading,
            counts.revised
        ),
        (3, 1, 0, 0, 0)
    );

    let stats = library.shelf_stats().unwrap();
    assert_eq!(
        (stats.total_works, stats.pixiv_count, stats.fanbox_count),
        (3, 3, 0)
    );

    library.sync_search_index(&mut |_, _| {}).unwrap();
    let index = library.search_index_status(false).unwrap();
    assert_eq!((index.total_works, index.pending_works), (3, 0));
    assert!(index.is_complete && !index.semantic_enabled);

    let summary = library.dashboard(&index).unwrap();
    assert_eq!(summary.favorite_count, 1);
    let tags: Vec<_> = summary
        .top_tags
        .iter()
        .map(|t| (t.name.as_str(), t.count))
        .collect();
    assert_eq!(tags, vec![("雨", 2), ("夏", 1)]);
    let authors: Vec<_> = summary
        .top_authors
        .iter()
        .map(|t| (t.name.as_str(), t.count))
        .collect();
    assert_eq!(authors, vec![("甲", 2), ("乙", 1)]);
    assert_eq!(summary.source_breakdown.len(), 1);
    assert_eq!(
        summary.monthly_saves.iter().map(|m| m.count).sum::<i64>(),
        3
    );
}

#[test]
fn authors_never_fetched_are_listed_once_for_profile_repair() {
    let (_temp, library) = shelf();
    save(&library, "1", "甲", &[], "一");
    save(&library, "2", "甲", &[], "二");
    let rows = library.incomplete_profiles().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (
            rows[0].entity_type.as_str(),
            rows[0].source.as_str(),
            rows[0].source_key.as_str(),
            rows[0].display_name.as_str()
        ),
        ("person", "pixiv", "7", "甲")
    );
    let status = library.profile_repair_status().unwrap();
    assert_eq!(
        (status.person_count, status.series_count, status.total_count),
        (1, 0, 1)
    );
}

#[test]
fn an_unindexed_work_is_pending_until_the_index_catches_up() {
    let (_temp, library) = shelf();
    save(&library, "1", "甲", &[], "本文");
    library.sync_search_index(&mut |_, _| {}).unwrap();
    save(&library, "2", "甲", &[], "本文");
    let status = library.search_index_status(false).unwrap();
    assert_eq!((status.indexed_works, status.pending_works), (1, 1));
    assert!(!status.is_complete);
}

#[test]
fn diagnostics_measure_the_ledger_and_maintenance_keeps_it_readable() {
    let (_temp, library) = shelf();
    save(&library, "1", "甲", &["雨"], "あいうえお");
    library.sync_search_index(&mut |_, _| {}).unwrap();
    let index = library.search_index_status(false).unwrap();
    let report = library.diagnostics(index).unwrap();
    assert_eq!(
        (report.works, report.versions, report.text_length),
        (1, 1, 5)
    );
    assert!(report.files > 0 && report.logical_bytes > 0 && report.stored_bytes > 0);
    assert!(report.database_bytes > 0);
    assert!(report.lexical_index_bytes > 0);

    let result = library.maintain_database(true).unwrap();
    assert!(result.compacted);
    let optimized = library.optimize_search_index().unwrap();
    assert!(optimized.after_segments <= optimized.before_segments.max(1));
    assert!(library.verify().unwrap() > 0);
    assert_eq!(library.count_works(WorkFilter::default()).unwrap(), 1);
}
