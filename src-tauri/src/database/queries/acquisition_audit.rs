use super::{AssetEntry, Database};
use regex::Regex;
use rusqlite::params;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::LazyLock;

static REFERENCES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[(pixivimage|uploadedimage):([^\]]+)\]").unwrap());
static PIXIV_SPEC: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+)(?:-(\d+))?$").unwrap());

/// Bump when the audit learns to find a new kind of loss, so versions checked
/// under an older rule are inspected once more instead of on every launch.
/// 2: masked Pixiv title/caption/tags behind an R-18 cover placeholder.
pub(crate) const SAVED_AUDIT_RULE: i64 = 2;

/// Fields the App API blanked when it gated an R-18 novel. The age-gate cover
/// is the evidence: an empty caption on its own is an ordinary author choice.
/// Once the web detail has been consulted, a blank field is what the source
/// really holds and is no longer reported.
pub(crate) fn masked_pixiv_metadata(value: &Value) -> Vec<&'static str> {
    let gated = [value.get("cover_url"), value.pointer("/detail/cover_url")]
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(crate::source_assets::is_pixiv_placeholder_image);
    if !gated || value.get("web_detail").is_some_and(Value::is_object) {
        return Vec::new();
    }
    let detail = value.get("detail").unwrap_or(&Value::Null);
    ["title", "caption", "tags"]
        .into_iter()
        .filter(|key| {
            detail.get(*key).is_none_or(|field| {
                field.is_null()
                    || field.as_str().is_some_and(|text| text.trim().is_empty())
                    || field.as_array().is_some_and(Vec::is_empty)
            })
        })
        .collect()
}

pub(crate) fn masked_metadata_label(fields: &[&str]) -> String {
    fields
        .iter()
        .map(|field| match *field {
            "title" => "題名",
            "caption" => "説明",
            _ => "タグ",
        })
        .collect::<Vec<_>>()
        .join("・")
}

fn missing_saved_references(text: &str, filenames: &HashSet<String>) -> Vec<String> {
    let mut missing = Vec::new();
    for capture in REFERENCES.captures_iter(text) {
        let spec = &capture[2];
        let prefixes = if &capture[1] == "uploadedimage" {
            vec![format!("inline_{spec}.")]
        } else if let Some(parts) = PIXIV_SPEC.captures(spec) {
            let page = parts
                .get(2)
                .map_or(Some(1), |value| value.as_str().parse::<u64>().ok());
            match page.and_then(|page| page.checked_sub(1)) {
                Some(index) => vec![
                    format!("{}_p{index}.", &parts[1]),
                    format!("{spec}_p{index}."),
                ],
                None => Vec::new(),
            }
        } else {
            Vec::new()
        };
        let available = filenames.iter().any(|filename| {
            let filename = filename.strip_prefix("illust_").unwrap_or(filename);
            prefixes.iter().any(|prefix| filename.starts_with(prefix))
        });
        let reference = capture.get(0).unwrap().as_str();
        if !available && !missing.iter().any(|existing| existing == reference) {
            missing.push(reference.to_string());
        }
    }
    missing
}

pub(crate) fn saved_pixiv_missing_references(value: &Value, assets: &[AssetEntry]) -> Vec<String> {
    let filenames = assets
        .iter()
        .filter(|asset| {
            !asset
                .original_url
                .as_deref()
                .is_some_and(crate::source_assets::is_pixiv_placeholder_image)
                && std::fs::metadata(&asset.local_path)
                    .is_ok_and(|metadata| metadata.is_file() && metadata.len() > 0)
        })
        .map(|asset| asset.filename.clone())
        .collect::<Vec<_>>();
    pixiv_missing_references_for_filenames(value, filenames)
}

pub(crate) fn pixiv_missing_references_for_filenames(
    value: &Value,
    filenames: impl IntoIterator<Item = String>,
) -> Vec<String> {
    let text = value
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    missing_saved_references(text, &filenames.into_iter().collect())
}

impl Database {
    /// Revisit only current versions containing provider gate icons. Historical
    /// placeholder files remain intact and do not restart audits on every launch.
    pub(crate) fn requeue_placeholder_illustrations(&self) -> Result<usize, String> {
        let rows = {
            let conn = self.read_conn()?;
            let mut statement = conn.prepare("SELECT d.id,d.json_path,a.local_path FROM downloads d JOIN assets a ON a.download_id=d.id WHERE d.source='pixiv' AND a.asset_type<>'cover' AND a.original_url LIKE '%/common/images/limit_%'").map_err(|error|error.to_string())?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        let ids = rows
            .into_iter()
            .filter(|(_, json, asset)| {
                super::asset_belongs_to_version_path(
                    std::path::Path::new(asset),
                    std::path::Path::new(json),
                )
            })
            .map(|(id, _, _)| id)
            .collect::<HashSet<_>>();
        let conn = self.conn.lock().map_err(|error| error.to_string())?;
        let mut count = 0;
        for id in ids {
            count += conn
                .execute(
                    "DELETE FROM saved_asset_audits WHERE download_id=?1",
                    params![id],
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(count)
    }

    /// Inspect each saved version once per audit rule, in small idle batches.
    /// Older archives did not record missing_illustrations or masked details,
    /// so job history alone cannot recover them.
    pub(crate) fn audit_saved_pixiv_assets(
        &self,
        after_id: i64,
        limit: usize,
    ) -> Result<(usize, Vec<i64>, i64), String> {
        let rows = {
            let conn = self.read_conn()?;
            let mut statement = conn.prepare(
                "SELECT d.id,d.source_id,d.title,d.json_path,d.current_version FROM downloads d
                 LEFT JOIN saved_asset_audits a ON a.download_id=d.id
                 WHERE d.id>?2 AND d.source='pixiv' AND d.content_type='novel'
                   AND (a.download_id IS NULL OR a.json_path<>d.json_path OR a.version<>d.current_version
                        OR a.rule_version<?3)
                 ORDER BY d.id LIMIT ?1",
            ).map_err(|error| error.to_string())?;
            let rows = statement
                .query_map(
                    [limit.clamp(1, 32) as i64, after_id, SAVED_AUDIT_RULE],
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    },
                )
                .map_err(|error| error.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|error| error.to_string())?;
            rows
        };
        let mut findings = Vec::new();
        for (id, source_id, title, path, version) in &rows {
            let content = std::fs::read(path)
                .map_err(|error| format!("保存済み作品 {id} を検査できません: {error}"))?;
            let value: Value = serde_json::from_slice(&content)
                .map_err(|error| format!("保存済み作品 {id} のJSONを検査できません: {error}"))?;
            let text = value
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let missing = if REFERENCES.is_match(text) {
                saved_pixiv_missing_references(
                    &value,
                    &self.get_current_assets(&self.get_download(*id)?)?,
                )
            } else {
                Vec::new()
            };
            let masked = masked_pixiv_metadata(&value);
            if !missing.is_empty() || !masked.is_empty() {
                let mut losses = Vec::new();
                if !missing.is_empty() {
                    losses.push(format!("挿絵参照を表示できません: {}", missing.join(", ")));
                }
                if !masked.is_empty() {
                    losses.push(format!(
                        "作品情報（{}）が取得元で伏せられた状態です",
                        masked_metadata_label(&masked)
                    ));
                }
                let payload = serde_json::json!({
                    "kind":"revision", "source":"pixiv", "sourceId":source_id,
                    "title":title, "subtitle":"本文保存済み", "holdKind":"incomplete",
                    "holdReason":format!("[一部保存] 本文は保存済みですが、{}", losses.join("。")),
                    "missingReferences":missing, "missingMetadata":masked,
                    "originalData":{"localVersion":version}
                });
                findings.push((*id, source_id, title, payload.to_string()));
            }
        }
        let mut conn = self.conn.lock().map_err(|error| error.to_string())?;
        let tx = conn.transaction().map_err(|error| error.to_string())?;
        let mut changed = Vec::new();
        for (id, source_id, title, payload) in findings {
            // A pending revision or a user's dismissal remains authoritative.
            let inserted = tx.execute(
                "INSERT INTO update_candidates(source,source_id,kind,title,payload_json,target_type,status,first_seen_at,updated_at)
                 VALUES('pixiv',?1,'revision',?2,?3,'work','held',CURRENT_TIMESTAMP,CURRENT_TIMESTAMP)
                 ON CONFLICT(source,source_id) DO NOTHING", params![source_id,title,payload],
            ).map_err(|error| error.to_string())?;
            if inserted > 0 {
                changed.push(id);
            }
        }
        for (id, _, _, path, version) in &rows {
            tx.execute("INSERT INTO saved_asset_audits(download_id,json_path,version,rule_version) VALUES(?1,?2,?3,?4)
                ON CONFLICT(download_id) DO UPDATE SET json_path=excluded.json_path,version=excluded.version,
                    rule_version=excluded.rule_version",
                params![id,path,version,SAVED_AUDIT_RULE]).map_err(|error| error.to_string())?;
        }
        tx.commit().map_err(|error| error.to_string())?;
        Ok((
            rows.len(),
            changed,
            rows.last().map_or(after_id, |row| row.0),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_require_the_exact_page_and_real_filename() {
        let names = HashSet::from([
            "123_p1.jpg".into(),
            "illust_456-3_p2.png".into(),
            "inline_789.jpg".into(),
        ]);
        assert_eq!(missing_saved_references(
            "[pixivimage:123-2][pixivimage:456-3][uploadedimage:789][pixivimage:123][pixivimage:123-0][pixivimage:id=123][pixivimage:123]", &names),
            vec!["[pixivimage:123]", "[pixivimage:123-0]", "[pixivimage:id=123]"]);
    }

    #[test]
    fn masked_details_need_the_age_gate_and_an_unconsulted_web_detail() {
        let gate = "https://s.pximg.net/common/images/limit_r18g_100.png";
        let masked = serde_json::json!({"detail":{"title":"","caption":" ","cover_url":gate}});
        assert_eq!(
            masked_pixiv_metadata(&masked),
            vec!["title", "caption", "tags"]
        );
        assert_eq!(
            masked_metadata_label(&masked_pixiv_metadata(&masked)),
            "題名・説明・タグ"
        );
        let filled = serde_json::json!({"cover_url":gate,"detail":{"title":"題","caption":"説明","tags":[{"name":"R-18"}]}});
        assert!(masked_pixiv_metadata(&filled).is_empty());
        let ungated = serde_json::json!({"cover_url":"https://i.pximg.net/a.jpg","detail":{"title":"題","caption":"","tags":[]}});
        assert!(masked_pixiv_metadata(&ungated).is_empty());
        let consulted = serde_json::json!({"cover_url":gate,"web_detail":{},"detail":{"title":"題","caption":"","tags":[]}});
        assert!(masked_pixiv_metadata(&consulted).is_empty());
    }
}
