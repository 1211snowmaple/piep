//! コレクションの提案の材料を、新しい棚から作業用の表へ写す。
//!
//! 提案の計算（`database::queries` の走査・種からの提案・追加の候補・名前の案）は、旧棚の形の表を
//! SQL で引く。ここでは棚の今の姿から同じ形の表をメモリ上に組み、その計算をそのまま走らせる。
//! 作品の番号は作業用の表の中だけのもので、棚の作品 ID との対応をここで持つ。
//!
//! 写すもの：
//! - `downloads`：棚にある作品（今の版の題・作者・本文の長さ・公開日・表紙）。
//! - `download_series`：今の版のシリーズと話数。
//! - `tags`・`download_tags`：今の版の取得元のタグと、持ち主のタグ。
//! - `work_links`：本文リンク（根拠ごとに一行）。棚に無い作品へのリンクは番号を持たない。
//! - `collection_pair_feedback`：組の採否。
//! - `work_collections`・`work_collection_members`：今あるコレクションの顔ぶれ。

use crate::database::queries::{Corpus, SuggestionSource};
use crate::library::{Library, LibraryId};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};

const SCHEMA: &str = "
CREATE TABLE downloads (
    id INTEGER PRIMARY KEY,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    title TEXT NOT NULL,
    author_name TEXT NOT NULL,
    author_id TEXT NOT NULL,
    content_type TEXT NOT NULL,
    cover_path TEXT,
    downloaded_at TEXT NOT NULL,
    source_created_at TEXT,
    text_length INTEGER DEFAULT 0,
    UNIQUE(source, source_id)
);
CREATE TABLE download_series (
    download_id INTEGER NOT NULL,
    series_source TEXT NOT NULL,
    series_key TEXT NOT NULL,
    title TEXT NOT NULL,
    content_order INTEGER,
    PRIMARY KEY(download_id, series_source, series_key)
);
CREATE TABLE tags (id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE);
CREATE TABLE download_tags (
    download_id INTEGER NOT NULL,
    tag_id INTEGER NOT NULL,
    PRIMARY KEY(download_id, tag_id)
);
CREATE TABLE work_links (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_source TEXT NOT NULL,
    from_source_id TEXT NOT NULL,
    from_download_id INTEGER,
    to_source TEXT NOT NULL,
    to_source_id TEXT NOT NULL,
    to_download_id INTEGER,
    relation_type TEXT NOT NULL,
    evidence_type TEXT NOT NULL,
    anchor_text TEXT,
    context_text TEXT,
    confidence REAL NOT NULL DEFAULT 0,
    status TEXT NOT NULL DEFAULT 'observed',
    discovered_at TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT '',
    UNIQUE(from_source, from_source_id, to_source, to_source_id, evidence_type)
);
CREATE TABLE collection_pair_feedback (
    left_source TEXT NOT NULL,
    left_source_id TEXT NOT NULL,
    right_source TEXT NOT NULL,
    right_source_id TEXT NOT NULL,
    decision TEXT NOT NULL,
    rule_version TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(left_source, left_source_id, right_source, right_source_id)
);
CREATE TABLE work_collections (
    id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    description TEXT,
    collection_kind TEXT NOT NULL DEFAULT 'ordered',
    created_at TEXT NOT NULL DEFAULT '',
    updated_at TEXT NOT NULL DEFAULT ''
);
CREATE TABLE work_collection_members (
    collection_id TEXT NOT NULL,
    source TEXT NOT NULL,
    source_id TEXT NOT NULL,
    download_id INTEGER,
    title_snapshot TEXT NOT NULL,
    author_snapshot TEXT NOT NULL,
    position INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT '',
    PRIMARY KEY(collection_id, source, source_id)
);
";

fn time(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .unwrap_or_default()
}

/// 公開日時。旧棚は取得元の日本時間（`+09:00`）のまま持っていた。走査は日付の部分で前後や
/// 同じ日の投稿を見るので、UTC で書くと日本時間の 0〜9 時の投稿が前の日になる。
fn published_time(ms: i64) -> String {
    let japan = chrono::FixedOffset::east_opt(9 * 3600).expect("日本時間");
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&japan).to_rfc3339())
        .unwrap_or_default()
}

fn uuid(bytes: &[u8]) -> crate::library::Result<LibraryId> {
    LibraryId::from_slice(bytes)
        .map_err(|_| crate::library::Error::Integrity("識別子が不正です".into()))
}

/// 棚から組んだ作業用の表と、作品の番号の対応。
pub struct ShelfSuggestions<'a> {
    library: &'a Library,
    conn: Connection,
    /// 番号 - 1 が添字。
    works: Vec<LibraryId>,
    by_work: HashMap<LibraryId, i64>,
    /// 意味索引の文書の鍵（番号 → 鍵、鍵 → 番号）。
    doc_keys: HashMap<i64, i64>,
    from_doc: HashMap<i64, i64>,
}

impl<'a> ShelfSuggestions<'a> {
    pub fn load(library: &'a Library) -> crate::library::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        let shelf = library.reader()?;
        let mut works = Vec::new();
        let mut by_work = HashMap::new();
        {
            let tx = conn.unchecked_transaction()?;
            // 作品。番号は旧棚から移した作品は旧棚の番号の順、そのあとに保存した作品は保存した順
            // （旧棚と同じく古いものほど小さい。強さが同じ候補は番号で並ぶので、旧棚と同じ順にする）。
            let mut stmt = shelf.prepare(
                "SELECT p.work_id,p.provider,p.external_key,p.title,p.author_name,
                        CAST(coalesce(json_extract(s.metadata_json,'$.author_key'),'') AS TEXT),
                        p.content_type,
                        (SELECT r.file_id FROM snapshot_media sm JOIN media_renditions r ON r.id=sm.rendition_id
                          WHERE sm.snapshot_id=p.snapshot_id AND sm.role='cover' ORDER BY sm.slot LIMIT 1),
                        p.saved_at,p.published_at,p.text_length
                 FROM work_current_projection p
                 JOIN library_items li ON li.work_id=p.work_id
                 JOIN revision_snapshots s ON s.id=p.snapshot_id
                 LEFT JOIN (SELECT rs.entity_id AS work_id,min(CAST(r.source_key AS INTEGER)) AS legacy
                              FROM migration_records r JOIN revision_snapshots rs ON rs.id=r.target_id
                             WHERE r.source_table='download_current' AND r.status='imported'
                             GROUP BY rs.entity_id) m ON m.work_id=p.work_id
                 ORDER BY m.legacy IS NULL,m.legacy,p.saved_at,p.work_id",
            )?;
            let mut rows = stmt.query([])?;
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO downloads(id,source,source_id,title,author_name,author_id,
                     content_type,cover_path,downloaded_at,source_created_at,text_length)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            )?;
            while let Some(r) = rows.next()? {
                let work = uuid(&r.get::<_, Vec<u8>>(0)?)?;
                let id = works.len() as i64 + 1;
                let cover: Option<Vec<u8>> = r.get(7)?;
                let published: Option<i64> = r.get(9)?;
                let inserted = insert.execute(params![
                    id,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, String>(6)?,
                    crate::library_entity::fingerprint(cover)?,
                    time(r.get(8)?),
                    published.map(published_time),
                    r.get::<_, i64>(10)?,
                ])?;
                // 同じ取得元の鍵を持つ作品が二つある棚は無いはずだが、あっても一つ目だけを使う。
                if inserted == 1 {
                    works.push(work);
                    by_work.insert(work, id);
                }
            }
            drop(insert);
            drop(rows);
            drop(stmt);

            // シリーズと話数。
            let mut stmt = shelf.prepare(
                "SELECT p.work_id,x.provider,x.external_key,ss.title,ss.position
                 FROM work_current_projection p
                 JOIN snapshot_series ss ON ss.snapshot_id=p.snapshot_id
                 JOIN external_identities x ON x.entity_id=ss.series_id AND x.kind='series'",
            )?;
            let mut rows = stmt.query([])?;
            let mut insert =
                tx.prepare("INSERT OR IGNORE INTO download_series VALUES(?1,?2,?3,?4,?5)")?;
            while let Some(r) = rows.next()? {
                let Some(id) = by_work.get(&uuid(&r.get::<_, Vec<u8>>(0)?)?) else {
                    continue;
                };
                insert.execute(params![
                    id,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                ])?;
            }
            drop(insert);
            drop(rows);
            drop(stmt);

            // タグ（取得元のタグと持ち主のタグ）。
            let mut stmt = shelf.prepare(
                "SELECT p.work_id,t.name FROM work_current_projection p
                 JOIN snapshot_tags st ON st.snapshot_id=p.snapshot_id
                 JOIN tags t ON t.id=st.tag_id
                 UNION
                 SELECT wt.work_id,ut.name FROM work_user_tags wt
                 JOIN user_tags ut ON ut.user_id=wt.user_id AND ut.id=wt.tag_id
                 WHERE wt.user_id=?1",
            )?;
            let mut rows = stmt.query([library.id.as_bytes().as_slice()])?;
            let mut tag = tx.prepare("INSERT OR IGNORE INTO tags(name) VALUES(?1)")?;
            let mut link = tx.prepare(
                "INSERT OR IGNORE INTO download_tags SELECT ?1,id FROM tags WHERE name=?2",
            )?;
            while let Some(r) = rows.next()? {
                let Some(id) = by_work.get(&uuid(&r.get::<_, Vec<u8>>(0)?)?) else {
                    continue;
                };
                let name: String = r.get(1)?;
                tag.execute([&name])?;
                link.execute(params![id, name])?;
            }
            drop(tag);
            drop(link);
            drop(rows);
            drop(stmt);

            // 本文リンク。根拠ごとに一行（旧棚と同じ）。
            let mut stmt = shelf.prepare(
                "SELECT l.from_work_id,fx.provider,fx.external_key,l.to_work_id,tx.provider,tx.external_key,
                        l.relation,e.kind,e.anchor_text,e.context_text,e.confidence,
                        CASE WHEN e.decision='rejected' THEN 'rejected' ELSE l.status END
                 FROM work_links l
                 JOIN link_evidence e ON e.link_id=l.id
                 JOIN external_identities fx ON fx.entity_id=l.from_work_id AND fx.kind='work'
                 JOIN external_identities tx ON tx.entity_id=l.to_work_id AND tx.kind='work'
                 ORDER BY l.id,e.confidence DESC,e.id",
            )?;
            let mut rows = stmt.query([])?;
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO work_links(from_source,from_source_id,from_download_id,to_source,
                     to_source_id,to_download_id,relation_type,evidence_type,anchor_text,context_text,
                     confidence,status) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
            )?;
            while let Some(r) = rows.next()? {
                let from = by_work.get(&uuid(&r.get::<_, Vec<u8>>(0)?)?).copied();
                let to = by_work.get(&uuid(&r.get::<_, Vec<u8>>(3)?)?).copied();
                insert.execute(params![
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    from,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    to,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                    r.get::<_, Option<String>>(8)?,
                    r.get::<_, Option<String>>(9)?,
                    r.get::<_, f64>(10)?,
                    r.get::<_, String>(11)?,
                ])?;
            }
            drop(insert);
            drop(rows);
            drop(stmt);

            // 組の採否。
            let mut stmt = shelf.prepare(
                "SELECT lx.provider,lx.external_key,rx.provider,rx.external_key,f.decision,f.rule_version
                 FROM collection_pair_feedback f
                 JOIN external_identities lx ON lx.entity_id=f.left_work_id AND lx.kind='work'
                 JOIN external_identities rx ON rx.entity_id=f.right_work_id AND rx.kind='work'",
            )?;
            let mut rows = stmt.query([])?;
            let mut insert = tx.prepare(
                "INSERT OR REPLACE INTO collection_pair_feedback(left_source,left_source_id,right_source,
                     right_source_id,decision,rule_version) VALUES(?1,?2,?3,?4,?5,?6)",
            )?;
            while let Some(r) = rows.next()? {
                // 旧棚の組は取得元の鍵の小さいほうを左に置く。
                let mut left = (r.get::<_, String>(0)?, r.get::<_, String>(1)?);
                let mut right = (r.get::<_, String>(2)?, r.get::<_, String>(3)?);
                if left > right {
                    std::mem::swap(&mut left, &mut right);
                }
                insert.execute(params![
                    left.0,
                    left.1,
                    right.0,
                    right.1,
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                ])?;
            }
            drop(insert);
            drop(rows);
            drop(stmt);

            // 今あるコレクションと顔ぶれ。
            let mut stmt = shelf.prepare("SELECT id,title,description,ordered FROM collections")?;
            let mut rows = stmt.query([])?;
            let mut insert = tx.prepare(
                "INSERT INTO work_collections(id,name,description,collection_kind) VALUES(?1,?2,?3,?4)",
            )?;
            while let Some(r) = rows.next()? {
                let ordered: bool = r.get(3)?;
                insert.execute(params![
                    uuid(&r.get::<_, Vec<u8>>(0)?)?.to_string(),
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    if ordered { "ordered" } else { "unordered" },
                ])?;
            }
            drop(insert);
            drop(rows);
            drop(stmt);
            let mut stmt = shelf.prepare(
                "SELECT m.collection_id,x.provider,x.external_key,m.work_id,m.title_at_add,m.author_at_add,m.position
                 FROM collection_members m
                 JOIN external_identities x ON x.entity_id=m.work_id AND x.kind='work'",
            )?;
            let mut rows = stmt.query([])?;
            let mut insert = tx.prepare(
                "INSERT OR IGNORE INTO work_collection_members(collection_id,source,source_id,download_id,
                     title_snapshot,author_snapshot,position) VALUES(?1,?2,?3,?4,?5,?6,?7)",
            )?;
            while let Some(r) = rows.next()? {
                insert.execute(params![
                    uuid(&r.get::<_, Vec<u8>>(0)?)?.to_string(),
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    by_work.get(&uuid(&r.get::<_, Vec<u8>>(3)?)?),
                    r.get::<_, String>(4)?,
                    r.get::<_, String>(5)?,
                    r.get::<_, i64>(6)?,
                ])?;
            }
            drop(insert);
            drop(rows);
            drop(stmt);
            tx.commit()?;
        }

        // 意味索引の文書の鍵。
        let mut doc_keys = HashMap::new();
        let mut from_doc = HashMap::new();
        let mut stmt = shelf.prepare("SELECT work_id,doc_key FROM semantic_documents")?;
        let mut rows = stmt.query([])?;
        while let Some(r) = rows.next()? {
            if let Some(id) = by_work.get(&uuid(&r.get::<_, Vec<u8>>(0)?)?) {
                let key: i64 = r.get(1)?;
                doc_keys.insert(*id, key);
                from_doc.insert(key, *id);
            }
        }
        drop(rows);
        drop(stmt);
        drop(shelf);
        Ok(Self {
            library,
            conn,
            works,
            by_work,
            doc_keys,
            from_doc,
        })
    }

    /// 作業用の表の番号に当たる棚の作品。
    pub fn work(&self, id: i64) -> Option<LibraryId> {
        usize::try_from(id - 1)
            .ok()
            .and_then(|index| self.works.get(index))
            .copied()
    }

    /// 棚の作品に当たる作業用の表の番号。
    pub fn id_of(&self, work: LibraryId) -> Option<i64> {
        self.by_work.get(&work).copied()
    }

    pub fn library(&self) -> &Library {
        self.library
    }

    fn semantic_storage(&self) -> std::path::PathBuf {
        self.library.semantic_storage()
    }
}

impl SuggestionSource for ShelfSuggestions<'_> {
    fn corpus(&self) -> Result<Corpus<'_>, String> {
        Ok(Corpus::Borrowed(&self.conn))
    }

    fn centroids(&self, wanted: &HashSet<i64>) -> Result<HashMap<i64, Vec<f32>>, String> {
        let keys = wanted
            .iter()
            .filter_map(|id| self.doc_keys.get(id))
            .copied()
            .collect::<HashSet<_>>();
        if keys.is_empty() {
            return Ok(HashMap::new());
        }
        let vectors =
            crate::database::semantic_index::work_centroids(&self.semantic_storage(), &keys)?;
        Ok(vectors
            .into_iter()
            .filter_map(|(key, vector)| self.from_doc.get(&key).map(|id| (*id, vector)))
            .collect())
    }

    fn semantic_ready(&self) -> bool {
        !self.doc_keys.is_empty()
            && crate::database::semantic_index::status(&self.semantic_storage()).indexed_works > 0
    }

    fn similar_works(&self, seeds: &[i64], limit: usize) -> Result<Vec<(i64, f64)>, String> {
        let keys = seeds
            .iter()
            .filter_map(|id| self.doc_keys.get(id))
            .copied()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(Vec::new());
        }
        let hits =
            crate::database::semantic_index::similar_works(&self.semantic_storage(), &keys, limit)?;
        Ok(hits
            .into_iter()
            .filter_map(|(key, score)| self.from_doc.get(&key).map(|id| (*id, score)))
            .collect())
    }

    fn plain_text(&self, id: i64) -> Option<String> {
        let work = self.work(id)?;
        self.library
            .search_document(work)
            .ok()
            .flatten()
            .map(|doc| doc.body)
    }
}
