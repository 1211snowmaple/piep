//! 実際の `ObjectStore` で、フォルダーのファイルを保管する時間を測る。読むだけで、元には書かない。
//!
//! ```text
//! cargo run --release --example store_put -- <読むフォルダー> <保管先> [件数]
//! ```

use piep_storage::ObjectStore;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        match entry.file_type() {
            Ok(kind) if kind.is_dir() => collect(&path, out),
            Ok(kind) if kind.is_file() => out.push(path),
            _ => {}
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let limit: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(2000);
    let mut files = Vec::new();
    collect(Path::new(&args[1]), &mut files);
    files.sort();
    let step = (files.len() / limit).max(1);
    std::fs::create_dir_all(&args[2]).unwrap();
    let store = ObjectStore::open(Path::new(&args[2])).unwrap();
    let started = Instant::now();
    let mut count = 0;
    let mut bytes = 0u64;
    for path in files.iter().step_by(step).take(limit) {
        if let Ok(file) = store.put_path(path) {
            count += 1;
            bytes += file.length;
        }
    }
    println!(
        "{{\"files\":{count},\"mib\":{:.1},\"seconds\":{:.2}}}",
        bytes as f64 / 1048576.0,
        started.elapsed().as_secs_f64()
    );
}
