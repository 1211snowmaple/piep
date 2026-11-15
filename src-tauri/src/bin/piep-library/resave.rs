//! 旧棚の各作品の今の版を、移した棚へアプリの保存（`save_work`）でそのまま保存し直す。
//! 変わっていない作品で版が増えないことを確かめる（更新の確認は変わっていない作品を何度も
//! 保存し直す）。新しい棚は書き換わるので、捨ててよい棚に対して使う。

use piep_lib::library::{LegacyImporter, Library, LibraryId, WorkSave};
use serde_json::{json, Value};
use std::path::Path;
use std::time::Instant;

type Error = Box<dyn std::error::Error>;

pub fn run(old: &Path, new: &Path) -> Result<Value, Error> {
    let source = LegacyImporter::open(old)?;
    let library = Library::open(new)?;
    let started = Instant::now();
    let (mut same, mut revised, mut failed) = (0usize, Vec::new(), Vec::new());
    let mut slowest = 0f64;
    source.current_save_inputs(|provider, key, input| {
        let label = format!("{provider}:{key}");
        let (data, assets) = match input {
            Ok(input) => input,
            Err(error) => {
                failed.push(json!([label, format!("旧棚を読めません: {error}")]));
                return Ok(());
            }
        };
        let t = Instant::now();
        let saved = library.save_work(WorkSave {
            request: LibraryId::now_v7(),
            provider,
            key,
            route: "resave-check",
            data: &data,
            assets,
        });
        slowest = slowest.max(t.elapsed().as_secs_f64() * 1000.0);
        match saved {
            Ok(saved) if saved.created_revision => revised.push(json!(label)),
            Ok(_) => same += 1,
            Err(error) => failed.push(json!([label, error.to_string()])),
        }
        Ok(())
    })?;
    let seconds = started.elapsed().as_secs_f64();
    library.verify()?;
    Ok(json!({
        "same": same,
        "revised": revised.len(),
        "revised_works": revised,
        "failed": failed,
        "seconds": seconds,
        "slowest_ms": slowest,
    }))
}
