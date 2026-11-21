//! 監視の画面とジョブが使う口。切り替えた棚では旧 DB の監視表を読まず、書かない。

use crate::database::{UpdateTarget, WatchGroup};
use crate::library::{
    EntityKind, Library, LibraryId, ResourceReference, ResourceScope, WatchEdit, WatchResult,
};
use std::sync::Arc;
use tauri::Manager;

pub(super) async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|e| e.to_string())?
}

pub(super) fn active(app: &tauri::AppHandle) -> Result<Arc<Library>, String> {
    app.try_state::<Arc<super::shelf::Shelf>>()
        .ok_or_else(|| "棚は開いていません".to_string())?
        .library()
}

fn date(value: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|v| v.to_rfc3339())
        .ok_or("監視の日時が不正です".into())
}

fn icon(library: &Library, target: &crate::library::WatchTarget) -> Result<Option<String>, String> {
    let kind = match target.kind.as_str() {
        "author" => EntityKind::Person,
        "series" => EntityKind::Series,
        _ => return Ok(None),
    };
    let Some(entity) = library
        .entity_by_key(kind, &target.provider, &target.key)
        .map_err(|e| e.to_string())?
    else {
        return Ok(None);
    };
    let snapshot = match library.current_snapshot(entity) {
        Ok(v) => v,
        Err(crate::library::Error::Sql(rusqlite::Error::QueryReturnedNoRows)) => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let slot = if target.kind == "author" {
        "icon"
    } else {
        "cover"
    };
    Ok(library
        .resource_manifest(entity, ResourceScope::Snapshot(snapshot))
        .map_err(|e| e.to_string())?
        .resources
        .into_iter()
        .find(|r| r.reference == ResourceReference::Media { slot: slot.into() })
        .and_then(|r| r.file)
        .map(|file| format!("shelf:{}", file.id)))
}

pub(super) fn targets(
    library: &Library,
    kind: Option<&str>,
    enabled: bool,
) -> Result<Vec<UpdateTarget>, String> {
    for _ in 0..3 {
        let generation = library.watch_generation().map_err(|e| e.to_string())?;
        let saved = library.watch_targets().map_err(|e| e.to_string())?;
        if library.watch_generation().map_err(|e| e.to_string())? != generation {
            continue;
        }
        return saved
            .into_iter()
            .filter(|t| {
                (!enabled || t.enabled)
                    && kind.is_none_or(|k| k.is_empty() || k == "all" || k == t.kind)
            })
            .map(|target| {
                let icon_path = icon(library, &target)?;
                Ok(UpdateTarget {
                    id: Some(target.id),
                    watch_generation: Some(generation),
                    target_type: target.kind,
                    source: target.provider,
                    source_key: target.key,
                    display_name: target.name,
                    enabled: target.enabled,
                    last_checked_at: target.checked_at.map(date).transpose()?,
                    last_seen_source_id: target.seen_key,
                    last_seen_source_updated_at: target.seen_updated_at.map(date).transpose()?,
                    metadata_json: Some(target.options),
                    created_at: date(target.created_at)?,
                    updated_at: date(target.updated_at)?,
                    last_hit_at: target.hit_at.map(date).transpose()?,
                    consecutive_errors: target.errors,
                    watch_groups: target.groups,
                    icon_path,
                })
            })
            .collect();
    }
    Err("監視の設定が変更されています。読み直してください".into())
}

pub(super) fn find(
    library: &Library,
    kind: &str,
    source: &str,
    key: &str,
) -> Result<Option<UpdateTarget>, String> {
    Ok(targets(library, Some(kind), false)?
        .into_iter()
        .find(|t| t.source == source && t.source_key == key))
}

pub(super) fn groups(library: &Library) -> Result<Vec<WatchGroup>, String> {
    for _ in 0..3 {
        let generation = library.watch_generation().map_err(|e| e.to_string())?;
        let saved = library.watch_groups().map_err(|e| e.to_string())?;
        if library.watch_generation().map_err(|e| e.to_string())? != generation {
            continue;
        }
        return saved
            .into_iter()
            .map(|g| {
                Ok(WatchGroup {
                    id: g.id,
                    watch_generation: Some(generation),
                    name: g.name,
                    sort_order: g.position,
                    created_at: date(g.created_at)?,
                    updated_at: date(g.updated_at)?,
                    target_ids: g.targets,
                })
            })
            .collect();
    }
    Err("監視グループが変更されています。読み直してください".into())
}

/// 棚にある監視対象か。
pub(super) fn target_id(library: &Library, key: LibraryId) -> Result<LibraryId, String> {
    library
        .watch_targets()
        .map_err(|e| e.to_string())?
        .into_iter()
        .any(|t| t.id == key)
        .then_some(key)
        .ok_or("監視対象が見つかりません".into())
}

pub(super) fn edit(
    library: &Library,
    request: Option<LibraryId>,
    expected: Option<i64>,
    change: WatchEdit,
) -> Result<WatchResult, String> {
    let request = request.unwrap_or_else(LibraryId::now_v7);
    let expected = expected
        .or(library
            .watch_request_expected(request)
            .map_err(|e| e.to_string())?)
        .map(Ok)
        .unwrap_or_else(|| library.watch_generation().map_err(|e| e.to_string()))?;
    library
        .edit_watch(request, expected, change)
        .map_err(|e| e.to_string())
}

pub(super) fn selected_targets(
    library: &Library,
    ids: Option<&[LibraryId]>,
) -> Result<Vec<UpdateTarget>, String> {
    let selected = ids
        .map(|ids| {
            ids.iter()
                .map(|id| target_id(library, *id))
                .collect::<Result<std::collections::HashSet<_>, _>>()
        })
        .transpose()?;
    Ok(targets(library, None, true)?
        .into_iter()
        .filter(|t| {
            selected
                .as_ref()
                .is_none_or(|ids| t.id.is_some_and(|id| ids.contains(&id)))
        })
        .collect())
}

pub(super) fn series_authors(
    library: &Library,
    source: &str,
    key: &str,
) -> Result<Vec<String>, String> {
    let Some(series) = library
        .entity_by_key(EntityKind::Series, source, key)
        .map_err(|e| e.to_string())?
    else {
        return Ok(Vec::new());
    };
    let mut query = crate::library::WorkListQuery {
        limit: 500,
        count: false,
        filter: crate::library::WorkFilter {
            series: Some(series),
            provider: Some(source.into()),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut authors = std::collections::HashSet::new();
    loop {
        let page = library.list_works(&query).map_err(|e| e.to_string())?;
        for card in page.items {
            let author = library
                .work_source_state(card.id, card.snapshot_id)
                .map_err(|e| e.to_string())?
                .author_key;
            if !author.is_empty() {
                authors.insert(author);
            }
        }
        query.after = page.next;
        if query.after.is_none() {
            break;
        }
    }
    Ok(authors.into_iter().collect())
}

pub(super) fn record(
    library: &Library,
    target: &UpdateTarget,
    seen: Option<&str>,
    updated: Option<&str>,
    found: i64,
    failed: bool,
) -> Result<(), String> {
    let Some(current) = find(
        library,
        &target.target_type,
        &target.source,
        &target.source_key,
    )?
    else {
        return Ok(());
    };
    if target.id.is_none() || target.id != current.id {
        return Ok(());
    }
    let updated = updated
        .map(|v| {
            chrono::DateTime::parse_from_rfc3339(v)
                .map(|v| v.timestamp_millis())
                .map_err(|e| e.to_string())
        })
        .transpose()?;
    if let Some(id) = current.id {
        library
            .record_watch_check(id, seen, updated, found, failed)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}
