//! 詳細設定の命令（設定 › 詳細設定）。値と範囲は [`crate::advanced_settings`] が持つ。

use crate::advanced_settings::{self, SettingSpec};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdvancedSettings {
    /// 項目の名前・単位・既定・範囲。画面はこれを見て入力欄を作る。
    specs: Vec<SettingSpec>,
    /// 今の値。
    values: BTreeMap<&'static str, i64>,
}

fn snapshot(values: advanced_settings::Values) -> AdvancedSettings {
    AdvancedSettings {
        specs: advanced_settings::specs().to_vec(),
        values: values.to_map(),
    }
}

/// 詳細設定の項目と今の値を返す。
#[tauri::command]
pub fn settings_advanced_get() -> AdvancedSettings {
    snapshot(advanced_settings::current())
}

/// 詳細設定を丸ごと置き換える。書いていない項目は既定に戻る。
///
/// # Errors
/// 知らない名前や範囲の外の値があれば、何も変えずに断る。ファイルに書けなかったときも。
#[tauri::command]
pub fn settings_advanced_save(values: BTreeMap<String, i64>) -> Result<AdvancedSettings, String> {
    advanced_settings::save(&values).map(snapshot)
}
