use super::*;
use crate::library::{
    Capture, EntityKind, TagProvenance, TagUpdate, UserTag, WorkListQuery, WorkSort,
};
use crate::library_search::{SearchMode, TextSearch, VersionScope};
use serde_json::json;
use std::collections::BTreeMap;

struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        crate::database::tantivy_index::release_runtime(&self.0.join("derived"));
        crate::database::tantivy_index::release_runtime(&self.0.join("derived").join("history"));
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp(name: &str) -> Temp {
    Temp(std::env::temp_dir().join(format!("piep-{name}-{}", LibraryId::now_v7())))
}

fn work(key: &str, body: &str, acquired: i64) -> Capture {
    Capture {
        acquired_at: Some(acquired),
        request_id: LibraryId::now_v7(),
        provider: "pixiv".into(),
        external_key: key.into(),
        kind: EntityKind::Work,
        route: "test".into(),
        raw: serde_json::to_vec(&json!({"key": key, "body": body}))
            .unwrap()
            .into(),
        metadata: BTreeMap::from([
            ("title".into(), json!(format!("題{key}"))),
            ("author_name".into(), json!("作者")),
            ("caption".into(), json!("")),
            ("tags".into(), json!(["物語"])),
        ]),
        field_states: BTreeMap::new(),
        body: Some(body.into()),
        media: vec![],
        attachments: vec![],
    }
}

fn never() -> bool {
    false
}

fn relevance() -> WorkListQuery {
    WorkListQuery {
        sort: WorkSort::Relevance,
        ..Default::default()
    }
}

#[test]
fn meaning_search_ranks_by_similarity_and_reembeds_only_what_changed() {
    let temp = temp("semantic");
    let library = Library::create(&temp.0).unwrap();
    let sea = library
        .capture(work("1", "海と波と砂浜", 1_000))
        .unwrap()
        .entity_id;
    let mountain = library
        .capture(work("2", "山と森と川", 2_000))
        .unwrap()
        .entity_id;
    let colour = library
        .capture(work("3", "海の色", 3_000))
        .unwrap()
        .entity_id;
    library.sync_search_index(&mut |_, _| {}).unwrap();

    // 意味検索の索引がなければ、空の結果でなく「使えない」と返す。
    assert!(matches!(
        library.search_works(&TextSearch::meaning("海"), &relevance()),
        Err(Error::ResourceUnavailable)
    ));
    let first = library
        .sync_semantic_index(None, &never, &mut |_, _| {})
        .unwrap();
    assert_eq!((first.indexed, first.skipped, first.removed), (3, 0, 0));
    assert_eq!((first.reused_metadata, first.reused_content), (0, 0));
    assert_eq!(first.embedded_passages, 6, "説明と本文が一断片ずつ");
    assert_eq!(
        library.semantic_index_state().unwrap(),
        SemanticIndexState {
            indexed: 3,
            works: 3
        }
    );
    // 変わっていなければ何もしない。
    assert_eq!(
        library
            .sync_semantic_index(None, &never, &mut |_, _| {})
            .unwrap(),
        SemanticIndexReport::default()
    );

    let found = library
        .search_works(&TextSearch::meaning("海"), &relevance())
        .unwrap();
    let ids: Vec<LibraryId> = found.page.items.iter().map(|c| c.id).collect();
    assert!(ids.contains(&sea) && ids.contains(&colour));
    assert!(ids
        .iter()
        .position(|id| *id == mountain)
        .is_none_or(|p| p == 2));
    assert_eq!(
        found.matches.iter().map(|m| m.work).collect::<Vec<_>>(),
        ids
    );
    assert!(found.matches.windows(2).all(|p| p[0].score >= p[1].score));
    for hit in &found.matches {
        let evidence = hit.semantic.as_ref().expect("意味検索の根拠");
        assert_eq!(hit.score, evidence.score);
        assert!(evidence.score >= 0.18);
    }
    // 根拠は理由と抜粋の先頭に出る。箇所は示さない。
    let marks = library.search_decorations("海", &found.matches).unwrap();
    let mark = marks.iter().find(|m| m.work == sea).unwrap();
    assert!(mark
        .score_reasons
        .iter()
        .any(|r| r.match_type == "semantic"));
    assert_eq!(mark.highlights[0].match_type.as_deref(), Some("semantic"));
    assert!(mark.highlights[0].segments.iter().all(|s| !s.matched));

    // 除く語は語の一致で除く。
    let excluded = library
        .search_works(&TextSearch::meaning("海 -波"), &relevance())
        .unwrap();
    assert!(!excluded.page.items.iter().any(|c| c.id == sea));
    assert!(excluded.page.items.iter().any(|c| c.id == colour));
    // 過去の改稿も探すときは語の一致にする。
    let words = TextSearch {
        text: "海".into(),
        versions: VersionScope::All,
        mode: SearchMode::Meaning,
    };
    let page = library.search_works(&words, &relevance()).unwrap();
    assert!(page.matches.iter().all(|m| m.semantic.is_none()));
    assert_eq!(page.page.total, Some(2));

    // タグを付け替えた作品は、説明の断片だけを埋め込み直す。
    library
        .replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work: mountain,
            user: library.id,
            expected_generation: 0,
            tags: vec![UserTag {
                name: "登山".into(),
                provenance: TagProvenance::Manual,
            }],
        })
        .unwrap();
    let retagged = library
        .sync_semantic_index(None, &never, &mut |_, _| {})
        .unwrap();
    assert_eq!(retagged.indexed, 1);
    assert_eq!((retagged.reused_content, retagged.reused_metadata), (1, 0));
    assert_eq!(retagged.embedded_passages, 1);

    // 止めたら、それまでの分だけ残る。
    library.capture(work("2", "山と森と川と湖", 4_000)).unwrap();
    let canceled = library
        .sync_semantic_index(None, &|| true, &mut |_, _| {})
        .unwrap();
    assert_eq!((canceled.indexed, canceled.canceled), (0, true));
    let resumed = library
        .sync_semantic_index(None, &never, &mut |_, _| {})
        .unwrap();
    assert_eq!((resumed.indexed, resumed.reused_metadata), (1, 1));
}

#[test]
fn vectors_from_the_old_shelf_are_reused_when_the_text_is_the_same() {
    let temp = temp("semantic-takeover");
    let library = Library::create(&temp.0.join("new")).unwrap();
    let same = library
        .capture(work("1", "海と波と砂浜", 1_000))
        .unwrap()
        .entity_id;
    let retold = library
        .capture(work("2", "山と森と川", 2_000))
        .unwrap()
        .entity_id;
    let fresh = library.capture(work("3", "空", 3_000)).unwrap().entity_id;
    library.sync_search_index(&mut |_, _| {}).unwrap();

    // 旧棚の索引を作る。1 は同じ文字列、2 は本文だけ同じ（説明が違う）、3 は旧棚にない。
    let old = temp.0.join("old").join("downloads");
    let doc = |work: LibraryId, id: i64| {
        semantic_document(id, library.search_document(work).unwrap().unwrap())
    };
    let mut changed = doc(retold, 20);
    changed.title = "昔の題".into();
    semantic_index::upsert_documents(&old, &[doc(same, 10), changed]).unwrap();
    {
        let conn = library.conn.lock().unwrap();
        for (work, old_id) in [(same, 10), (retold, 20)] {
            let snapshot: Vec<u8> = conn
                .query_row(
                    "SELECT snapshot_id FROM work_current_projection WHERE work_id=?1",
                    [work.as_bytes().as_slice()],
                    |r| r.get(0),
                )
                .unwrap();
            conn.execute(
                "INSERT INTO migration_records(source_table,source_key,target_id,status)
                 VALUES('download_current',?1,?2,'imported')",
                params![old_id.to_string(), snapshot],
            )
            .unwrap();
        }
    }
    let report = library
        .sync_semantic_index(Some(&old), &never, &mut |_, _| {})
        .unwrap();
    assert_eq!(report.indexed, 3);
    assert_eq!(report.reused_content, 2);
    assert_eq!(report.reused_metadata, 1);
    assert_eq!(
        report.embedded_passages,
        1 + 2,
        "2 の説明と、3 の説明と本文"
    );

    // 写したベクトルで、埋め込み直したときと同じ近さになる。
    let found = library
        .search_works(&TextSearch::meaning("海"), &relevance())
        .unwrap();
    let rebuilt = temp.0.join("rebuilt").join("downloads");
    semantic_index::upsert_documents(&rebuilt, &[doc(same, 10)]).unwrap();
    let expected = semantic_index::search(&rebuilt, "海", 10).unwrap();
    let hit = found.matches.iter().find(|m| m.work == same).unwrap();
    assert!((hit.score - expected[0].score).abs() < 1e-6);
    let _ = fresh;
}
