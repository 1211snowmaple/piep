//! 保存した検索を、切り替えた棚と旧 DB のどちらでも同じ形で返す。
//!
//! 画面は ID を文字列として扱う。棚では検索の UUID、旧 DB では番号の文字列になる。
//! 棚の持ち主は棚そのもの（`library.id`）で、書き込みは要求 ID と世代で重複なく一度だけ効く。

use crate::database::{SavedSearch as LegacySavedSearch, SavedSearchInput as LegacyInput};
use crate::library::{Library, LibraryId, SavedSearch, SavedSearchDefinition, SavedSearchUpdate};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearchView {
    pub id: String,
    pub name: String,
    pub query: Option<String>,
    pub params_json: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedSearchViewInput {
    pub id: Option<String>,
    pub name: String,
    pub query: Option<String>,
    pub params_json: String,
}

fn time(ms: i64) -> Result<String, String> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.to_rfc3339())
        .ok_or_else(|| "保存した検索の日時が不正です".to_string())
}

fn view(search: SavedSearch) -> Result<SavedSearchView, String> {
    Ok(SavedSearchView {
        id: search.id.to_string(),
        name: search.definition.name,
        query: search.definition.query,
        params_json: search.definition.params_json,
        created_at: time(search.created_at)?,
        updated_at: time(search.updated_at)?,
    })
}

pub(super) fn legacy_view(search: LegacySavedSearch) -> SavedSearchView {
    SavedSearchView {
        id: search.id.to_string(),
        name: search.name,
        query: search.query,
        params_json: search.params_json,
        created_at: search.created_at,
        updated_at: search.updated_at,
    }
}

pub(super) fn legacy_input(input: SavedSearchViewInput) -> Result<LegacyInput, String> {
    let id = input
        .id
        .map(|id| {
            id.parse::<i64>()
                .map_err(|_| "保存した検索の ID が不正です".to_string())
        })
        .transpose()?;
    Ok(LegacyInput {
        id,
        name: input.name,
        query: input.query,
        params_json: input.params_json,
    })
}

fn parse_id(id: &str) -> Result<LibraryId, String> {
    id.parse::<LibraryId>()
        .map_err(|_| "保存した検索の ID が不正です".to_string())
}

pub(super) fn list(library: &Library) -> Result<Vec<SavedSearchView>, String> {
    library
        .saved_searches(library.id)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(view)
        .collect()
}

/// 検索を保存する。ID が無ければ同じ名前の検索を上書きし、それも無ければ新しく作る。
pub(super) fn upsert(
    library: &Library,
    input: SavedSearchViewInput,
) -> Result<SavedSearchView, String> {
    let user = library.id;
    let name = input.name.trim().to_string();
    if name.is_empty() {
        return Err("保存する検索の名前を入力してください".into());
    }
    if name.chars().count() > 80 {
        return Err("名前は80文字以内にしてください".into());
    }
    if input.params_json.len() > 16 * 1024 {
        return Err("検索条件が大きすぎます".into());
    }
    if !serde_json::from_str::<serde_json::Value>(&input.params_json).is_ok_and(|v| v.is_object()) {
        return Err("検索条件を保存できませんでした".into());
    }
    let existing = match input.id.as_deref() {
        Some(id) => {
            let search = library
                .saved_search(user, parse_id(id)?)
                .map_err(|e| e.to_string())?;
            if search.deleted_at.is_some() {
                return Err("保存した検索は削除されています".into());
            }
            Some(search)
        }
        None => library
            .saved_searches(user)
            .map_err(|e| e.to_string())?
            .into_iter()
            .find(|s| s.definition.name == name),
    };
    let (id, expected_generation) = existing
        .map(|s| (s.id, s.generation))
        .unwrap_or_else(|| (LibraryId::now_v7(), 0));
    let saved = library
        .update_saved_search(SavedSearchUpdate {
            request_id: LibraryId::now_v7(),
            id,
            user,
            expected_generation,
            definition: Some(SavedSearchDefinition {
                name,
                // 旧 DB と同じく、長すぎる検索語は切り詰めて保存する。
                query: input.query.map(|q| q.chars().take(512).collect()),
                params_json: input.params_json,
            }),
        })
        .map_err(|e| e.to_string())?;
    view(saved)
}

/// 保存した検索を消す。`false` は「その検索が無かった（もう消えていた）」を意味する。
pub(super) fn delete(library: &Library, id: &str) -> Result<bool, String> {
    let user = library.id;
    let Ok(id) = id.parse::<LibraryId>() else {
        return Ok(false);
    };
    let search = match library.saved_search(user, id) {
        Ok(search) if search.deleted_at.is_none() => search,
        _ => return Ok(false),
    };
    library
        .update_saved_search(SavedSearchUpdate {
            request_id: LibraryId::now_v7(),
            id,
            user,
            expected_generation: search.generation,
            definition: None,
        })
        .map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            assert_eq!(self.0.parent(), Some(std::env::temp_dir().as_path()));
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn shelf() -> (Temp, Library) {
        let root =
            std::env::temp_dir().join(format!("piep-saved-search-shelf-{}", LibraryId::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let library = Library::create(&root.join("shelf")).unwrap();
        (Temp(root), library)
    }

    fn input(id: Option<&str>, name: &str, query: &str) -> SavedSearchViewInput {
        SavedSearchViewInput {
            id: id.map(str::to_string),
            name: name.into(),
            query: Some(query.into()),
            params_json: "{\"tab\":\"works\"}".into(),
        }
    }

    #[test]
    fn saves_overwrites_by_name_or_id_and_deletes_on_the_shelf() {
        let (_temp, library) = shelf();
        let first = upsert(&library, input(None, " 甲 ", "一")).unwrap();
        assert_eq!(first.name, "甲");
        assert!(first.id.parse::<LibraryId>().is_ok());
        let second = upsert(&library, input(None, "乙", "二")).unwrap();
        // 同じ名前は上書きし、ID と並びを保つ。
        let renamed = upsert(&library, input(None, "甲", "三")).unwrap();
        assert_eq!(renamed.id, first.id);
        assert_eq!(renamed.query.as_deref(), Some("三"));
        assert_eq!(renamed.created_at, first.created_at);
        // ID を指せば名前ごと変えられる。
        let moved = upsert(&library, input(Some(&first.id), "丙", "四")).unwrap();
        assert_eq!(moved.id, first.id);
        let names: Vec<_> = list(&library)
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(names, ["丙", "乙"]);

        assert!(delete(&library, &second.id).unwrap());
        assert!(!delete(&library, &second.id).unwrap());
        assert!(!delete(&library, "12").unwrap());
        assert!(!delete(&library, &LibraryId::now_v7().to_string()).unwrap());
        assert_eq!(list(&library).unwrap().len(), 1);
        // 消した検索は ID で上書きできず、同じ名前で保存すれば新しい検索になる。
        assert!(upsert(&library, input(Some(&second.id), "乙", "五")).is_err());
        let again = upsert(&library, input(None, "乙", "五")).unwrap();
        assert_ne!(again.id, second.id);
        assert!(upsert(&library, input(Some("7"), "丁", "六")).is_err());
    }

    #[test]
    fn legacy_ids_cross_as_number_strings() {
        let view = legacy_view(LegacySavedSearch {
            id: 42,
            name: "甲".into(),
            query: None,
            params_json: "{}".into(),
            created_at: "2026-11-19T00:00:00+00:00".into(),
            updated_at: "2026-11-19T00:00:00+00:00".into(),
        });
        assert_eq!(view.id, "42");
        assert_eq!(
            legacy_input(input(Some("42"), "甲", "")).unwrap().id,
            Some(42)
        );
        assert_eq!(legacy_input(input(None, "甲", "")).unwrap().id, None);
        assert!(legacy_input(input(Some("abc"), "甲", "")).is_err());
    }
}
