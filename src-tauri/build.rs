fn main() {
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
