//! 実体を一つ保管するときの時間が、どの段で使われているかを測る。
//!
//! `ObjectStore::put_chunk` と同じ順で同じ操作をし、段ごとに時間を足す。実際の棚の
//! ファイルを読むだけで、棚には何も書かない。保管先は引数で渡す空のフォルダー。
//!
//! ```text
//! cargo run --release --example put_cost -- <読むフォルダー> <保管先> [件数] [sync|nosync|batch|samehandle]
//! ```
//!
//! `nosync` は確定（ファイルと親フォルダーの flush）を省いたときの下限、`batch` は
//! ファイルごとの確定をやめて最後にまとめて確定する形を測る。

use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Cost {
    read: Duration,
    hash: Duration,
    probe: Duration,
    compress: Duration,
    write: Duration,
    sync_file: Duration,
    read_back: Duration,
    publish: Duration,
    sync_dir: Duration,
    final_sync: Duration,
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
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

fn sync_directory(path: &Path) {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt;
        OpenOptions::new()
            .read(true)
            .custom_flags(0x02000000)
            .open(path)
    };
    #[cfg(not(windows))]
    let file = File::open(path);
    if let Ok(file) = file {
        let _ = file.sync_all();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let source = PathBuf::from(&args[1]);
    let target = PathBuf::from(&args[2]);
    let limit: usize = args.get(3).and_then(|n| n.parse().ok()).unwrap_or(2000);
    let mode = args.get(4).map(String::as_str).unwrap_or("sync");
    let mut files = Vec::new();
    collect(&source, &mut files);
    files.sort();
    let step = (files.len() / limit).max(1);
    let sample: Vec<&PathBuf> = files.iter().step_by(step).take(limit).collect();
    fs::create_dir_all(&target).unwrap();

    let mut cost = Cost::default();
    let mut bytes = 0u64;
    let mut stored = 0u64;
    let mut written = Vec::new();
    let started = Instant::now();
    for (index, path) in sample.iter().enumerate() {
        let t = Instant::now();
        let mut original = Vec::new();
        if File::open(path)
            .and_then(|mut f| f.read_to_end(&mut original))
            .is_err()
        {
            continue;
        }
        cost.read += t.elapsed();
        bytes += original.len() as u64;

        let t = Instant::now();
        let id = Sha256::digest(&original);
        cost.hash += t.elapsed();
        let hex: String = id.iter().map(|b| format!("{b:02x}")).collect();
        let dir = target.join(&hex[..2]);
        let dest = dir.join(format!("{hex}-{index}"));

        let t = Instant::now();
        fs::create_dir_all(&dir).unwrap();
        let _ = fs::metadata(&dest);
        cost.probe += t.elapsed();

        let t = Instant::now();
        let mut encoder = zstd::stream::Encoder::new(Vec::new(), 3).unwrap();
        encoder.include_checksum(true).unwrap();
        encoder.write_all(&original).unwrap();
        let compressed = encoder.finish().unwrap();
        cost.compress += t.elapsed();
        let payload = if compressed.len() + 1024.max(original.len().div_ceil(100)) <= original.len()
        {
            &compressed
        } else {
            &original
        };
        stored += payload.len() as u64;

        let temporary = dest.with_extension("part");
        let t = Instant::now();
        let mut file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(&temporary)
            .unwrap();
        file.write_all(&[0u8; 57]).unwrap();
        file.write_all(payload).unwrap();
        cost.write += t.elapsed();
        if mode == "sync" || mode == "samehandle" {
            let t = Instant::now();
            file.sync_all().unwrap();
            cost.sync_file += t.elapsed();
        }
        let t = Instant::now();
        let mut back = Vec::new();
        if mode == "samehandle" {
            // 開き直さず、書いた手でそのまま読み戻す。
            use std::io::Seek;
            file.rewind().unwrap();
            file.read_to_end(&mut back).unwrap();
            drop(file);
        } else {
            drop(file);
            File::open(&temporary)
                .unwrap()
                .read_to_end(&mut back)
                .unwrap();
        }
        if payload.len() != original.len() {
            let decoded = zstd::stream::decode_all(&back[57..]).unwrap();
            assert_eq!(decoded.len(), original.len());
        }
        let _ = Sha256::digest(&original);
        cost.read_back += t.elapsed();

        let t = Instant::now();
        fs::rename(&temporary, &dest).unwrap();
        cost.publish += t.elapsed();
        if mode == "sync" || mode == "samehandle" {
            let t = Instant::now();
            sync_directory(&dir);
            cost.sync_dir += t.elapsed();
        }
        written.push(dest);
    }
    if mode == "batch" {
        // ファイルごとには待たず、最後にまとめて確定する。
        let t = Instant::now();
        for path in &written {
            OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .sync_all()
                .unwrap();
        }
        for byte in 0..=255u8 {
            let dir = target.join(format!("{byte:02x}"));
            if dir.is_dir() {
                sync_directory(&dir);
            }
        }
        cost.final_sync = t.elapsed();
    }
    let total = started.elapsed();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    println!(
        "{{\"mode\":\"{mode}\",\"files\":{},\"mib\":{:.1},\"stored_mib\":{:.1},\"total_s\":{:.2},\"read_ms\":{:.0},\"hash_ms\":{:.0},\"probe_ms\":{:.0},\"compress_ms\":{:.0},\"write_ms\":{:.0},\"sync_file_ms\":{:.0},\"read_back_ms\":{:.0},\"publish_ms\":{:.0},\"sync_dir_ms\":{:.0},\"final_sync_ms\":{:.0}}}",
        written.len(),
        bytes as f64 / 1048576.0,
        stored as f64 / 1048576.0,
        total.as_secs_f64(),
        ms(cost.read),
        ms(cost.hash),
        ms(cost.probe),
        ms(cost.compress),
        ms(cost.write),
        ms(cost.sync_file),
        ms(cost.read_back),
        ms(cost.publish),
        ms(cost.sync_dir),
        ms(cost.final_sync),
    );
}
