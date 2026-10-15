//! 抽出器そのものの回帰試験。
//!
//! **境界を守っている当のコードが、いちばん検証されていなかった。** ここが
//! `#[tauri::command]` を静かに取りこぼしても、それを教えてくれるものは何も
//! 無い。ドリフト検査は「取りこぼしが無い」という前提の上に建っている。
//!
//! 小さな Rust 断片を実際に書き出して走査させる。書式（属性の書き方、引数の
//! 並び、戻り値の形）が変わったときに落ちてほしいので、実物の構文で試す。

use super::rust_scan;
use std::path::{Path, PathBuf};

/// 使い捨ての作業ディレクトリ。番号で衝突を避ける。
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "ipc-extract-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn write(&self, name: &str, body: &str) -> PathBuf {
        let path = self.0.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn only_functions_carrying_the_attribute_are_commands() {
    let dir = TempDir::new("attr");
    dir.write(
        "commands/database.rs",
        r#"
#[tauri::command]
pub async fn db_get_thing(app: tauri::AppHandle, thing_id: i64) -> Result<Thing, String> {
    Ok(Thing)
}

/// 説明のある同期コマンド。短い形の属性も拾う。
#[command]
pub fn db_count_things(state: State<'_, Arc<AppState>>) -> Result<i64, String> {
    Ok(0)
}

// 属性が無いので、これはコマンドではない。
pub async fn db_helper(thing_id: i64) -> Result<(), String> {
    Ok(())
}
"#,
    );

    let commands = rust_scan::scan_commands(&dir.path().join("commands"), dir.path()).unwrap();
    let names: Vec<_> = commands.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["db_count_things", "db_get_thing"]);
}

#[test]
fn arguments_carry_the_name_the_front_end_must_send() {
    let dir = TempDir::new("args");
    dir.write(
        "commands/database.rs",
        r#"
#[tauri::command]
pub async fn db_get_thing(
    app: tauri::AppHandle,
    state: State<'_, Arc<AppState>>,
    download_id: i64,
    source_key: String,
) -> Result<Vec<Thing>, String> {
    Ok(Vec::new())
}
"#,
    );

    let commands = rust_scan::scan_commands(&dir.path().join("commands"), dir.path()).unwrap();
    let command = &commands[0];
    // Tauri が注入するものは、フロントから渡さない。ここを取りこぼすと
    // 「引数が足りない」と誤って報告することになる。
    let sent: Vec<_> = command
        .args
        .iter()
        .filter(|arg| !arg.injected)
        .map(|arg| arg.js_name.as_str())
        .collect();
    assert_eq!(sent, vec!["downloadId", "sourceKey"]);
    assert!(command
        .args
        .iter()
        .any(|arg| arg.rust_name == "app" && arg.injected));
    assert!(command
        .args
        .iter()
        .any(|arg| arg.rust_name == "state" && arg.injected));
    assert_eq!(command.returns.ok.as_deref(), Some("Vec<Thing>"));
    assert_eq!(command.returns.err.as_deref(), Some("String"));
    assert!(command.is_async);
}

#[test]
fn a_doc_comment_becomes_the_description() {
    let dir = TempDir::new("doc");
    dir.write(
        "commands/epub.rs",
        r#"
/// 本を組み立てて書き出す。
///
/// 二段落目は説明の続き。
#[tauri::command]
pub async fn export_epub_batch(app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}

#[tauri::command]
pub async fn undocumented(app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}
"#,
    );

    let commands = rust_scan::scan_commands(&dir.path().join("commands"), dir.path()).unwrap();
    let documented = commands
        .iter()
        .find(|c| c.name == "export_epub_batch")
        .unwrap();
    assert!(documented
        .doc
        .as_deref()
        .unwrap()
        .contains("本を組み立てて"));
    let bare = commands.iter().find(|c| c.name == "undocumented").unwrap();
    assert_eq!(bare.doc, None);
}

/// `mod.rs` は再エクスポートだけなので走査しない。ここを拾うと、同じコマンドが
/// 二度数えられる。
#[test]
fn the_module_file_itself_is_skipped() {
    let dir = TempDir::new("modfile");
    dir.write(
        "commands/mod.rs",
        r#"
#[tauri::command]
pub async fn should_not_be_found(app: tauri::AppHandle) -> Result<(), String> {
    Ok(())
}
"#,
    );

    let commands = rust_scan::scan_commands(&dir.path().join("commands"), dir.path()).unwrap();
    assert!(commands.is_empty());
}

#[test]
fn every_name_listed_in_generate_handler_is_collected() {
    let dir = TempDir::new("handler");
    let lib = dir.write(
        "lib.rs",
        r#"
pub fn run() -> tauri::Result<()> {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            // 認証
            commands::auth::verify_pixiv_token,
            commands::browser::open_embedded_browser,
            commands::database::db_get_downloads,
        ])
        .run(tauri::generate_context!())
}
"#,
    );

    let registered = rust_scan::scan_registered(&lib).unwrap();
    assert_eq!(
        registered,
        vec![
            "db_get_downloads",
            "open_embedded_browser",
            "verify_pixiv_token"
        ]
    );
}

#[test]
fn emitted_event_names_are_collected_with_their_location() {
    let dir = TempDir::new("events");
    dir.write(
        "src/commands/epub.rs",
        r#"
fn report(app: &tauri::AppHandle) {
    let _ = app.emit("epub-export-progress", 1);
    let _ = app.emit(
        "search-index-progress",
        2,
    );
}
"#,
    );

    let events = rust_scan::scan_events(&dir.path().join("src"), dir.path()).unwrap();
    let names: Vec<_> = events.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"epub-export-progress"), "{names:?}");
    assert!(names.contains(&"search-index-progress"), "{names:?}");
    assert!(events.iter().all(|event| event.line > 0));
}
