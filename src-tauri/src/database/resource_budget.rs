const MIB: u64 = 1024 * 1024;

/// A single application-wide memory envelope shared by SQLite, Tantivy and
/// in-process caches. `PIEP_MEMORY_BUDGET_MB` is intentionally supported for
/// constrained machines and deterministic performance tests.
pub fn application_memory_budget_bytes() -> u64 {
    if let Some(configured) = std::env::var("PIEP_MEMORY_BUDGET_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
    {
        return configured.clamp(256, 8 * 1024).saturating_mul(MIB);
    }
    available_memory_bytes()
        .unwrap_or(1024 * MIB)
        .saturating_div(2)
        .clamp(256 * MIB, 2 * 1024 * MIB)
}

pub fn sqlite_cache_bytes() -> u64 {
    // `cache_size` is per connection. The read pool can open up to sixteen
    // connections in addition to the writer, so assigning the old app-wide
    // 1/16 share to every connection could consume the whole envelope on the
    // read caches alone. Keep the aggregate below roughly one quarter.
    application_memory_budget_bytes()
        .saturating_div(64)
        .clamp(4 * MIB, 16 * MIB)
}

pub fn sqlite_mmap_bytes() -> u64 {
    application_memory_budget_bytes()
        .saturating_div(8)
        .clamp(64 * MIB, 256 * MIB)
}

pub fn tantivy_writer_bytes() -> usize {
    application_memory_budget_bytes()
        .saturating_div(4)
        .clamp(64 * MIB, 384 * MIB) as usize
}

pub fn semantic_ann_bytes() -> u64 {
    application_memory_budget_bytes()
        .saturating_div(2)
        .clamp(128 * MIB, 1024 * MIB)
}

/// Disk quota shared by ephemeral search snapshots. These files replace
/// million-element in-memory vectors and are safe to discard at any time.
pub fn search_snapshot_disk_bytes() -> u64 {
    std::env::var("PIEP_SEARCH_SNAPSHOT_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(512)
        .clamp(64, 4 * 1024)
        .saturating_mul(MIB)
}

/// SQLite page cache per snapshot connection. Snapshot rows are streamed and
/// indexed on disk, so a small cache is enough even for multi-million hits.
pub fn search_snapshot_cache_bytes() -> u64 {
    application_memory_budget_bytes()
        .saturating_div(64)
        .clamp(4 * MIB, 16 * MIB)
}

#[cfg(windows)]
pub(crate) fn available_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }
    let mut status = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: `status` is initialized and writable and Win32 receives its
    // exact structure size in the first field.
    let success = unsafe { GlobalMemoryStatusEx(&mut status) };
    (success != 0).then_some(status.avail_phys)
}

#[cfg(not(windows))]
pub(crate) fn available_memory_bytes() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    let kib = text
        .lines()
        .find_map(|line| line.strip_prefix("MemAvailable:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_budgets_are_bounded_by_application_envelope() {
        let total = application_memory_budget_bytes();
        assert!(sqlite_cache_bytes() <= total);
        assert!(sqlite_cache_bytes().saturating_mul(17) <= total);
        assert!(sqlite_mmap_bytes() <= total);
        assert!(tantivy_writer_bytes() as u64 <= total);
        assert!(semantic_ann_bytes() <= total);
        assert!(search_snapshot_cache_bytes() <= total);
        assert!(search_snapshot_disk_bytes() >= 64 * MIB);
    }
}
