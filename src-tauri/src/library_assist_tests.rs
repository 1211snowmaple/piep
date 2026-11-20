use super::*;
use crate::assist::FEATURE_WORK_SYNOPSIS;
use crate::library::WorkSave;

struct Temp(std::path::PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shelf() -> (Temp, Library) {
    let root = std::env::temp_dir().join(format!("piep-assist-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Temp(root), library)
}

fn save(library: &Library, key: &str, author: (&str, &str), tags: &[&str]) -> LibraryId {
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
                "detail": {
                    "title": format!("作品{key}"),
                    "user": {"id": author.0, "name": author.1},
                    "tags": tags,
                },
                "text": format!("本文{key}"),
            }),
            assets: vec![],
        })
        .unwrap()
        .entity_id
}

fn tags(library: &Library, work: LibraryId) -> Vec<(String, String)> {
    library
        .work_tags_with_source(work)
        .unwrap()
        .into_iter()
        .map(|t| (t.name, t.source))
        .collect()
}

fn pair(name: &str, source: &str) -> (String, String) {
    (name.into(), source.into())
}

#[test]
fn accepted_and_manual_tags_keep_their_source_and_come_off_separately() {
    let (_temp, library) = shelf();
    let work = save(&library, "1", ("10", "甲"), &["雨"]);
    save(&library, "2", ("11", "乙"), &["傘"]);

    // 取得元にある名前は足さない。棚に無い名前は、候補から採るときは誤り。
    let added = library
        .add_user_tags(
            work,
            &["雨".into(), "傘".into()],
            TagProvenance::Manual,
            true,
        )
        .unwrap();
    assert_eq!(added, vec!["傘".to_string()]);
    assert!(library
        .add_user_tags(work, &["棚に無い".into()], TagProvenance::Manual, true)
        .is_err());
    let added = library
        .add_user_tags(
            work,
            &["新しい".into(), " ".into()],
            TagProvenance::AcceptedModel,
            false,
        )
        .unwrap();
    assert_eq!(added, vec!["新しい".to_string()]);
    assert_eq!(
        tags(&library, work),
        vec![
            pair("雨", "origin"),
            pair("傘", "manual"),
            pair("新しい", "llm")
        ]
    );

    // 出どころが違えば外さない。取得元のタグは外せない。
    assert!(!library
        .remove_user_tag(work, "傘", TagProvenance::AcceptedModel)
        .unwrap());
    assert!(!library
        .remove_user_tag(work, "雨", TagProvenance::Manual)
        .unwrap());
    assert!(library
        .remove_user_tag(work, "新しい", TagProvenance::AcceptedModel)
        .unwrap());
    assert_eq!(
        tags(&library, work),
        vec![pair("雨", "origin"), pair("傘", "manual")]
    );

    // 材料のタグにも、語彙にも、自分で付けたタグが入る。
    assert_eq!(
        library.assist_work_facts(work).unwrap().tags,
        vec!["傘", "雨"]
    );
    assert_eq!(library.assist_tag_vocabulary().unwrap(), vec!["傘", "雨"]);
}

#[test]
fn related_tags_count_other_authors_by_tag_name() {
    let (_temp, library) = shelf();
    let target = save(&library, "1", ("10", "甲"), &["雨", "夏"]);
    save(&library, "2", ("11", "乙"), &["雨", "傘"]);
    save(&library, "3", ("12", "丙"), &["雨", "傘", "一回"]);
    save(&library, "4", ("13", "丁"), &["夏", "傘"]);
    // 同じ作者の作品は数えない。
    save(&library, "5", ("10", "甲"), &["雨", "秘密"]);
    save(&library, "6", ("10", "甲"), &["夏", "秘密"]);

    let found = library.related_tag_candidates(target).unwrap();
    let names: Vec<_> = found.iter().map(|c| c.tag.as_str()).collect();
    assert_eq!(names, vec!["傘"]);
    assert_eq!(found[0].matching_work_count, 3);
    assert_eq!(found[0].cooccurring_work_count, 3);
    assert_eq!(
        found[0].reason,
        "同じタグを持つ3作品のうち3作品に一緒に付いています"
    );

    // 候補から採ったタグは、次から候補に出ない。
    library
        .add_user_tags(target, &["傘".into()], TagProvenance::Manual, true)
        .unwrap();
    assert!(library.related_tag_candidates(target).unwrap().is_empty());
}

#[test]
fn the_model_tools_read_the_shelf() {
    let (_temp, library) = shelf();
    save(&library, "1", ("10", "甲"), &["雨", "夏"]);
    save(&library, "2", ("11", "乙"), &["雨", "雨上がり"]);
    let third = save(&library, "3", ("12", "丙"), &["秋"]);
    library
        .add_user_tags(third, &["雨".into()], TagProvenance::Manual, false)
        .unwrap();
    let tools = &library;

    let found = tools.assist_tags_containing("雨", 5).unwrap();
    let found: Vec<_> = found.iter().map(|t| (t.name.as_str(), t.works)).collect();
    assert_eq!(found, vec![("雨", 3), ("雨上がり", 1)]);
    assert_eq!(
        tools
            .assist_existing_tags(&["雨".into(), "無い".into()])
            .unwrap(),
        vec!["雨".to_string()]
    );
    assert_eq!(
        tools
            .assist_count_works(&["雨".into()], false, &[])
            .unwrap(),
        3
    );
    assert_eq!(
        tools
            .assist_count_works(&["雨".into()], false, &["夏".into()])
            .unwrap(),
        2
    );
    assert_eq!(
        tools
            .assist_count_works(&["夏".into(), "秋".into()], true, &[])
            .unwrap(),
        2
    );

    let pairs = tools
        .assist_tag_pairs("雨", &["夏".into(), "無い".into()])
        .unwrap()
        .unwrap();
    assert_eq!((pairs.total, pairs.works), (3, 3));
    assert_eq!(pairs.others.len(), 1);
    assert_eq!((pairs.others[0].works, pairs.others[0].together), (1, 1));
    assert!(tools.assist_tag_pairs("無い", &[]).unwrap().is_none());

    let profile = tools.assist_tag_profile("雨").unwrap().unwrap();
    assert_eq!(profile.works, 3);
    assert_eq!(profile.titles.len(), 3);
    assert!(profile.related.iter().any(|t| t.name == "夏"));
    assert!(tools.assist_tag_profile("無い").unwrap().is_none());
}

#[test]
fn author_facts_and_body_come_from_the_current_version() {
    let (_temp, library) = shelf();
    let first = save(&library, "1", ("10", "甲"), &["雨"]);
    save(&library, "2", ("10", "甲"), &["夏"]);
    save(&library, "3", ("11", "乙"), &["秋"]);

    let (author, works) = library.assist_author_facts("pixiv", "10").unwrap();
    assert_eq!(author, "甲");
    assert_eq!(works.len(), 2);
    assert!(works.iter().all(|w| w.excerpt.is_none()));
    assert!(library.assist_author_facts("pixiv", "99").is_err());

    let (facts, body) = library.assist_work_facts_with_body(first).unwrap();
    assert_eq!(facts.title, "作品1");
    assert!(body.contains("本文1"), "{body}");
}

#[test]
fn notes_are_replaced_and_go_stale_when_their_input_changes() {
    let (_temp, library) = shelf();
    let work = save(&library, "1", ("10", "甲"), &["雨"]);
    let next = save(&library, "2", ("10", "甲"), &["雨"]);
    let (facts, body) = library.assist_work_facts_with_body(work).unwrap();
    let provenance = |text: &str| crate::assist::AssistProvenance {
        feature_id: FEATURE_WORK_SYNOPSIS.into(),
        prompt_version: crate::assist::current_prompt_version(FEATURE_WORK_SYNOPSIS).into(),
        model_id: "model".into(),
        input_fingerprint: crate::assist::generation_input_fingerprint(
            FEATURE_WORK_SYNOPSIS,
            &serde_json::json!({ "work": &facts, "body": &body }),
        )
        .unwrap(),
        config_fingerprint: "config".into(),
        created_at: text.into(),
    };
    let note = |text: &str| AssistNote {
        text: text.into(),
        source_tail: None,
        claims: None,
    };
    let key = work.to_string();
    library
        .save_ai_note("work", &key, "synopsis", &note("一度目"), &provenance("1"))
        .unwrap();
    library
        .save_ai_note("work", &key, "synopsis", &note("二度目"), &provenance("2"))
        .unwrap();
    let loaded = library
        .load_ai_note("work", &key, "synopsis")
        .unwrap()
        .unwrap();
    assert_eq!(loaded.text, "二度目");
    assert!(!loaded.input_stale && !loaded.prompt_stale);

    // タグを足すと材料が変わる。
    library
        .add_user_tags(work, &["夏".into()], TagProvenance::Manual, false)
        .unwrap();
    assert!(
        library
            .load_ai_note("work", &key, "synopsis")
            .unwrap()
            .unwrap()
            .input_stale
    );

    // 前回のあらすじは、読もうとしている話の覚え書きとして次の回へ渡る。
    let recap = format!("{next}:{work}");
    library
        .save_ai_note("work", &recap, "recap", &note("前回"), &provenance("3"))
        .unwrap();
    assert_eq!(
        library.recap_memory_before(next).unwrap().as_deref(),
        Some("前回")
    );
    assert!(library.recap_memory_before(work).unwrap().is_none());

    assert!(library.delete_ai_note("work", &key, "synopsis").unwrap());
    assert!(!library.delete_ai_note("work", &key, "synopsis").unwrap());
    assert!(library
        .load_ai_note("work", &key, "synopsis")
        .unwrap()
        .is_none());
}
