//! 作品コレクションとその候補生成。
//!
//! 取得や検索とは独立した、利用者が自分で組み立てる集合を扱う。`queries` の
//! 子モジュールに置いてあるのは、`Database` の接続やプールといった非公開の
//! 内部へ触れる必要があるためで、親の実装詳細をここだけに開いている。

use super::*;

use crate::database::collection_rules;

/// 別版を探すために、いちどに読む同一作者の作品数の上限。
///
/// 1作者が800作持つ棚があるので、無制限にすると詳細画面を開くたびに棚を
/// なめることになる。ここを超える作者では別版の畳み込みをあきらめる。
/// 畳めなくても束は正しく、行が数本増えるだけで済む。
const EDITION_SCAN_LIMIT: usize = 3_000;

/// 公式シリーズの話数がこれだけ離れていたら、隣り合っているとは言わない。
///
/// 抜けが1つある連載を切らない程度に広く、151作のシリーズの端どうしを
/// つながないくらいには狭く。
const SERIES_ADJACENT_GAP: i64 = 2;

/// 投稿がこれだけ近ければ、同じ時期の作品として文脈に数える。
const PUBLISH_NEARBY_DAYS: i64 = 60;

/// メンバーに、作品そのものと別版を差し込む。
///
/// 縮小した投影を返していたころは、棚で使っている `WorkCard` に渡す型が無く、
/// コレクションだけが自前の劣化した行を持っていた。棚と同じ投影を返せば、
/// 一覧の作り込みがそのまま束の中でも効く。
fn attach_member_works(
    conn: &Connection,
    members: &mut [WorkCollectionMember],
) -> Result<(), String> {
    let member_ids = members
        .iter()
        .filter_map(|member| member.download_id)
        .collect::<Vec<_>>();
    if member_ids.is_empty() {
        return Ok(());
    }
    let mut works = load_download_entries(conn, &member_ids)?
        .into_iter()
        .map(|entry| (entry.id, entry))
        .collect::<HashMap<_, _>>();

    let editions = load_member_editions(conn, &works, &member_ids)?;
    for member in members.iter_mut() {
        let Some(id) = member.download_id else {
            continue;
        };
        member.work = works.remove(&id);
        member.editions = editions.get(&id).cloned().unwrap_or_default();
    }
    Ok(())
}

/// 指定した作品を、棚のカードと同じ形で読む。
fn load_download_entries(conn: &Connection, ids: &[i64]) -> Result<Vec<DownloadEntry>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded =
        serde_json::to_string(ids).map_err(|e| format!("Failed to encode work IDs: {e}"))?;
    let sql = format!(
        "{} WHERE d.id IN (SELECT value FROM json_each(?1))",
        download_select_sql_for_projection(Some("libraryGallery"), "NULL", "NULL")
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("Failed to prepare member works: {e}"))?;
    let rows = stmt
        .query_map(params![encoded], download_entry_from_row)
        .map_err(|e| format!("Failed to query member works: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read member works: {e}"))
}

/// メンバーごとの別版を集める。
///
/// 版の呼び名を落とした鍵が一致し、作者が同じで、そのメンバー自身ではない
/// 作品を別版とみなす。**前編と後編は鍵が違う**ので、ここで畳まれることはない。
fn load_member_editions(
    conn: &Connection,
    works: &HashMap<i64, DownloadEntry>,
    member_ids: &[i64],
) -> Result<HashMap<i64, Vec<DownloadEntry>>, String> {
    let mut keys: HashMap<String, Vec<i64>> = HashMap::new();
    let mut authors: HashSet<String> = HashSet::new();
    let mut author_names: HashSet<String> = HashSet::new();
    for entry in works.values() {
        keys.entry(collection_rules::edition_match_key(&entry.title))
            .or_default()
            .push(entry.id);
        let author = author_identity(entry);
        if !author.is_empty() {
            authors.insert(author);
            author_names.insert(entry.author_name.trim().to_string());
        }
    }
    if authors.is_empty() {
        return Ok(HashMap::new());
    }
    let author_json = serde_json::to_string(&author_names.iter().collect::<Vec<_>>())
        .map_err(|e| format!("Failed to encode edition authors: {e}"))?;
    // SQL 側では表示名でざっくり絞り、正規化した一致は Rust 側で見る。
    // 正規化の規則を SQL に写すと、二箇所が別々にずれていく。
    let mut stmt = conn
        .prepare(
            "SELECT id, title, author_name
             FROM downloads
             WHERE author_name IN (SELECT value FROM json_each(?1))
             LIMIT ?2",
        )
        .map_err(|e| format!("Failed to prepare edition scan: {e}"))?;
    let member_set = member_ids.iter().copied().collect::<HashSet<_>>();
    let mut edition_ids: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut scanned = 0usize;
    let rows = stmt
        .query_map(params![author_json, EDITION_SCAN_LIMIT as i64 + 1], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|e| format!("Failed to query edition scan: {e}"))?;
    for row in rows {
        let (id, title, author) = row.map_err(|e| format!("Failed to read edition scan: {e}"))?;
        scanned += 1;
        if scanned > EDITION_SCAN_LIMIT {
            // ここまでに集めたぶんは捨てない。
            //
            // 空を返していたが、SQL は束に居る**すべての作者**をまとめて
            // 引いている。つまり中くらいの作者が数人そろっただけで上限に触れ、
            // その一人のためではなく**全員ぶんの別版畳み込みが消えていた**。
            //
            // 途中まででも畳めた組は正しい。畳み損ねたぶんは行が数本増える
            // だけで、これはこの上限がもともと引き受けている損である。
            log::warn!(
                "Edition scan stopped at {EDITION_SCAN_LIMIT} rows; folding what was gathered"
            );
            break;
        }
        if member_set.contains(&id) {
            continue;
        }
        // SQL は生の表示名で絞っただけ。正規化した名前でもう一度突き合わせる。
        if !authors.contains(&crate::database::search::normalize_search_text(&author)) {
            continue;
        }
        if let Some(owners) = keys.get(&collection_rules::edition_match_key(&title)) {
            for owner in owners {
                // 鍵は題名全体から作るので、同じ作者が同じ題名を二度使って
                // いることを意味する。実データではその多くが pixiv と FANBOX に
                // 同じ作品を出したもので、続きではなく別版だった。
                edition_ids.entry(*owner).or_default().push(id);
            }
        }
    }
    if edition_ids.is_empty() {
        return Ok(HashMap::new());
    }
    let flat = edition_ids
        .values()
        .flatten()
        .copied()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let loaded = load_download_entries(conn, &flat)?
        .into_iter()
        .map(|entry| (entry.id, entry))
        .collect::<HashMap<_, _>>();
    Ok(edition_ids
        .into_iter()
        .map(|(owner, ids)| {
            let mut entries = ids
                .into_iter()
                .filter_map(|id| loaded.get(&id).cloned())
                .collect::<Vec<_>>();
            // 長い本文が先。サンプルは導入だけのことが多く、後ろが読みやすい。
            entries.sort_by(|left, right| {
                right
                    .text_length
                    .cmp(&left.text_length)
                    .then_with(|| left.id.cmp(&right.id))
            });
            (owner, entries)
        })
        .collect())
}

/// 行の id から、束のメンバーとして保存できる形を作る。
///
/// 並びは投稿日、それが無ければ保存日。同着は id で決める。順序は毎回
/// 同じでなければならない — 同じ操作が二度目に違う並びになると、直したのか
/// 壊れたのかが分からなくなる。
fn resolve_member_inputs(
    conn: &Connection,
    download_ids: &[i64],
) -> Result<Vec<WorkCollectionMemberInput>, String> {
    if download_ids.is_empty() {
        return Err("作品が選ばれていません".to_string());
    }
    if download_ids.len() > 5_000 {
        return Err("Too many collection members in one request".to_string());
    }
    let mut seen_ids = HashSet::new();
    let unique_ids = download_ids
        .iter()
        .copied()
        .filter(|id| seen_ids.insert(*id))
        .collect::<Vec<_>>();
    let encoded = serde_json::to_string(&unique_ids)
        .map_err(|e| format!("Failed to encode selected work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT source, source_id, title, author_name
             FROM downloads
             WHERE id IN (SELECT value FROM json_each(?1))
             ORDER BY COALESCE(source_created_at, downloaded_at) ASC, id ASC",
        )
        .map_err(|e| format!("Failed to prepare selected works: {e}"))?;
    let rows = stmt
        .query_map(params![encoded], |row| {
            Ok(WorkCollectionMemberInput {
                source: row.get(0)?,
                source_id: row.get(1)?,
                title_snapshot: Some(row.get(2)?),
                author_snapshot: Some(row.get(3)?),
                position: None,
                // 既存メンバーの注釈・ピン・追加元を壊さない。
                // 新規行では INSERT 側の既定値が使われる。
                member_role: None,
                added_by: None,
                pinned: None,
                note: None,
            })
        })
        .map_err(|e| format!("Failed to query selected works: {e}"))?;
    let inputs = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read selected works: {e}"))?;
    if inputs.len() != unique_ids.len() {
        return Err("選んだ作品の一部が見つかりません。棚を更新してやり直してください".to_string());
    }
    Ok(inputs)
}

/// 並べ替えの基準になる日付。投稿日が無ければ保存日、それも無ければ加入日。
fn member_published_at(member: &WorkCollectionMember) -> String {
    member
        .work
        .as_ref()
        .and_then(|work| {
            work.source_created_at
                .clone()
                .or(Some(work.downloaded_at.clone()))
        })
        .unwrap_or_else(|| member.created_at.clone())
}

/// 取得元をまたいで同じ作者を指す鍵。
///
/// `author_id` は取得元ごとに違うので、別版探しには使えない。同じ人が pixiv と
/// FANBOX に同じ作品を出したものを見つけるには、両方に共通する表示名で引く
/// しかない。題名の一致も同時に要求するので、名前だけで畳むことはない。
fn author_identity(entry: &DownloadEntry) -> String {
    let name = entry.author_name.trim();
    if !name.is_empty() {
        crate::database::search::normalize_search_text(name)
    } else if !entry.author_id.is_empty() {
        format!("{}:{}", entry.source, entry.author_id)
    } else {
        String::new()
    }
}

/// コレクション本体の書き込み。呼び出し側が開いたトランザクションに
/// 相乗りできるよう `Connection` だけを受け取る。
fn write_collection_row(
    conn: &Connection,
    input: &WorkCollectionInput,
    id: &str,
    updating: bool,
    now: &str,
) -> Result<(), String> {
    let name = validate_collection_name(&input.name)?;
    let description = normalize_bounded_optional(&input.description, 10_000, "Description")?;
    let kind = validate_collection_kind(&input.collection_kind)?;
    let cover_download_patch = input.cover_download_id_patch();
    if let Some(Some(cover_id)) = cover_download_patch {
        let exists = conn
            .query_row(
                "SELECT 1 FROM downloads WHERE id = ?1",
                params![cover_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|e| format!("Failed to validate collection cover: {e}"))?
            .is_some();
        if !exists {
            return Err("Collection cover work was not found".to_string());
        }
    }
    let cover_mode = validate_cover_mode(input.cover_mode.as_deref())?;
    let cover_image_patch = match input.cover_image_path_patch() {
        None => None,
        Some(value) => {
            let value = value.map(ToOwned::to_owned);
            Some(normalize_bounded_optional(
                &value,
                2_000,
                "Cover image path",
            )?)
        }
    };
    let name_source = validate_name_source(input.name_source.as_deref())?;
    let track = validate_collection_track(input.track.as_deref())?;

    if updating {
        let changed = conn
            .execute(
                "UPDATE work_collections
                 SET name = ?1, description = ?2, collection_kind = ?3,
                     cover_download_id = CASE WHEN ?7 THEN ?4 ELSE cover_download_id END,
                     revision = revision + 1, updated_at = ?5,
                     cover_mode = COALESCE(?8, cover_mode),
                     cover_image_path = CASE WHEN ?9 THEN ?10 ELSE cover_image_path END,
                     name_source = COALESCE(?11, name_source),
                     track = COALESCE(?12, track)
                 WHERE id = ?6",
                params![
                    name,
                    description,
                    kind,
                    cover_download_patch.flatten(),
                    now,
                    id,
                    cover_download_patch.is_some(),
                    cover_mode,
                    cover_image_patch.is_some(),
                    cover_image_patch.flatten(),
                    name_source,
                    track,
                ],
            )
            .map_err(|e| format!("Failed to update collection: {e}"))?;
        if changed == 0 {
            return Err("Collection not found".to_string());
        }
    } else {
        conn.execute(
            "INSERT INTO work_collections (
                id, name, description, collection_kind, cover_download_id,
                revision, created_at, updated_at,
                cover_mode, cover_image_path, name_source, track
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6,
                       COALESCE(?7, 'mosaic'), ?8,
                       COALESCE(?9, 'manual'), COALESCE(?10, 'manual'))",
            params![
                id,
                name,
                description,
                kind,
                cover_download_patch.flatten(),
                now,
                cover_mode,
                cover_image_patch.flatten(),
                name_source,
                track,
            ],
        )
        .map_err(|e| format!("Failed to create collection: {e}"))?;
    }
    Ok(())
}

/// メンバー追加を呼び出し側のトランザクションに参加させる。
fn add_collection_members_in_connection(
    conn: &Connection,
    collection_id: &str,
    inputs: &[WorkCollectionMemberInput],
    now: &str,
) -> Result<(), String> {
    if inputs.len() > 5_000 {
        return Err("Too many collection members in one request".to_string());
    }
    let exists = conn
        .query_row(
            "SELECT 1 FROM work_collections WHERE id = ?1",
            params![collection_id],
            |_| Ok(()),
        )
        .optional()
        .map_err(|e| format!("Failed to validate collection: {e}"))?
        .is_some();
    if !exists {
        return Err("Collection not found".to_string());
    }
    let mut next_position: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(position), -1) + 1
             FROM work_collection_members WHERE collection_id = ?1",
            params![collection_id],
            |row| row.get(0),
        )
        .map_err(|e| format!("Failed to determine collection position: {e}"))?;
    let mut seen = HashSet::new();
    for input in inputs {
        let source = validate_work_key_part(&input.source, "Source")?;
        let source_id = validate_work_key_part(&input.source_id, "Source ID")?;
        if !seen.insert((source.clone(), source_id.clone())) {
            continue;
        }
        let (download_id, title, author): (i64, String, String) = conn
            .query_row(
                "SELECT id, title, author_name FROM downloads
                 WHERE source = ?1 AND source_id = ?2",
                params![source, source_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to resolve collection member: {e}"))?
            .ok_or_else(|| format!("Saved work not found: {source}/{source_id}"))?;
        let position = input.position.unwrap_or(next_position).max(0);
        next_position = next_position.max(position + 1);
        let member_role = normalize_member_role(input.member_role.as_deref())?;
        let added_by = normalize_added_by(input.added_by.as_deref())?;
        let note = normalize_bounded_optional(&input.note, 10_000, "Member note")?;
        conn.execute(
            "INSERT INTO work_collection_members (
                collection_id, source, source_id, download_id,
                title_snapshot, author_snapshot, position, member_role,
                added_by, pinned, note, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
             ON CONFLICT(collection_id, source, source_id) DO UPDATE SET
                download_id = excluded.download_id,
                title_snapshot = excluded.title_snapshot,
                author_snapshot = excluded.author_snapshot,
                position = CASE WHEN ?13 THEN excluded.position
                                ELSE work_collection_members.position END,
                member_role = CASE WHEN ?14 THEN excluded.member_role
                                   ELSE work_collection_members.member_role END,
                added_by = CASE WHEN ?15 THEN excluded.added_by
                                ELSE work_collection_members.added_by END,
                pinned = CASE WHEN ?16 THEN excluded.pinned
                              ELSE work_collection_members.pinned END,
                note = CASE WHEN ?17 THEN excluded.note
                            ELSE work_collection_members.note END,
                updated_at = excluded.updated_at",
            params![
                collection_id,
                source,
                source_id,
                download_id,
                title,
                author,
                position,
                member_role,
                added_by,
                if input.pinned.unwrap_or(false) { 1 } else { 0 },
                note,
                now,
                input.position.is_some(),
                input.member_role.is_some(),
                input.added_by.is_some(),
                input.pinned.is_some(),
                input.note.is_some(),
            ],
        )
        .map_err(|e| format!("Failed to add collection member: {e}"))?;
    }
    conn.execute(
        "UPDATE work_collections
         SET revision = revision + 1, updated_at = ?1 WHERE id = ?2",
        params![now, collection_id],
    )
    .map_err(|e| format!("Failed to update collection revision: {e}"))?;
    Ok(())
}

impl Database {
    /// 利用者が作成した作品コレクションを、最近更新した順で返す。
    pub fn list_work_collections(&self) -> Result<Vec<WorkCollectionSummary>, String> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{COLLECTION_SUMMARY_SELECT}{COLLECTION_SUMMARY_TAIL}"
            ))
            .map_err(|e| format!("Failed to prepare collection list: {e}"))?;
        let rows = stmt
            .query_map([], work_collection_summary_from_row)
            .map_err(|e| format!("Failed to query collections: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read collections: {e}"))
    }

    /// コレクションと、その安定した並び順のメンバーを返す。
    pub fn get_work_collection(&self, collection_id: &str) -> Result<WorkCollection, String> {
        let collection_id = validate_collection_id(collection_id)?;
        let conn = self.read_conn()?;
        let summary = conn
            .query_row(
                &format!("{COLLECTION_SUMMARY_SELECT} WHERE c.id = ?1 GROUP BY c.id"),
                params![collection_id],
                work_collection_summary_from_row,
            )
            .optional()
            .map_err(|e| format!("Failed to query collection: {e}"))?
            .ok_or_else(|| "Collection not found".to_string())?;
        let mut stmt = conn
            .prepare(
                "SELECT m.collection_id, m.source, m.source_id, m.download_id,
                        COALESCE(d.title, m.title_snapshot),
                        COALESCE(d.author_name, m.author_snapshot), d.cover_path,
                        COALESCE(d.text_length, 0), m.position, m.member_role,
                        m.added_by, m.pinned, m.note,
                        CASE WHEN m.download_id IS NULL THEN 1 ELSE 0 END,
                        m.created_at, m.updated_at
                 FROM work_collection_members m
                 LEFT JOIN downloads d ON d.id = m.download_id
                 WHERE m.collection_id = ?1
                 ORDER BY m.position ASC, m.created_at ASC, m.source ASC, m.source_id ASC",
            )
            .map_err(|e| format!("Failed to prepare collection members: {e}"))?;
        let mut members = stmt
            .query_map(params![collection_id], work_collection_member_from_row)
            .map_err(|e| format!("Failed to query collection members: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read collection members: {e}"))?;
        attach_member_works(&conn, &mut members)?;
        Ok(WorkCollection { summary, members })
    }

    /// コレクションを新規作成、または指定 ID のコレクションを更新する。
    pub fn upsert_work_collection(
        &self,
        input: &WorkCollectionInput,
    ) -> Result<WorkCollection, String> {
        let id = match input.id.as_deref() {
            Some(value) => validate_collection_id(value)?.to_string(),
            None => new_collection_id("collection"),
        };
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection transaction failed: {e}"))?;
            write_collection_row(&tx, input, &id, input.id.is_some(), &now)?;
            tx.commit()
                .map_err(|e| format!("Collection commit failed: {e}"))?;
        }
        self.get_work_collection(&id)
    }

    pub fn delete_work_collection(&self, collection_id: &str) -> Result<(), String> {
        let collection_id = validate_collection_id(collection_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let changed = conn
            .execute(
                "DELETE FROM work_collections WHERE id = ?1",
                params![collection_id],
            )
            .map_err(|e| format!("Failed to delete collection: {e}"))?;
        if changed == 0 {
            return Err("Collection not found".to_string());
        }
        Ok(())
    }

    /// バックアップ復元中の外側トランザクションへ、コレクションを合流させる。
    /// ここでは新しいトランザクションを開始しないため、複数分割バックアップの
    /// 部分メンバーも同じ安定キーへ安全に積み上がる。
    pub(crate) fn restore_work_collection(
        &self,
        input: &WorkCollectionInput,
        cover_work: Option<&WorkKey>,
        members: &[WorkCollectionMemberInput],
    ) -> Result<(), String> {
        let id = validate_collection_id(
            input
                .id
                .as_deref()
                .ok_or_else(|| "Restored collection requires an ID".to_string())?,
        )?;
        let name = validate_collection_name(&input.name)?;
        let description = normalize_bounded_optional(&input.description, 10_000, "Description")?;
        let kind = validate_collection_kind(&input.collection_kind)?;
        let cover_mode = validate_cover_mode(input.cover_mode.as_deref())?;
        let cover_image_path = match input.cover_image_path_patch() {
            None => None,
            Some(value) => normalize_bounded_optional(
                &value.map(ToOwned::to_owned),
                2_000,
                "Cover image path",
            )?,
        };
        let name_source = validate_name_source(input.name_source.as_deref())?;
        let track = validate_collection_track(input.track.as_deref())?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let cover_download_id = cover_work
            .map(|key| {
                conn.query_row(
                    "SELECT id FROM downloads WHERE source = ?1 AND source_id = ?2",
                    params![key.source, key.source_id],
                    |row| row.get::<_, i64>(0),
                )
                .optional()
                .map_err(|e| format!("Failed to resolve restored collection cover: {e}"))
            })
            .transpose()?
            .flatten();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO work_collections (
                id, name, description, collection_kind, cover_download_id,
                revision, created_at, updated_at,
                cover_mode, cover_image_path, name_source, track
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6,
                       COALESCE(?7, 'mosaic'), ?8,
                       COALESCE(?9, 'manual'), COALESCE(?10, 'manual'))
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                collection_kind = excluded.collection_kind,
                cover_download_id = COALESCE(excluded.cover_download_id, work_collections.cover_download_id),
                cover_mode = excluded.cover_mode,
                cover_image_path = COALESCE(excluded.cover_image_path, work_collections.cover_image_path),
                name_source = excluded.name_source,
                track = excluded.track,
                revision = work_collections.revision + 1,
                updated_at = excluded.updated_at",
            params![
                id,
                name,
                description,
                kind,
                cover_download_id,
                now,
                cover_mode,
                cover_image_path,
                name_source,
                track,
            ],
        )
        .map_err(|e| format!("Failed to restore collection: {e}"))?;
        for member in members {
            let source = validate_work_key_part(&member.source, "Source")?;
            let source_id = validate_work_key_part(&member.source_id, "Source ID")?;
            let resolved = conn
                .query_row(
                    "SELECT id, title, author_name FROM downloads
                     WHERE source = ?1 AND source_id = ?2",
                    params![source, source_id],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                        ))
                    },
                )
                .optional()
                .map_err(|e| format!("Failed to resolve restored collection member: {e}"))?;
            let (download_id, title, author) = resolved
                .map(|(id, title, author)| (Some(id), title, author))
                .unwrap_or_else(|| {
                    (
                        None,
                        member
                            .title_snapshot
                            .clone()
                            .filter(|value| !value.trim().is_empty())
                            .unwrap_or_else(|| source_id.clone()),
                        member.author_snapshot.clone().unwrap_or_default(),
                    )
                });
            conn.execute(
                "INSERT INTO work_collection_members (
                    collection_id, source, source_id, download_id,
                    title_snapshot, author_snapshot, position, member_role,
                    added_by, pinned, note, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                 ON CONFLICT(collection_id, source, source_id) DO UPDATE SET
                    download_id = excluded.download_id,
                    title_snapshot = CASE WHEN excluded.download_id IS NULL
                        THEN work_collection_members.title_snapshot ELSE excluded.title_snapshot END,
                    author_snapshot = CASE WHEN excluded.download_id IS NULL
                        THEN work_collection_members.author_snapshot ELSE excluded.author_snapshot END,
                    position = excluded.position,
                    member_role = excluded.member_role,
                    added_by = excluded.added_by,
                    pinned = excluded.pinned,
                    note = excluded.note,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    source,
                    source_id,
                    download_id,
                    title,
                    author,
                    member.position.unwrap_or(0).max(0),
                    normalize_member_role(member.member_role.as_deref())?,
                    normalize_added_by(member.added_by.as_deref())?,
                    if member.pinned.unwrap_or(false) { 1 } else { 0 },
                    normalize_bounded_optional(&member.note, 10_000, "Member note")?,
                    now,
                ],
            )
            .map_err(|e| format!("Failed to restore collection member: {e}"))?;
        }
        Ok(())
    }

    /// 保存済み作品を追加する。既存メンバーは重複させず、メタデータだけ更新する。
    /// 棚で選んだ作品を、そのまま束へ入れる。
    ///
    /// 画面から `source` と `source_id` を組み立てさせない。棚の複数選択が
    /// 持っているのは行の id だけで、そこから作品の識別子を引き直すために
    /// 一覧をもう一度読ませるのは無駄である。
    ///
    /// 並びは**投稿日順**にする。選んだ順は、選ぶときの都合であって読む順
    /// ではない。クリックした順に前後編が入れ替わるのがいちばん困る。
    pub fn add_downloads_to_collection(
        &self,
        collection_id: &str,
        download_ids: &[i64],
    ) -> Result<WorkCollection, String> {
        let collection_id = validate_collection_id(collection_id)?.to_string();
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection member transaction failed: {e}"))?;
            let inputs = resolve_member_inputs(&tx, download_ids)?;
            add_collection_members_in_connection(&tx, &collection_id, &inputs, &now)?;
            tx.commit()
                .map_err(|e| format!("Collection member commit failed: {e}"))?;
        }
        self.get_work_collection(&collection_id)
    }

    /// 選んだ作品から、新しい束をひとつ作る。
    ///
    /// 作成と追加を1往復にまとめる。二段階にすると、名前だけの空の束が
    /// 残る失敗の仕方ができてしまう。
    pub fn create_collection_from_downloads(
        &self,
        name: &str,
        collection_kind: &str,
        download_ids: &[i64],
    ) -> Result<WorkCollection, String> {
        let input = WorkCollectionInput {
            name: name.to_string(),
            collection_kind: collection_kind.to_string(),
            // 表紙は先頭のメンバーから自動で作る。選ばせるのは後からでよい。
            cover_mode: Some("mosaic".to_string()),
            name_source: Some("manual".to_string()),
            track: Some("manual".to_string()),
            ..Default::default()
        };
        let id = new_collection_id("collection");
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection creation transaction failed: {e}"))?;
            let inputs = resolve_member_inputs(&tx, download_ids)?;
            write_collection_row(&tx, &input, &id, false, &now)?;
            add_collection_members_in_connection(&tx, &id, &inputs, &now)?;
            tx.commit()
                .map_err(|e| format!("Collection creation commit failed: {e}"))?;
        }
        self.get_work_collection(&id)
    }

    /// 命名エンジンへ渡す材料を組み立てる。
    ///
    /// **本文は含めない。** 題名・作者・タグ・公式シリーズ名だけを取る。
    /// 名前を付けるのにそれ以上は要らないし、要らないものを外へ出す理由は無い。
    pub fn collection_naming_works(
        &self,
        download_ids: &[i64],
    ) -> Result<Vec<crate::assist::NamingWork>, String> {
        if download_ids.is_empty() {
            return Ok(Vec::new());
        }
        let encoded = serde_json::to_string(download_ids)
            .map_err(|e| format!("Failed to encode naming work IDs: {e}"))?;
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT d.title, d.author_name,
                        (SELECT ds.title FROM download_series ds WHERE ds.download_id = d.id LIMIT 1),
                        COALESCE((
                          SELECT json_group_array(t.name)
                          FROM download_tags dt JOIN tags t ON t.id = dt.tag_id
                          WHERE dt.download_id = d.id
                        ), '[]')
                 FROM downloads d
                 WHERE d.id IN (SELECT value FROM json_each(?1))
                 ORDER BY COALESCE(d.source_created_at, d.downloaded_at) ASC, d.id ASC",
            )
            .map_err(|e| format!("Failed to prepare naming works: {e}"))?;
        let rows = stmt
            .query_map(params![encoded], |row| {
                let tags = row
                    .get::<_, Option<String>>(3)?
                    .and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok())
                    .unwrap_or_default();
                Ok(crate::assist::NamingWork {
                    title: row.get(0)?,
                    author_name: row.get(1)?,
                    series_title: row
                        .get::<_, Option<String>>(2)?
                        .filter(|title| !collection_rules::is_administrative_series_label(title)),
                    tags,
                })
            })
            .map_err(|e| format!("Failed to query naming works: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read naming works: {e}"))
    }

    /// 試し書きに使う作品を、棚から拾う。
    ///
    /// 束がまだ無いうちでもエンジンを試せるようにする。同じタグを持つ作品を
    /// 選ぶのは、**束らしい入力**でないと試したことにならないため — 無関係な
    /// 作品を並べて「まとまりの名前」を求めても、返るものを判断できない。
    pub fn sample_naming_works(&self) -> Result<Vec<crate::assist::NamingWork>, String> {
        let ids = self.sample_bundle_like_ids()?;
        self.collection_naming_works(&ids)
    }

    /// 束らしい入力になる作品を、いくつか拾う。
    ///
    /// 接続だけを見ても意味が無い。無関係な作品を並べて「まとまりの名前」を
    /// 求めても、返ってきたものが良いのか悪いのか判断できない。
    pub(crate) fn sample_bundle_like_ids(&self) -> Result<Vec<i64>, String> {
        let conn = self.read_conn()?;
        // 締まった束になるタグを起点にする。「♡喘ぎ」のように広いタグで拾うと
        // 材料がばらけ、返ってきた名前が良いのか悪いのか判断できない。
        // 3〜8作のタグなら、人物名や作品名であることが多い。
        let mut stmt = conn
            .prepare(
                "SELECT t.name FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 GROUP BY t.id
                 HAVING COUNT(*) BETWEEN 3 AND 8
                 ORDER BY COUNT(*) DESC, t.name COLLATE NOCASE ASC
                 LIMIT 40",
            )
            .map_err(|e| format!("Failed to prepare sample tags: {e}"))?;
        let anchor = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query sample tags: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read sample tags: {e}"))?
            .into_iter()
            .find(|tag| collection_rules::is_informative_tag(tag));
        drop(stmt);

        // タグが一つも無い棚でも試せるように、最近保存した作品で代用する。
        let (sql, tag) = match anchor {
            Some(tag) => (
                "SELECT dt.download_id FROM download_tags dt
                 JOIN tags t ON t.id = dt.tag_id
                 WHERE t.name = ?1
                 ORDER BY dt.download_id
                 LIMIT 6",
                tag,
            ),
            // 束ねる手がかりが無いので、束ではなく最近の作品で試す。
            // 引数の数をそろえるために、当たらない値を1つ渡す。
            None => (
                "SELECT id FROM downloads
                 WHERE ?1 IS NOT NULL
                 ORDER BY downloaded_at DESC
                 LIMIT 4",
                String::new(),
            ),
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare sample works: {e}"))?;
        let ids = stmt
            .query_map(params![tag], |row| row.get::<_, i64>(0))
            .map_err(|e| format!("Failed to query sample works: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read sample works: {e}"))?;
        Ok(ids)
    }

    /// すでにあるコレクションに、名前と説明の案を出し直す。
    ///
    /// 束は作ったあとで中身が変わる。作品を足せば共有タグが変わり、外せば
    /// 題名の共通部分も変わる。**作ったときの名前に縛られる理由は無い。**
    /// 決めるのは利用者なので、ここは案を並べるだけで、保存はしない。
    pub fn collection_name_proposals(
        &self,
        collection_id: &str,
    ) -> Result<Vec<CollectionNameCandidate>, String> {
        let collection = self.get_work_collection(collection_id)?;
        let ids = collection
            .members
            .iter()
            .filter_map(|member| member.download_id)
            .collect::<Vec<_>>();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let conn = self.read_conn()?;
        let works = load_suggestion_works(&conn, &ids)?;
        let ranked = works
            .into_iter()
            .map(|work| RankedSuggestionMember {
                episode_order: collection_rules::episode_order(&work.title),
                work,
                member_score: 1.0,
                title_stem: String::new(),
                default_selected: true,
                evidence: Vec::new(),
                series_order: None,
                link_order: None,
                link_depth: None,
            })
            .collect::<Vec<_>>();
        let ids_json = serde_json::to_string(&ids)
            .map_err(|e| format!("Failed to encode collection members: {e}"))?;
        let mut options = collection_name_options(&conn, &ranked, &ids_json)?;
        // いまの名前も案として残す。付け直すつもりが無かったときに、
        // 戻る先が画面に無いのは不親切である。
        if !options
            .iter()
            .any(|option| option.name == collection.summary.name)
        {
            options.insert(
                0,
                CollectionNameCandidate {
                    source: collection.summary.name_source.clone(),
                    label: "いまの名前".to_string(),
                    name: collection.summary.name.clone(),
                    ..Default::default()
                },
            );
        }
        Ok(options)
    }

    /// 命名エンジンへ渡す材料を、すでにあるコレクションから組み立てる。
    pub fn collection_naming_works_for(
        &self,
        collection_id: &str,
    ) -> Result<Vec<crate::assist::NamingWork>, String> {
        let collection = self.get_work_collection(collection_id)?;
        let ids = collection
            .members
            .iter()
            .filter_map(|member| member.download_id)
            .collect::<Vec<_>>();
        self.collection_naming_works(&ids)
    }

    /// 提案のモデル候補を、最新の生成結果へ入れ替える。
    ///
    /// 通常案は消さない。**モデルの案は候補の一つ**であって、決定ではない。
    pub fn attach_llm_name_option(
        &self,
        suggestion_id: &str,
        named: &crate::assist::NamedBundle,
        provenance: &crate::assist::AssistProvenance,
    ) -> Result<CollectionSuggestion, String> {
        let suggestion_id = validate_collection_id(suggestion_id)?.to_string();
        let mut suggestion = self
            .list_collection_suggestions(Some("all"))?
            .into_iter()
            .find(|value| value.id == suggestion_id)
            .ok_or_else(|| "Collection suggestion not found".to_string())?;
        // Regeneration replaces the previous model answer but never touches
        // deterministic title/tag/series candidates.
        suggestion
            .name_options
            .retain(|option| option.source != "llm");
        suggestion.name_options.push(CollectionNameCandidate {
            source: "llm".to_string(),
            label: "モデルの案".to_string(),
            name: named.name.clone(),
            model_id: Some(provenance.model_id.clone()),
            prompt_version: Some(provenance.prompt_version.clone()),
            created_at: Some(provenance.created_at.clone()),
        });
        let encoded = serde_json::to_string(&suggestion.name_options)
            .map_err(|e| format!("Failed to encode suggestion names: {e}"))?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        conn.execute(
            "UPDATE collection_suggestions SET name_options_json = ?1 WHERE id = ?2",
            params![encoded, suggestion_id],
        )
        .map_err(|e| format!("Failed to save suggestion names: {e}"))?;
        Ok(suggestion)
    }

    /// 束の並びを、決まった基準で一度に整える。
    ///
    /// 前後編が20組ある束を1つずつ矢印で動かす人はいない。基準は二つだけに
    /// する — 投稿日と、題名の連番。どちらも**題名か日付を見れば人が確かめ
    /// られる**もので、確かめられない基準で勝手に並べ替えることはしない。
    ///
    /// 話数の語彙は `collection_rules` にしかない。画面側で同じ正規表現を
    /// 持たせると、片方だけ直したときに黙ってずれる。
    pub fn sort_work_collection_members(
        &self,
        collection_id: &str,
        mode: &str,
    ) -> Result<WorkCollection, String> {
        let collection = self.get_work_collection(collection_id)?;
        let mut members = collection.members;
        match mode {
            "published" => members.sort_by(|left, right| {
                member_published_at(left)
                    .cmp(&member_published_at(right))
                    .then_with(|| left.position.cmp(&right.position))
            }),
            "episode" => members.sort_by(|left, right| {
                // 話数を持たないものは末尾へ。持っている中では番号順、
                // 同じ番号なら投稿順。
                let a = collection_rules::episode_order(&left.title);
                let b = collection_rules::episode_order(&right.title);
                match (a, b) {
                    (Some(a), Some(b)) => a.cmp(&b),
                    (Some(_), None) => Ordering::Less,
                    (None, Some(_)) => Ordering::Greater,
                    (None, None) => Ordering::Equal,
                }
                .then_with(|| member_published_at(left).cmp(&member_published_at(right)))
                .then_with(|| left.position.cmp(&right.position))
            }),
            _ => return Err("Unknown collection sort mode".to_string()),
        }
        let keys = members
            .iter()
            .map(|member| WorkKey {
                source: member.source.clone(),
                source_id: member.source_id.clone(),
            })
            .collect::<Vec<_>>();
        self.reorder_work_collection_members(collection_id, &keys)
    }

    pub fn add_work_collection_members(
        &self,
        collection_id: &str,
        inputs: &[WorkCollectionMemberInput],
    ) -> Result<WorkCollection, String> {
        let collection_id = validate_collection_id(collection_id)?.to_string();
        if inputs.is_empty() {
            return self.get_work_collection(&collection_id);
        }
        if inputs.len() > 5_000 {
            return Err("Too many collection members in one request".to_string());
        }
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection member transaction failed: {e}"))?;
            add_collection_members_in_connection(&tx, &collection_id, inputs, &now)?;
            tx.commit()
                .map_err(|e| format!("Collection member commit failed: {e}"))?;
        }
        self.get_work_collection(&collection_id)
    }

    pub fn remove_work_collection_members(
        &self,
        collection_id: &str,
        members: &[WorkKey],
    ) -> Result<WorkCollection, String> {
        let collection_id = validate_collection_id(collection_id)?.to_string();
        if members.len() > 5_000 {
            return Err("Too many collection members in one request".to_string());
        }
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection member transaction failed: {e}"))?;
            for member in members {
                let source = validate_work_key_part(&member.source, "Source")?;
                let source_id = validate_work_key_part(&member.source_id, "Source ID")?;
                tx.execute(
                    "DELETE FROM work_collection_members
                     WHERE collection_id = ?1 AND source = ?2 AND source_id = ?3",
                    params![collection_id, source, source_id],
                )
                .map_err(|e| format!("Failed to remove collection member: {e}"))?;
            }
            let changed = tx
                .execute(
                    "UPDATE work_collections
                     SET revision = revision + 1, updated_at = ?1 WHERE id = ?2",
                    params![now, collection_id],
                )
                .map_err(|e| format!("Failed to update collection revision: {e}"))?;
            if changed == 0 {
                return Err("Collection not found".to_string());
            }
            normalize_collection_positions(&tx, &collection_id)?;
            tx.commit()
                .map_err(|e| format!("Collection member commit failed: {e}"))?;
        }
        self.get_work_collection(&collection_id)
    }

    /// 全メンバーを指定された順に並べる。欠落や重複を許さず、部分更新事故を防ぐ。
    pub fn reorder_work_collection_members(
        &self,
        collection_id: &str,
        members: &[WorkKey],
    ) -> Result<WorkCollection, String> {
        let collection_id = validate_collection_id(collection_id)?.to_string();
        if members.len() > 5_000 {
            return Err("Too many collection members in one request".to_string());
        }
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Collection reorder transaction failed: {e}"))?;
            let expected: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM work_collection_members WHERE collection_id = ?1",
                    params![collection_id],
                    |row| row.get(0),
                )
                .map_err(|e| format!("Failed to count collection members: {e}"))?;
            if expected != members.len() as i64 {
                return Err("Reorder request must contain every collection member".to_string());
            }
            let mut seen = HashSet::new();
            for (position, member) in members.iter().enumerate() {
                let source = validate_work_key_part(&member.source, "Source")?;
                let source_id = validate_work_key_part(&member.source_id, "Source ID")?;
                if !seen.insert((source.clone(), source_id.clone())) {
                    return Err("Reorder request contains a duplicate work".to_string());
                }
                let changed = tx
                    .execute(
                        "UPDATE work_collection_members SET position = ?1, updated_at = ?2
                         WHERE collection_id = ?3 AND source = ?4 AND source_id = ?5",
                        params![position as i64, now, collection_id, source, source_id],
                    )
                    .map_err(|e| format!("Failed to reorder collection member: {e}"))?;
                if changed == 0 {
                    return Err(format!("Collection member not found: {source}/{source_id}"));
                }
            }
            let changed = tx
                .execute(
                    "UPDATE work_collections
                     SET revision = revision + 1, updated_at = ?1 WHERE id = ?2",
                    params![now, collection_id],
                )
                .map_err(|e| format!("Failed to update collection revision: {e}"))?;
            if changed == 0 {
                return Err("Collection not found".to_string());
            }
            tx.commit()
                .map_err(|e| format!("Collection reorder commit failed: {e}"))?;
        }
        self.get_work_collection(&collection_id)
    }

    pub fn list_collections_for_work(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Vec<WorkCollectionSummary>, String> {
        let source = validate_work_key_part(source, "Source")?;
        let source_id = validate_work_key_part(source_id, "Source ID")?;
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{COLLECTION_SUMMARY_SELECT}
                     WHERE EXISTS (
                         SELECT 1 FROM work_collection_members sel
                         WHERE sel.collection_id = c.id
                           AND sel.source = ?1 AND sel.source_id = ?2
                     ){COLLECTION_SUMMARY_TAIL}"
            ))
            .map_err(|e| format!("Failed to prepare work collections: {e}"))?;
        let collections = stmt
            .query_map(params![source, source_id], work_collection_summary_from_row)
            .map_err(|e| format!("Failed to query work collections: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read work collections: {e}"))?;
        Ok(collections)
    }

    /// この人物の作品を 1 件でも含むコレクションを返す。作者詳細から、その人が
    /// 関わっているまとまりへ辿れるようにするための一覧である。
    ///
    /// 所属判定は `EXISTS` で行う。人物の作品が同じコレクションに複数入って
    /// いても、`JOIN` で行が増えて作品数や総文字数が水増しされないようにする。
    pub fn list_collections_for_person(
        &self,
        source: &str,
        person_key: &str,
    ) -> Result<Vec<WorkCollectionSummary>, String> {
        let source = validate_work_key_part(source, "Source")?;
        let person_key = validate_work_key_part(person_key, "Person key")?;
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{COLLECTION_SUMMARY_SELECT}
                     WHERE EXISTS (
                         SELECT 1 FROM work_collection_members sel
                         JOIN download_people dp ON dp.download_id = sel.download_id
                         WHERE sel.collection_id = c.id
                           AND dp.person_source = ?1 AND dp.person_key = ?2
                     ){COLLECTION_SUMMARY_TAIL}"
            ))
            .map_err(|e| format!("Failed to prepare person collections: {e}"))?;
        let collections = stmt
            .query_map(
                params![source, person_key],
                work_collection_summary_from_row,
            )
            .map_err(|e| format!("Failed to query person collections: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read person collections: {e}"))?;
        Ok(collections)
    }

    /// シリーズの作品を 1 件でも含むコレクション。
    ///
    /// 作者に対する [`Self::list_collections_for_person`] と同じ役目で、辿る
    /// 経路が `download_people` ではなく `download_series` になるだけである。
    /// **片方だけにあると、同じ画面で作りが割れる。**
    pub fn list_collections_for_series(
        &self,
        source: &str,
        series_key: &str,
    ) -> Result<Vec<WorkCollectionSummary>, String> {
        let source = validate_work_key_part(source, "Source")?;
        let series_key = validate_work_key_part(series_key, "Series key")?;
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(&format!(
                "{COLLECTION_SUMMARY_SELECT}
                     WHERE EXISTS (
                         SELECT 1 FROM work_collection_members sel
                         JOIN download_series ds ON ds.download_id = sel.download_id
                         WHERE sel.collection_id = c.id
                           AND ds.series_source = ?1 AND ds.series_key = ?2
                     ){COLLECTION_SUMMARY_TAIL}"
            ))
            .map_err(|e| format!("Failed to prepare series collections: {e}"))?;
        let collections = stmt
            .query_map(
                params![source, series_key],
                work_collection_summary_from_row,
            )
            .map_err(|e| format!("Failed to query series collections: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read series collections: {e}"))?;
        Ok(collections)
    }

    /// 本文・キャプションに現れる pixiv/FANBOX の作品 URL を解析し、
    /// コレクション候補生成で利用できる有向グラフとして更新する。
    pub fn refresh_work_links(&self, download_id: i64) -> Result<Vec<WorkLink>, String> {
        let document = self.get_reader_document(download_id, None)?;
        let from_source = document.download.source.clone();
        let from_source_id = document.download.source_id.clone();
        let mut evidence_text = document.html;
        evidence_text.push('\n');
        evidence_text.push_str(&document.plain_text);
        if let Some(excerpt) = document.download.excerpt.as_deref() {
            evidence_text.push('\n');
            evidence_text.push_str(excerpt);
        }
        let discovered = extract_work_link_evidence(&evidence_text, &from_source, &from_source_id);
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Work link transaction failed: {e}"))?;
            tx.execute(
                "DELETE FROM work_links
                 WHERE from_source = ?1 AND from_source_id = ?2
                   AND evidence_type = 'content_link' AND status = 'observed'",
                params![from_source, from_source_id],
            )
            .map_err(|e| format!("Failed to clear stale work links: {e}"))?;
            for link in discovered {
                let to_download_id = tx
                    .query_row(
                        "SELECT id FROM downloads WHERE source = ?1 AND source_id = ?2",
                        params![link.to_source, link.to_source_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()
                    .map_err(|e| format!("Failed to resolve linked work: {e}"))?;
                tx.execute(
                    "INSERT INTO work_links (
                        from_source, from_source_id, from_download_id,
                        to_source, to_source_id, to_download_id,
                        relation_type, evidence_type, anchor_text, context_text,
                        confidence, status, discovered_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'content_link', ?8, ?9, ?10,
                               'observed', ?11, ?11)
                     ON CONFLICT(from_source, from_source_id, to_source, to_source_id, evidence_type)
                     DO UPDATE SET
                        from_download_id = excluded.from_download_id,
                        to_download_id = excluded.to_download_id,
                        relation_type = excluded.relation_type,
                        anchor_text = excluded.anchor_text,
                        context_text = excluded.context_text,
                        confidence = excluded.confidence,
                        updated_at = excluded.updated_at",
                    params![
                        from_source,
                        from_source_id,
                        download_id,
                        link.to_source,
                        link.to_source_id,
                        to_download_id,
                        link.relation_type,
                        link.anchor_text,
                        link.context_text,
                        link.confidence,
                        now,
                    ],
                )
                .map_err(|e| format!("Failed to save work link: {e}"))?;
            }
            tx.commit()
                .map_err(|e| format!("Work link commit failed: {e}"))?;
        }
        self.list_work_links_for_work(&from_source, &from_source_id)
    }

    pub fn list_work_links_for_work(
        &self,
        source: &str,
        source_id: &str,
    ) -> Result<Vec<WorkLink>, String> {
        let source = validate_work_key_part(source, "Source")?;
        let source_id = validate_work_key_part(source_id, "Source ID")?;
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare(
                "SELECT id, from_source, from_source_id, from_download_id,
                        to_source, to_source_id, to_download_id, relation_type,
                        evidence_type, anchor_text, context_text, confidence,
                        status, discovered_at, updated_at
                 FROM work_links
                 WHERE (from_source = ?1 AND from_source_id = ?2)
                    OR (to_source = ?1 AND to_source_id = ?2)
                 ORDER BY confidence DESC, updated_at DESC, id DESC",
            )
            .map_err(|e| format!("Failed to prepare work links: {e}"))?;
        let links = stmt
            .query_map(params![source, source_id], work_link_from_row)
            .map_err(|e| format!("Failed to query work links: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read work links: {e}"))?;
        Ok(links)
    }

    /// 複数の弱い根拠を説明可能なスコアへ合成し、利用者が編集・承認できる
    /// コレクションの「ひな型」を作る。ここでは確定コレクションを変更しない。
    pub fn generate_collection_suggestion(
        &self,
        request: &CollectionSuggestionRequest,
    ) -> Result<CollectionSuggestion, String> {
        let mut seed_ids = request
            .seed_download_ids
            .iter()
            .copied()
            .filter(|id| *id > 0)
            .collect::<Vec<_>>();
        seed_ids.sort_unstable();
        seed_ids.dedup();
        if seed_ids.is_empty() {
            return Err("At least one seed work is required".to_string());
        }
        if seed_ids.len() > 20 {
            return Err("A suggestion can use at most 20 seed works".to_string());
        }
        let mut refreshed_link_ids = HashSet::new();
        for seed_id in &seed_ids {
            refreshed_link_ids.insert(*seed_id);
            if let Err(error) = self.refresh_work_links(*seed_id) {
                log::warn!("Failed to refresh suggestion seed links for {seed_id}: {error}");
            }
        }

        let seed_json = serde_json::to_string(&seed_ids)
            .map_err(|e| format!("Failed to encode suggestion seeds: {e}"))?;
        let (seed_works, mut candidate_ids) = {
            let conn = self.read_conn()?;
            let seeds = load_suggestion_works(&conn, &seed_ids)?;
            if seeds.len() != seed_ids.len() {
                return Err("One or more seed works no longer exist".to_string());
            }
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT candidate.id
                     FROM downloads candidate
                     JOIN downloads seed ON seed.id IN (SELECT value FROM json_each(?1))
                     WHERE candidate.id IN (SELECT value FROM json_each(?1))
                        OR (
                            candidate.author_id != '' AND seed.author_id != ''
                            AND candidate.source = seed.source
                            AND candidate.author_id = seed.author_id
                        )
                        OR (
                            LENGTH(TRIM(candidate.author_name)) >= 2
                            AND candidate.author_name = seed.author_name COLLATE NOCASE
                        )
                        OR EXISTS (
                            SELECT 1 FROM download_series candidate_series
                            JOIN download_series seed_series
                              ON seed_series.series_source = candidate_series.series_source
                             AND seed_series.series_key = candidate_series.series_key
                            WHERE candidate_series.download_id = candidate.id
                              AND seed_series.download_id = seed.id
                        )
                        OR EXISTS (
                            SELECT 1 FROM work_links link
                            WHERE link.status != 'rejected'
                              AND (
                                (link.from_download_id = seed.id AND link.to_download_id = candidate.id)
                                OR (link.to_download_id = seed.id AND link.from_download_id = candidate.id)
                              )
                        )
                     LIMIT 1200",
                )
                .map_err(|e| format!("Failed to prepare suggestion candidates: {e}"))?;
            let ids = stmt
                .query_map(params![seed_json], |row| row.get::<_, i64>(0))
                .map_err(|e| format!("Failed to query suggestion candidates: {e}"))?
                .collect::<Result<HashSet<_>, _>>()
                .map_err(|e| format!("Failed to read suggestion candidates: {e}"))?;
            (seeds, ids)
        };

        let link_paths =
            discover_linked_collection_component(self, &seed_works, &mut refreshed_link_ids)?;
        candidate_ids.extend(link_paths.keys().copied());

        // 作品から作品を引く経路では、保存済みの作品ベクトル同士を比べる。
        // 自由文の埋め込みは行わないので、提案作成がモデル取得を突然始める
        // ことも、本文断片の多い長編だけが有利になることもない。
        let semantic_status = crate::database::semantic_index::status(&self.storage_dir);
        let mut semantic_scores: HashMap<i64, f64> = HashMap::new();
        if semantic_status.indexed_works > 0 {
            match crate::database::semantic_index::similar_works(&self.storage_dir, &seed_ids, 160)
            {
                Ok(hits) => {
                    for (download_id, score) in hits {
                        candidate_ids.insert(download_id);
                        semantic_scores.insert(download_id, score);
                    }
                }
                Err(error) => {
                    log::warn!("Semantic collection suggestion failed: {error}");
                }
            }
        }
        candidate_ids.extend(seed_ids.iter().copied());
        let candidate_ids = candidate_ids.into_iter().collect::<Vec<_>>();
        let conn = self.read_conn()?;
        let candidates = load_suggestion_works(&conn, &candidate_ids)?;
        let seed_id_set = seed_ids.iter().copied().collect::<HashSet<_>>();
        let limit = request.limit.unwrap_or(60).clamp(2, 200) as usize;
        let mut ranked = Vec::new();
        for candidate in candidates {
            let is_seed = seed_id_set.contains(&candidate.id);
            // 告知・活動報告は束ねない。「2023年5月進捗のご報告」が5件で
            // 1つの連載に見えるのを止める。利用者が自分で基準に選んだものは
            // そのまま尊重するので、除くのは種以外だけ。
            if !is_seed
                && collection_rules::is_administrative_post(
                    &candidate.title,
                    &candidate.content_type,
                    candidate.text_length,
                )
            {
                continue;
            }
            if !is_seed && suggestion_pair_is_rejected(&conn, &candidate, &seed_works)? {
                continue;
            }
            // 手がかりを二つに分ける。
            //
            //   binding  この二作が**同じ束に属する**と言える証拠
            //   context  順位づけと説明にだけ使う、周辺の事情
            //
            // 以前は両方を一つの点数へ足していた。公式シリーズ 0.58 と
            // 同一作者 0.12 を足すと必ず 0.70 になり、採用閾値 0.44 を越える。
            // その結果、151作のシリーズを種にすると「同じ棚に載っている」だけの
            // 作品が60件、チェックの付かないまま並んだ。足すのをやめる。
            let mut evidence = Vec::new();
            let mut binding = 0.0_f64;
            let mut context = 0.0_f64;
            let link_order = None;
            let link_depth = link_paths.get(&candidate.id).map(|path| path.depth);
            if is_seed {
                binding = 1.0;
                evidence.push(CollectionSuggestionEvidence {
                    kind: "seed".to_string(),
                    label: "選択した基準作品".to_string(),
                    contribution: 1.0,
                });
            }

            // --- 束の証拠 ------------------------------------------------

            if let Some(path) = link_paths.get(&candidate.id).copied().filter(|_| !is_seed) {
                let direct = strongest_link_with_seeds(&conn, candidate.id, &seed_json)?;
                let (label, confidence) = if let Some((relation, confidence, _)) = direct {
                    (
                        format!("本文・キャプションの直接リンク（{relation}）"),
                        confidence,
                    )
                } else {
                    (
                        format!("本文リンクを{}段追跡して接続", path.depth),
                        path.confidence,
                    )
                };
                let depth_decay = path.depth.saturating_sub(1) as f64 * 0.08;
                let contribution = (confidence * (0.82 - depth_decay)).clamp(0.46, 0.80);
                add_suggestion_evidence(
                    &mut evidence,
                    &mut binding,
                    "content_link",
                    &label,
                    contribution,
                );
            }

            let (candidate_stem, _) = title_stem_and_order(&candidate.title);
            // 話数は表示側の語彙で読む。`title_stem_and_order` は検索用の
            // 正規化を通すので、丸数字が裸の数字に化けて読めなくなっていた。
            let episode_order = collection_rules::episode_order(&candidate.title);
            let title_similarity = seed_works
                .iter()
                .map(|seed| {
                    let (seed_stem, _) = title_stem_and_order(&seed.title);
                    if seed_stem.is_empty() || candidate_stem.is_empty() {
                        0.0
                    } else {
                        normalized_levenshtein(&seed_stem, &candidate_stem)
                    }
                })
                .fold(0.0_f64, f64::max);
            if title_similarity >= 0.58 && !is_seed {
                let contribution =
                    (((title_similarity - 0.58) / 0.42) * 0.48 + 0.14).clamp(0.14, 0.62);
                add_suggestion_evidence(
                    &mut evidence,
                    &mut binding,
                    "title_similarity",
                    // 割合ではなく、何が同じなのかを書く。74% は根拠にならない。
                    &format!(
                        "題名が「{}」で共通",
                        collection_rules::clamp_name(
                            &collection_rules::display_title_stem(&candidate.title),
                            18
                        )
                    ),
                    contribution,
                );
            }

            // 同じシリーズに載っていることと、続けて読むべきことは違う。
            // 話数が隣り合っているときだけ束の証拠として数える。
            let series_run = shared_series_run_with_seeds(&conn, candidate.id, &seed_json)?;
            if let Some((title, gap)) = series_run.as_ref().filter(|_| !is_seed) {
                if *gap <= SERIES_ADJACENT_GAP {
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut binding,
                        "series_run",
                        &format!("公式シリーズ「{title}」で話数が隣接"),
                        0.52,
                    );
                }
            }

            // --- 文脈 ----------------------------------------------------

            let shared_series = shared_series_with_seeds(&conn, candidate.id, &seed_json)?;
            let series_order = shared_series.iter().filter_map(|value| value.1).min();
            if let Some((title, _)) = shared_series.first() {
                let already_bound = evidence.iter().any(|value| value.kind == "series_run");
                if !already_bound {
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut context,
                        "official_series",
                        &format!("公式シリーズ「{title}」に同居"),
                        0.10,
                    );
                }
            }

            let same_author = seed_works.iter().any(|seed| {
                (!candidate.author_id.is_empty()
                    && candidate.source == seed.source
                    && candidate.author_id == seed.author_id)
                    || (!candidate.author_name.trim().is_empty()
                        && candidate
                            .author_name
                            .eq_ignore_ascii_case(&seed.author_name))
            });
            if same_author && !is_seed {
                add_suggestion_evidence(
                    &mut evidence,
                    &mut context,
                    "same_author",
                    if seed_works
                        .iter()
                        .any(|seed| seed.source != candidate.source)
                    {
                        "同一作者名（取得元を横断）"
                    } else {
                        "同一作者"
                    },
                    0.08,
                );
            }

            // タグは方針の文書に書いてあったのに使われていなかった。
            // 束の証拠にはしない（同じタグの作品は棚に何百とある）が、
            // 何が共通なのかを言い当てるのはたいていタグである。
            if !is_seed {
                let tags = shared_tags_with_seeds(&conn, candidate.id, &seed_json)?;
                if tags.len() >= 2 {
                    let contribution = (tags.len() as f64 * 0.03).clamp(0.06, 0.14);
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut context,
                        "shared_tags",
                        &format!(
                            "共有タグ {}",
                            tags.iter().take(3).cloned().collect::<Vec<_>>().join("・")
                        ),
                        contribution,
                    );
                }
            }

            // 投稿時期。続き物は近い時期に出ることが多い。
            if !is_seed && !candidate.published_at.is_empty() {
                let nearest = seed_works
                    .iter()
                    .filter_map(|seed| {
                        published_days_apart(&candidate.published_at, &seed.published_at)
                    })
                    .min();
                if let Some(days) = nearest.filter(|days| *days <= PUBLISH_NEARBY_DAYS) {
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut context,
                        "published_nearby",
                        &if days == 0 {
                            "同じ日に投稿".to_string()
                        } else {
                            format!("投稿が{days}日違い")
                        },
                        0.06,
                    );
                }
            }

            if let Some(semantic_score) = semantic_scores.get(&candidate.id).copied() {
                if semantic_score >= 0.45 && !is_seed {
                    let contribution = ((semantic_score - 0.4) * 0.35).clamp(0.02, 0.20);
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut context,
                        "semantic_similarity",
                        "本文の内容が近い",
                        contribution,
                    );
                }
            }

            // **束の証拠が一つも無いものは候補にしない。** ここが要である。
            // 文脈だけで並ぶ候補は、利用者が選べないまま画面を埋めるだけだった。
            if !is_seed && binding <= 0.0 {
                continue;
            }
            let score = (binding + context).clamp(0.0, 1.0);
            // 設計提案 6-6 は弱い信号だけで 2 段以上連鎖させないとしている。
            // 遠いリンク先は候補として見せるが、既定では選ばない。題名や公式順で
            // 裏付けが取れているものはこの制限から外す。
            let corroborated = series_order.is_some()
                || episode_order.is_some()
                || evidence
                    .iter()
                    .any(|value| matches!(value.kind.as_str(), "title_similarity" | "series_run"));
            let distant_link = link_depth.is_some_and(|depth| depth >= 2) && !corroborated;
            let default_selected = is_seed || (binding >= 0.45 && !distant_link);
            ranked.push(RankedSuggestionMember {
                work: candidate,
                member_score: score,
                title_stem: candidate_stem,
                default_selected,
                evidence,
                series_order,
                link_order,
                link_depth,
                episode_order,
            });
        }
        assign_link_graph_order(&conn, &mut ranked)?;
        assign_unnumbered_opening_order(&mut ranked);
        flag_duplicate_content_members(&mut ranked);
        ranked.sort_by(compare_ranked_suggestion_members);
        let effective_limit = limit.max(seed_ids.len());
        if ranked.len() > effective_limit {
            let mut retained = ranked
                .iter()
                .filter(|value| seed_id_set.contains(&value.work.id))
                .cloned()
                .collect::<Vec<_>>();
            let remaining = effective_limit.saturating_sub(retained.len());
            retained.extend(
                ranked
                    .iter()
                    .filter(|value| !seed_id_set.contains(&value.work.id))
                    .take(remaining)
                    .cloned(),
            );
            ranked = retained;
            ranked.sort_by(compare_ranked_suggestion_members);
        }
        if !ranked
            .iter()
            .any(|value| !seed_id_set.contains(&value.work.id))
        {
            return Err("関連作品は見つかりませんでした".to_string());
        }
        let name_options = collection_name_options(&conn, &ranked, &seed_json)?;
        let proposed_name = name_options
            .first()
            .map(|value| value.name.clone())
            .unwrap_or_else(|| "読書コレクション".to_string());
        // 「1 件でも順序を持てば全体が順序付き」にはしない。20 件の短編集に
        // たまたま話数付きが 1 件混じっただけで、通読順のある連載として
        // 提示してしまうためである。過半数が順序を持つときだけ順序付きとする。
        let ordered_members = ranked
            .iter()
            .filter(|value| {
                value.series_order.is_some()
                    || value.link_order.is_some()
                    || value.episode_order.is_some()
            })
            .count();
        let collection_kind = if ranked.len() > 1 && ordered_members * 2 > ranked.len() {
            "ordered"
        } else {
            "unordered"
        }
        .to_string();
        let overall_score = ranked
            .iter()
            .filter(|value| !seed_id_set.contains(&value.work.id))
            .map(|value| value.member_score)
            .reduce(f64::max)
            .unwrap_or(0.0);
        let members = ranked
            .into_iter()
            .enumerate()
            .map(|(position, value)| CollectionSuggestionMember {
                source: value.work.source,
                source_id: value.work.source_id,
                download_id: Some(value.work.id),
                title: value.work.title,
                author_name: value.work.author_name,
                cover_path: value.work.cover_path,
                text_length: value.work.text_length,
                proposed_position: position as i64,
                score: value.member_score,
                selected: value.default_selected,
                evidence: value.evidence,
            })
            .collect::<Vec<_>>();
        let id = new_collection_id("suggestion");
        let rule_version = COLLECTION_SUGGEST_RULE_VERSION.to_string();
        let evidence_summary = suggestion_evidence_summary(&members, &seed_id_set);
        let members_json = serde_json::to_string(&members)
            .map_err(|e| format!("Failed to encode suggestion members: {e}"))?;
        let name_options_json = serde_json::to_string(&name_options)
            .map_err(|e| format!("Failed to encode suggestion names: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        drop(conn);
        {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO collection_suggestions (
                    id, seed_json, proposed_name, collection_kind, members_json,
                    score, rule_version, state, created_at, updated_at,
                    name_options_json, track, origin, evidence_summary
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?8,
                           ?9, 'sequence', 'seed', ?10)",
                params![
                    id,
                    seed_json,
                    proposed_name,
                    collection_kind,
                    members_json,
                    overall_score,
                    rule_version,
                    now,
                    name_options_json,
                    evidence_summary,
                ],
            )
            .map_err(|e| format!("Failed to save collection suggestion: {e}"))?;
        }
        Ok(CollectionSuggestion {
            id,
            proposed_name,
            name_options,
            collection_kind,
            track: "sequence".to_string(),
            origin: "seed".to_string(),
            evidence_summary,
            score: overall_score,
            rule_version,
            state: "pending".to_string(),
            members,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn list_collection_suggestions(
        &self,
        state: Option<&str>,
    ) -> Result<Vec<CollectionSuggestion>, String> {
        let state = match state.unwrap_or("pending") {
            "pending" => "pending",
            "accepted" => "accepted",
            "rejected" => "rejected",
            "all" => "all",
            _ => return Err("Invalid suggestion state".to_string()),
        };
        let conn = self.read_conn()?;
        let sql = if state == "all" {
            "SELECT id, proposed_name, collection_kind, members_json, score,
                    rule_version, state, created_at, updated_at,
                    name_options_json, track, origin, evidence_summary
             FROM collection_suggestions ORDER BY score DESC, updated_at DESC, id DESC"
        } else {
            "SELECT id, proposed_name, collection_kind, members_json, score,
                    rule_version, state, created_at, updated_at,
                    name_options_json, track, origin, evidence_summary
             FROM collection_suggestions WHERE state = ?1
             -- 確かなものを先に。走査で作った候補は同じ時刻に一括で入るので、
             -- updated_at で並べると残りは id の順、つまり無作為だった。
             ORDER BY score DESC, updated_at DESC, id DESC"
        };
        let mut stmt = conn
            .prepare(sql)
            .map_err(|e| format!("Failed to prepare collection suggestions: {e}"))?;
        let mut suggestions = Vec::new();
        if state == "all" {
            let rows = stmt
                .query_map([], collection_suggestion_from_row)
                .map_err(|e| format!("Failed to query collection suggestions: {e}"))?;
            for row in rows {
                suggestions.push(row.map_err(|e| format!("Invalid collection suggestion: {e}"))?);
            }
        } else {
            let rows = stmt
                .query_map(params![state], collection_suggestion_from_row)
                .map_err(|e| format!("Failed to query collection suggestions: {e}"))?;
            for row in rows {
                suggestions.push(row.map_err(|e| format!("Invalid collection suggestion: {e}"))?);
            }
        }
        Ok(suggestions)
    }

    /// 提案を却下する。`member_keys` を渡すと、その作品と基準作品の組だけを
    /// 否定フィードバックに残す。利用者がチェックを外して「これは違う」と示した
    /// 作品だけを対象にできるようにするための引数で、省略すると従来どおり提案に
    /// 含まれる全作品を対象にする。
    ///
    /// 30 件の候補のうち 3 件だけが誤りだった場合に、残る 27 件まで巻き添えで
    /// 恒久ブロックしないための粒度である。
    pub fn reject_collection_suggestion(
        &self,
        suggestion_id: &str,
        member_keys: Option<&[WorkKey]>,
    ) -> Result<bool, String> {
        let suggestion_id = validate_collection_id(suggestion_id)?;
        let rejected_keys = member_keys
            .map(|keys| {
                keys.iter()
                    .map(|key| {
                        Ok((
                            validate_work_key_part(&key.source, "Source")?,
                            validate_work_key_part(&key.source_id, "Source ID")?,
                        ))
                    })
                    .collect::<Result<HashSet<_>, String>>()
            })
            .transpose()?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let payload = conn
            .query_row(
                "SELECT seed_json, members_json, rule_version
                 FROM collection_suggestions WHERE id = ?1 AND state = 'pending'",
                params![suggestion_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| format!("Failed to read collection suggestion: {e}"))?;
        let Some((seed_json, members_json, rule_version)) = payload else {
            return Ok(false);
        };
        let seed_ids: Vec<i64> = serde_json::from_str(&seed_json)
            .map_err(|e| format!("Invalid suggestion seeds: {e}"))?;
        let members: Vec<CollectionSuggestionMember> = serde_json::from_str(&members_json)
            .map_err(|e| format!("Invalid suggestion members: {e}"))?;
        let seeds = load_suggestion_works(&conn, &seed_ids)?;
        let tx = conn
            .unchecked_transaction()
            .map_err(|e| format!("Suggestion rejection transaction failed: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        for seed in &seeds {
            for member in &members {
                if seed.source == member.source && seed.source_id == member.source_id {
                    continue;
                }
                if let Some(keys) = rejected_keys.as_ref() {
                    if !keys.contains(&(member.source.clone(), member.source_id.clone())) {
                        continue;
                    }
                }
                save_pair_feedback(
                    &tx,
                    &WorkKey {
                        source: seed.source.clone(),
                        source_id: seed.source_id.clone(),
                    },
                    &WorkKey {
                        source: member.source.clone(),
                        source_id: member.source_id.clone(),
                    },
                    "reject",
                    &rule_version,
                    &now,
                )?;
            }
        }
        tx.execute(
            "UPDATE collection_suggestions SET state = 'rejected', updated_at = ?1
             WHERE id = ?2 AND state = 'pending'",
            params![now, suggestion_id],
        )
        .map_err(|e| format!("Failed to reject collection suggestion: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Suggestion rejection commit failed: {e}"))?;
        Ok(true)
    }

    /// ひな型を一覧から消すだけの操作。却下と違い否定フィードバックを残さない
    /// ため、同じ組合せは次回以降も候補になりうる。「今はいらない」と
    /// 「二度と出すな」を利用者が区別できるようにするための分岐である。
    /// 走査で出た候補を、まとめて閉じる。
    ///
    /// 300件を1件ずつ閉じる人はいない。**一括で消せないなら、出さないほうが
    /// まし**になってしまう。ここで消すのは下書きだけで、否定の記憶
    /// （`collection_pair_feedback`）は残さない — 規則が変われば、また出てくる。
    ///
    /// 系統を選んで閉じる道は無くした。畳んだメニューの中に「続き物だけ」
    /// 「テーマだけ」を置いていたが、系統を選んで閉じたい人はいなかった。
    /// いまは窓を閉じれば全部片付く。
    pub fn dismiss_swept_suggestions(&self) -> Result<usize, String> {
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM collection_suggestions WHERE origin = 'sweep' AND state = 'pending'",
                [],
            )
            .map_err(|e| format!("Failed to dismiss swept suggestions: {e}"))?;
        Ok(removed)
    }

    pub fn dismiss_collection_suggestion(&self, suggestion_id: &str) -> Result<bool, String> {
        let suggestion_id = validate_collection_id(suggestion_id)?;
        let conn = self.conn.lock().map_err(|e| e.to_string())?;
        let removed = conn
            .execute(
                "DELETE FROM collection_suggestions WHERE id = ?1",
                params![suggestion_id],
            )
            .map_err(|e| format!("Failed to dismiss collection suggestion: {e}"))?;
        Ok(removed > 0)
    }

    pub fn accept_collection_suggestion(
        &self,
        input: &AcceptCollectionSuggestionInput,
    ) -> Result<WorkCollection, String> {
        let suggestion_id = validate_collection_id(&input.suggestion_id)?.to_string();
        let suggestion = self
            .list_collection_suggestions(Some("pending"))?
            .into_iter()
            .find(|value| value.id == suggestion_id)
            .ok_or_else(|| "Pending collection suggestion not found".to_string())?;
        let selected_keys = input.member_keys.as_ref().map(|keys| {
            keys.iter()
                .map(|key| {
                    (
                        key.source.trim().to_string(),
                        key.source_id.trim().to_string(),
                    )
                })
                .collect::<HashSet<_>>()
        });
        let selected = suggestion
            .members
            .iter()
            .filter(|member| {
                selected_keys.as_ref().map_or(member.selected, |keys| {
                    keys.contains(&(member.source.clone(), member.source_id.clone()))
                })
            })
            .collect::<Vec<_>>();
        if selected.is_empty() {
            return Err("Select at least one work for the collection".to_string());
        }
        let name = input
            .name
            .clone()
            .unwrap_or_else(|| suggestion.proposed_name.clone());
        let kind = input
            .collection_kind
            .clone()
            .unwrap_or_else(|| suggestion.collection_kind.clone());
        // 説明は「自動ひな型から作成」を固定で書き込んでいた。どの束にも同じ
        // 一行が付くだけで、何ひとつ説明していない。代わりに、なぜ束なのかを
        // 書く。それも無ければ空のままにして、利用者が書ける場所を空けておく。
        let description =
            Some(suggestion.evidence_summary.clone()).filter(|value| !value.trim().is_empty());
        // 画面で選ばれた既存案なら、その案の出どころを保つ。とくにモデルの案を
        // 選んだだけなのに `manual` と記録すると、後から出自を説明できない。
        // 候補にない自由入力だけを、利用者が付けた名前として扱う。
        let name_source = accepted_suggestion_name_source(&suggestion, input.name.as_deref());
        let collection_input = WorkCollectionInput {
            id: None,
            name,
            description,
            collection_kind: kind,
            cover_download_id: selected.first().and_then(|member| member.download_id),
            cover_mode: Some("mosaic".to_string()),
            cover_image_path: None,
            name_source: Some(name_source),
            track: Some(suggestion.track.clone()),
        };
        let member_inputs = selected
            .iter()
            .enumerate()
            .map(|(position, member)| WorkCollectionMemberInput {
                source: member.source.clone(),
                source_id: member.source_id.clone(),
                title_snapshot: Some(member.title.clone()),
                author_snapshot: Some(member.author_name.clone()),
                position: Some(position as i64),
                member_role: Some("main".to_string()),
                added_by: Some("suggestion".to_string()),
                pinned: Some(false),
                note: None,
            })
            .collect::<Vec<_>>();
        let collection_id = new_collection_id("collection");
        let now = chrono::Utc::now().to_rfc3339();
        {
            let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
            let tx = conn
                .transaction()
                .map_err(|e| format!("Suggestion acceptance transaction failed: {e}"))?;
            write_collection_row(&tx, &collection_input, &collection_id, false, &now)?;
            add_collection_members_in_connection(&tx, &collection_id, &member_inputs, &now)?;
            for left_index in 0..selected.len() {
                for right_index in (left_index + 1)..selected.len() {
                    save_pair_feedback(
                        &tx,
                        &WorkKey {
                            source: selected[left_index].source.clone(),
                            source_id: selected[left_index].source_id.clone(),
                        },
                        &WorkKey {
                            source: selected[right_index].source.clone(),
                            source_id: selected[right_index].source_id.clone(),
                        },
                        "accept",
                        &suggestion.rule_version,
                        &now,
                    )?;
                }
            }
            let changed = tx
                .execute(
                    "UPDATE collection_suggestions SET state = 'accepted', updated_at = ?1
                     WHERE id = ?2 AND state = 'pending'",
                    params![now, suggestion_id],
                )
                .map_err(|e| format!("Failed to accept collection suggestion: {e}"))?;
            if changed != 1 {
                return Err("Collection suggestion is no longer pending".to_string());
            }
            tx.commit()
                .map_err(|e| format!("Suggestion acceptance commit failed: {e}"))?;
        }
        self.get_work_collection(&collection_id)
    }
}

fn accepted_suggestion_name_source(
    suggestion: &CollectionSuggestion,
    selected_name: Option<&str>,
) -> String {
    match selected_name {
        Some(name) => suggestion
            .name_options
            .iter()
            .find(|option| option.name == name)
            .map(|option| option.source.clone())
            .unwrap_or_else(|| "manual".to_string()),
        None => suggestion
            .name_options
            .first()
            .map(|option| option.source.clone())
            .unwrap_or_else(|| "title".to_string()),
    }
}

#[cfg(test)]
mod collection_write_tests {
    use super::*;

    fn connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::database::schema::initialize(&conn).unwrap();
        conn
    }

    fn insert_download(conn: &Connection, source_id: &str) -> i64 {
        conn.execute(
            "INSERT INTO downloads (
                source, source_id, title, author_name, author_id, content_type,
                json_path, downloaded_at
             ) VALUES ('pixiv', ?1, ?2, 'author', 'author-1', 'novel', ?3, ?4)",
            params![
                source_id,
                format!("title-{source_id}"),
                format!("{source_id}.json"),
                "2026-01-01T00:00:00Z"
            ],
        )
        .unwrap();
        conn.last_insert_rowid()
    }

    #[test]
    fn readding_download_preserves_member_metadata() {
        let conn = connection();
        let download_id = insert_download(&conn, "member-1");
        let input = WorkCollectionInput {
            name: "bundle".to_string(),
            collection_kind: "ordered".to_string(),
            ..Default::default()
        };
        write_collection_row(&conn, &input, "collection-test", false, "now").unwrap();
        add_collection_members_in_connection(
            &conn,
            "collection-test",
            &[WorkCollectionMemberInput {
                source: "pixiv".to_string(),
                source_id: "member-1".to_string(),
                title_snapshot: None,
                author_snapshot: None,
                position: None,
                member_role: Some("supplement".to_string()),
                added_by: Some("suggestion".to_string()),
                pinned: Some(true),
                note: Some("残す注釈".to_string()),
            }],
            "first",
        )
        .unwrap();

        let resolved = resolve_member_inputs(&conn, &[download_id]).unwrap();
        add_collection_members_in_connection(&conn, "collection-test", &resolved, "second")
            .unwrap();
        let metadata: (String, String, i64, Option<String>) = conn
            .query_row(
                "SELECT member_role, added_by, pinned, note
                 FROM work_collection_members WHERE collection_id = 'collection-test'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            metadata,
            (
                "supplement".to_string(),
                "suggestion".to_string(),
                1,
                Some("残す注釈".to_string())
            )
        );
    }

    #[test]
    fn selected_download_ids_must_all_exist_and_must_not_be_empty() {
        let conn = connection();
        let download_id = insert_download(&conn, "member-1");
        assert!(resolve_member_inputs(&conn, &[]).is_err());
        assert!(resolve_member_inputs(&conn, &[download_id, 999_999]).is_err());
    }

    #[test]
    fn collection_cover_patch_preserves_omitted_values_and_clears_nulls() {
        let conn = connection();
        let cover_download_id = insert_download(&conn, "cover-1");
        let create = WorkCollectionInput {
            name: "bundle".to_string(),
            collection_kind: "ordered".to_string(),
            cover_download_id: Some(cover_download_id),
            cover_image_path: Some("managed/cover.png".to_string()),
            ..Default::default()
        };
        write_collection_row(&conn, &create, "collection-cover", false, "first").unwrap();

        let omitted: WorkCollectionInput = serde_json::from_str(
            r#"{"id":"collection-cover","name":"renamed","collectionKind":"ordered"}"#,
        )
        .unwrap();
        write_collection_row(&conn, &omitted, "collection-cover", true, "second").unwrap();
        let preserved: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT cover_download_id, cover_image_path FROM work_collections WHERE id = ?1",
                params!["collection-cover"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            preserved,
            (
                Some(cover_download_id),
                Some("managed/cover.png".to_string())
            )
        );

        let cleared: WorkCollectionInput = serde_json::from_str(
            r#"{"id":"collection-cover","name":"renamed","collectionKind":"ordered","coverDownloadId":null,"coverImagePath":null}"#,
        )
        .unwrap();
        write_collection_row(&conn, &cleared, "collection-cover", true, "third").unwrap();
        let cleared: (Option<i64>, Option<String>) = conn
            .query_row(
                "SELECT cover_download_id, cover_image_path FROM work_collections WHERE id = ?1",
                params!["collection-cover"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(cleared, (None, None));
    }

    #[test]
    fn collection_and_members_roll_back_together() {
        let mut conn = connection();
        insert_download(&conn, "member-1");
        {
            let tx = conn.transaction().unwrap();
            let input = WorkCollectionInput {
                name: "bundle".to_string(),
                collection_kind: "ordered".to_string(),
                ..Default::default()
            };
            write_collection_row(&tx, &input, "collection-atomic", false, "now").unwrap();
            let result = add_collection_members_in_connection(
                &tx,
                "collection-atomic",
                &[
                    WorkCollectionMemberInput {
                        source: "pixiv".to_string(),
                        source_id: "member-1".to_string(),
                        title_snapshot: None,
                        author_snapshot: None,
                        position: None,
                        member_role: None,
                        added_by: None,
                        pinned: None,
                        note: None,
                    },
                    WorkCollectionMemberInput {
                        source: "pixiv".to_string(),
                        source_id: "missing".to_string(),
                        title_snapshot: None,
                        author_snapshot: None,
                        position: None,
                        member_role: None,
                        added_by: None,
                        pinned: None,
                        note: None,
                    },
                ],
                "now",
            );
            assert!(result.is_err());
        }
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM work_collections WHERE id = 'collection-atomic'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn accepted_suggestion_keeps_the_selected_name_provenance() {
        let suggestion = CollectionSuggestion {
            id: "suggestion-test".to_string(),
            proposed_name: "題名からの案".to_string(),
            name_options: vec![
                CollectionNameCandidate {
                    source: "title".to_string(),
                    name: "題名からの案".to_string(),
                    label: "題名".to_string(),
                    ..Default::default()
                },
                CollectionNameCandidate {
                    source: "llm".to_string(),
                    name: "モデルの案".to_string(),
                    label: "モデル".to_string(),
                    ..Default::default()
                },
            ],
            collection_kind: "ordered".to_string(),
            track: "sequence".to_string(),
            origin: "sweep".to_string(),
            evidence_summary: String::new(),
            score: 1.0,
            rule_version: "test".to_string(),
            state: "pending".to_string(),
            members: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
        };

        assert_eq!(
            accepted_suggestion_name_source(&suggestion, Some("モデルの案")),
            "llm"
        );
        assert_eq!(
            accepted_suggestion_name_source(&suggestion, Some("自分で付けた名前")),
            "manual"
        );
        assert_eq!(accepted_suggestion_name_source(&suggestion, None), "title");
    }
}
