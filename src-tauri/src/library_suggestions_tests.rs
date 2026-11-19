use super::*;
use crate::database::queries::SuggestionSource;
use crate::library::WorkSave;

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shelf() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-suggest-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Temp(root), library)
}

/// 同じ作者の pixiv の小説を保存し、保存の後と同じく本文リンクを取り直す。
fn save(library: &Library, key: &str, title: &str, text: &str) -> LibraryId {
    let saved = library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key,
            route: "test",
            data: &serde_json::json!({
                "detail": {"title": title, "user": {"id": 77, "name": "試験の作者"},
                           "tags": {"tags": [{"tag": "雨"}, {"tag": "連作"}]}},
                "text": text,
            }),
            assets: vec![],
        })
        .unwrap();
    library.refresh_shelf_work_links(saved.entity_id).unwrap();
    saved.entity_id
}

fn next_link(key: &str) -> String {
    format!("本文です。\n次の話 https://www.pixiv.net/novel/show.php?id={key}\n")
}

/// 前編 → 中編 → 後編と、本文のリンクでつながった三作。
fn chain(library: &Library) -> [LibraryId; 3] {
    [
        save(library, "101", "雨の話 前編", &next_link("102")),
        save(library, "102", "雨の話 中編", &next_link("103")),
        save(library, "103", "雨の話 後編", "おしまい。"),
    ]
}

fn links_from(library: &Library, work: LibraryId) -> Vec<(LibraryId, String)> {
    let conn = library.reader().unwrap();
    let mut stmt = conn
        .prepare(
            "SELECT l.to_work_id,l.status FROM work_links l WHERE l.from_work_id=?1
             AND EXISTS(SELECT 1 FROM link_evidence e WHERE e.link_id=l.id)",
        )
        .unwrap();
    let rows = stmt
        .query_map([work.as_bytes().as_slice()], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
        })
        .unwrap()
        .map(|row| {
            let (to, status) = row.unwrap();
            (LibraryId::from_slice(&to).unwrap(), status)
        })
        .collect();
    rows
}

#[test]
fn saving_a_work_records_the_links_in_its_body_and_drops_them_when_they_go() {
    let (_temp, library) = shelf();
    let [first, second, _] = chain(&library);
    assert_eq!(
        links_from(&library, first),
        vec![(second, "observed".to_string())]
    );

    // 改稿でリンクが消えたら、本文から拾った関係も消える。
    let again = save(&library, "101", "雨の話 前編", "リンクのない本文。");
    assert_eq!(again, first);
    assert!(links_from(&library, first).is_empty());
    let left: i64 = library
        .reader()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM work_links WHERE from_work_id=?1",
            [first.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(left, 0, "根拠の無くなった関係は残さない");
}

#[test]
fn links_to_works_not_on_the_shelf_point_at_a_placeholder_work() {
    let (_temp, library) = shelf();
    let first = save(&library, "201", "一話", &next_link("999"));
    let links = links_from(&library, first);
    assert_eq!(links.len(), 1);
    let corpus = ShelfSuggestions::load(&library).unwrap();
    // 棚に無い作品は作業用の表の作品にならない（リンクだけが残る）。
    assert!(corpus.id_of(links[0].0).is_none());
    assert!(corpus.id_of(first).is_some());
}

#[test]
fn a_seed_suggestion_follows_the_body_links_and_accepting_it_makes_a_collection() {
    let (_temp, library) = shelf();
    let works = chain(&library);
    let suggestion = library
        .seed_collection_suggestion(&works[..1], None)
        .unwrap();
    let found: Vec<_> = suggestion
        .members
        .iter()
        .map(|member| member.work.clone().unwrap().parse::<LibraryId>().unwrap())
        .collect();
    for work in &works {
        assert!(found.contains(work), "{work} が候補に無い: {found:?}");
    }
    assert!(suggestion
        .members
        .iter()
        .all(|member| member.download_id.is_none()));
    assert_eq!(suggestion.state, "pending");

    let pending = library
        .collection_suggestion_views(Some("pending"))
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, suggestion.id);

    let collection = library
        .accept_collection_suggestion_view(&AcceptCollectionSuggestionInput {
            suggestion_id: suggestion.id.clone(),
            name: Some("雨の連作".to_string()),
            collection_kind: None,
            member_keys: None,
        })
        .unwrap();
    let detail = library.collection_detail(collection).unwrap();
    assert_eq!(detail.summary.name, "雨の連作");
    assert_eq!(detail.members.len(), works.len());
    assert!(library
        .collection_suggestion_views(Some("pending"))
        .unwrap()
        .is_empty());
    let accepted = library
        .collection_suggestion_views(Some("accepted"))
        .unwrap();
    assert_eq!(accepted.len(), 1);
    // 採った候補は、もう一度は採れない。
    assert!(library
        .accept_collection_suggestion_view(&AcceptCollectionSuggestionInput {
            suggestion_id: suggestion.id,
            name: None,
            collection_kind: None,
            member_keys: None,
        })
        .is_err());
}

#[test]
fn rejecting_remembers_the_pairs_and_dismissing_remembers_nothing() {
    let (_temp, library) = shelf();
    let works = chain(&library);
    let first = library
        .seed_collection_suggestion(&works[..1], None)
        .unwrap();
    assert!(library
        .dismiss_collection_suggestion_view(&first.id)
        .unwrap());
    assert!(!library
        .dismiss_collection_suggestion_view(&first.id)
        .unwrap());
    let pairs = |library: &Library| -> i64 {
        library
            .reader()
            .unwrap()
            .query_row("SELECT count(*) FROM collection_pair_feedback", [], |r| {
                r.get(0)
            })
            .unwrap()
    };
    assert_eq!(pairs(&library), 0);

    let second = library
        .seed_collection_suggestion(&works[..1], None)
        .unwrap();
    let keys: Vec<WorkKey> = second
        .members
        .iter()
        .map(|member| WorkKey {
            source: member.source.clone(),
            source_id: member.source_id.clone(),
        })
        .collect();
    assert!(library
        .reject_collection_suggestion_view(&second.id, Some(&keys))
        .unwrap());
    assert!(pairs(&library) > 0);
    assert_eq!(
        library
            .collection_suggestion_views(Some("rejected"))
            .unwrap()
            .len(),
        1
    );
    assert!(!library
        .reject_collection_suggestion_view(&second.id, None)
        .unwrap());
}

#[test]
fn sweeping_the_shelf_finds_the_linked_works_and_closing_drops_only_the_swept_drafts() {
    let (_temp, library) = shelf();
    let works = chain(&library);
    let seed = library
        .seed_collection_suggestion(&works[..1], None)
        .unwrap();
    let result = library.sweep_collection_suggestions().unwrap();
    let swept: Vec<_> = result
        .bundles
        .iter()
        .filter(|bundle| bundle.origin == "sweep")
        .collect();
    assert!(
        swept.iter().any(|bundle| bundle.members.len() >= 2),
        "リンクでつながった作品が束にならない: {:?}",
        result
            .bundles
            .iter()
            .map(|b| b.members.len())
            .collect::<Vec<_>>()
    );
    assert!(swept
        .iter()
        .flat_map(|bundle| &bundle.members)
        .all(|member| member.work.is_some()));
    // 走査し直しても同じ候補が二重にならない。
    let again = library.sweep_collection_suggestions().unwrap();
    assert_eq!(
        again.bundles.iter().filter(|b| b.origin == "sweep").count(),
        swept.len()
    );
    assert_eq!(
        library.dismiss_swept_collection_suggestions().unwrap(),
        swept.len()
    );
    let left = library
        .collection_suggestion_views(Some("pending"))
        .unwrap();
    assert_eq!(left.len(), 1, "種から作った候補は残る");
    assert_eq!(left[0].id, seed.id);
}

#[test]
fn additions_and_names_are_proposed_from_the_collection_on_the_shelf() {
    let (_temp, library) = shelf();
    let works = chain(&library);
    let collection = LibraryId::now_v7();
    library
        .edit_collection(
            LibraryId::now_v7(),
            collection,
            CollectionEdit::Create {
                title: "雨の束".to_string(),
                description: None,
                ordered: true,
                works: works[..2].to_vec(),
            },
        )
        .unwrap();
    let additions = library.collection_additions_on_shelf(collection).unwrap();
    for candidate in &additions.candidates {
        let work = candidate
            .work
            .as_deref()
            .unwrap()
            .parse::<LibraryId>()
            .unwrap();
        assert!(!works[..2].contains(&work), "入っている作品は候補にしない");
        assert_eq!(candidate.download_id, 0);
    }
    assert!(
        additions
            .candidates
            .iter()
            .any(|candidate| candidate.work.as_deref() == Some(works[2].to_string().as_str())),
        "中編からリンクされた後編が候補に無い"
    );

    let naming = library
        .collection_naming_works_on_shelf(collection)
        .unwrap();
    assert_eq!(naming.len(), 2);
    let names = library
        .collection_name_proposals_on_shelf(collection)
        .unwrap();
    assert!(
        names.iter().any(|name| name.name == "雨の束"),
        "今の名前も案に残す"
    );
    assert!(!library.sample_naming_works_on_shelf().is_err());
}

#[test]
fn the_working_tables_see_the_shelf_text_and_have_no_old_shelf_to_backfill() {
    let (_temp, library) = shelf();
    let [first, ..] = chain(&library);
    let corpus = ShelfSuggestions::load(&library).unwrap();
    let id = corpus.id_of(first).unwrap();
    assert_eq!(corpus.work(id), Some(first));
    assert!(corpus.plain_text(id).unwrap().contains("本文です"));
    assert!(corpus.link_backfill().is_none());
    let downloads: i64 = corpus
        .corpus()
        .unwrap()
        .query_row("SELECT count(*) FROM downloads", [], |r| r.get(0))
        .unwrap();
    assert_eq!(downloads, 3);
    let links: i64 = corpus
        .corpus()
        .unwrap()
        .query_row("SELECT count(*) FROM work_links", [], |r| r.get(0))
        .unwrap();
    assert_eq!(links, 2);
}
