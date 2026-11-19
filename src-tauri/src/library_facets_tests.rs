use super::*;
use crate::library::{LibraryId, TagProvenance, TagUpdate, UserTag, WorkSave};

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shelf() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-facets-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Temp(root), library)
}

fn save(library: &Library, key: &str, author: &str, tags: &[&str], kind: &str) -> LibraryId {
    let tags: Vec<_> = tags
        .iter()
        .map(|t| serde_json::json!({ "name": t }))
        .collect();
    let mut detail = serde_json::json!({
        "title": format!("作品{key}"), "user": {"id": key, "name": author}, "tags": tags,
    });
    if kind == "illust" {
        detail["illustType"] = serde_json::json!(0);
    }
    library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "pixiv",
            key,
            route: "test",
            data: &serde_json::json!({ "detail": detail, "text": "本文" }),
            assets: vec![],
        })
        .unwrap()
        .entity_id
}

fn names(facets: &[FacetCount]) -> Vec<(&str, i64)> {
    facets.iter().map(|f| (f.name.as_str(), f.count)).collect()
}

#[test]
fn the_drawer_counts_tags_with_my_own_and_authors_by_name() {
    let (_temp, library) = shelf();
    let a = save(&library, "1", "甲", &["雨", "夏"], "novel");
    save(&library, "2", "甲", &["雨"], "novel");
    save(&library, "3", "乙", &["秋"], "novel");
    // 自分で付けたタグも、同じ名前なら一つに数える（同じ作品は一度）。
    library
        .replace_user_tags(TagUpdate {
            request_id: LibraryId::now_v7(),
            work: a,
            user: library.id,
            expected_generation: 0,
            tags: vec![
                UserTag {
                    name: "雨".into(),
                    provenance: TagProvenance::Manual,
                },
                UserTag {
                    name: "宝物".into(),
                    provenance: TagProvenance::Manual,
                },
            ],
        })
        .unwrap();

    let facets = library.filter_facets().unwrap();
    assert_eq!(
        names(&facets.tags),
        vec![("雨", 2), ("夏", 1), ("宝物", 1), ("秋", 1)]
    );
    assert_eq!(names(&facets.authors), vec![("甲", 2), ("乙", 1)]);
    assert_eq!(facets.authors[0].sources, vec!["pixiv".to_string()]);
    assert_eq!(names(&facets.content_types), vec![("novel", 3)]);
    assert!(facets.asset_types.is_empty());
}

#[test]
fn searching_puts_the_closest_names_first() {
    let (_temp, library) = shelf();
    save(&library, "1", "甲", &["雨上がり", "夏"], "novel");
    save(&library, "2", "甲", &["雨"], "novel");
    save(&library, "3", "乙", &["小雨", "雨"], "novel");

    let found = library
        .search_filter_facets(FacetKind::Tags, Some("雨"), 2)
        .unwrap();
    assert_eq!(names(&found), vec![("雨", 2), ("雨上がり", 1)]);
    let authors = library
        .search_filter_facets(FacetKind::Authors, Some("乙"), 5)
        .unwrap();
    assert_eq!(names(&authors)[0], ("乙", 1));
    // 問いが空なら多い順。
    let popular = library
        .search_filter_facets(FacetKind::Tags, None, 1)
        .unwrap();
    assert_eq!(names(&popular), vec![("雨", 2)]);
    assert!(FacetKind::parse("series").is_err());
}

#[test]
fn a_link_finds_the_listed_work_by_its_source() {
    let (_temp, library) = shelf();
    let work = save(&library, "41", "甲", &[], "novel");
    let found = library.find_listed_work("pixiv", "41").unwrap();
    assert_eq!(found.map(|(id, _)| id), Some(work));
    assert!(library.find_listed_work("pixiv", "42").unwrap().is_none());
    assert!(library.find_listed_work("fanbox", "41").unwrap().is_none());
}
