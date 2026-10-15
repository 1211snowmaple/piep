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

#[cfg(target_os = "linux")]
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

#[cfg(target_os = "macos")]
pub(crate) fn available_memory_bytes() -> Option<u64> {
    type MachPort = u32;
    type KernReturn = i32;
    type MachCount = u32;

    #[repr(C)]
    #[derive(Default)]
    struct VmStatistics64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
        swapped_count: u64,
    }

    const HOST_VM_INFO64: i32 = 4;
    #[link(name = "System", kind = "dylib")]
    extern "C" {
        fn mach_host_self() -> MachPort;
        fn host_page_size(host: MachPort, page_size: *mut u32) -> KernReturn;
        fn host_statistics64(
            host: MachPort,
            flavor: i32,
            info: *mut i32,
            count: *mut MachCount,
        ) -> KernReturn;
    }

    let host = unsafe { mach_host_self() };
    let mut page_size = 0u32;
    // SAFETY: both functions receive writable pointers to correctly sized C
    // layouts. The count is expressed in `integer_t` units, as Mach expects.
    if unsafe { host_page_size(host, &mut page_size) } != 0 || page_size == 0 {
        return None;
    }
    let mut stats = VmStatistics64::default();
    let mut count = (std::mem::size_of::<VmStatistics64>() / std::mem::size_of::<i32>()) as u32;
    if unsafe {
        host_statistics64(
            host,
            HOST_VM_INFO64,
            (&mut stats as *mut VmStatistics64).cast::<i32>(),
            &mut count,
        )
    } != 0
    {
        return None;
    }
    // XNU documents speculative pages as already included in `free_count`.
    // Keep this estimate conservative and avoid counting either speculative or
    // purgeable pages twice through another VM bucket.
    let reclaimable_pages =
        u64::from(stats.free_count).saturating_add(u64::from(stats.inactive_count));
    Some(reclaimable_pages.saturating_mul(u64::from(page_size)))
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub(crate) fn available_memory_bytes() -> Option<u64> {
    None
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
