/// `admin-dist`（`npm run build:admin` の出力）から、実行ファイルに埋め込む一覧を作る。
/// 無ければ空にする。配布物を組むときは `PIEP_REQUIRE_ADMIN_UI` で、空なら止める。
fn embed_admin_ui() {
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    fn walk(directory: &Path, found: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        let mut entries: Vec<_> = entries.flatten().map(|entry| entry.path()).collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(&path, found);
            } else {
                found.push(path);
            }
        }
    }

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest.join("admin-dist");
    // 無い道を見張ると毎回作り直しになるので、先に作っておく。
    std::fs::create_dir_all(&dist).expect("admin-dist");
    println!("cargo:rerun-if-changed=admin-dist");
    println!("cargo:rerun-if-env-changed=PIEP_REQUIRE_ADMIN_UI");
    let mut files = Vec::new();
    walk(&dist, &mut files);
    let required = std::env::var_os("PIEP_REQUIRE_ADMIN_UI").is_some();
    if required && !files.iter().any(|path| path.ends_with("index.html")) {
        panic!("admin-dist に管理画面がありません。先に npm run build:admin を実行してください");
    }
    let mut table = String::from("pub static ADMIN_ASSETS: &[AdminAsset] = &[\n");
    for path in &files {
        let relative = path
            .strip_prefix(&dist)
            .expect("under dist")
            .to_string_lossy()
            .replace('\\', "/");
        let mime = match path.extension().and_then(|extension| extension.to_str()) {
            Some("html") => "text/html; charset=utf-8",
            Some("js") => "text/javascript; charset=utf-8",
            Some("css") => "text/css; charset=utf-8",
            Some("svg") => "image/svg+xml",
            Some("png") => "image/png",
            Some("ico") => "image/x-icon",
            Some("woff2") => "font/woff2",
            _ => "application/octet-stream",
        };
        writeln!(
            table,
            "    AdminAsset {{ path: {relative:?}, mime: {mime:?}, bytes: include_bytes!({:?}) }},",
            path.display().to_string()
        )
        .expect("table");
    }
    table.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("admin_assets.rs");
    if std::fs::read_to_string(&out).ok().as_deref() != Some(table.as_str()) {
        std::fs::write(out, table).expect("admin_assets.rs");
    }
}

fn main() {
    embed_admin_ui();
    // Tauri embeds the Windows icon in the executable at build time. Watching
    // the config alone leaves `tauri dev` running an old debug executable after
    // icon.ico changes, even though a release build shows the new artwork.
    for icon in [
        "icons/icon.ico",
        "icons/32x32.png",
        "icons/128x128.png",
        "icons/128x128@2x.png",
    ] {
        println!("cargo:rerun-if-changed={icon}");
    }
    // aws-lc（tests/examples では oniguruma も）のCシンボルが最終EXEから
    // export されるため、MSVC は誰もリンクしない import library (.lib/.exp)
    // まで作る。その通常メッセージが Rust 1.97 の linker_messages lint に
    // warning として拾われる。警告を隠すのではなく、不要な成果物の生成を止める。
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg=/NOIMPLIB");
        println!("cargo:rustc-link-arg=/NOEXP");
    }
    tauri_build::build()
}
