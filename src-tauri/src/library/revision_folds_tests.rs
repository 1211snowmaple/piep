use super::*;
use crate::library::WorkSave;
use serde_json::{json, Value};
use std::path::PathBuf;

struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn library() -> (Fixture, Library) {
    let root = std::env::temp_dir().join(format!("piep-revision-folds-{}", LibraryId::now_v7()));
    let library = Library::create(&root.join("shelf")).unwrap();
    (Fixture(root), library)
}

/// 本文に別の投稿のカードを貼った FANBOX の記事。
fn post(text: &str, linked_title: &str, likes: i64) -> Value {
    json!({
        "id": "7", "title": "投稿", "creatorId": "creator", "type": "article",
        "user": {"userId": "1", "name": "作者"},
        "publishedDatetime": "2026-01-01T00:00:00+09:00",
        "updatedDatetime": "2026-01-01T00:00:00+09:00",
        "body": {
            "blocks": [{"type": "p", "text": text}, {"type": "url_embed", "urlEmbedId": "card"}],
            "imageMap": {}, "fileMap": {}, "embedMap": {},
            "urlEmbedMap": {"card": {"id": "card", "type": "fanbox.post", "postInfo": {
                "id": "9", "creatorId": "other", "title": linked_title, "excerpt": "抜粋",
                "likeCount": likes, "commentCount": likes, "isLiked": false, "isRestricted": false,
                "updatedDatetime": format!("2026-02-0{}T00:00:00+09:00", likes.clamp(1, 9)),
                "user": {"userId": "2", "name": "別の作者", "iconUrl": format!("https://example.invalid/{likes}.png")}
            }}}
        }
    })
}
fn save(library: &Library, data: &Value) -> crate::library::CaptureResult {
    library
        .save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider: "fanbox",
            key: "7",
            route: "save",
            data,
            assets: vec![],
        })
        .unwrap()
}
fn revisions(library: &Library, entity: LibraryId) -> i64 {
    library
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM source_revisions WHERE entity_id=?1",
            [entity.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .unwrap()
}
fn current(library: &Library, entity: LibraryId) -> Vec<u8> {
    library
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT snapshot_id FROM entity_current WHERE entity_id=?1",
            [entity.as_bytes().as_slice()],
            |r| r.get(0),
        )
        .unwrap()
}

/// 規則 1 のころにできた版を写す：二つ目の版の本文を一つ目と同じ本文にし、指紋を規則 1 で
/// 記録し、畳む手入れを予約する。
fn split_by_rule_one(
    library: &Library,
    first: &crate::library::CaptureResult,
    second: &crate::library::CaptureResult,
) {
    {
        let conn = library.conn.lock().unwrap();
        conn.execute_batch(
            "CREATE TEMP TABLE guard AS SELECT sql FROM sqlite_master WHERE name='published_snapshot_immutable';
             DROP TRIGGER published_snapshot_immutable;",
        )
        .unwrap();
        let file: Vec<u8> = conn
            .query_row(
                "SELECT body_file_id FROM revision_snapshots WHERE id=?1",
                [first.snapshot_id.as_bytes().as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        let body = library
            .source_body_from_conn(&conn, first.entity_id, first.snapshot_id)
            .unwrap()
            .unwrap();
        let old = body.semantic_fingerprint_under(1).unwrap();
        assert_ne!(
            old,
            body.semantic_fingerprint().unwrap(),
            "規則 1 はカードのいいね数まで比べていた"
        );
        conn.execute(
            "UPDATE revision_snapshots SET body_file_id=?2,body_semantic_hash=?3,body_semantic_rule=1 WHERE id=?1",
            rusqlite::params![second.snapshot_id.as_bytes().as_slice(), file, old.0.as_slice()],
        )
        .unwrap();
        let guard: String = conn
            .query_row("SELECT sql FROM temp.guard", [], |r| r.get(0))
            .unwrap();
        conn.execute_batch(&format!("{guard}; DROP TABLE temp.guard;"))
            .unwrap();
        conn.execute(
            "INSERT INTO library_tasks(name,created_at) VALUES('fold_volatile_revisions',0)",
            [],
        )
        .unwrap();
    }
}

#[test]
fn a_linked_posts_counts_do_not_make_a_version_but_its_title_does() {
    let (_fixture, library) = library();
    let first = save(&library, &post("本文", "リンク先", 1));
    let liked = save(&library, &post("本文", "リンク先", 5));
    assert!(!liked.created_revision);
    assert_eq!(revisions(&library, first.entity_id), 1);
    let renamed = save(&library, &post("本文", "書き直したリンク先", 5));
    assert!(renamed.created_revision);
    assert_eq!(revisions(&library, first.entity_id), 2);
    library.verify().unwrap();
}

#[test]
fn a_version_split_only_by_volatile_values_is_folded_into_the_one_before() {
    let (_fixture, library) = library();
    let first = save(&library, &post("本文", "リンク先", 1));
    let second = save(&library, &post("書き直した本文", "リンク先", 1));
    assert!(second.created_revision);
    // 作者が本文を書き直した版は畳まない。
    {
        let mut conn = library.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        assert_eq!(library.fold_volatile_revisions(&tx).unwrap(), 0);
    }
    assert_eq!(revisions(&library, first.entity_id), 2);

    // 規則 1 のころにできた版を写す：二つ目の版の本文を一つ目と同じ本文にし、指紋を規則 1 で記録する。
    split_by_rule_one(&library, &first, &second);
    {
        let mut conn = library.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        let candidates = library.volatile_revisions(&tx).unwrap();
        assert_eq!(candidates.len(), 1, "規則 1 で分かれた版を候補に挙げる");
    }
    library.run_library_tasks().unwrap();
    assert_eq!(
        current(&library, first.entity_id),
        first.snapshot_id.as_bytes().to_vec()
    );
    assert_eq!(revisions(&library, first.entity_id), 1);
    let pending: i64 = library
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM library_tasks", [], |r| r.get(0))
        .unwrap();
    assert_eq!(pending, 0);
    assert_eq!(
        library.work_card(first.entity_id).unwrap().version_number,
        1
    );
    library.verify().unwrap();
    // 畳んだあとも、同じ内容は前の版のまま保存できる。
    let again = save(&library, &post("本文", "リンク先", 9));
    assert!(!again.created_revision);
    assert_eq!(revisions(&library, first.entity_id), 1);
    library.verify().unwrap();
}

fn cover(fixture: &Fixture, name: &str, bytes: &[u8]) -> crate::library::StagedAsset {
    let path = fixture.0.join(name);
    std::fs::write(&path, bytes).unwrap();
    crate::library::StagedAsset {
        role: "cover".into(),
        name: name.into(),
        path,
        url: Some(COVER.into()),
        mime_type: Some("image/jpeg".into()),
    }
}
const COVER: &str = "https://downloads.fanbox.cc/images/post/7/cover/a.jpeg";

/// 新しい版でだけ取れた画像（原寸など）があれば畳まない。畳むと今の版からも版の履歴からも
/// 選べなくなる。
#[test]
fn a_version_that_holds_an_image_the_one_before_lacks_is_not_folded() {
    let (fixture, library) = library();
    let mut data = post("本文", "リンク先", 1);
    data["coverImageUrl"] = json!(COVER);
    let save = |data: &Value, asset| {
        library
            .save_work(WorkSave {
                request: LibraryId::now_v7(),
                provider: "fanbox",
                key: "7",
                route: "save",
                data,
                assets: vec![asset],
            })
            .unwrap()
    };
    let first = save(&data, cover(&fixture, "small.jpeg", b"small cover"));
    let mut rewritten = post("書き直した本文", "リンク先", 1);
    rewritten["coverImageUrl"] = json!(COVER);
    let second = save(&rewritten, cover(&fixture, "large.jpeg", b"large cover"));
    assert!(second.created_revision);
    split_by_rule_one(&library, &first, &second);
    {
        let mut conn = library.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        assert!(library.volatile_revisions(&tx).unwrap().is_empty());
    }
    library.run_library_tasks().unwrap();
    assert_eq!(
        current(&library, first.entity_id),
        second.snapshot_id.as_bytes().to_vec()
    );
    assert_eq!(revisions(&library, first.entity_id), 2);
    // 完全検査はしない。規則 1 の版を写す仕込みは本文だけを手で差し替えるので、畳まないままだと
    // 構成の指紋が合わない（実際の棚の版は保存で作られ、指紋は合っている）。
}

/// その版を基準に始めた取得があれば畳まない。確定済みの取得の入力（券）は今の版を指して
/// いて、付け替えると台帳と食い違い、棚の検査が通らなくなる。
#[test]
fn a_version_that_an_acquisition_started_from_is_not_folded() {
    let (_fixture, library) = library();
    let first = save(&library, &post("本文", "リンク先", 1));
    let second = save(&library, &post("書き直した本文", "リンク先", 1));
    // 二つ目の版を基準に、内容の変わらない取得がもう一度あった。
    let again = save(&library, &post("書き直した本文", "リンク先", 3));
    assert!(!again.created_revision);
    split_by_rule_one(&library, &first, &second);
    {
        let mut conn = library.conn.lock().unwrap();
        let tx = conn.transaction().unwrap();
        assert!(library.volatile_revisions(&tx).unwrap().is_empty());
    }
    library.run_library_tasks().unwrap();
    assert_eq!(revisions(&library, first.entity_id), 2);
    // 完全検査はしない。規則 1 の版を写す仕込みは本文だけを手で差し替えるので、畳まないままだと
    // 構成の指紋が合わない（実際の棚の版は保存で作られ、指紋は合っている）。
}

/// 実際の棚の写しで畳みを確かめる（手で回す）。`PIEP_FOLD_SHELF` に写しの置き場を渡す。
/// 写しは移行と畳みで書き換わるので、本物の棚を渡さない。
#[test]
#[ignore]
fn folds_on_a_copy_of_a_real_shelf() {
    let Ok(root) = std::env::var("PIEP_FOLD_SHELF") else {
        return;
    };
    let root = PathBuf::from(root);
    let count =
        |conn: &Connection, sql: &str| -> i64 { conn.query_row(sql, [], |r| r.get(0)).unwrap() };
    let before = {
        let conn = Connection::open(root.join("library.db")).unwrap();
        (
            count(&conn, "SELECT count(*) FROM source_revisions"),
            count(&conn, "SELECT count(*) FROM revision_snapshots"),
        )
    };
    let started = std::time::Instant::now();
    let library = Library::open(&root).unwrap();
    eprintln!("開いて畳むまで {:?}", started.elapsed());
    let conn = library.conn.lock().unwrap();
    let after = (
        count(&conn, "SELECT count(*) FROM source_revisions"),
        count(&conn, "SELECT count(*) FROM revision_snapshots"),
    );
    assert_eq!(
        count(&conn, "SELECT count(*) FROM library_tasks"),
        0,
        "手入れが残った"
    );
    let mut folded = conn
        .prepare(
            "SELECT x.provider || ':' || x.external_key, o.payload_json FROM outbox o
               JOIN external_identities x ON x.entity_id=o.entity_id AND x.kind='work'
              WHERE o.operation='fold_revision'",
        )
        .unwrap()
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    folded.sort();
    eprintln!(
        "版 {} → {}、スナップショット {} → {}",
        before.0, after.0, before.1, after.1
    );
    for (work, payload) in &folded {
        eprintln!("畳んだ {work} {payload}");
    }
    drop(conn);
    let started = std::time::Instant::now();
    eprintln!(
        "確かめた {} 件 {:?}",
        library.verify().unwrap(),
        started.elapsed()
    );
}
