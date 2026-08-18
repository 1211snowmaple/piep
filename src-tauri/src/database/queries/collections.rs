//! 作品コレクションとその候補生成。
//!
//! 取得や検索とは独立した、利用者が自分で組み立てる集合を扱う。`queries` の
//! 子モジュールに置いてあるのは、`Database` の接続やプールといった非公開の
//! 内部へ触れる必要があるためで、親の実装詳細をここだけに開いている。

use super::*;

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
        let members = stmt
            .query_map(params![collection_id], work_collection_member_from_row)
            .map_err(|e| format!("Failed to query collection members: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read collection members: {e}"))?;
        Ok(WorkCollection { summary, members })
    }

    /// コレクションを新規作成、または指定 ID のコレクションを更新する。
    pub fn upsert_work_collection(
        &self,
        input: &WorkCollectionInput,
    ) -> Result<WorkCollection, String> {
        let name = validate_collection_name(&input.name)?;
        let description = normalize_bounded_optional(&input.description, 10_000, "Description")?;
        let kind = validate_collection_kind(&input.collection_kind)?;
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
            if let Some(cover_id) = input.cover_download_id {
                let exists = tx
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
            if input.id.is_some() {
                let changed = tx
                    .execute(
                        "UPDATE work_collections
                         SET name = ?1, description = ?2, collection_kind = ?3,
                             cover_download_id = ?4, revision = revision + 1, updated_at = ?5
                         WHERE id = ?6",
                        params![name, description, kind, input.cover_download_id, now, id],
                    )
                    .map_err(|e| format!("Failed to update collection: {e}"))?;
                if changed == 0 {
                    return Err("Collection not found".to_string());
                }
            } else {
                tx.execute(
                    "INSERT INTO work_collections (
                        id, name, description, collection_kind, cover_download_id,
                        revision, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)",
                    params![id, name, description, kind, input.cover_download_id, now],
                )
                .map_err(|e| format!("Failed to create collection: {e}"))?;
            }
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
                revision, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?6)
             ON CONFLICT(id) DO UPDATE SET
                name = excluded.name,
                description = excluded.description,
                collection_kind = excluded.collection_kind,
                cover_download_id = COALESCE(excluded.cover_download_id, work_collections.cover_download_id),
                revision = work_collections.revision + 1,
                updated_at = excluded.updated_at",
            params![id, name, description, kind, cover_download_id, now],
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
            let exists = tx
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
            let mut next_position: i64 = tx
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
                let (download_id, title, author): (i64, String, String) = tx
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
                tx.execute(
                    "INSERT INTO work_collection_members (
                        collection_id, source, source_id, download_id,
                        title_snapshot, author_snapshot, position, member_role,
                        added_by, pinned, note, created_at, updated_at
                     ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?12)
                     ON CONFLICT(collection_id, source, source_id) DO UPDATE SET
                        download_id = excluded.download_id,
                        title_snapshot = excluded.title_snapshot,
                        author_snapshot = excluded.author_snapshot,
                        position = CASE
                            WHEN ?13 IS NULL THEN work_collection_members.position
                            ELSE excluded.position END,
                        member_role = excluded.member_role,
                        added_by = excluded.added_by,
                        pinned = excluded.pinned,
                        note = excluded.note,
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
                        input.position,
                    ],
                )
                .map_err(|e| format!("Failed to add collection member: {e}"))?;
            }
            tx.execute(
                "UPDATE work_collections
                 SET revision = revision + 1, updated_at = ?1 WHERE id = ?2",
                params![now, collection_id],
            )
            .map_err(|e| format!("Failed to update collection revision: {e}"))?;
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

        // 意味索引は準備済みの場合だけ使う。提案作成がモデル取得を突然始めることはない。
        let semantic_status = crate::database::semantic_index::status(&self.storage_dir);
        let mut semantic_scores: HashMap<i64, f64> = HashMap::new();
        if semantic_status.indexed_chunks > 0 && semantic_status.model_ready {
            for seed in &seed_works {
                let query = format!("{} {}", seed.title, seed.author_name);
                match crate::database::semantic_index::search(&self.storage_dir, &query, 80) {
                    Ok(hits) => {
                        for hit in hits {
                            candidate_ids.insert(hit.download_id);
                            semantic_scores
                                .entry(hit.download_id)
                                .and_modify(|score| *score = score.max(hit.score))
                                .or_insert(hit.score);
                        }
                    }
                    Err(error) => {
                        log::warn!("Semantic collection suggestion failed: {error}");
                        break;
                    }
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
            if !is_seed && suggestion_pair_is_rejected(&conn, &candidate, &seed_works)? {
                continue;
            }
            let mut evidence = Vec::new();
            let mut score = 0.0_f64;
            let link_order = None;
            let link_depth = link_paths.get(&candidate.id).map(|path| path.depth);
            if is_seed {
                score = 1.0;
                evidence.push(CollectionSuggestionEvidence {
                    kind: "seed".to_string(),
                    label: "選択した基準作品".to_string(),
                    contribution: 1.0,
                });
            }

            let shared_series = shared_series_with_seeds(&conn, candidate.id, &seed_json)?;
            let series_order = shared_series.iter().filter_map(|value| value.1).min();
            if let Some((title, _)) = shared_series.first() {
                add_suggestion_evidence(
                    &mut evidence,
                    &mut score,
                    "official_series",
                    &format!("公式シリーズ「{title}」"),
                    0.58,
                );
            }
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
                    &mut score,
                    "content_link",
                    &label,
                    contribution,
                );
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
                    &mut score,
                    "same_author",
                    if seed_works
                        .iter()
                        .any(|seed| seed.source != candidate.source)
                    {
                        "同一作者名（取得元を横断）"
                    } else {
                        "同一作者"
                    },
                    0.12,
                );
            }

            let (candidate_stem, episode_order) = title_stem_and_order(&candidate.title);
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
                    &mut score,
                    "title_similarity",
                    &format!("タイトル語幹の類似度 {:.0}%", title_similarity * 100.0),
                    contribution,
                );
            }
            if let Some(semantic_score) = semantic_scores.get(&candidate.id).copied() {
                if semantic_score >= 0.45 && !is_seed {
                    let contribution = ((semantic_score - 0.4) * 0.35).clamp(0.02, 0.20);
                    add_suggestion_evidence(
                        &mut evidence,
                        &mut score,
                        "semantic_similarity",
                        &format!("本文の意味的な近さ {:.0}%", semantic_score * 100.0),
                        contribution,
                    );
                }
            }
            score = score.clamp(0.0, 1.0);
            if !is_seed && score < 0.44 {
                continue;
            }
            // 公式シリーズと作者が同じだけの作品は「同じシリーズに載っている」
            // 以上のことを示さない。題名・リンク・話数のいずれかで結びつく
            // 作品だけを既定で選び、それ以外は利用者が明示的に選ぶ。
            let has_specific_evidence = evidence.iter().any(|value| {
                matches!(
                    value.kind.as_str(),
                    "content_link" | "title_similarity" | "seed"
                )
            });
            // 設計提案 6-6 は弱い信号だけで 2 段以上連鎖させないとしている。
            // 遠いリンク先は候補として見せるが、既定では選ばない。題名や公式順で
            // 裏付けが取れているものはこの制限から外す。
            let corroborated = series_order.is_some()
                || episode_order.is_some()
                || evidence
                    .iter()
                    .any(|value| value.kind == "title_similarity");
            let distant_link = link_depth.is_some_and(|depth| depth >= 2) && !corroborated;
            let default_selected = is_seed
                || ((has_specific_evidence || episode_order.is_some() || score >= 0.85)
                    && !distant_link);
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
        let proposed_name = proposed_collection_name(&conn, &ranked, &seed_json)?;
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
        let members_json = serde_json::to_string(&members)
            .map_err(|e| format!("Failed to encode suggestion members: {e}"))?;
        let now = chrono::Utc::now().to_rfc3339();
        drop(conn);
        {
            let conn = self.conn.lock().map_err(|e| e.to_string())?;
            conn.execute(
                "INSERT INTO collection_suggestions (
                    id, seed_json, proposed_name, collection_kind, members_json,
                    score, rule_version, state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'pending', ?8, ?8)",
                params![
                    id,
                    seed_json,
                    proposed_name,
                    collection_kind,
                    members_json,
                    overall_score,
                    rule_version,
                    now,
                ],
            )
            .map_err(|e| format!("Failed to save collection suggestion: {e}"))?;
        }
        Ok(CollectionSuggestion {
            id,
            proposed_name,
            collection_kind,
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
                    rule_version, state, created_at, updated_at
             FROM collection_suggestions ORDER BY updated_at DESC, id DESC"
        } else {
            "SELECT id, proposed_name, collection_kind, members_json, score,
                    rule_version, state, created_at, updated_at
             FROM collection_suggestions WHERE state = ?1
             ORDER BY updated_at DESC, id DESC"
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
        let created = self.upsert_work_collection(&WorkCollectionInput {
            id: None,
            name,
            description: Some("自動ひな型から作成".to_string()),
            collection_kind: kind,
            cover_download_id: selected.first().and_then(|member| member.download_id),
        })?;
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
        let collection = match self.add_work_collection_members(&created.summary.id, &member_inputs)
        {
            Ok(collection) => collection,
            Err(error) => {
                let _ = self.delete_work_collection(&created.summary.id);
                return Err(error);
            }
        };
        let now = chrono::Utc::now().to_rfc3339();
        let mut conn = self.conn.lock().map_err(|e| e.to_string())?;
        let tx = conn
            .transaction()
            .map_err(|e| format!("Suggestion acceptance transaction failed: {e}"))?;
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
        tx.execute(
            "UPDATE collection_suggestions SET state = 'accepted', updated_at = ?1
             WHERE id = ?2 AND state = 'pending'",
            params![now, suggestion_id],
        )
        .map_err(|e| format!("Failed to accept collection suggestion: {e}"))?;
        tx.commit()
            .map_err(|e| format!("Suggestion acceptance commit failed: {e}"))?;
        Ok(collection)
    }
}
