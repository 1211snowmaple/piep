//! 旧棚の絞り込みと、新しい棚の同じ絞り込みを全件で比べる。作品の集まりと時間を並べる。
//! 旧棚は写しを開く（旧 DB は開くときに後片付けを書き込む）。新しい棚は読むだけ。

use piep_lib::database::{models::SearchV2Params, Database};
use piep_lib::library::{
    AcquisitionFilter, CountRange, DateField, DateRange, EntityKind, Library, LibraryId,
    MediaFilter, RevisionFilter, WorkFilter, WorkListQuery,
};
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;
type Key = (String, String);

fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1000.0
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

fn old_walk(old: &Database, params: &SearchV2Params) -> Result<BTreeSet<Key>, Error> {
    let mut keys = BTreeSet::new();
    let mut params = params.clone();
    params.limit = Some(200);
    loop {
        let page = old.search_downloads_v2(&params)?;
        keys.extend(
            page.items
                .iter()
                .map(|d| (d.source.clone(), d.source_id.clone())),
        );
        params.cursor = page.next_cursor;
        if params.cursor.is_none() {
            return Ok(keys);
        }
    }
}

fn new_walk(new: &Library, filter: &WorkFilter) -> Result<(BTreeSet<Key>, u64), Error> {
    let mut keys = BTreeSet::new();
    let mut query = WorkListQuery {
        limit: 500,
        filter: filter.clone(),
        ..WorkListQuery::default()
    };
    let mut total = None;
    loop {
        let page = new.list_works(&query)?;
        total = total.or(page.total);
        keys.extend(page.items.into_iter().map(|c| (c.provider, c.external_key)));
        match page.next {
            Some(next) => {
                query.after = Some(next);
                query.count = false;
            }
            None => return Ok((keys, total.unwrap_or(0))),
        }
    }
}

/// 旧棚の `date(列)` は UTC の暦日で比べる。新しい棚へは、その日の 0 時から翌日の 0 時の手前まで。
fn day(date: &str, next: bool) -> Result<i64, Error> {
    let day = chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d")?;
    let day = if next {
        day.succ_opt().ok_or("日付")?
    } else {
        day
    };
    Ok(day
        .and_hms_opt(0, 0, 0)
        .ok_or("日付")?
        .and_utc()
        .timestamp_millis())
}

struct Case {
    name: String,
    old: SearchV2Params,
    new: WorkFilter,
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|s| s.to_string()).collect()
}

pub fn run(old_db: &Path, storage: &Path, new_root: &Path) -> Result<Value, Error> {
    {
        let conn = rusqlite::Connection::open(old_db)?;
        let journal: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='download_save_journal'",
            [],
            |r| r.get(0),
        )?;
        if journal > 0 {
            conn.execute("DELETE FROM download_save_journal", [])?;
        }
    }
    // 絞り込みに使う値は、旧棚の写しの実際の分布から選ぶ。
    let sql =
        rusqlite::Connection::open_with_flags(old_db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let column = |q: &str| -> Result<Vec<String>, Error> {
        Ok(sql
            .prepare(q)?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?)
    };
    let pair = |q: &str| -> Result<(String, String), Error> {
        Ok(sql.query_row(q, [], |r| Ok((r.get(0)?, r.get(1)?)))?)
    };
    let tags = column("SELECT t.name FROM download_tags dt JOIN tags t ON t.id=dt.tag_id GROUP BY t.name ORDER BY count(*) DESC, t.name LIMIT 60")?;
    let authors = column(
        "SELECT author_name FROM downloads GROUP BY author_name ORDER BY count(*) DESC, author_name LIMIT 2",
    )?;
    // 名前を変えた作者の、いまの作品では使われていない古い名前。
    let renamed = column(
        "SELECT dp.display_name FROM download_people dp JOIN downloads d ON d.id=dp.download_id
         WHERE (dp.person_source,dp.person_key) IN (SELECT person_source,person_key FROM download_people GROUP BY 1,2 HAVING count(DISTINCT display_name)>1)
           AND dp.display_name NOT IN (SELECT author_name FROM downloads)
         GROUP BY dp.display_name ORDER BY count(*) DESC LIMIT 1",
    )?;
    let person = pair("SELECT person_source,person_key FROM download_people GROUP BY 1,2 ORDER BY count(*) DESC LIMIT 1")?;
    let series = pair("SELECT series_source,series_key FROM download_series GROUP BY 1,2 ORDER BY count(*) DESC LIMIT 1")?;
    let sampled: Vec<(i64, String, String)> = sql
        .prepare("SELECT id,source,source_id FROM downloads WHERE id % 500 = 7 ORDER BY id")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<_, _>>()?;
    drop(sql);

    let old = Database::open(old_db, storage)?;
    let new = Library::open(new_root)?;
    let mut ids: HashMap<Key, LibraryId> = HashMap::new();
    {
        let mut query = WorkListQuery {
            limit: 500,
            ..WorkListQuery::default()
        };
        loop {
            let page = new.list_works(&query)?;
            for c in page.items {
                ids.insert((c.provider, c.external_key), c.id);
            }
            match page.next {
                Some(next) => {
                    query.after = Some(next);
                    query.count = false;
                }
                None => break,
            }
        }
    }

    let o = SearchV2Params::default;
    let f = WorkFilter::default;
    let mut cases = Vec::new();
    let mut case = |name: &str, old: SearchV2Params, new: WorkFilter| {
        cases.push(Case {
            name: name.into(),
            old,
            new,
        })
    };
    for provider in ["pixiv", "fanbox"] {
        case(
            &format!("取得元 {provider}"),
            SearchV2Params {
                source: Some(provider.into()),
                ..o()
            },
            WorkFilter {
                provider: Some(provider.into()),
                ..f()
            },
        );
    }
    case(
        "お気に入り",
        SearchV2Params {
            favorite: Some(true),
            ..o()
        },
        WorkFilter {
            favorite: Some(true),
            ..f()
        },
    );
    case(
        "監視",
        SearchV2Params {
            watch_filter: Some("watched".into()),
            ..o()
        },
        WorkFilter {
            watching: Some(true),
            ..f()
        },
    );
    case(
        "編集あり",
        SearchV2Params {
            edit_filter: Some("edited".into()),
            ..o()
        },
        WorkFilter {
            edited: Some(true),
            ..f()
        },
    );
    case(
        "編集なし",
        SearchV2Params {
            edit_filter: Some("unedited".into()),
            ..o()
        },
        WorkFilter {
            edited: Some(false),
            ..f()
        },
    );
    case(
        "表紙あり",
        SearchV2Params {
            cover_filter: Some("has_cover".into()),
            ..o()
        },
        WorkFilter {
            has_cover: Some(true),
            ..f()
        },
    );
    case(
        "表紙なし",
        SearchV2Params {
            cover_filter: Some("no_cover".into()),
            ..o()
        },
        WorkFilter {
            has_cover: Some(false),
            ..f()
        },
    );
    case(
        "シリーズ",
        SearchV2Params {
            series_filter: Some("in_series".into()),
            ..o()
        },
        WorkFilter {
            in_series: Some(true),
            ..f()
        },
    );
    case(
        "単発",
        SearchV2Params {
            series_filter: Some("standalone".into()),
            ..o()
        },
        WorkFilter {
            in_series: Some(false),
            ..f()
        },
    );
    for (old_name, media) in [
        ("has_assets", MediaFilter::Any),
        ("no_assets", MediaFilter::None),
        ("has_images", MediaFilter::Images),
        ("has_files", MediaFilter::Files),
        ("has_images_and_files", MediaFilter::ImagesAndFiles),
    ] {
        case(
            &format!("画像 {old_name}"),
            SearchV2Params {
                asset_filter: Some(old_name.into()),
                ..o()
            },
            WorkFilter {
                media: Some(media),
                ..f()
            },
        );
    }
    for (old_name, revision) in [
        ("revised", RevisionFilter::Revised),
        ("pending_revision", RevisionFilter::PendingRevision),
        ("saved_revisions", RevisionFilter::SavedRevisions),
        ("first_version", RevisionFilter::FirstVersion),
    ] {
        case(
            &format!("改稿 {old_name}"),
            SearchV2Params {
                revision_filter: Some(old_name.into()),
                ..o()
            },
            WorkFilter {
                revision: Some(revision),
                ..f()
            },
        );
    }
    for (old_name, acquisition) in [
        ("complete", AcquisitionFilter::Complete),
        ("attention", AcquisitionFilter::Attention),
        ("incomplete", AcquisitionFilter::Incomplete),
        ("unavailable", AcquisitionFilter::Unavailable),
        ("restricted", AcquisitionFilter::Restricted),
        ("error", AcquisitionFilter::Error),
        ("dismissed", AcquisitionFilter::Dismissed),
    ] {
        case(
            &format!("要確認 {old_name}"),
            SearchV2Params {
                acquisition_filter: Some(old_name.into()),
                ..o()
            },
            WorkFilter {
                acquisition: Some(acquisition),
                ..f()
            },
        );
    }
    case(
        "文字数 1万〜5万",
        SearchV2Params {
            min_char_count: Some(10_000),
            max_char_count: Some(50_000),
            ..o()
        },
        WorkFilter {
            text_length: CountRange {
                min: Some(10_000),
                max: Some(50_000),
            },
            ..f()
        },
    );
    case(
        "画像 5 以上",
        SearchV2Params {
            min_asset_count: Some(5),
            ..o()
        },
        WorkFilter {
            media_count: CountRange {
                min: Some(5),
                max: None,
            },
            ..f()
        },
    );
    case(
        "容量 10MB 以上",
        SearchV2Params {
            min_file_size_bytes: Some(10_000_000),
            ..o()
        },
        WorkFilter {
            logical_size: CountRange {
                min: Some(10_000_000),
                max: None,
            },
            ..f()
        },
    );
    for (old_field, field, from, to) in [
        (None, DateField::Saved, "2026-08-01", "2026-08-31"),
        (
            Some("source_created_at"),
            DateField::Published,
            "2024-01-01",
            "2024-12-31",
        ),
        (
            Some("source_updated_at"),
            DateField::Updated,
            "2025-01-01",
            "2025-06-30",
        ),
    ] {
        case(
            &format!("日付 {field:?} {from}〜{to}"),
            SearchV2Params {
                date_field: old_field.map(Into::into),
                date_from: Some(from.into()),
                date_to: Some(to.into()),
                ..o()
            },
            WorkFilter {
                date: Some(DateRange {
                    field,
                    from: Some(day(from, false)?),
                    until: Some(day(to, true)?),
                }),
                ..f()
            },
        );
    }
    let first_two = strings(&[&tags[0], &tags[1]]);
    case(
        "タグ 1 位",
        SearchV2Params {
            tags_include: Some(vec![tags[0].clone()]),
            ..o()
        },
        WorkFilter {
            tags: vec![tags[0].clone()],
            ..f()
        },
    );
    case(
        "タグ 60 位",
        SearchV2Params {
            tags_include: Some(vec![tags[59].clone()]),
            ..o()
        },
        WorkFilter {
            tags: vec![tags[59].clone()],
            ..f()
        },
    );
    case(
        "タグ 1・2 位のすべて",
        SearchV2Params {
            tags_include: Some(first_two.clone()),
            ..o()
        },
        WorkFilter {
            tags: first_two.clone(),
            ..f()
        },
    );
    case(
        "タグ 1・2 位のどれか",
        SearchV2Params {
            tags_include: Some(first_two.clone()),
            tag_filter_mode: Some("or".into()),
            ..o()
        },
        WorkFilter {
            tags: first_two.clone(),
            any_tag: true,
            ..f()
        },
    );
    case(
        "タグ 1 位を除く",
        SearchV2Params {
            tags_exclude: Some(vec![tags[0].clone()]),
            ..o()
        },
        WorkFilter {
            excluded_tags: vec![tags[0].clone()],
            ..f()
        },
    );
    case(
        "作者名 1 位",
        SearchV2Params {
            authors_include: Some(vec![authors[0].clone()]),
            ..o()
        },
        WorkFilter {
            authors: vec![authors[0].clone()],
            ..f()
        },
    );
    case(
        "作者名 1 位を除く",
        SearchV2Params {
            authors_exclude: Some(vec![authors[0].clone()]),
            ..o()
        },
        WorkFilter {
            excluded_authors: vec![authors[0].clone()],
            ..f()
        },
    );
    if let Some(name) = renamed.first() {
        case(
            "作者の古い名前",
            SearchV2Params {
                authors_include: Some(vec![name.clone()]),
                ..o()
            },
            WorkFilter {
                authors: vec![name.clone()],
                ..f()
            },
        );
    }
    case(
        "作者 1 位",
        SearchV2Params {
            person_source: Some(person.0.clone()),
            person_key: Some(person.1.clone()),
            ..o()
        },
        WorkFilter {
            person: new.entity_by_key(EntityKind::Person, &person.0, &person.1)?,
            ..f()
        },
    );
    case(
        "シリーズ 1 位",
        SearchV2Params {
            series_source: Some(series.0.clone()),
            series_key: Some(series.1.clone()),
            ..o()
        },
        WorkFilter {
            series: new.entity_by_key(EntityKind::Series, &series.0, &series.1)?,
            ..f()
        },
    );
    case(
        "作品の集まり",
        SearchV2Params {
            ids_include: Some(sampled.iter().map(|s| s.0).collect()),
            ..o()
        },
        WorkFilter {
            works: Some(
                sampled
                    .iter()
                    .filter_map(|s| ids.get(&(s.1.clone(), s.2.clone())).copied())
                    .collect(),
            ),
            ..f()
        },
    );
    case(
        "重ねがけ（pixiv・タグ 1 位・初版・表紙あり）",
        SearchV2Params {
            source: Some("pixiv".into()),
            tags_include: Some(vec![tags[0].clone()]),
            revision_filter: Some("first_version".into()),
            cover_filter: Some("has_cover".into()),
            ..o()
        },
        WorkFilter {
            provider: Some("pixiv".into()),
            tags: vec![tags[0].clone()],
            revision: Some(RevisionFilter::FirstVersion),
            has_cover: Some(true),
            ..f()
        },
    );

    let mut results = Vec::new();
    for case in cases {
        let old_keys = old_walk(&old, &case.old)?;
        let (new_keys, total) = new_walk(&new, &case.new)?;
        let first = |run: &mut dyn FnMut() -> Result<(), Error>| -> Result<f64, Error> {
            let mut v = Vec::new();
            for _ in 0..5 {
                let started = Instant::now();
                run()?;
                v.push(ms(started));
            }
            Ok(median(v))
        };
        let old_ms = first(&mut || {
            let mut params = case.old.clone();
            params.limit = Some(80);
            old.search_downloads_v2(&params)?;
            Ok(())
        })?;
        let new_ms = first(&mut || {
            new.list_works(&WorkListQuery {
                limit: 80,
                filter: case.new.clone(),
                ..WorkListQuery::default()
            })?;
            Ok(())
        })?;
        let only_old: Vec<&Key> = old_keys.difference(&new_keys).collect();
        let only_new: Vec<&Key> = new_keys.difference(&old_keys).collect();
        eprintln!(
            "{}: 旧 {} 件・新 {} 件（旧だけ {}・新だけ {}）",
            case.name,
            old_keys.len(),
            new_keys.len(),
            only_old.len(),
            only_new.len()
        );
        results.push(json!({
            "name": case.name,
            "old_count": old_keys.len(),
            "new_count": new_keys.len(),
            "new_total": total,
            "only_old": only_old.len(),
            "only_old_examples": only_old.iter().take(5).collect::<Vec<_>>(),
            "only_new": only_new.len(),
            "only_new_examples": only_new.iter().take(5).collect::<Vec<_>>(),
            "old_first_ms": old_ms,
            "new_first_ms": new_ms,
        }));
    }
    Ok(
        json!({"values": {"tags": [&tags[0], &tags[1], &tags[59]], "authors": authors, "renamed": renamed, "person": person, "series": series, "sampled": sampled.len()}, "filters": results}),
    )
}
