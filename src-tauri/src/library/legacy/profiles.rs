use super::{
    rows::{anomaly, guarded_row, mark_row, mark_table, mark_unresolved, marked, LegacyRow},
    LegacyImporter,
};
use crate::library::{
    cover_identity, Capture, CaptureSource, CapturedMedia, EntityKind, Error, Library, LibraryId,
    Result,
};
use rusqlite::{params, OptionalExtension};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

/// 旧プロフィールの版のフォルダーにあった画像。
struct ProfileImage {
    name: String,
    path: PathBuf,
    size: u64,
    /// 中身がまったく同じ別名のファイル。同じ画像として扱い、どちらのパスも移行済みにする。
    duplicates: Vec<PathBuf>,
}

/// `icon.png` のように、版が書いた固定名のファイルか。
fn fixed_name(name: &str, slot: &str) -> bool {
    std::path::Path::new(name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        == Some(slot)
}

fn same_bytes(left: &ProfileImage, right: &ProfileImage) -> Result<bool> {
    // プロフィール画像は小さい。大きなものを読み比べて止まらないよう上限を置く。
    const LIMIT: u64 = 64 * 1024 * 1024;
    if left.size != right.size || left.size > LIMIT {
        return Ok(false);
    }
    Ok(fs::read(&left.path)? == fs::read(&right.path)?)
}

impl LegacyImporter {
    /// 移行できた版の数と、個別に未解決として残した記録を返す。
    /// 一つの版・一人の作者の異常で、ほかのプロフィールの移行を止めない。
    pub(super) fn normalize_profiles(
        &self,
        target: &Library,
        cancel: &AtomicBool,
    ) -> Result<(usize, Vec<String>)> {
        let mut versions = self.conn.prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name='entity_versions'",
        )?;
        if !versions.exists([])? {
            return Ok((0, Vec::new()));
        }
        let mut count = 0;
        let mut anomalies = Vec::new();
        // each_row は表の物理順ではなく、版の順を指定して呼ぶ。
        let mut stmt = self.conn.prepare(
            "SELECT id FROM entity_versions ORDER BY entity_type,source,source_key,version,id",
        )?;
        for old in stmt.query_map([], |r| r.get::<_, i64>(0))? {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Canceled);
            }
            let old = old?;
            match self.each_row_matching("entity_versions", "id", old, |row| {
                self.import_profile_version(target, row)
            }) {
                Ok(_) => count += 1,
                Err(error) if anomaly(&error) => {
                    let reason = error.to_string();
                    let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
                    let tx = conn.transaction()?;
                    mark_unresolved(target, &tx, "entity_versions", &old.to_string(), &reason)?;
                    tx.commit()?;
                    anomalies.push(format!("entity_versions {old}: {reason}"));
                }
                Err(error) => return Err(error),
            }
        }
        let versions_complete = anomalies.is_empty();
        // 現在の行だけが持つ値を、同じ改稿への補完として足す。表の正規化より前に済ませる。
        for (table, kind) in [
            ("people", EntityKind::Person),
            ("series", EntityKind::Series),
        ] {
            {
                let conn = target.conn.lock().map_err(|_| Error::Busy)?;
                if marked(&conn, table)? {
                    continue;
                }
            }
            let mut rows = Vec::new();
            self.each_row(table, |row| {
                rows.push(row);
                Ok(())
            })?;
            for row in rows {
                if cancel.load(Ordering::Acquire) {
                    return Err(Error::Canceled);
                }
                match self.supplement_current_profile(target, table, kind, &row) {
                    Ok(()) => {}
                    Err(error) if anomaly(&error) => {
                        anomalies.push(format!("{table} {} の補完: {error}", row.label(&["id"])))
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        for (table, kind) in [("people", "person"), ("series", "series")] {
            if marked(&tx, table)? {
                continue;
            }
            let before = anomalies.len();
            self.each_row(table,|row| {
                let label = row.label(&["id"]);
                guarded_row(target, &tx, table, &label, &mut anomalies, || {
                let (source,key)=(row.text("source")?,row.text("source_key")?);
                let version=row.integer("current_version")?;
                let old:i64=self.conn.query_row("SELECT id FROM entity_versions WHERE entity_type=?1 AND source=?2 AND source_key=?3 AND version=?4",params![kind,source,key,version],|r|r.get(0))?;
                let snapshot:Vec<u8>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='entity_versions' AND source_key=?1 AND status='imported'",[old.to_string()],|r|r.get(0))?;
                let entity:Vec<u8>=tx.query_row("SELECT entity_id FROM external_identities WHERE provider=?1 AND kind=?2 AND external_key=?3",params![source,kind,key],|r|r.get(0))?;
                let current:Vec<u8>=tx.query_row("SELECT snapshot_id FROM entity_current WHERE entity_id=?1",[&entity],|r|r.get(0))?;
                // 現在の状態は、履歴の末尾の版か、その版への補完のどちらか。
                let supplement:Option<Vec<u8>>=tx.query_row("SELECT target_id FROM migration_records WHERE source_table='__profile_supplements__' AND source_key=?1 AND status='imported'",[format!("{table}:{label}")],|r|r.get(0)).optional()?;
                if current!=snapshot && Some(&current)!=supplement.as_ref() {return Err(Error::Integrity(format!("{kind} {key} の現在版が履歴の末尾と一致しません")));}
                let metadata:String=tx.query_row("SELECT metadata_json FROM revision_snapshots WHERE id=?1",[&current],|r|r.get(0))?;
                let metadata:Value=serde_json::from_str(&metadata)?;
                for (old,new) in [("display_name","display_name"),("title","title"),("description","description"),("is_concluded","is_concluded"),("published_content_count","published_content_count")] {
                    if let Some(value)=row.0.get(old).filter(|v|!v.is_null()) {
                        let equivalent=metadata.get(new)==Some(value) || (new=="is_concluded" && metadata.get(new).and_then(Value::as_bool).map(i64::from)==value.as_i64());
                        if !equivalent {return Err(Error::Integrity(format!("{kind} {key} の現在の {old} が保存された原本と一致しません")));}
                    }
                }
                tx.execute("INSERT INTO source_entity_state(entity_id,last_checked_at,last_fetched_at) VALUES(?1,?2,?3) ON CONFLICT(entity_id) DO UPDATE SET last_checked_at=excluded.last_checked_at,last_fetched_at=excluded.last_fetched_at",params![entity,row.optional_time("last_checked_at")?,row.optional_time("last_fetched_at")?])?;
                let id=LibraryId::from_slice(&entity).map_err(|_|Error::Integrity("プロフィールの識別子が不正です".into()))?;
                mark_row(&tx,table,&label,id)
                })
            })?;
            if anomalies.len() == before {
                mark_table(&tx, table)?;
            }
        }
        if versions_complete {
            mark_table(&tx, "entity_versions")?;
        }
        tx.commit()?;
        Ok((count, anomalies))
    }

    /// 現在の行だけが持つ値を、履歴の末尾の版への補完として保存する。
    ///
    /// 旧アプリは、軽い確認で分かった値を版を作らずに行へ書き込んでいた。
    /// - シリーズの「完結」の印: 版の原本には項目がなく、行にだけ値がある。
    /// - 表紙・アイコン: 版を保存した後で同じフォルダーへ足され、行のパスがそれを指している。
    ///
    /// どちらも作者による変更ではなく、同じ改稿について後から分かったことなので、
    /// 改稿は作らず、保存状態を一つ足す。行のパスが指さない画像には触れない。
    fn supplement_current_profile(
        &self,
        target: &Library,
        table: &str,
        kind: EntityKind,
        row: &LegacyRow,
    ) -> Result<()> {
        let (source, key) = (row.text("source")?, row.text("source_key")?);
        let label = row.label(&["id"]);
        let kind_name = if kind == EntityKind::Person {
            "person"
        } else {
            "series"
        };
        let version = row.integer("current_version")?;
        // 版の行や移行済みの版がなければ、何も足さない。表の正規化が未解決として報告する。
        let Some((old, json_path)) = self.conn.query_row("SELECT id,json_path FROM entity_versions WHERE entity_type=?1 AND source=?2 AND source_key=?3 AND version=?4",params![kind_name,source,key,version],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()? else {
            return Ok(());
        };
        let (known, saved) = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let Some(snapshot) = conn.query_row("SELECT target_id FROM migration_records WHERE source_table='entity_versions' AND source_key=?1 AND status='imported'",[old.to_string()],|r|r.get::<_,Vec<u8>>(0)).optional()? else {
                return Ok(());
            };
            let metadata: String = conn.query_row(
                "SELECT metadata_json FROM revision_snapshots WHERE id=?1",
                [&snapshot],
                |r| r.get(0),
            )?;
            let known: BTreeMap<String, Value> = serde_json::from_str(&metadata)?;
            let mut saved = BTreeMap::<String, Option<Vec<u8>>>::new();
            let mut stmt = conn.prepare("SELECT m.slot,r.file_id FROM snapshot_media m LEFT JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1")?;
            for item in stmt.query_map([&snapshot], |r| Ok((r.get(0)?, r.get(1)?)))? {
                let (slot, file) = item?;
                saved.insert(slot, file);
            }
            (known, saved)
        };
        let mut metadata = BTreeMap::new();
        let mut states = BTreeMap::new();
        if kind == EntityKind::Series {
            // 版の原本に項目がなく、行にだけ値があるもの。稼働棚では同じ 142 件が両方を持つ。
            if !known.contains_key("is_concluded") {
                if let Some(value) = row.0.get("is_concluded").filter(|v| !v.is_null()) {
                    let concluded = match value.as_i64() {
                        Some(0) => false,
                        Some(1) => true,
                        _ => {
                            return Err(Error::Integrity(format!(
                                "{table} {label} の is_concluded が真偽値ではありません"
                            )))
                        }
                    };
                    metadata.insert("is_concluded".to_owned(), Value::Bool(concluded));
                    states.insert("is_concluded".to_owned(), "known".to_owned());
                }
            }
            if !known.contains_key("published_content_count") {
                if let Some(value) = row
                    .0
                    .get("published_content_count")
                    .filter(|v| !v.is_null())
                {
                    let count = value.as_u64().ok_or_else(|| {
                        Error::Integrity(format!(
                            "{table} {label} の published_content_count が件数ではありません"
                        ))
                    })?;
                    metadata.insert("published_content_count".to_owned(), Value::from(count));
                    states.insert("published_content_count".to_owned(), "known".to_owned());
                }
            }
        }
        let assets = self
            .managed_path(&json_path)?
            .parent()
            .ok_or_else(|| Error::UnsafePath(json_path.clone().into()))?
            .join("assets");
        let mut media = Vec::new();
        let mut image_paths = Vec::new();
        for (slot, column) in [("icon", "icon_path"), ("cover", "cover_path")] {
            let Some(stored) = row
                .0
                .get(column)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let path = self.managed_path(stored)?;
            // 現在の版のフォルダーにある画像だけを扱う。ほかの場所を指すパスは推測しない。
            if path.parent() != Some(assets.as_path()) || !path.try_exists()? {
                continue;
            }
            super::super::store::regular_file(&path)?;
            let bytes = fs::read(&path)?;
            let id = crate::library::Fingerprint::of(&bytes);
            if saved.get(slot).and_then(Option::as_deref) == Some(id.0.as_slice()) {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    Error::Integrity("プロフィール画像名が UTF-8 ではありません".into())
                })?
                .to_owned();
            let mime = match path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            media.push(CapturedMedia {
                slot: slot.into(),
                role: if slot == "cover" && kind == EntityKind::Person {
                    "banner"
                } else {
                    slot
                }
                .into(),
                // どこから取った画像かは記録がない。URL を推測して付けない。
                source_key: None,
                source_url: None,
                quality: "unknown".into(),
                original_name: name,
                mime_type: mime.into(),
                bytes: Some(CaptureSource::File(path.clone())),
            });
            image_paths.push((slot, path));
        }
        if metadata.is_empty() && media.is_empty() {
            return Ok(());
        }
        let record = format!("{table}:{label}");
        let request = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let request:Option<Vec<u8>>=conn.query_row("SELECT request_id FROM migration_requests WHERE source_table='__profile_supplements__' AND source_key=?1",[&record],|r|r.get(0)).optional()?;
            match request {
                Some(id) => LibraryId::from_slice(&id)
                    .map_err(|_| Error::Integrity("補完の移行要求 ID が不正です".into()))?,
                None => {
                    let id = LibraryId::now_v7();
                    conn.execute(
                        "INSERT INTO migration_requests VALUES('__profile_supplements__',?1,?2)",
                        params![record, id.as_bytes().as_slice()],
                    )?;
                    id
                }
            }
        };
        // 補完の根拠は旧い行そのもの。行をそのまま原本として残す。
        let raw = serde_json::to_vec(&serde_json::json!({"legacy_table": table, "row": row.0}))?;
        let result = target.capture_with_legacy_classification(
            Capture {
                acquired_at: row
                    .0
                    .contains_key("updated_at")
                    .then(|| row.optional_time("updated_at"))
                    .transpose()?
                    .flatten(),
                request_id: request,
                provider: source.into(),
                external_key: key.into(),
                kind,
                route: "legacy-current-row".into(),
                raw: CaptureSource::Bytes(raw),
                metadata,
                field_states: states,
                body: None,
                media,
                attachments: vec![],
            },
            // 旧記録の補完として扱う。作者側の差が見つからないかぎり、改稿を作らない。
            Some("repair"),
        )?;
        if result.created_revision {
            return Err(Error::Integrity(format!(
                "{table} {label} の補完が改稿として保存されました"
            )));
        }
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        mark_row(&tx, "__profile_supplements__", &record, result.snapshot_id)?;
        for (slot, path) in image_paths {
            let file:Vec<u8>=tx.query_row("SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 AND m.slot=?2",params![result.snapshot_id.as_bytes().as_slice(),slot],|r|r.get(0))?;
            self.mark_used_file(&tx, &path, &file)?;
        }
        tx.commit()?;
        Ok(())
    }

    fn import_profile_version(&self, target: &Library, row: LegacyRow) -> Result<()> {
        let old = row.integer("id")?.to_string();
        let provider = row.text("source")?;
        let key = row.text("source_key")?;
        let kind = match row.text("entity_type")? {
            "person" => EntityKind::Person,
            "series" => EntityKind::Series,
            _ => return Err(Error::Unsupported("旧プロフィールの型が不明です".into())),
        };
        let path = self.managed_path(row.text("json_path")?)?;
        let mut bytes = Vec::new();
        fs::File::open(&path)?
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error::Integrity(
                "プロフィールが読み込み上限を超えています".into(),
            ));
        }
        let data: Value = serde_json::from_slice(&bytes)?;
        if data.get("source").and_then(Value::as_str) != Some(provider)
            || data.get("sourceKey").and_then(Value::as_str) != Some(key)
        {
            return Err(Error::Integrity(format!(
                "プロフィール {old} の取得元が履歴行と違います"
            )));
        }
        let mut metadata = BTreeMap::new();
        let mut states = BTreeMap::from([
            ("media".into(), "known".into()),
            ("attachments".into(), "known".into()),
        ]);
        for (field, name) in [
            ("displayName", "display_name"),
            ("title", "title"),
            ("description", "description"),
            ("links", "links"),
            ("profileItems", "profile_items"),
            ("isConcluded", "is_concluded"),
            ("publishedContentCount", "published_content_count"),
            ("sampleNovelCount", "sample_novel_count"),
        ] {
            match data.get(field) {
                Some(value) if !value.is_null() => {
                    let valid = match name {
                        "display_name" | "title" | "description" => value.is_string(),
                        "links" | "profile_items" => value.is_array(),
                        "is_concluded" => value.is_boolean(),
                        "published_content_count" | "sample_novel_count" => {
                            value.as_u64().is_some()
                        }
                        _ => false,
                    };
                    if !valid {
                        return Err(Error::Integrity(format!(
                            "プロフィール {old} の {field} の型が不正です"
                        )));
                    }
                    metadata.insert(name.into(), value.clone());
                }
                _ => {
                    states.insert(name.into(), "unknown".into());
                }
            }
        }
        let dir = path
            .parent()
            .ok_or_else(|| Error::UnsafePath(path.clone()))?
            .join("assets");
        let mut found = BTreeMap::<&'static str, Vec<ProfileImage>>::new();
        if dir.try_exists()? {
            super::super::store::regular_directory(&dir)?;
            for entry in fs::read_dir(&dir)? {
                let entry = entry?;
                let file = entry.path();
                super::super::store::regular_file(&file)?;
                let name = entry.file_name().into_string().map_err(|_| {
                    Error::Integrity("プロフィール画像名が UTF-8 ではありません".into())
                })?;
                let stem = std::path::Path::new(&name)
                    .file_stem()
                    .and_then(|s| s.to_str());
                let slot = if name.starts_with("icon_") || stem == Some("icon") {
                    "icon"
                } else if name.starts_with("cover_") || stem == Some("cover") {
                    "cover"
                } else {
                    return Err(Error::Integrity(format!(
                        "プロフィール {old} に用途の不明なファイルがあります"
                    )));
                };
                found.entry(slot).or_default().push(ProfileImage {
                    size: fs::metadata(&file)?.len(),
                    name,
                    path: file,
                    duplicates: Vec::new(),
                });
            }
        }
        let recorded_url = |slot: &str| {
            data.get(if slot == "icon" {
                "iconUrl"
            } else {
                "coverUrl"
            })
            .and_then(Value::as_str)
            .filter(|url| !url.is_empty())
        };
        let mut images = BTreeMap::<&'static str, ProfileImage>::new();
        for (slot, mut files) in found {
            files.sort_by(|left, right| left.name.cmp(&right.name));
            // 旧実装は名前の付け方を途中で変えたので、同じ中身が別名で並ぶ版がある。
            // 中身がまったく同じものは一つの画像として扱い、版が書いた固定名を残す。
            let mut distinct = Vec::<ProfileImage>::new();
            for mut file in files {
                let mut same = None;
                for (index, kept) in distinct.iter().enumerate() {
                    if same_bytes(kept, &file)? {
                        same = Some(index);
                        break;
                    }
                }
                match same {
                    Some(index) => {
                        let kept = &mut distinct[index];
                        if fixed_name(&file.name, slot) && !fixed_name(&kept.name, slot) {
                            std::mem::swap(kept, &mut file);
                            kept.duplicates.append(&mut file.duplicates);
                        }
                        kept.duplicates.push(file.path);
                    }
                    None => distinct.push(file),
                }
            }
            let chosen = if distinct.len() == 1 {
                distinct.remove(0)
            } else {
                // 中身の違う画像が並ぶ版は、後から別の画像が同じ場所へ足されたもの。
                // その版の原本が記録した URL から付いた名前の一枚だけを、当時の画像とする。
                // 残りは使わずに置き、管理領域の全件照合で所属の未解決として残す。
                let expected = recorded_url(slot).map(|url| {
                    let base = url.rsplit('/').next().unwrap_or(url);
                    format!("{slot}_{}", base.split(['?', '#']).next().unwrap_or(base))
                });
                let mut named = distinct
                    .into_iter()
                    .filter(|file| Some(&file.name) == expected.as_ref())
                    .collect::<Vec<_>>();
                if named.len() != 1 {
                    return Err(Error::Integrity(format!(
                        "プロフィール {old} の画像が重複しています"
                    )));
                }
                named.remove(0)
            };
            images.insert(slot, chosen);
        }
        let asset_count = row.optional_integer("asset_count")?.unwrap_or(0);
        let recorded_size = row.optional_integer("file_size_bytes")?;
        let accounted = |images: &BTreeMap<&'static str, ProfileImage>| -> Result<bool> {
            let mut assets = 0u64;
            for image in images.values() {
                assets = assets.checked_add(image.size).ok_or_else(|| {
                    Error::Integrity("プロフィールの容量が上限を超えました".into())
                })?;
            }
            // 初期の人物スナップショット（icon.png 等の固定名）は画像だけを集計していた。
            // その既知の形式でも JSON 自体は必ず保管し、任意の容量差を許容しない。
            let legacy_asset_accounting = kind == EntityKind::Person
                && !images.is_empty()
                && images
                    .iter()
                    .all(|(slot, image)| fixed_name(&image.name, slot));
            // 画像を一枚も持たない版は、作者・シリーズとも容量を 0 と記録していた。
            let recorded_no_assets = images.is_empty() && recorded_size == Some(0);
            Ok(asset_count == images.len() as i64
                && recorded_size.is_none_or(|size| {
                    size >= 0
                        && (size as u64 == bytes.len() as u64 + assets
                            || (legacy_asset_accounting && size as u64 == assets)
                            || recorded_no_assets)
                }))
        };
        if !accounted(&images)? {
            // 版より前の配置から残った画像が同じ場所にある版がある。原本に URL の記録がない
            // 用途の画像を外すと版の記録（個数・容量）にちょうど合う場合だけ、その版の画像と
            // みなさない。外した画像は使わずに置き、全件照合で未解決として残す。
            let total = images.len();
            let (recorded, _left): (BTreeMap<_, _>, BTreeMap<_, _>) = images
                .into_iter()
                .partition(|(slot, _)| recorded_url(slot).is_some());
            let none = BTreeMap::new();
            images = if recorded.len() < total && accounted(&recorded)? {
                recorded
            } else if accounted(&none)? {
                // 版は「画像なし・容量 0」と記録している。フォルダーにある画像は、その版を
                // 保存した後で同じ場所へ足されたもの。当時は未取得として移し、足された
                // 画像は使わずに置いて、全件照合で未解決として残す。
                none
            } else {
                return Err(Error::Integrity(format!(
                    "プロフィール {old} のファイル数・容量が記録と違います"
                )));
            };
        }
        let mut media = Vec::new();
        let mut image_paths = Vec::new();
        for (slot, url_field) in [("icon", "iconUrl"), ("cover", "coverUrl")] {
            let saved = images.remove(slot).map(|image| {
                image_paths.push((slot, image.path.clone()));
                for duplicate in image.duplicates {
                    image_paths.push((slot, duplicate));
                }
                (image.name, image.path)
            });
            let url = data
                .get(url_field)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty());
            if saved.is_none() && url.is_none() {
                continue;
            }
            let (name, file) = saved.map_or_else(
                || (slot.to_owned(), None),
                |(name, file)| (name, Some(CaptureSource::File(file))),
            );
            let mime = match std::path::Path::new(&name)
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase()
                .as_str()
            {
                "png" => "image/png",
                "jpg" | "jpeg" => "image/jpeg",
                "gif" => "image/gif",
                "webp" => "image/webp",
                _ => "application/octet-stream",
            };
            media.push(CapturedMedia {
                slot: slot.into(),
                role: if slot == "cover" && kind == EntityKind::Person {
                    "banner"
                } else {
                    slot
                }
                .into(),
                source_key: url.map(|u| cover_identity(provider, u)),
                source_url: url.map(str::to_owned),
                quality: "unknown".into(),
                original_name: name,
                mime_type: mime.into(),
                bytes: file,
            });
        }
        let request = {
            let conn = target.conn.lock().map_err(|_| Error::Busy)?;
            let request:Option<Vec<u8>>=conn.query_row("SELECT request_id FROM migration_requests WHERE source_table='entity_versions' AND source_key=?1",[&old],|r|r.get(0)).optional()?;
            match request {
                Some(id) => LibraryId::from_slice(&id)
                    .map_err(|_| Error::Integrity("プロフィールの移行要求 ID が不正です".into()))?,
                None => {
                    let id = LibraryId::now_v7();
                    conn.execute(
                        "INSERT INTO migration_requests VALUES('entity_versions',?1,?2)",
                        params![old, id.as_bytes().as_slice()],
                    )?;
                    id
                }
            }
        };
        let result = target.capture_with_legacy_classification(
            Capture {
                acquired_at: row.optional_time("created_at")?,
                request_id: request,
                provider: provider.into(),
                external_key: key.into(),
                kind,
                route: "legacy-profile-import".into(),
                raw: CaptureSource::File(path.clone()),
                metadata,
                field_states: states,
                body: None,
                media,
                attachments: vec![],
            },
            Some("legacy_unclassified"),
        )?;
        let mut conn = target.conn.lock().map_err(|_| Error::Busy)?;
        let tx = conn.transaction()?;
        mark_row(&tx, "entity_versions", &old, result.snapshot_id)?;
        let raw: Vec<u8> = tx.query_row(
            "SELECT raw_file_id FROM acquisition_observations WHERE request_id=?1",
            [request.as_bytes().as_slice()],
            |r| r.get(0),
        )?;
        self.mark_used_file(&tx, &path, &raw)?;
        for (slot, path) in image_paths {
            let file:Vec<u8>=tx.query_row("SELECT r.file_id FROM snapshot_media m JOIN media_renditions r ON r.id=m.rendition_id WHERE m.snapshot_id=?1 AND m.slot=?2",params![result.snapshot_id.as_bytes().as_slice(),slot],|r|r.get(0))?;
            self.mark_used_file(&tx, &path, &file)?;
        }
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Fingerprint;
    use serde_json::json;

    struct Fixture(std::path::PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("piep-profile-import-{}", LibraryId::now_v7()));
            let old = root.join("old");
            fs::create_dir_all(&old).unwrap();
            let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
            conn.execute_batch("CREATE TABLE downloads(id INTEGER,source TEXT,source_id TEXT,title TEXT,author_name TEXT,favorite INTEGER);
CREATE TABLE download_versions(id INTEGER,download_id INTEGER,version INTEGER,json_path TEXT,original_json_path TEXT,change_kind TEXT);
CREATE TABLE assets(id INTEGER,download_id INTEGER,asset_type TEXT,filename TEXT,local_path TEXT,original_url TEXT,mime_type TEXT,file_size_bytes INTEGER);
CREATE TABLE entity_versions(id INTEGER,entity_type TEXT,source TEXT,source_key TEXT,version INTEGER,json_path TEXT,asset_count INTEGER,file_size_bytes INTEGER);
CREATE TABLE people(id INTEGER,source TEXT,source_key TEXT,display_name TEXT,description TEXT,current_version INTEGER,last_checked_at TEXT,last_fetched_at TEXT);
CREATE TABLE series(id INTEGER,source TEXT,source_key TEXT,title TEXT,current_version INTEGER,last_checked_at TEXT,last_fetched_at TEXT,is_concluded INTEGER,published_content_count INTEGER);
INSERT INTO people VALUES(7,'pixiv','author','New name','Description',2,'2026-01-02 00:00:00','2026-01-01 00:00:00');
INSERT INTO series VALUES(8,'pixiv','series','Series title',1,NULL,NULL,1,3);").unwrap();
            for (id, kind, key, version) in [
                (1, "person", "author", 1),
                (2, "person", "author", 2),
                (3, "series", "series", 1),
            ] {
                let dir = format!(
                    "{}/pixiv/{key}/v{version}",
                    if kind == "person" {
                        "profiles"
                    } else {
                        "series"
                    }
                );
                fs::create_dir_all(old.join(&dir).join("assets")).unwrap();
                let data = if kind == "person" {
                    json!({"source":"pixiv","sourceKey":key,"displayName":if version==1 {"Old name"}else{"New name"},"description":if version==1 {Value::Null}else{json!("Description")},"links":[],"iconUrl":format!("https://i.pximg.net/icon{version}.png")})
                } else {
                    json!({"source":"pixiv","sourceKey":key,"title":"Series title","isConcluded":true,"publishedContentCount":3,"coverUrl":"https://i.pximg.net/series.png"})
                };
                let raw = serde_json::to_vec(&data).unwrap();
                let image = format!("exact image bytes {id}\0ICC").into_bytes();
                let image_name = if kind == "person" {
                    "icon_image.png"
                } else {
                    "cover_image.png"
                };
                fs::write(old.join(&dir).join("original.json"), &raw).unwrap();
                fs::write(old.join(&dir).join("assets").join(image_name), &image).unwrap();
                conn.execute(
                    "INSERT INTO entity_versions VALUES(?1,?2,'pixiv',?3,?4,?5,1,?6)",
                    params![
                        id,
                        kind,
                        key,
                        version,
                        format!("{dir}/original.json"),
                        (raw.len() + image.len()) as i64
                    ],
                )
                .unwrap();
            }
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn profile_history_current_state_and_files_survive_without_the_old_folder() {
        let fixture = Fixture::new();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert_eq!(report.profile_snapshots, 3);
        assert!(library.works(10).unwrap().is_empty());
        let person = {
            let conn = library.conn.lock().unwrap();
            let id: Vec<u8> = conn
                .query_row(
                    "SELECT entity_id FROM external_identities WHERE kind='person'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            LibraryId::from_slice(&id).unwrap()
        };
        let states = library.snapshots(person).unwrap();
        assert_eq!(states.len(), 2);
        assert_eq!(states[0].metadata["display_name"], json!("Old name"));
        assert_eq!(states[0].field_states["description"], "unknown");
        assert_eq!(states[1].metadata["description"], json!("Description"));
        assert!(states
            .iter()
            .all(|s| s.classification == "legacy_unclassified"));
        assert_eq!(library.current_snapshot(person).unwrap(), states[1].id);
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert_eq!(library.snapshots(person).unwrap().len(), 2);
        library.backup(&fixture.0.join("backup")).unwrap();
        drop(source);
        drop(library);
        fs::rename(fixture.0.join("old"), fixture.0.join("old-unavailable")).unwrap();
        let backup = Library::open(&fixture.0.join("backup")).unwrap();
        assert_eq!(backup.current_snapshot(person).unwrap(), states[1].id);
        let file = backup.media_file(person, states[0].id, "icon").unwrap();
        assert_eq!(file.id, Fingerprint::of(b"exact image bytes 1\0ICC"));
        let mut bytes = Vec::new();
        backup.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, b"exact image bytes 1\0ICC");
        backup.verify().unwrap();
    }

    #[test]
    fn duplicate_names_and_later_additions_resolve_while_one_broken_version_stays_alone() {
        // 稼働棚で見つかった三つの形。同じ中身の別名、後から同じ場所へ足された別の表紙、
        // どちらとも決められない二枚。最後の一つだけが未解決に残り、ほかは移行する。
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let person = old.join("profiles/pixiv/author/v1/assets");
        fs::copy(person.join("icon_image.png"), person.join("icon.png")).unwrap();
        let series = old.join("series/pixiv/series/v1/assets");
        fs::rename(
            series.join("cover_image.png"),
            series.join("cover_series.png"),
        )
        .unwrap();
        fs::write(series.join("cover_later.png"), b"a different, later cover").unwrap();
        let broken = old.join("profiles/pixiv/other/v1");
        fs::create_dir_all(broken.join("assets")).unwrap();
        let raw = serde_json::to_vec(&json!({"source":"pixiv","sourceKey":"other","displayName":"Other","links":[],"iconUrl":"https://i.pximg.net/unrelated.png"})).unwrap();
        fs::write(broken.join("original.json"), &raw).unwrap();
        fs::write(broken.join("assets/icon_a.png"), b"first").unwrap();
        fs::write(broken.join("assets/icon_b.png"), b"second").unwrap();
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        conn.execute(
            "INSERT INTO entity_versions VALUES(4,'person','pixiv','other',1,'profiles/pixiv/other/v1/original.json',1,?1)",
            [raw.len() as i64 + 5],
        )
        .unwrap();
        drop(conn);
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&old).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(report.profile_snapshots, 3, "{:?}", report.unresolved);
        assert!(
            report
                .unresolved
                .iter()
                .any(|line| line.starts_with("entity_versions 4: ")),
            "{:?}",
            report.unresolved
        );
        let (series_id, snapshot) = {
            let conn = library.conn.lock().unwrap();
            let status = |table: &str, key: &str| -> Option<String> {
                conn.query_row(
                    "SELECT status FROM migration_records WHERE source_table=?1 AND source_key=?2",
                    [table, key],
                    |r| r.get(0),
                )
                .ok()
            };
            for key in ["1", "2", "3"] {
                assert_eq!(status("entity_versions", key).as_deref(), Some("imported"));
            }
            assert_eq!(
                status("entity_versions", "4").as_deref(),
                Some("unresolved")
            );
            assert_ne!(
                status("entity_versions", "__all_rows__").as_deref(),
                Some("imported")
            );
            // 現在の状態の照合は、異常のあった版と関係のない作者・シリーズでは終わる。
            assert_eq!(status("people", "7").as_deref(), Some("imported"));
            assert_eq!(status("series", "8").as_deref(), Some("imported"));
            // 同じ中身の別名は、どちらのパスも移行済みになる。
            for name in ["icon.png", "icon_image.png"] {
                assert_eq!(
                    status(
                        "__filesystem__",
                        &format!("profiles/pixiv/author/v1/assets/{name}")
                    )
                    .as_deref(),
                    Some("imported")
                );
            }
            // 後から足された一枚は当時の表紙にせず、所属の未解決として残す。
            assert_eq!(
                status(
                    "__filesystem__",
                    "series/pixiv/series/v1/assets/cover_series.png"
                )
                .as_deref(),
                Some("imported")
            );
            assert_eq!(
                status(
                    "__filesystem__",
                    "series/pixiv/series/v1/assets/cover_later.png"
                )
                .as_deref(),
                Some("unresolved")
            );
            let id: Vec<u8> = conn
                .query_row(
                    "SELECT entity_id FROM external_identities WHERE kind='series'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            let id = LibraryId::from_slice(&id).unwrap();
            let snapshot: Vec<u8> = conn
                .query_row(
                    "SELECT snapshot_id FROM entity_current WHERE entity_id=?1",
                    [id.as_bytes().as_slice()],
                    |r| r.get(0),
                )
                .unwrap();
            (id, LibraryId::from_slice(&snapshot).unwrap())
        };
        let cover = library.media_file(series_id, snapshot, "cover").unwrap();
        assert_eq!(cover.id, Fingerprint::of(b"exact image bytes 3\0ICC"));
        // 未解決は、足された一枚と、決められなかった版の三ファイル。
        assert_eq!(report.unresolved_files, 4);
        assert!(!library.activation_problems().unwrap().is_empty());
        library.verify().unwrap();
        // やり直しても、移行済みの版を重ねない。
        let again = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(again.unresolved_files, 4);
        assert_eq!(library.snapshots(series_id).unwrap().len(), 1);
    }

    #[test]
    fn versions_recorded_without_images_migrate_and_leave_later_files_unresolved() {
        // 稼働棚の 784 版と同じ形。画像なし・容量 0 と記録され、うち一部は後から
        // 同じ場所へ表紙が足されている。
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        for (id, key, later) in [(4, "bare", false), (5, "added", true)] {
            let dir = format!("series/pixiv/{key}/v1");
            fs::create_dir_all(old.join(&dir).join("assets")).unwrap();
            let raw = serde_json::to_vec(&json!({"source":"pixiv","sourceKey":key,"title":"Title","coverUrl":"https://i.pximg.net/c/240x480_80/cover.jpg"})).unwrap();
            fs::write(old.join(&dir).join("original.json"), &raw).unwrap();
            if later {
                fs::write(
                    old.join(&dir).join("assets/cover_later.jpg"),
                    b"added later",
                )
                .unwrap();
            }
            conn.execute(
                "INSERT INTO entity_versions VALUES(?1,'series','pixiv',?2,1,?3,0,0)",
                params![id, key, format!("{dir}/original.json")],
            )
            .unwrap();
        }
        drop(conn);
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&old).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(report.profile_snapshots, 5, "{:?}", report.unresolved);
        assert_eq!(report.unresolved_files, 1, "{:?}", report.unresolved);
        // 件数は説明の行数でなく、未解決の移行記録の数で数える。
        assert_eq!(report.unresolved_records, 1, "{:?}", report.unresolved);
        assert!(report.stages.iter().any(|s| s.stage == "filesystem"));
        assert_eq!(report.stages.last().unwrap().stage, "verify");
        let conn = library.conn.lock().unwrap();
        // 当時の表紙は、URL だけが分かっている未取得として残る。
        let unsaved: i64 = conn
            .query_row(
                "SELECT count(*) FROM snapshot_media m JOIN revision_snapshots s ON s.id=m.snapshot_id JOIN external_identities e ON e.entity_id=s.entity_id WHERE e.external_key IN ('bare','added') AND m.slot='cover' AND m.rendition_id IS NULL",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(unsaved, 2);
        assert_eq!(
            conn.query_row(
                "SELECT status FROM migration_records WHERE source_table='__filesystem__' AND source_key='series/pixiv/added/v1/assets/cover_later.jpg'",
                [],
                |r| r.get::<_, String>(0),
            )
            .unwrap(),
            "unresolved"
        );
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn values_only_the_current_row_holds_become_supplements_of_the_same_revision() {
        // 稼働棚の形。軽い確認が版を作らずに書いた「完結」の印（142 件）と、
        // 版の保存後に足されて行が指している表紙（シリーズ 73 件）。
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        conn.execute_batch("ALTER TABLE series ADD COLUMN cover_path TEXT; ALTER TABLE series ADD COLUMN updated_at TEXT;").unwrap();
        // 原本に完結の項目も表紙の URL もなく、画像なしと記録された版。行だけが値を持つ。
        for (id, key, concluded) in [(4, "later", 1), (5, "open", 0)] {
            let dir = format!("series/pixiv/{key}/v1");
            fs::create_dir_all(old.join(&dir).join("assets")).unwrap();
            let raw =
                serde_json::to_vec(&json!({"source":"pixiv","sourceKey":key,"title":"Title"}))
                    .unwrap();
            fs::write(old.join(&dir).join("original.json"), &raw).unwrap();
            fs::write(
                old.join(&dir).join("assets/cover_later.jpg"),
                format!("added later {key}"),
            )
            .unwrap();
            conn.execute(
                "INSERT INTO entity_versions VALUES(?1,'series','pixiv',?2,1,?3,0,0)",
                params![id, key, format!("{dir}/original.json")],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO series VALUES(?1,'pixiv',?2,'Title',1,NULL,NULL,?3,12,?4,'2026-02-01 00:00:00')",
                params![id + 10, key, concluded, format!("{dir}/assets/cover_later.jpg")],
            )
            .unwrap();
        }
        drop(conn);
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&old).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        // 行が指していない画像を補完にしないことは、上の試験（行のない版）が見ている。
        assert_eq!(report.unresolved_files, 0, "{:?}", report.unresolved);
        assert_eq!(report.unresolved_records, 0, "{:?}", report.unresolved);
        {
            let conn = library.conn.lock().unwrap();
            for (key, concluded) in [("later", true), ("open", false)] {
                let (revisions, snapshots, metadata, cover): (i64, i64, String, Vec<u8>) = conn
                    .query_row(
                        "SELECT (SELECT count(*) FROM source_revisions v WHERE v.entity_id=e.entity_id),(SELECT count(*) FROM revision_snapshots s WHERE s.entity_id=e.entity_id),s.metadata_json,r.file_id FROM external_identities e JOIN entity_current c ON c.entity_id=e.entity_id JOIN revision_snapshots s ON s.id=c.snapshot_id JOIN snapshot_media m ON m.snapshot_id=s.id AND m.slot='cover' JOIN media_renditions r ON r.id=m.rendition_id WHERE e.external_key=?1",
                        [key],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .unwrap();
                // 改稿は増えず、保存状態が一つ増える。現在の状態は補完のほう。
                assert_eq!((revisions, snapshots), (1, 2), "{key}");
                let metadata: Value = serde_json::from_str(&metadata).unwrap();
                assert_eq!(metadata["is_concluded"], json!(concluded), "{key}");
                assert_eq!(metadata["published_content_count"], json!(12), "{key}");
                assert_eq!(
                    cover,
                    crate::library::Fingerprint::of(format!("added later {key}").as_bytes()).0
                );
                for (name, status) in [("cover_later.jpg", "imported")] {
                    assert_eq!(
                        conn.query_row(
                            "SELECT status FROM migration_records WHERE source_table='__filesystem__' AND source_key=?1",
                            [format!("series/pixiv/{key}/v1/assets/{name}")],
                            |r| r.get::<_, String>(0),
                        )
                        .unwrap(),
                        status,
                        "{key} {name}"
                    );
                }
            }
        }
        library.verify().unwrap();
        // やり直しても、補完を重ねない。
        drop(source);
        let source = LegacyImporter::open(&old).unwrap();
        let again = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert_eq!(again.unresolved_records, 0, "{:?}", again.unresolved);
        let snapshots: i64 = library
            .conn
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM revision_snapshots s JOIN external_identities e ON e.entity_id=s.entity_id WHERE e.external_key IN ('later','open')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(snapshots, 4);
        library.verify().unwrap();
    }

    #[test]
    fn an_old_version_copy_of_an_image_saved_by_a_later_version_is_not_left_unresolved() {
        // 稼働棚の 35 枚と同じ形。v1 は画像なしと記録され、後から v2 の表紙と同じ中身の
        // ファイルが v1 のフォルダーにも置かれた。行は v2 の表紙を指している。
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        let cover = b"the cover both folders hold";
        for (id, version, recorded) in [(4, 1, false), (5, 2, true)] {
            let dir = format!("series/pixiv/copied/v{version}");
            fs::create_dir_all(old.join(&dir).join("assets")).unwrap();
            let raw = serde_json::to_vec(&json!({"source":"pixiv","sourceKey":"copied","title":format!("Title {version}"),"coverUrl":"https://i.pximg.net/cover.jpg"})).unwrap();
            fs::write(old.join(&dir).join("original.json"), &raw).unwrap();
            fs::write(old.join(&dir).join("assets/cover_cover.jpg"), cover).unwrap();
            let (count, size) = if recorded {
                (1, (raw.len() + cover.len()) as i64)
            } else {
                (0, 0)
            };
            conn.execute(
                "INSERT INTO entity_versions VALUES(?1,'series','pixiv','copied',?2,?3,?4,?5)",
                params![id, version, format!("{dir}/original.json"), count, size],
            )
            .unwrap();
        }
        // DB に版の行がない作者のフォルダー（保存の途中で残ったもの）。
        fs::create_dir_all(old.join("profiles/pixiv/gone/v3/assets")).unwrap();
        fs::write(old.join("profiles/pixiv/gone/v3/original.json"), b"{}").unwrap();
        fs::write(
            old.join("profiles/pixiv/gone/v3/assets/icon_left.png"),
            b"left behind",
        )
        .unwrap();
        // 別のシリーズにある同じ中身は、持ち主が違うので済んだものとしない。
        fs::create_dir_all(old.join("series/pixiv/series/v1/assets")).unwrap();
        fs::write(
            old.join("series/pixiv/series/v1/assets/cover_other.jpg"),
            cover,
        )
        .unwrap();
        drop(conn);
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&old).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        let conn = library.conn.lock().unwrap();
        let status = |table: &str, key: &str| -> Option<String> {
            conn.query_row(
                "SELECT status FROM migration_records WHERE source_table=?1 AND source_key=?2",
                [table, key],
                |r| r.get(0),
            )
            .optional()
            .unwrap()
        };
        let copy = "series/pixiv/copied/v1/assets/cover_cover.jpg";
        assert_eq!(
            status("__filesystem__", copy).as_deref(),
            Some("imported"),
            "{:?}",
            report.unresolved
        );
        assert_eq!(
            status("__same_as_saved_media__", copy).as_deref(),
            Some("imported")
        );
        let other = "series/pixiv/series/v1/assets/cover_other.jpg";
        assert_eq!(status("__same_as_saved_media__", other), None);
        assert_eq!(status("__unrecorded_version__", other), None);
        // 版の行がないフォルダーは、中身を保管して理由を残す。
        for name in ["original.json", "assets/icon_left.png"] {
            let key = format!("profiles/pixiv/gone/v3/{name}");
            assert_eq!(
                status("__filesystem__", &key).as_deref(),
                Some("imported"),
                "{key}"
            );
            assert_eq!(
                status("__unrecorded_version__", &key).as_deref(),
                Some("imported"),
                "{key}"
            );
        }
        drop(conn);
        library.verify().unwrap();
    }

    #[test]
    fn mismatched_profile_counts_and_current_pointers_remain_unresolved() {
        for change in [
            "UPDATE entity_versions SET asset_count=0 WHERE id=2",
            "UPDATE people SET current_version=1",
            "UPDATE people SET display_name='Not in the original'",
        ] {
            let fixture = Fixture::new();
            let conn = rusqlite::Connection::open(fixture.0.join("old/piep.db")).unwrap();
            conn.execute_batch(change).unwrap();
            drop(conn);
            let library = Library::create(&fixture.0.join("new")).unwrap();
            let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
            let report = source
                .import(&library, &AtomicBool::new(false), |_| {})
                .unwrap();
            assert!(!report.unresolved.is_empty(), "{change}");
            assert!(!library.activation_problems().unwrap().is_empty());
            library.verify().unwrap();
        }
    }

    #[test]
    fn the_known_fixed_icon_format_keeps_json_even_when_the_old_total_excluded_it() {
        let fixture = Fixture::new();
        let old = fixture.0.join("old");
        let assets = old.join("profiles/pixiv/author/v1/assets");
        fs::rename(assets.join("icon_image.png"), assets.join("icon.png")).unwrap();
        let conn = rusqlite::Connection::open(old.join("piep.db")).unwrap();
        conn.execute(
            "UPDATE entity_versions SET file_size_bytes=?1 WHERE id=1",
            [fs::metadata(assets.join("icon.png")).unwrap().len() as i64],
        )
        .unwrap();
        drop(conn);
        let source = LegacyImporter::open(&old).unwrap();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        let raw = fs::read(old.join("profiles/pixiv/author/v1/original.json")).unwrap();
        let file = library.file(Fingerprint::of(&raw)).unwrap();
        let mut bytes = Vec::new();
        library.objects.export(&file, &mut bytes).unwrap();
        assert_eq!(bytes, raw);
        library.verify().unwrap();
    }

    #[test]
    fn a_failed_recheck_stays_unresolved_until_the_original_source_is_available_again() {
        let fixture = Fixture::new();
        let library = Library::create(&fixture.0.join("new")).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        drop(source);
        let path = fixture
            .0
            .join("old/profiles/pixiv/author/v2/assets/icon_image.png");
        let original = fs::read(&path).unwrap();
        let mut changed = original.clone();
        changed[0] ^= 1;
        fs::write(&path, changed).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        assert!(!source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap()
            .unresolved
            .is_empty());
        assert!(library
            .activation_problems()
            .unwrap()
            .iter()
            .any(|s| s.contains("profiles")));
        library.verify().unwrap();
        drop(source);
        fs::write(&path, original).unwrap();
        let source = LegacyImporter::open(&fixture.0.join("old")).unwrap();
        let report = source
            .import(&library, &AtomicBool::new(false), |_| {})
            .unwrap();
        assert!(report.unresolved.is_empty(), "{:?}", report.unresolved);
        assert!(library.activation_problems().unwrap().is_empty());
        let conn = library.conn.lock().unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM revision_snapshots", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            3
        );
    }
}
