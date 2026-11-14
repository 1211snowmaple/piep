//! 棚の一覧。保存状態・有効な編集・利用者の状態から作り直せる派生の表を、並べ替えの鍵ごとの
//! 索引で引く。続きのページは位置（鍵と作品）で引くので、何ページ目でも同じ速さで返る。

use super::{Error, Fingerprint, Library, LibraryId, Result};
use rusqlite::{params, Connection, OptionalExtension, ToSql};
use serde::{Deserialize, Serialize};

/// 一覧の行の作り方の版。変えたら、書き込みで開いたときに全行を作り直す。
pub(super) const RECIPE: i64 = 2;
const PAGE_LIMIT: usize = 500;

/// 一作品の一覧の行。?1 作品、?2 一覧の持ち主、?3 有効な編集、?4 編集の題、?5 作り方の版。
/// 題は旧棚と同じく、編集の題（前後の空白を除いて空でなければ）か、保存版の題。
/// 容量は保存した原本・画像・添付で、同じ中身は一度だけ数える。原本から取り出した本文は数えない。
const ROW_SQL: &str = "SELECT c.entity_id,c.snapshot_id,?3,
 coalesce((SELECT provider FROM external_identities INDEXED BY external_identity_entity WHERE entity_id=c.entity_id AND kind='work' ORDER BY provider,external_key LIMIT 1),''),
 coalesce((SELECT external_key FROM external_identities INDEXED BY external_identity_entity WHERE entity_id=c.entity_id AND kind='work' ORDER BY provider,external_key LIMIT 1),''),
 w.content_type,
 coalesce(nullif(trim(?4),''),trim(CAST(coalesce(json_extract(s.metadata_json,'$.title'),'') AS TEXT))),
 CAST(coalesce(json_extract(s.metadata_json,'$.author_name'),'') AS TEXT),
 r.discovered_at,
 CASE json_type(s.metadata_json,'$.create_date') WHEN 'integer' THEN json_extract(s.metadata_json,'$.create_date') END,
 (SELECT max(v) FROM (SELECT st.source_updated_at AS v FROM source_entity_state st WHERE st.entity_id=c.entity_id
   UNION ALL SELECT CASE json_type(s.metadata_json,'$.update_date') WHEN 'integer' THEN json_extract(s.metadata_json,'$.update_date') END)),
 coalesce((SELECT t.text_length FROM body_text_stats t WHERE t.file_id=s.body_file_id),
   CASE json_type(s.metadata_json,'$.text_length') WHEN 'integer' THEN max(json_extract(s.metadata_json,'$.text_length'),0) END,0),
 (SELECT coalesce(sum(f.original_size),0) FROM content_files f JOIN (
   SELECT file_id FROM snapshot_files WHERE snapshot_id=c.snapshot_id
   UNION SELECT mr.file_id FROM snapshot_media m JOIN media_renditions mr ON mr.id=m.rendition_id WHERE m.snapshot_id=c.snapshot_id
   UNION SELECT o.raw_file_id FROM snapshot_observations so JOIN acquisition_observations o ON o.id=so.observation_id WHERE so.snapshot_id=c.snapshot_id AND o.result_snapshot_id=c.snapshot_id
   ) selected ON selected.file_id=f.id),
 (SELECT count(*) FROM snapshot_media WHERE snapshot_id=c.snapshot_id)
   +(SELECT count(*) FROM snapshot_files WHERE snapshot_id=c.snapshot_id AND role<>'raw'),
 r.ordinal,
 coalesce((SELECT favorite FROM work_user_state WHERE work_id=c.entity_id AND user_id=?2),0),
 ?5
FROM entity_current c
JOIN library_items li ON li.work_id=c.entity_id
JOIN works w ON w.id=c.entity_id
JOIN revision_snapshots s ON s.id=c.snapshot_id
JOIN source_revisions r ON r.id=s.revision_id";
const COLUMNS: &str = "work_id,snapshot_id,edit_id,provider,external_key,content_type,title,author_name,saved_at,published_at,updated_at,text_length,logical_size,media_count,version_number,favorite,recipe";

/// 並べ替えの鍵。旧棚の並べ替えと同じ意味を持たせ、同じ値の中は作品の登録順で並べる。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkSort {
    /// 保存日（いまの改稿を初めて保存した日）。
    Saved,
    /// 公開日。なければ保存日。
    Published,
    /// 取得元の更新日。なければ公開日、保存日。
    Updated,
    Title,
    Author,
    Length,
    Size,
    Media,
    Version,
}

impl WorkSort {
    /// 索引の式と一字一句同じにする。違うと索引を使わず全件を並べ替える。
    fn expression(self) -> &'static str {
        match self {
            Self::Saved => "saved_at",
            Self::Published => "coalesce(published_at,saved_at)",
            Self::Updated => "coalesce(updated_at,published_at,saved_at)",
            Self::Title => "title COLLATE NOCASE",
            Self::Author => "author_name COLLATE NOCASE",
            Self::Length => "text_length",
            Self::Size => "logical_size",
            Self::Media => "media_count",
            Self::Version => "version_number",
        }
    }

    fn is_text(self) -> bool {
        matches!(self, Self::Title | Self::Author)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum SortValue {
    Number(i64),
    Text(String),
}

/// 続きのページの位置。前のページの最後の行の鍵と作品。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkCursor {
    pub value: SortValue,
    pub work: LibraryId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkListQuery {
    pub sort: WorkSort,
    pub descending: bool,
    pub limit: usize,
    pub after: Option<WorkCursor>,
    pub provider: Option<String>,
    pub content_type: Option<String>,
    pub favorite: Option<bool>,
    /// 件数を数えるか。続きのページでは数え直さない。
    pub count: bool,
}

impl Default for WorkListQuery {
    fn default() -> Self {
        Self {
            sort: WorkSort::Saved,
            descending: true,
            limit: 60,
            after: None,
            provider: None,
            content_type: None,
            favorite: None,
            count: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct WorkCard {
    pub id: LibraryId,
    pub snapshot_id: LibraryId,
    pub edit_id: Option<LibraryId>,
    pub provider: String,
    pub external_key: String,
    pub content_type: String,
    pub title: String,
    pub author_name: String,
    pub saved_at: i64,
    pub published_at: Option<i64>,
    pub updated_at: Option<i64>,
    pub text_length: u64,
    pub logical_size: u64,
    pub media_count: u64,
    pub version_number: u64,
    pub favorite: bool,
    pub excerpt: Option<String>,
    pub tags: Vec<String>,
    pub cover: Option<Fingerprint>,
    pub series_title: Option<String>,
    pub series_position: Option<i64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkListPage {
    pub items: Vec<WorkCard>,
    pub next: Option<WorkCursor>,
    pub total: Option<u64>,
}

fn id(bytes: Vec<u8>) -> Result<LibraryId> {
    LibraryId::from_slice(&bytes).map_err(|_| Error::Integrity("一覧の識別子が不正です".into()))
}

fn size(value: i64) -> Result<u64> {
    u64::try_from(value).map_err(|_| Error::Integrity("一覧の数が負です".into()))
}

type Row = (
    Vec<u8>,
    Vec<u8>,
    Option<Vec<u8>>,
    String,
    String,
    String,
    String,
    String,
    i64,
    Option<i64>,
    Option<i64>,
    [i64; 4],
    bool,
);

/// `COLUMNS` の順に読む。作り方の版（最後の列）は読まない。
fn read_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Row> {
    Ok((
        r.get(0)?,
        r.get(1)?,
        r.get(2)?,
        r.get(3)?,
        r.get(4)?,
        r.get(5)?,
        r.get(6)?,
        r.get(7)?,
        r.get(8)?,
        r.get(9)?,
        r.get(10)?,
        [r.get(11)?, r.get(12)?, r.get(13)?, r.get(14)?],
        r.get(15)?,
    ))
}

fn card(row: Row) -> Result<WorkCard> {
    let (
        work,
        snapshot,
        edit,
        provider,
        external_key,
        content_type,
        title,
        author_name,
        saved_at,
        published_at,
        updated_at,
        numbers,
        favorite,
    ) = row;
    Ok(WorkCard {
        id: id(work)?,
        snapshot_id: id(snapshot)?,
        edit_id: edit.map(id).transpose()?,
        provider,
        external_key,
        content_type,
        title,
        author_name,
        saved_at,
        published_at,
        updated_at,
        text_length: size(numbers[0])?,
        logical_size: size(numbers[1])?,
        media_count: size(numbers[2])?,
        version_number: size(numbers[3])?,
        favorite,
        excerpt: None,
        tags: Vec::new(),
        cover: None,
        series_title: None,
        series_position: None,
    })
}

/// 一ページを引く問い合わせ。件数は絞り込みの値だけ、ページは続きの位置と上限まで使う。
pub(super) struct PageSql {
    pub(super) count: String,
    pub(super) page: String,
    pub(super) values: Vec<Box<dyn ToSql>>,
    pub(super) filter_values: usize,
}

impl PageSql {
    pub(super) fn new(query: &WorkListQuery, limit: usize) -> Result<Self> {
        let key = query.sort.expression();
        let mut filters = Vec::new();
        let mut values: Vec<Box<dyn ToSql>> = Vec::new();
        if let Some(provider) = &query.provider {
            values.push(Box::new(provider.clone()));
            filters.push(format!("provider=?{}", values.len()));
        }
        if let Some(content_type) = &query.content_type {
            values.push(Box::new(content_type.clone()));
            filters.push(format!("content_type=?{}", values.len()));
        }
        if let Some(favorite) = query.favorite {
            values.push(Box::new(favorite));
            filters.push(format!("favorite=?{}", values.len()));
        }
        let filter_values = values.len();
        let clause = |filters: &[String]| {
            if filters.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", filters.join(" AND "))
            }
        };
        let count = format!(
            "SELECT count(*) FROM work_current_projection{}",
            clause(&filters)
        );
        let direction = if query.descending { "DESC" } else { "ASC" };
        if let Some(after) = &query.after {
            match (&after.value, query.sort.is_text()) {
                (SortValue::Text(value), true) => values.push(Box::new(value.clone())),
                (SortValue::Number(value), false) => values.push(Box::new(*value)),
                _ => {
                    return Err(Error::Integrity(
                        "続きの位置が並べ替えの鍵と合いません".into(),
                    ))
                }
            }
            values.push(Box::new(after.work.as_bytes().to_vec()));
            let operator = if query.descending { "<" } else { ">" };
            filters.push(format!(
                "({key},work_id){operator}(?{},?{})",
                values.len() - 1,
                values.len()
            ));
        }
        values.push(Box::new(limit as i64 + 1));
        let value_column = key.trim_end_matches(" COLLATE NOCASE");
        let page = format!(
            "SELECT {COLUMNS},{value_column} FROM work_current_projection{} ORDER BY {key} {direction},work_id {direction} LIMIT ?{}",
            clause(&filters),
            values.len()
        );
        Ok(Self {
            count,
            page,
            values,
            filter_values,
        })
    }
}

impl Library {
    /// 一作品の一覧の行を作り直す。保存・編集・お気に入りなど、行の値を変える書き込みの中で呼ぶ。
    pub(super) fn refresh_work_list(&self, tx: &Connection, work: LibraryId) -> Result<()> {
        let edit: Option<(Vec<u8>, Vec<u8>)> = tx
            .query_row(
                "SELECT d.active_id,r.content_file_id FROM edit_documents d JOIN edit_revisions r ON r.id=d.active_id WHERE d.work_id=?1 AND d.user_id=?2",
                params![work.as_bytes().as_slice(), self.id.as_bytes().as_slice()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let (edit, title) = match edit {
            Some((edit, file)) => (Some(edit), self.edit_title(tx, &file)?),
            None => (None, None),
        };
        tx.execute(
            "DELETE FROM work_current_projection WHERE work_id=?1",
            [work.as_bytes().as_slice()],
        )?;
        tx.prepare_cached(&format!(
            "INSERT INTO work_current_projection({COLUMNS}) {ROW_SQL} WHERE c.entity_id=?1"
        ))?
        .execute(params![
            work.as_bytes().as_slice(),
            self.id.as_bytes().as_slice(),
            edit,
            title,
            RECIPE
        ])?;
        Ok(())
    }

    /// 一覧を全部作り直す。編集のない作品は一度の問い合わせで、編集のある作品だけを一つずつ。
    pub(super) fn rebuild_work_list(&self, tx: &Connection) -> Result<()> {
        tx.execute("DELETE FROM work_current_projection", [])?;
        tx.execute(
            &format!("INSERT INTO work_current_projection({COLUMNS}) {ROW_SQL}"),
            params![
                None::<Vec<u8>>,
                self.id.as_bytes().as_slice(),
                None::<Vec<u8>>,
                None::<String>,
                RECIPE
            ],
        )?;
        let edited = tx
            .prepare(
                "SELECT work_id FROM edit_documents WHERE user_id=?1 AND active_id IS NOT NULL",
            )?
            .query_map([self.id.as_bytes().as_slice()], |r| r.get::<_, Vec<u8>>(0))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        for work in edited {
            self.refresh_work_list(tx, id(work)?)?;
        }
        Ok(())
    }

    /// 一覧の行が欠けているか、作り方が古ければ作り直す。書き込みで開いたときに一度だけ確かめる。
    pub(super) fn ensure_work_list(&self) -> Result<()> {
        let mut conn = self.conn.lock().map_err(|_| Error::Busy)?;
        let stale: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM library_items li LEFT JOIN work_current_projection p ON p.work_id=li.work_id WHERE p.work_id IS NULL OR p.recipe<>?1)",
            [RECIPE],
            |r| r.get(0),
        )?;
        if stale {
            let tx = conn.transaction()?;
            self.rebuild_work_list(&tx)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// 一覧の全行が、いまの保存状態から作った値と一致するかを確かめる。
    pub(crate) fn verify_work_list(&self, conn: &Connection) -> Result<()> {
        let missing: i64 = conn.query_row(
            "SELECT count(*) FROM library_items li LEFT JOIN work_current_projection p ON p.work_id=li.work_id WHERE p.work_id IS NULL",
            [],
            |r| r.get(0),
        )?;
        let orphan: i64 = conn.query_row(
            "SELECT count(*) FROM work_current_projection p LEFT JOIN library_items li ON li.work_id=p.work_id WHERE li.work_id IS NULL",
            [],
            |r| r.get(0),
        )?;
        let uncounted: i64 = conn.query_row(
            "SELECT count(*) FROM work_current_projection p JOIN revision_snapshots s ON s.id=p.snapshot_id LEFT JOIN body_text_stats t ON t.file_id=s.body_file_id WHERE s.body_file_id IS NOT NULL AND t.file_id IS NULL",
            [],
            |r| r.get(0),
        )?;
        if uncounted != 0 {
            return Err(Error::Integrity(format!(
                "本文の文字数を数えていない作品が {uncounted} 件あります"
            )));
        }
        if missing != 0 || orphan != 0 {
            return Err(Error::Integrity(format!(
                "一覧の行が棚と一致しません（欠け {missing}・余り {orphan}）"
            )));
        }
        let edited: std::collections::HashMap<Vec<u8>, (Vec<u8>, Vec<u8>)> = conn
            .prepare("SELECT d.work_id,d.active_id,r.content_file_id FROM edit_documents d JOIN edit_revisions r ON r.id=d.active_id WHERE d.user_id=?1")?
            .query_map([self.id.as_bytes().as_slice()], |r| {
                Ok((r.get(0)?, (r.get(1)?, r.get(2)?)))
            })?
            .collect::<std::result::Result<_, _>>()?;
        // 編集のない作品は一度に比べる。値の食い違いは EXCEPT の両向きで見つける。
        let differing: i64 = conn.query_row(
            &format!(
                "SELECT count(*) FROM (
                   SELECT * FROM ({ROW_SQL}) WHERE NOT EXISTS(SELECT 1 FROM edit_documents d WHERE d.work_id=entity_id AND d.user_id=?2 AND d.active_id IS NOT NULL)
                   EXCEPT SELECT {COLUMNS} FROM work_current_projection)"
            ),
            params![
                None::<Vec<u8>>,
                self.id.as_bytes().as_slice(),
                None::<Vec<u8>>,
                None::<String>,
                RECIPE
            ],
            |r| r.get(0),
        )?;
        if differing != 0 {
            return Err(Error::Integrity(format!(
                "一覧の {differing} 行が保存状態と一致しません"
            )));
        }
        for (work, (edit, file)) in edited {
            let title = self.edit_title(conn, &file)?;
            let same: bool = conn.query_row(
                &format!(
                    "SELECT EXISTS(SELECT * FROM ({ROW_SQL} WHERE c.entity_id=?1) INTERSECT SELECT {COLUMNS} FROM work_current_projection WHERE work_id=?1)"
                ),
                params![work, self.id.as_bytes().as_slice(), edit, title, RECIPE],
                |r| r.get(0),
            )?;
            if !same {
                return Err(Error::Integrity(
                    "編集のある作品の一覧の行が一致しません".into(),
                ));
            }
        }
        Ok(())
    }

    /// 一作品のカード。作品の詳細の画面の見出しに使う。棚にない作品は見つからない扱い。
    pub fn work_card(&self, work: LibraryId) -> Result<WorkCard> {
        let conn = self.reader()?;
        let row = conn
            .prepare_cached(&format!(
                "SELECT {COLUMNS} FROM work_current_projection WHERE work_id=?1"
            ))?
            .query_row([work.as_bytes().as_slice()], read_row)
            .optional()?
            .ok_or(Error::ResourceUnavailable)?;
        Self::work_card_from_conn(&conn, self.id, card(row)?)
    }

    /// 棚の一覧を一ページ返す。続きは `next` を `after` に渡す。
    pub fn list_works(&self, query: &WorkListQuery) -> Result<WorkListPage> {
        let conn = self.reader()?;
        Self::list_works_from_conn(&conn, self.id, query)
    }

    pub(super) fn list_works_from_conn(
        conn: &Connection,
        owner: LibraryId,
        query: &WorkListQuery,
    ) -> Result<WorkListPage> {
        let limit = query.limit.clamp(1, PAGE_LIMIT);
        let page = PageSql::new(query, limit)?;
        let total = if query.count {
            let count: i64 = conn.prepare_cached(&page.count)?.query_row(
                rusqlite::params_from_iter(
                    page.values[..page.filter_values].iter().map(|v| v.as_ref()),
                ),
                |r| r.get(0),
            )?;
            Some(size(count)?)
        } else {
            None
        };
        let values = page.values;
        let sql = page.page;
        let mut statement = conn.prepare_cached(&sql)?;
        let rows = statement
            .query_map(
                rusqlite::params_from_iter(values.iter().map(|v| v.as_ref())),
                |r| Ok((read_row(r)?, r.get::<_, rusqlite::types::Value>(17)?)),
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        drop(statement);
        let more = rows.len() > limit;
        let mut items = Vec::with_capacity(limit.min(rows.len()));
        let mut next = None;
        for (row, value) in rows.into_iter().take(limit) {
            let value = match value {
                rusqlite::types::Value::Integer(n) => SortValue::Number(n),
                rusqlite::types::Value::Text(s) => SortValue::Text(s),
                _ => return Err(Error::Integrity("一覧の並べ替えの鍵が不正です".into())),
            };
            let card = Self::work_card_from_conn(conn, owner, card(row)?)?;
            next = Some(WorkCursor {
                value,
                work: card.id,
            });
            items.push(card);
        }
        if !more {
            next = None;
        }
        Ok(WorkListPage { items, next, total })
    }

    /// カードに出す、一覧の表に持たない値（抜粋・タグ・表紙・シリーズ）を、ページの行だけ引く。
    fn work_card_from_conn(
        conn: &Connection,
        owner: LibraryId,
        mut card: WorkCard,
    ) -> Result<WorkCard> {
        let work = card.id.as_bytes().to_vec();
        let snapshot = card.snapshot_id.as_bytes().to_vec();
        card.excerpt = conn
            .prepare_cached("SELECT CAST(coalesce(json_extract(metadata_json,'$.caption'),json_extract(metadata_json,'$.excerpt')) AS TEXT) FROM revision_snapshots WHERE id=?1")?
            .query_row([&snapshot], |r| r.get(0))?;
        let mut tags: Vec<String> = conn
            .prepare_cached("SELECT t.name FROM snapshot_tags st JOIN tags t ON t.id=st.tag_id WHERE st.snapshot_id=?1
                UNION SELECT u.name FROM work_user_tags wt JOIN user_tags u ON u.user_id=wt.user_id AND u.id=wt.tag_id WHERE wt.work_id=?2 AND wt.user_id=?3
                ORDER BY 1")?
            .query_map(params![snapshot, work, owner.as_bytes().as_slice()], |r| r.get(0))?
            .collect::<std::result::Result<_, _>>()?;
        tags.dedup();
        card.tags = tags;
        card.cover = conn
            .prepare_cached("SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 AND m.role='cover' ORDER BY m.slot LIMIT 1")?
            .query_row([&snapshot], |r| r.get::<_, Vec<u8>>(0))
            .optional()?
            .map(|bytes| {
                bytes
                    .try_into()
                    .map(Fingerprint)
                    .map_err(|_| Error::Integrity("表紙のハッシュが不正です".into()))
            })
            .transpose()?;
        if let Some((title, position)) = conn
            .prepare_cached("SELECT title,position FROM snapshot_series WHERE snapshot_id=?1 ORDER BY series_id LIMIT 1")?
            .query_row([&snapshot], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?
        {
            card.series_title = Some(title);
            card.series_position = position;
        }
        Ok(card)
    }
}

#[cfg(test)]
#[path = "work_list_tests.rs"]
mod tests;
