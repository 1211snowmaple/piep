//! 取得元タグの併用統計から、利用者が確認する候補を作る。
//!
//! 意味上その作品に当てはまるという判定ではない。作品本文や概要を読まず、
//! 同じ棚で取得元が付けたタグがどの程度併用されたかだけを返す。

use super::Database;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MAX_RELATED_TAG_CANDIDATES: usize = 5;

/// 取得元タグの併用統計。候補は意味的な正解を保証しない。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RelatedTagCandidate {
    pub tag: String,
    /// `共通タグを持つN作品中M作品で併用` の説明。
    pub reason: String,
    /// 対象の既存タグを一つ以上持つ、同じ作者を除いた取得元タグ付き作品数。
    pub matching_work_count: i64,
    /// 上記のうち候補タグも取得元から付与されている作品数。
    pub cooccurring_work_count: i64,
}

#[derive(Debug)]
struct PairStat {
    candidate_id: i64,
    candidate_name: String,
    seed_df: i64,
    candidate_df: i64,
    co_count: i64,
    matching_work_count: i64,
    co_work_count: i64,
}

impl Database {
    /// 対象作品の既存タグとの併用統計を返す。
    ///
    /// 対象作品自身と、同じ取得元・同じ非空author_idの作品は訓練から外す。
    /// author_idが空の場合は対象作品自身だけを外し、同一作者の判定はしない。
    /// 対象の種タグは全出どころから集める一方、集計側は `origin` のみを使い、
    /// 手動やLLM採用タグが統計を自己増幅しないようにする。
    pub fn related_tag_candidates(
        &self,
        download_id: i64,
    ) -> Result<Vec<RelatedTagCandidate>, String> {
        let conn = self.read_conn()?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Failed to start related-tag snapshot: {e}"))?;

        let target_exists: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM downloads WHERE id = ?1)",
                params![download_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check related-tag target: {e}"))?;
        if !target_exists {
            return Ok(Vec::new());
        }

        let seed_count: i64 = tx
            .query_row(
                "SELECT COUNT(DISTINCT tag_id) FROM download_tags WHERE download_id = ?1",
                params![download_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to count target tags: {e}"))?;
        if seed_count == 0 {
            return Ok(Vec::new());
        }

        // Seed数に比例したJOINだけを行う。全tag同士の組を先に作らない。
        // pair_statsはseed tagごとの併用数、candidate_commonは重複seedを
        // 持つ作品を一度だけ数えたMを返す。
        let mut stmt = tx
            .prepare(
                "WITH target AS MATERIALIZED (
                     SELECT id, source, author_id FROM downloads WHERE id = ?1
                 ),
                 target_tags AS MATERIALIZED (
                     SELECT DISTINCT tag_id FROM download_tags WHERE download_id = ?1
                 ),
                 eligible AS MATERIALIZED (
                     SELECT d.id FROM downloads d CROSS JOIN target t
                     WHERE d.id <> t.id
                       AND NOT (
                           COALESCE(TRIM(t.author_id), '') <> ''
                           AND d.source = t.source
                           AND COALESCE(d.author_id, '') = t.author_id
                       )
                 ),
                 seed_works AS MATERIALIZED (
                     SELECT DISTINCT dt.download_id
                     FROM target_tags s
                     JOIN download_tags dt ON dt.tag_id = s.tag_id
                     JOIN eligible e ON e.id = dt.download_id
                     WHERE dt.tag_source = 'origin'
                 ),
                 seed_df AS (
                     SELECT s.tag_id, COUNT(DISTINCT dt.download_id) AS df
                     FROM target_tags s
                     JOIN download_tags dt ON dt.tag_id = s.tag_id
                     JOIN eligible e ON e.id = dt.download_id
                     WHERE dt.tag_source = 'origin'
                     GROUP BY s.tag_id
                 ),
                 candidate_df AS (
                     SELECT dt.tag_id, COUNT(DISTINCT dt.download_id) AS df
                     FROM download_tags dt
                     JOIN eligible e ON e.id = dt.download_id
                     WHERE dt.tag_source = 'origin'
                     GROUP BY dt.tag_id
                 ),
                 pair_stats AS (
                     SELECT seed.tag_id AS seed_id,
                            candidate.tag_id AS candidate_id,
                            COUNT(DISTINCT candidate.download_id) AS co_count
                     FROM target_tags seed
                     JOIN download_tags seed_assignment ON seed_assignment.tag_id = seed.tag_id
                     JOIN eligible e ON e.id = seed_assignment.download_id
                     JOIN download_tags candidate
                       ON candidate.download_id = e.id AND candidate.tag_source = 'origin'
                     WHERE seed_assignment.tag_source = 'origin'
                       AND NOT EXISTS (
                           SELECT 1 FROM target_tags existing
                           WHERE existing.tag_id = candidate.tag_id
                       )
                     GROUP BY seed.tag_id, candidate.tag_id
                 ),
                 candidate_common AS (
                     SELECT dt.tag_id AS candidate_id,
                            COUNT(DISTINCT dt.download_id) AS co_work_count
                     FROM download_tags dt
                     JOIN seed_works sw ON sw.download_id = dt.download_id
                     JOIN eligible e ON e.id = dt.download_id
                     WHERE dt.tag_source = 'origin'
                       AND NOT EXISTS (
                           SELECT 1 FROM target_tags existing
                           WHERE existing.tag_id = dt.tag_id
                       )
                     GROUP BY dt.tag_id
                     HAVING COUNT(DISTINCT dt.download_id) >= 2
                 )
                 SELECT p.candidate_id, tags.name, sd.df, cd.df, p.co_count,
                        (SELECT COUNT(*) FROM seed_works) AS matching_work_count,
                        cc.co_work_count
                 FROM pair_stats p
                 JOIN seed_df sd ON sd.tag_id = p.seed_id
                 JOIN candidate_df cd ON cd.tag_id = p.candidate_id
                 JOIN tags ON tags.id = p.candidate_id
                 JOIN candidate_common cc ON cc.candidate_id = p.candidate_id
                 ORDER BY p.candidate_id, p.seed_id",
            )
            .map_err(|e| format!("Failed to prepare related-tag query: {e}"))?;
        let rows = stmt
            .query_map(params![download_id], |row| {
                Ok(PairStat {
                    candidate_id: row.get(0)?,
                    candidate_name: row.get(1)?,
                    seed_df: row.get(2)?,
                    candidate_df: row.get(3)?,
                    co_count: row.get(4)?,
                    matching_work_count: row.get(5)?,
                    co_work_count: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query related-tag pairs: {e}"))?;

        let mut scores: HashMap<i64, (RelatedTagCandidate, f64)> = HashMap::new();
        for row in rows {
            let pair = row.map_err(|e| format!("Failed to read related-tag pair: {e}"))?;
            if pair.seed_df <= 0 || pair.candidate_df <= 0 || pair.co_count <= 0 {
                continue;
            }
            let cosine =
                pair.co_count as f64 / ((pair.seed_df as f64) * (pair.candidate_df as f64)).sqrt();
            let candidate = scores.entry(pair.candidate_id).or_insert_with(|| {
                (
                    RelatedTagCandidate {
                        tag: pair.candidate_name.clone(),
                        reason: format!(
                            "共通タグを持つ{}作品中{}作品で併用",
                            pair.matching_work_count, pair.co_work_count
                        ),
                        matching_work_count: pair.matching_work_count,
                        cooccurring_work_count: pair.co_work_count,
                    },
                    0.0,
                )
            });
            candidate.1 += cosine;
        }

        let denominator = seed_count as f64;
        let mut candidates: Vec<_> = scores
            .into_values()
            .filter(|(candidate, _)| {
                let trimmed = candidate.tag.trim();
                !trimmed.is_empty() && trimmed.chars().count() <= 100 && trimmed == candidate.tag
            })
            .map(|(candidate, score)| {
                // originで観測されなかったseedタグも0点として全seed数で平均。
                (candidate, score / denominator)
            })
            .collect();
        candidates.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| {
                    right
                        .0
                        .cooccurring_work_count
                        .cmp(&left.0.cooccurring_work_count)
                })
                .then_with(|| left.0.tag.cmp(&right.0.tag))
        });
        candidates.truncate(MAX_RELATED_TAG_CANDIDATES);
        drop(stmt);
        tx.commit()
            .map_err(|e| format!("Failed to finish related-tag snapshot: {e}"))?;
        Ok(candidates
            .into_iter()
            .map(|(candidate, _)| candidate)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DB: AtomicU64 = AtomicU64::new(1);

    struct TestDb {
        db: Option<Database>,
        root: PathBuf,
    }

    impl TestDb {
        fn new() -> Self {
            let unique = NEXT_DB.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "piep-tag-recommendation-test-{}-{unique}",
                std::process::id()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let db_path = root.join("test.db");
            let db = Database::open(&db_path, &root.join("storage")).unwrap();
            Self { db: Some(db), root }
        }

        fn related(&self, id: i64) -> Result<Vec<RelatedTagCandidate>, String> {
            self.db.as_ref().unwrap().related_tag_candidates(id)
        }

        fn allow_null_author_ids_for_legacy_fixture(&mut self) {
            drop(self.db.take());
            let db_path = self.root.join("test.db");
            let conn = Connection::open(&db_path).unwrap();
            conn.pragma_update(None, "writable_schema", true).unwrap();
            let schema_sql: String = conn
                .query_row(
                    "SELECT sql FROM sqlite_master WHERE type='table' AND name='downloads'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            let nullable_sql = regex::Regex::new(r"(\bauthor_id\s+TEXT)\s+NOT\s+NULL")
                .unwrap()
                .replace(&schema_sql, "$1")
                .into_owned();
            assert_ne!(nullable_sql, schema_sql);
            conn.execute(
                "UPDATE sqlite_master SET sql=?1 WHERE type='table' AND name='downloads'",
                [nullable_sql],
            )
            .unwrap();
            let schema_version: i64 = conn
                .pragma_query_value(None, "schema_version", |row| row.get(0))
                .unwrap();
            conn.pragma_update(None, "schema_version", schema_version + 1)
                .unwrap();
            conn.pragma_update(None, "writable_schema", false).unwrap();
            drop(conn);
            self.db = Some(Database::open(&db_path, &self.root.join("storage")).unwrap());
        }

        fn add_work(&self, id: i64, source: &str, author: &str, tags: &[(&str, &str)]) {
            let conn = Connection::open(self.root.join("test.db")).unwrap();
            conn.execute(
                "INSERT INTO downloads
                 (id, source, source_id, title, author_name, author_id, content_type,
                  json_path, downloaded_at)
                 VALUES (?1, ?2, ?3, '', '', ?4, 'novel', 'synthetic', '2026-10-08')",
                rusqlite::params![id, source, format!("synthetic-{id}"), author],
            )
            .unwrap();
            for (name, tag_source) in tags {
                conn.execute("INSERT OR IGNORE INTO tags(name) VALUES (?1)", [name])
                    .unwrap();
                let tag_id: i64 = conn
                    .query_row("SELECT id FROM tags WHERE name = ?1", [name], |row| {
                        row.get(0)
                    })
                    .unwrap();
                conn.execute(
                    "INSERT INTO download_tags(download_id, tag_id, tag_source)
                     VALUES (?1, ?2, ?3)",
                    rusqlite::params![id, tag_id, tag_source],
                )
                .unwrap();
            }
        }
    }

    impl Drop for TestDb {
        fn drop(&mut self) {
            drop(self.db.take());
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn counts_distinct_origin_works_and_excludes_same_source_author() {
        let fixture = TestDb::new();
        fixture.add_work(
            1,
            "pixiv",
            "author-a",
            &[
                ("seed", "origin"),
                ("seed-b", "manual"),
                ("already", "manual"),
            ],
        );
        fixture.add_work(
            2,
            "pixiv",
            "author-a",
            &[("seed", "origin"), ("author-leak", "origin")],
        );
        fixture.add_work(
            3,
            "fanbox",
            "author-a",
            &[
                ("seed", "origin"),
                ("seed-b", "origin"),
                ("common", "origin"),
                ("already", "origin"),
                ("author-leak", "origin"),
                ("manual-only", "manual"),
            ],
        );
        fixture.add_work(
            4,
            "pixiv",
            "author-b",
            &[
                ("seed", "origin"),
                ("seed-b", "origin"),
                ("common", "origin"),
                ("already", "origin"),
            ],
        );
        fixture.add_work(
            5,
            "pixiv",
            "author-c",
            &[
                ("seed", "origin"),
                ("seed-b", "origin"),
                ("common", "origin"),
                ("already", "origin"),
            ],
        );
        fixture.add_work(
            6,
            "pixiv",
            "author-d",
            &[("seed", "origin"), ("manual-only", "manual")],
        );
        fixture.add_work(
            7,
            "pixiv",
            "author-e",
            &[("seed", "origin"), ("already", "origin")],
        );
        fixture.add_work(
            8,
            "pixiv",
            "author-f",
            &[("other", "origin"), ("common", "origin")],
        );

        let result = fixture.related(1).unwrap();
        let common = result.iter().find(|entry| entry.tag == "common").unwrap();
        assert_eq!(common.matching_work_count, 5);
        assert_eq!(common.cooccurring_work_count, 3);
        assert_eq!(common.reason, "共通タグを持つ5作品中3作品で併用");
        assert!(!result.iter().any(|entry| entry.tag == "already"));
        assert!(!result.iter().any(|entry| entry.tag == "author-leak"));
        assert!(!result.iter().any(|entry| entry.tag == "manual-only"));
    }

    #[test]
    fn empty_author_excludes_only_self_and_candidate_requires_two_works() {
        let fixture = TestDb::new();
        fixture.add_work(1, "pixiv", "", &[("seed", "origin")]);
        fixture.add_work(2, "pixiv", "", &[("seed", "origin"), ("single", "origin")]);
        fixture.add_work(3, "fanbox", "", &[("seed", "origin"), ("pair", "origin")]);
        fixture.add_work(
            4,
            "pixiv",
            "another",
            &[("seed", "origin"), ("pair", "origin")],
        );

        let result = fixture.related(1).unwrap();
        assert!(!result.iter().any(|entry| entry.tag == "single"));
        assert_eq!(
            result
                .iter()
                .find(|entry| entry.tag == "pair")
                .unwrap()
                .cooccurring_work_count,
            2
        );
        assert_eq!(
            result
                .iter()
                .find(|entry| entry.tag == "pair")
                .unwrap()
                .matching_work_count,
            3
        );
    }

    #[test]
    fn known_author_does_not_exclude_another_work_with_null_author_id() {
        let mut fixture = TestDb::new();
        fixture.allow_null_author_ids_for_legacy_fixture();
        fixture.add_work(1, "pixiv", "known-author", &[("seed", "origin")]);
        fixture.add_work(
            2,
            "pixiv",
            "placeholder",
            &[("seed", "origin"), ("candidate", "origin")],
        );
        fixture.add_work(
            3,
            "pixiv",
            "other-author",
            &[("seed", "origin"), ("candidate", "origin")],
        );
        let conn = Connection::open(fixture.root.join("test.db")).unwrap();
        conn.execute("UPDATE downloads SET author_id=NULL WHERE id=2", [])
            .unwrap();

        let result = fixture.related(1).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tag, "candidate");
        assert_eq!(result[0].matching_work_count, 2);
        assert_eq!(result[0].cooccurring_work_count, 2);
    }

    #[test]
    fn missing_or_untagged_target_returns_empty() {
        let fixture = TestDb::new();
        fixture.add_work(1, "pixiv", "author", &[]);
        assert!(fixture.related(404).unwrap().is_empty());
        assert!(fixture.related(1).unwrap().is_empty());
    }

    #[test]
    fn candidate_names_must_match_the_adoptable_tag_contract() {
        let fixture = TestDb::new();
        fixture.add_work(1, "pixiv", "target", &[("seed", "origin")]);
        let too_long = "x".repeat(101);
        for id in 2..=4 {
            fixture.add_work(
                id,
                "pixiv",
                &format!("author-{id}"),
                &[("seed", "origin"), ("  ", "origin")],
            );
        }
        for id in 5..=7 {
            fixture.add_work(
                id,
                "pixiv",
                &format!("author-{id}"),
                &[("seed", "origin"), (" leading", "origin")],
            );
        }
        for id in 8..=10 {
            fixture.add_work(
                id,
                "pixiv",
                &format!("author-{id}"),
                &[("seed", "origin"), (&too_long, "origin")],
            );
        }
        for id in 11..=13 {
            fixture.add_work(
                id,
                "pixiv",
                &format!("author-{id}"),
                &[("seed", "origin"), ("valid", "origin")],
            );
        }

        let result = fixture.related(1).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].tag, "valid");
    }

    #[test]
    fn existing_tags_are_excluded_and_order_is_stable_and_bounded() {
        let fixture = TestDb::new();
        fixture.add_work(1, "pixiv", "target", &[("seed", "origin"), ("kept", "llm")]);
        for work in 2..=12 {
            let candidate = format!("candidate-{work:02}");
            // All tags have equal counts; lexical order should be deterministic.
            fixture.add_work(
                work,
                "pixiv",
                &format!("author-{work}"),
                &[("seed", "origin")],
            );
            fixture.add_work(
                work + 100,
                "pixiv",
                &format!("author-x{work}"),
                &[("seed", "origin"), (&candidate, "origin")],
            );
            fixture.add_work(
                work + 200,
                "pixiv",
                &format!("author-y{work}"),
                &[("seed", "origin"), (&candidate, "origin")],
            );
        }
        let first = fixture.related(1).unwrap();
        let second = fixture.related(1).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), MAX_RELATED_TAG_CANDIDATES);
        assert!(first.iter().all(|entry| entry.tag != "kept"));
        assert!(first.windows(2).all(|pair| pair[0].tag <= pair[1].tag));
    }
}
