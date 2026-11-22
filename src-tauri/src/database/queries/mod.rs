//! 棚が使う、作品の形の解釈と提案の計算。
//!
//! 保存した応答の解釈（本文・資産・改稿の情報）と、コレクションの提案の計算（棚から組んだ
//! メモリ上の表を読む）。

use regex::Regex;
use rusqlite::{params, Connection, OptionalExtension};
use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::collection_rules;
use super::models::*;
// 本文を組み立てるエスケープは、取り込み側と同じものを使う。同じ規則を二か所に
// 書いていたので、片方だけ直すとリーダーとエディタで表示が食い違う余地があった。
use super::search::{
    generate_ngrams_limited, normalize_search_text, normalized_levenshtein, query_ngrams,
};

#[cfg(test)]
use piep_reader_render::blocks::{block_gap, block_image_reference};
pub(crate) use piep_reader_render::blocks::{
    blocks_to_fanbox_blocks, blocks_to_pixiv_text, blocks_to_plain_text, BLOCK_GAP_KEY,
    BLOCK_MISSING_KEY, BLOCK_SOURCE_REF_KEY, EDITOR_IMAGE_ASSET_TYPE, MAX_BLOCK_GAP,
};
mod acquisition_audit;
mod assist_inputs;
/// 作品コレクションは `Database` の非公開内部へ触れるため、子モジュールに置く。
mod reader;
pub(crate) use piep_reader_render::pages::{reader_outline, search_reader_pages};
mod work_inspection;
pub(crate) use acquisition_audit::{
    masked_metadata_label, masked_pixiv_metadata, pixiv_missing_references_for_filenames,
};
pub use assist_inputs::{AiNote, TaggedName};
pub(crate) use work_inspection::revision_metadata;
pub(crate) use work_inspection::RevisionAsset;
pub use work_inspection::RevisionMetadata;
mod collection_additions;
mod collection_sweep;
mod collections;
mod suggestion_source;
mod tag_recommendations;
mod work_link_evidence;
pub(crate) use collection_additions::collection_additions;
pub(crate) use collection_sweep::{sweep_identity, sweep_plan, PreparedSweep};
pub(crate) use collections::{
    accepted_suggestion_name_source, name_proposals_in, naming_works_in, sample_bundle_like_ids_in,
    seed_suggestion_plan,
};
pub(crate) use suggestion_source::{Corpus, SuggestionSource};
pub use tag_recommendations::RelatedTagCandidate;

const DIAGNOSTIC_SCAN_MAX_DEPTH: usize = 64;
const DIAGNOSTIC_SCAN_MAX_ENTRIES: usize = 5_000_000;
const DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT: usize = 100;

fn bounded_diagnostic_path(path: &str) -> String {
    const MAX_CHARS: usize = 2_048;
    if path.chars().count() <= MAX_CHARS {
        return path.to_string();
    }
    let mut bounded = path.chars().take(MAX_CHARS).collect::<String>();
    bounded.push('…');
    bounded
}

#[derive(Debug, Clone, Copy)]
struct DiagnosticScanLimits {
    max_depth: usize,
    max_entries: usize,
}

impl Default for DiagnosticScanLimits {
    fn default() -> Self {
        Self {
            max_depth: DIAGNOSTIC_SCAN_MAX_DEPTH,
            max_entries: DIAGNOSTIC_SCAN_MAX_ENTRIES,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct DiagnosticFileStats {
    storage_size_bytes: u64,
    storage_unique_file_bytes: Option<u64>,
    lexical_index_size_bytes: u64,
    lexical_index_file_count: u64,
    lexical_index_segment_count: u64,
    semantic_index_size_bytes: u64,
    orphan_asset_files: u64,
    orphan_asset_file_bytes: u64,
    unregistered_asset_samples: Vec<LibraryFileIssue>,
    transient_files: u64,
    transient_file_bytes: u64,
    transient_file_samples: Vec<LibraryFileIssue>,
    visited_entries: usize,
}

#[cfg(windows)]
fn diagnostic_file_identity(path: &Path, _metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::windows::{fs::OpenOptionsExt, io::AsRawHandle};
    use windows::Win32::{
        Foundation::HANDLE,
        Storage::FileSystem::{
            GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_READ_ATTRIBUTES,
        },
    };
    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES.0)
        .share_mode(1 | 2 | 4)
        .open(path)
        .ok()?;
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // The std file owns the handle for the whole query, including error paths.
    unsafe {
        GetFileInformationByHandle(HANDLE(file.as_raw_handle()), &mut info).ok()?;
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    (index != 0).then_some((u64::from(info.dwVolumeSerialNumber), index))
}

#[cfg(unix)]
fn diagnostic_file_identity(_path: &Path, metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    Some((metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn diagnostic_file_identity(_path: &Path, _metadata: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

fn diagnostic_path_is_transient(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        name.ends_with(".part")
            || (name.starts_with('.') && name.contains(".stage"))
            || name.starts_with(".piep-export-")
    })
}

fn diagnostic_metadata_is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    false
}

fn diagnostic_scan_roots(storage: &Path, semantic: &Path) -> Vec<PathBuf> {
    if semantic.starts_with(storage) {
        vec![storage.to_path_buf()]
    } else if storage.starts_with(semantic) {
        vec![semantic.to_path_buf()]
    } else {
        vec![storage.to_path_buf(), semantic.to_path_buf()]
    }
}

fn collect_diagnostic_file_stats_with_limits(
    storage_root: &Path,
    semantic_root: &Path,
    is_known_asset: &mut dyn FnMut(&Path) -> Result<bool, String>,
    limits: DiagnosticScanLimits,
) -> Result<DiagnosticFileStats, String> {
    if limits.max_entries == 0 {
        return Err("Diagnostic scan entry limit must be positive".to_string());
    }
    let lexical_root = storage_root.join("search-index");
    let mut stats = DiagnosticFileStats {
        storage_unique_file_bytes: Some(0),
        ..Default::default()
    };
    let mut visited_directories = HashSet::new();
    let mut physical_files = HashSet::new();

    for root in diagnostic_scan_roots(storage_root, semantic_root) {
        let root_metadata = match std::fs::symlink_metadata(&root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => continue,
        };
        if diagnostic_metadata_is_link(&root_metadata) || !root_metadata.file_type().is_dir() {
            continue;
        }
        let canonical_root = match root.canonicalize() {
            Ok(path) => path,
            Err(_) => continue,
        };
        let mut pending = vec![(root, 0usize)];

        while let Some((directory, depth)) = pending.pop() {
            let metadata = match std::fs::symlink_metadata(&directory) {
                Ok(metadata) => metadata,
                Err(_) => continue,
            };
            if diagnostic_metadata_is_link(&metadata) || !metadata.file_type().is_dir() {
                continue;
            }
            let canonical_directory = match directory.canonicalize() {
                Ok(path) if path.starts_with(&canonical_root) => path,
                _ => continue,
            };
            if !visited_directories.insert(canonical_directory) {
                continue;
            }
            let entries = match std::fs::read_dir(&directory) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.filter_map(Result::ok) {
                stats.visited_entries = stats.visited_entries.saturating_add(1);
                if stats.visited_entries > limits.max_entries {
                    return Err(format!(
                        "Diagnostic scan exceeded the {}-entry safety limit",
                        limits.max_entries
                    ));
                }
                let path = entry.path();
                let metadata = match std::fs::symlink_metadata(&path) {
                    Ok(metadata) => metadata,
                    Err(_) => continue,
                };
                if diagnostic_metadata_is_link(&metadata) {
                    continue;
                }
                if metadata.file_type().is_dir() {
                    if depth >= limits.max_depth {
                        return Err(format!(
                            "Diagnostic scan exceeded the depth-{} safety limit",
                            limits.max_depth
                        ));
                    }
                    pending.push((path, depth + 1));
                    continue;
                }
                if !metadata.file_type().is_file() {
                    continue;
                }

                let bytes = metadata.len();
                let in_storage = path.starts_with(storage_root);
                if in_storage {
                    stats.storage_size_bytes = stats.storage_size_bytes.saturating_add(bytes);
                    match diagnostic_file_identity(&path, &metadata) {
                        Some(identity) if physical_files.insert(identity) => {
                            stats.storage_unique_file_bytes = stats
                                .storage_unique_file_bytes
                                .map(|total| total.saturating_add(bytes));
                        }
                        None => stats.storage_unique_file_bytes = None,
                        _ => {}
                    }
                    if diagnostic_path_is_transient(&path) {
                        stats.transient_files = stats.transient_files.saturating_add(1);
                        stats.transient_file_bytes =
                            stats.transient_file_bytes.saturating_add(bytes);
                        if stats.transient_file_samples.len() < DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT {
                            stats.transient_file_samples.push(LibraryFileIssue {
                                issue_type: "transient".to_string(),
                                category: "transient".to_string(),
                                path: bounded_diagnostic_path(path.to_string_lossy().as_ref()),
                                label: None,
                                expected_size_bytes: None,
                                actual_size_bytes: Some(bytes),
                            });
                        }
                    }
                }
                if path.starts_with(&lexical_root) {
                    stats.lexical_index_size_bytes =
                        stats.lexical_index_size_bytes.saturating_add(bytes);
                    stats.lexical_index_file_count =
                        stats.lexical_index_file_count.saturating_add(1);
                    if path.extension().and_then(|extension| extension.to_str()) == Some("store") {
                        stats.lexical_index_segment_count =
                            stats.lexical_index_segment_count.saturating_add(1);
                    }
                }
                if path.starts_with(semantic_root) {
                    stats.semantic_index_size_bytes =
                        stats.semantic_index_size_bytes.saturating_add(bytes);
                }
                if in_storage
                    && path.components().any(|part| {
                        part.as_os_str()
                            .to_string_lossy()
                            .eq_ignore_ascii_case("data_assets")
                    })
                    && !diagnostic_path_is_transient(&path)
                    && !is_known_asset(&path)?
                {
                    stats.orphan_asset_files = stats.orphan_asset_files.saturating_add(1);
                    stats.orphan_asset_file_bytes =
                        stats.orphan_asset_file_bytes.saturating_add(bytes);
                    if stats.unregistered_asset_samples.len() < DIAGNOSTIC_FILE_ISSUE_SAMPLE_LIMIT {
                        stats.unregistered_asset_samples.push(LibraryFileIssue {
                            issue_type: "unregistered".to_string(),
                            category: "work_asset".to_string(),
                            path: bounded_diagnostic_path(path.to_string_lossy().as_ref()),
                            label: None,
                            expected_size_bytes: None,
                            actual_size_bytes: Some(bytes),
                        });
                    }
                }
            }
        }
    }
    Ok(stats)
}

fn collect_diagnostic_file_stats(
    storage_root: &Path,
    semantic_root: &Path,
    is_known_asset: &mut dyn FnMut(&Path) -> Result<bool, String>,
) -> Result<DiagnosticFileStats, String> {
    collect_diagnostic_file_stats_with_limits(
        storage_root,
        semantic_root,
        is_known_asset,
        DiagnosticScanLimits::default(),
    )
}

pub(crate) fn recursive_file_size(root: &Path) -> u64 {
    let mut no_known_assets = |_path: &Path| Ok(false);
    collect_diagnostic_file_stats(
        root,
        &root.join(".piep-no-semantic-index"),
        &mut no_known_assets,
    )
    .map(|stats| stats.storage_size_bytes)
    .unwrap_or(0)
}

#[cfg(windows)]
pub(crate) fn available_space_bytes(path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetDiskFreeSpaceExW(
            directory_name: *const u16,
            free_bytes_available: *mut u64,
            total_bytes: *mut u64,
            total_free_bytes: *mut u64,
        ) -> i32;
    }
    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut available = 0u64;
    // SAFETY: `wide` is a NUL-terminated immutable UTF-16 path and the output
    // pointer is valid for a u64 for the duration of the call.
    let success = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    (success != 0).then_some(available)
}

#[cfg(unix)]
pub(crate) fn available_space_bytes(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    #[repr(C)]
    struct Statvfs {
        f_bsize: u64,
        f_frsize: u64,
        f_blocks: u64,
        f_bfree: u64,
        f_bavail: u64,
        f_files: u64,
        f_ffree: u64,
        f_favail: u64,
        f_fsid: u64,
        f_flag: u64,
        f_namemax: u64,
        __spare: [i32; 6],
    }
    extern "C" {
        fn statvfs(path: *const std::ffi::c_char, output: *mut Statvfs) -> i32;
    }
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut output = std::mem::MaybeUninit::<Statvfs>::uninit();
    // SAFETY: the C string is NUL terminated and statvfs initializes the
    // output struct when it returns zero.
    let success = unsafe { statvfs(path.as_ptr(), output.as_mut_ptr()) };
    if success != 0 {
        return None;
    }
    let output = unsafe { output.assume_init() };
    Some(output.f_bavail.saturating_mul(output.f_frsize))
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn available_space_bytes(_path: &Path) -> Option<u64> {
    None
}

#[cfg(windows)]
fn current_process_memory_bytes() -> Option<u64> {
    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentProcess() -> *mut std::ffi::c_void;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }
    let mut counters = ProcessMemoryCountersEx {
        cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
        private_usage: 0,
    };
    // SAFETY: both functions are stable Win32 APIs. `counters` is initialized,
    // writable and its exact byte size is passed to the operating system.
    let success = unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            &mut counters,
            std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
        )
    };
    (success != 0).then_some(counters.working_set_size as u64)
}

#[cfg(target_os = "linux")]
fn current_process_memory_bytes() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let kib = status
        .lines()
        .find_map(|line| line.strip_prefix("VmRSS:"))?
        .split_whitespace()
        .next()?
        .parse::<u64>()
        .ok()?;
    Some(kib.saturating_mul(1024))
}

#[cfg(target_os = "macos")]
fn current_process_memory_bytes() -> Option<u64> {
    type MachPort = u32;
    type KernReturn = i32;
    type MachCount = u32;

    #[repr(C)]
    #[derive(Default)]
    struct TimeValue {
        seconds: i32,
        microseconds: i32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct MachTaskBasicInfo {
        virtual_size: u64,
        resident_size: u64,
        resident_size_max: u64,
        user_time: TimeValue,
        system_time: TimeValue,
        policy: i32,
        suspend_count: i32,
    }

    const MACH_TASK_BASIC_INFO: i32 = 20;
    #[link(name = "System", kind = "dylib")]
    extern "C" {
        static mach_task_self_: MachPort;
        fn task_info(
            target_task: MachPort,
            flavor: i32,
            task_info_out: *mut i32,
            task_info_count: *mut MachCount,
        ) -> KernReturn;
    }

    let mut info = MachTaskBasicInfo::default();
    // Mach reports the count in 32-bit `natural_t` units rather than bytes.
    let mut count =
        (std::mem::size_of::<MachTaskBasicInfo>() / std::mem::size_of::<i32>()) as MachCount;
    // SAFETY: `mach_task_self_` is the task send right exported by libSystem;
    // `info` is a writable C-layout buffer and `count` describes its size.
    let success = unsafe {
        task_info(
            mach_task_self_,
            MACH_TASK_BASIC_INFO,
            (&mut info as *mut MachTaskBasicInfo).cast::<i32>(),
            &mut count,
        )
    };
    (success == 0).then_some(info.resident_size)
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn current_process_memory_bytes() -> Option<u64> {
    None
}

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct ProcessResourceSnapshot {
    pub(crate) working_set_bytes: Option<u64>,
    pub(crate) private_bytes: Option<u64>,
    pub(crate) gpu_dedicated_bytes: Option<u64>,
    pub(crate) gpu_shared_bytes: Option<u64>,
}

#[cfg(windows)]
pub(crate) fn process_resource_snapshot() -> ProcessResourceSnapshot {
    use std::collections::HashSet;

    #[repr(C)]
    struct ProcessEntry32W {
        size: u32,
        usage: u32,
        process_id: u32,
        default_heap_id: usize,
        module_id: u32,
        threads: u32,
        parent_process_id: u32,
        priority_class_base: i32,
        flags: u32,
        executable: [u16; 260],
    }

    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    const TH32CS_SNAPPROCESS: u32 = 0x0000_0002;
    const PROCESS_QUERY_INFORMATION: u32 = 0x0400;
    const PROCESS_VM_READ: u32 = 0x0010;
    const INVALID_HANDLE_VALUE: isize = -1;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, process_id: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry32W) -> i32;
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut std::ffi::c_void;
        fn CloseHandle(handle: isize) -> i32;
    }
    #[link(name = "psapi")]
    extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut std::ffi::c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }

    #[derive(Clone)]
    struct ProcessInfo {
        id: u32,
        parent_id: u32,
    }

    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return ProcessResourceSnapshot {
            working_set_bytes: current_process_memory_bytes(),
            ..Default::default()
        };
    }

    let mut entry = ProcessEntry32W {
        size: std::mem::size_of::<ProcessEntry32W>() as u32,
        usage: 0,
        process_id: 0,
        default_heap_id: 0,
        module_id: 0,
        threads: 0,
        parent_process_id: 0,
        priority_class_base: 0,
        flags: 0,
        executable: [0; 260],
    };
    let mut processes = Vec::new();
    let mut available = unsafe { Process32FirstW(snapshot, &mut entry) } != 0;
    while available {
        processes.push(ProcessInfo {
            id: entry.process_id,
            parent_id: entry.parent_process_id,
        });
        entry.size = std::mem::size_of::<ProcessEntry32W>() as u32;
        available = unsafe { Process32NextW(snapshot, &mut entry) } != 0;
    }
    unsafe { CloseHandle(snapshot) };

    let mut descendants = HashSet::from([std::process::id()]);
    loop {
        let before = descendants.len();
        for process in &processes {
            if descendants.contains(&process.parent_id) {
                descendants.insert(process.id);
            }
        }
        if descendants.len() == before {
            break;
        }
    }

    let mut working_set_bytes = 0u64;
    let mut private_bytes = 0u64;
    let mut measured = 0u32;
    for process_id in &descendants {
        let handle =
            unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, 0, *process_id) };
        if handle.is_null() {
            continue;
        }
        let mut counters = ProcessMemoryCountersEx {
            cb: std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
            private_usage: 0,
        };
        let success = unsafe {
            GetProcessMemoryInfo(
                handle,
                &mut counters,
                std::mem::size_of::<ProcessMemoryCountersEx>() as u32,
            )
        };
        unsafe { CloseHandle(handle as isize) };
        if success != 0 {
            working_set_bytes = working_set_bytes.saturating_add(counters.working_set_size as u64);
            private_bytes = private_bytes.saturating_add(counters.private_usage as u64);
            measured = measured.saturating_add(1);
        }
    }

    let (gpu_dedicated_bytes, gpu_shared_bytes) = gpu_memory_for_processes(&descendants);
    ProcessResourceSnapshot {
        working_set_bytes: (measured > 0).then_some(working_set_bytes),
        private_bytes: (measured > 0).then_some(private_bytes),
        gpu_dedicated_bytes,
        gpu_shared_bytes,
    }
}

#[cfg(all(windows, not(test)))]
fn gpu_memory_for_processes(
    process_ids: &std::collections::HashSet<u32>,
) -> (Option<u64>, Option<u64>) {
    use std::os::windows::process::CommandExt;

    let ids = process_ids
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let script = format!(
        "$ids=@({ids});$d=[uint64]0;$s=[uint64]0;\
         $c=(Get-Counter '\\GPU Process Memory(*)\\Dedicated Usage','\\GPU Process Memory(*)\\Shared Usage' -ErrorAction Stop).CounterSamples;\
         foreach($x in $c){{if($x.InstanceName -match '^pid_(\\d+)_' -and $ids -contains [uint32]$Matches[1]){{\
         if($x.Path -like '*\\dedicated usage'){{$d += [uint64]$x.CookedValue}}else{{$s += [uint64]$x.CookedValue}}}}}};\
         [Console]::WriteLine(('{{0}},{{1}}' -f $d,$s))"
    );
    let mut command = std::process::Command::new("powershell.exe");
    command
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .args(["-NoProfile", "-NonInteractive", "-Command", &script]);
    let Ok(output) = command.output() else {
        return (None, None);
    };
    if !output.status.success() {
        return (None, None);
    }
    let Ok(stdout) = String::from_utf8(output.stdout) else {
        return (None, None);
    };
    let Some((dedicated, shared)) = stdout.trim().split_once(',') else {
        return (None, None);
    };
    (dedicated.parse::<u64>().ok(), shared.parse::<u64>().ok())
}

#[cfg(all(windows, test))]
fn gpu_memory_for_processes(
    _process_ids: &std::collections::HashSet<u32>,
) -> (Option<u64>, Option<u64>) {
    (None, None)
}

#[cfg(not(windows))]
pub(crate) fn process_resource_snapshot() -> ProcessResourceSnapshot {
    ProcessResourceSnapshot {
        working_set_bytes: current_process_memory_bytes(),
        ..Default::default()
    }
}

#[derive(Debug, Clone)]
struct SuggestionWork {
    id: i64,
    source: String,
    source_id: String,
    title: String,
    author_name: String,
    author_id: String,
    cover_path: Option<String>,
    text_length: i64,
    published_at: String,
    /// 告知記事を束から外すのに要る。短い `article` は作品ではなく案内である
    /// ことが多いが、短い `novel` は掌編なので、字数だけでは決められない。
    content_type: String,
}

pub(crate) const COLLECTION_SUGGEST_RULE_VERSION: &str = "collection-suggest-v3";
const LINK_TRAVERSAL_MAX_DEPTH: usize = 8;
const LINK_TRAVERSAL_MAX_WORKS: usize = 240;

#[derive(Debug, Clone, Copy)]
struct LinkTraversalEvidence {
    depth: usize,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct StoredWorkLinkEdge {
    from_id: i64,
    to_id: i64,
    relation_type: String,
    confidence: f64,
}

#[derive(Debug, Clone)]
struct RankedSuggestionMember {
    work: SuggestionWork,
    member_score: f64,
    /// 番号なしの初回を兄弟と結びつけるための比較用語幹。
    title_stem: String,
    /// 既定でチェックを入れるか。設計提案 6-5 は強い候補だけを全選択の下書きに
    /// し、弱い候補は個別確認を求めている。公式シリーズに同居しているだけの
    /// 作品まで既定で選ぶと、短編集を丸ごと取り込んでしまう。
    default_selected: bool,
    evidence: Vec<CollectionSuggestionEvidence>,
    series_order: Option<i64>,
    link_order: Option<i64>,
    link_depth: Option<usize>,
    episode_order: Option<i64>,
}

fn load_suggestion_works(conn: &Connection, ids: &[i64]) -> Result<Vec<SuggestionWork>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded = serde_json::to_string(ids)
        .map_err(|e| format!("Failed to encode suggestion work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT id, source, source_id, title, author_name, COALESCE(author_id, ''),
                    cover_path, text_length,
                    COALESCE(source_created_at, downloaded_at, ''), content_type
             FROM downloads
             WHERE id IN (SELECT value FROM json_each(?1))",
        )
        .map_err(|e| format!("Failed to prepare suggestion works: {e}"))?;
    let works = stmt
        .query_map(params![encoded], |row| {
            Ok(SuggestionWork {
                id: row.get(0)?,
                source: row.get(1)?,
                source_id: row.get(2)?,
                title: row.get(3)?,
                author_name: row.get(4)?,
                author_id: row.get(5)?,
                cover_path: row.get(6)?,
                text_length: row.get(7)?,
                published_at: row.get(8)?,
                content_type: row.get(9)?,
            })
        })
        .map_err(|e| format!("Failed to query suggestion works: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read suggestion works: {e}"))?;
    Ok(works)
}

/// これだけの本数のリンクを持ち、かつ本文が薄い作品は、橋として使わない。
const LINK_HUB_MIN_DEGREE: i64 = 5;

/// ハブ判定で「薄い」とみなす字数。
///
/// 実データでは「【お知らせ】連載の更新一覧とリンク集」
/// （2,395字）が16本のリンクを持ち、無関係な17作を1つの束へ縫い合わせていた。
const LINK_HUB_THIN_TEXT: i64 = 4_000;

/// 橋として使わない作品の集合。
///
/// 告知記事は「関連作品はこちら」と何十作も並べる。リンクの向こう側に本当の
/// 続きがあるのではなく、**そこが目次だった**というだけである。目次を渡って
/// しまうと、目次に載っている全部が一つの束になる。
fn load_link_hub_ids(conn: &Connection) -> Result<HashSet<i64>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT d.id, d.title, d.content_type, d.text_length, COUNT(*) AS degree
             FROM downloads d
             JOIN (
               SELECT from_download_id AS id FROM work_links
                WHERE status != 'rejected' AND from_download_id IS NOT NULL
               UNION ALL
               SELECT to_download_id AS id FROM work_links
                WHERE status != 'rejected' AND to_download_id IS NOT NULL
             ) edge ON edge.id = d.id
             GROUP BY d.id
             HAVING degree >= ?1",
        )
        .map_err(|e| format!("Failed to prepare link hub lookup: {e}"))?;
    let rows = stmt
        .query_map(params![LINK_HUB_MIN_DEGREE], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })
        .map_err(|e| format!("Failed to query link hubs: {e}"))?;
    let mut hubs = HashSet::new();
    for row in rows {
        let (id, title, content_type, text_length) =
            row.map_err(|e| format!("Failed to read link hubs: {e}"))?;
        if collection_rules::is_administrative_post(&title, &content_type, text_length)
            || text_length < LINK_HUB_THIN_TEXT
        {
            hubs.insert(id);
        }
    }
    Ok(hubs)
}

fn load_link_edges_touching(
    conn: &Connection,
    download_ids: &[i64],
) -> Result<Vec<StoredWorkLinkEdge>, String> {
    if download_ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded = serde_json::to_string(download_ids)
        .map_err(|e| format!("Failed to encode linked work IDs: {e}"))?;
    let mut stmt = conn
        .prepare(
            "SELECT from_download_id, to_download_id, relation_type, confidence
             FROM work_links
             WHERE status != 'rejected'
               AND from_download_id IS NOT NULL
               AND to_download_id IS NOT NULL
               AND (
                 from_download_id IN (SELECT value FROM json_each(?1))
                 OR to_download_id IN (SELECT value FROM json_each(?1))
               )
             ORDER BY confidence DESC, id DESC
             LIMIT 4000",
        )
        .map_err(|e| format!("Failed to prepare linked work traversal: {e}"))?;
    let edges = stmt
        .query_map(params![encoded], |row| {
            Ok(StoredWorkLinkEdge {
                from_id: row.get(0)?,
                to_id: row.get(1)?,
                relation_type: row.get(2)?,
                confidence: row.get(3)?,
            })
        })
        .map_err(|e| format!("Failed to query linked work traversal: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read linked work traversal: {e}"))?;
    Ok(edges)
}

/// Build the connected component around the selected works before considering
/// fuzzy title or semantic matches. Links are walked in both directions: an
/// end chapter can therefore find a previous chapter that links to it, and
/// that previous chapter becomes a new starting point for the next hop.
fn discover_linked_collection_component(
    src: &dyn SuggestionSource,
    seeds: &[SuggestionWork],
) -> Result<HashMap<i64, LinkTraversalEvidence>, String> {
    let mut paths = seeds
        .iter()
        .map(|seed| {
            (
                seed.id,
                LinkTraversalEvidence {
                    depth: 0,
                    confidence: 1.0,
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut frontier = seeds.iter().map(|seed| seed.id).collect::<Vec<_>>();
    // 目次や告知は端点として拾ってよいが、そこを渡って先へ行ってはいけない。
    // 種は利用者が明示的に選んだものなので、この制限から外す。
    let seed_ids = seeds.iter().map(|seed| seed.id).collect::<HashSet<_>>();
    let hubs = {
        let conn = src.corpus()?;
        load_link_hub_ids(&conn)?
    };

    for _ in 0..LINK_TRAVERSAL_MAX_DEPTH {
        if frontier.is_empty() || paths.len() >= LINK_TRAVERSAL_MAX_WORKS {
            break;
        }

        let frontier_set = frontier.iter().copied().collect::<HashSet<_>>();
        let edges = {
            let conn = src.corpus()?;
            load_link_edges_touching(&conn, &frontier)?
        };
        let mut next = Vec::new();
        for edge in edges {
            for (current, neighbour) in [(edge.from_id, edge.to_id), (edge.to_id, edge.from_id)] {
                if !frontier_set.contains(&current)
                    || paths.contains_key(&neighbour)
                    || paths.len() >= LINK_TRAVERSAL_MAX_WORKS
                {
                    continue;
                }
                if hubs.contains(&current) && !seed_ids.contains(&current) {
                    continue;
                }
                let Some(parent) = paths.get(&current).copied() else {
                    continue;
                };
                paths.insert(
                    neighbour,
                    LinkTraversalEvidence {
                        depth: parent.depth + 1,
                        confidence: parent.confidence.min(edge.confidence),
                    },
                );
                next.push(neighbour);
            }
        }
        next.sort_unstable();
        next.dedup();
        frontier = next;
    }

    Ok(paths)
}

fn assign_link_graph_order(
    conn: &Connection,
    ranked: &mut [RankedSuggestionMember],
) -> Result<(), String> {
    let linked_ids = ranked
        .iter()
        .filter(|member| member.link_depth.is_some())
        .map(|member| member.work.id)
        .collect::<HashSet<_>>();
    if linked_ids.len() < 2 {
        return Ok(());
    }
    let ids = linked_ids.iter().copied().collect::<Vec<_>>();
    let edges = load_link_edges_touching(conn, &ids)?;
    let mut outgoing: HashMap<i64, HashSet<i64>> = HashMap::new();
    let mut indegree: HashMap<i64, usize> = HashMap::new();
    for edge in edges {
        if !linked_ids.contains(&edge.from_id) || !linked_ids.contains(&edge.to_id) {
            continue;
        }
        let ordered = match edge.relation_type.as_str() {
            "continues_to" => Some((edge.from_id, edge.to_id)),
            "continues_from" => Some((edge.to_id, edge.from_id)),
            _ => None,
        };
        let Some((before, after)) = ordered else {
            continue;
        };
        if before == after || !outgoing.entry(before).or_default().insert(after) {
            continue;
        }
        indegree.entry(before).or_insert(0);
        *indegree.entry(after).or_insert(0) += 1;
    }
    if indegree.is_empty() {
        return Ok(());
    }

    let fallback = ranked
        .iter()
        .map(|member| {
            (
                member.work.id,
                (member.work.published_at.clone(), member.work.id),
            )
        })
        .collect::<HashMap<_, _>>();
    let compare_ids = |left: &i64, right: &i64| {
        fallback
            .get(left)
            .cmp(&fallback.get(right))
            .then_with(|| left.cmp(right))
    };
    let mut ready = indegree
        .iter()
        .filter(|(_, degree)| **degree == 0)
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    ready.sort_by(compare_ids);
    let mut ordered = Vec::with_capacity(indegree.len());
    while !ready.is_empty() {
        let current = ready.remove(0);
        ordered.push(current);
        if let Some(targets) = outgoing.get(&current) {
            for target in targets {
                let Some(degree) = indegree.get_mut(target) else {
                    continue;
                };
                *degree = degree.saturating_sub(1);
                if *degree == 0 {
                    ready.push(*target);
                }
            }
            ready.sort_by(compare_ids);
        }
    }
    // A malformed cycle must not make ordering nondeterministic. Put the
    // remaining linked works after the acyclic prefix in a stable fallback.
    let mut remaining = indegree
        .keys()
        .filter(|id| !ordered.contains(id))
        .copied()
        .collect::<Vec<_>>();
    remaining.sort_by(compare_ids);
    ordered.extend(remaining);
    let positions = ordered
        .into_iter()
        .enumerate()
        .map(|(position, id)| (id, position as i64))
        .collect::<HashMap<_, _>>();
    for member in ranked {
        member.link_order = positions.get(&member.work.id).copied();
    }
    Ok(())
}

fn shared_series_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Vec<(String, Option<i64>)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT candidate.title, candidate.content_order
             FROM download_series candidate
             JOIN download_series seed
               ON seed.series_source = candidate.series_source
              AND seed.series_key = candidate.series_key
             WHERE candidate.download_id = ?1
               AND seed.download_id IN (SELECT value FROM json_each(?2))
             GROUP BY candidate.series_source, candidate.series_key
             ORDER BY candidate.content_order ASC, candidate.title COLLATE NOCASE ASC",
        )
        .map_err(|e| format!("Failed to prepare shared series: {e}"))?;
    let values = stmt
        .query_map(params![candidate_id, seed_json], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<i64>>(1)?))
        })
        .map_err(|e| format!("Failed to query shared series: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("Failed to read shared series: {e}"))?;
    Ok(values)
}

fn strongest_link_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Option<(String, f64, Option<i64>)>, String> {
    conn.query_row(
        "SELECT relation_type, confidence, from_download_id, to_download_id
         FROM work_links
         WHERE status != 'rejected'
           AND (
             (from_download_id = ?1 AND to_download_id IN (SELECT value FROM json_each(?2)))
             OR (to_download_id = ?1 AND from_download_id IN (SELECT value FROM json_each(?2)))
           )
         ORDER BY confidence DESC, id DESC LIMIT 1",
        params![candidate_id, seed_json],
        |row| {
            let relation = row.get::<_, String>(0)?;
            let confidence = row.get::<_, f64>(1)?;
            let from_id = row.get::<_, Option<i64>>(2)?;
            let to_id = row.get::<_, Option<i64>>(3)?;
            let relative_order = match relation.as_str() {
                // A --continues_to--> B means A precedes B.
                "continues_to" if from_id == Some(candidate_id) => Some(-1),
                "continues_to" if to_id == Some(candidate_id) => Some(1),
                // A --continues_from--> B means B precedes A.
                "continues_from" if from_id == Some(candidate_id) => Some(1),
                "continues_from" if to_id == Some(candidate_id) => Some(-1),
                _ => None,
            };
            Ok((relation, confidence, relative_order))
        },
    )
    .optional()
    .map_err(|e| format!("Failed to query work-link evidence: {e}"))
}

fn suggestion_pair_is_rejected(
    conn: &Connection,
    candidate: &SuggestionWork,
    seeds: &[SuggestionWork],
) -> Result<bool, String> {
    let candidate_key = WorkKey {
        source: candidate.source.clone(),
        source_id: candidate.source_id.clone(),
    };
    for seed in seeds {
        let seed_key = WorkKey {
            source: seed.source.clone(),
            source_id: seed.source_id.clone(),
        };
        let (left, right) = canonical_work_key_pair(&candidate_key, &seed_key);
        let rejected = conn
            .query_row(
                "SELECT decision, rule_version FROM collection_pair_feedback
                 WHERE left_source = ?1 AND left_source_id = ?2
                   AND right_source = ?3 AND right_source_id = ?4",
                params![left.source, left.source_id, right.source, right.source_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()
            .map_err(|e| format!("Failed to read collection feedback: {e}"))?
            .is_some_and(|(decision, rule_version)| {
                decision == "reject" && rule_version == COLLECTION_SUGGEST_RULE_VERSION
            });
        if rejected {
            return Ok(true);
        }
    }
    Ok(false)
}

fn add_suggestion_evidence(
    evidence: &mut Vec<CollectionSuggestionEvidence>,
    score: &mut f64,
    kind: &str,
    label: &str,
    contribution: f64,
) {
    *score += contribution;
    evidence.push(CollectionSuggestionEvidence {
        kind: kind.to_string(),
        label: label.to_string(),
        contribution,
    });
}

/// 話数を持たない特別な位置。本編の連番（`話数 * 10`）とぶつからない値を選ぶ。
const ORDER_PROLOGUE: i64 = -10;
const ORDER_FINALE: i64 = 1_000_000;
const ORDER_EPILOGUE: i64 = 1_000_010;
const ORDER_SIDE_STORY: i64 = 1_000_020;

/// 単独トークンの数字を話数とみなす上限。`作品 2026` のような西暦を
/// 話数として拾わないための歯止めで、助数詞を伴う明示的な形には適用しない。
const MAX_BARE_EPISODE_NUMBER: i64 = 300;

/// 漢数字を読む。`十二` `三十` `百二十三` と、`〇一二` のような桁並べに対応する。
fn kanji_number(token: &str) -> Option<i64> {
    let digit = |value: char| match value {
        '〇' | '零' => Some(0),
        '一' => Some(1),
        '二' => Some(2),
        '三' => Some(3),
        '四' => Some(4),
        '五' => Some(5),
        '六' => Some(6),
        '七' => Some(7),
        '八' => Some(8),
        '九' => Some(9),
        _ => None,
    };
    let mut total = 0_i64;
    let mut current = 0_i64;
    let mut saw_any = false;
    for value in token.chars() {
        match value {
            '百' => {
                total += if current == 0 { 100 } else { current * 100 };
                current = 0;
                saw_any = true;
            }
            '十' => {
                total += if current == 0 { 10 } else { current * 10 };
                current = 0;
                saw_any = true;
            }
            other => {
                current = current * 10 + digit(other)?;
                saw_any = true;
            }
        }
    }
    saw_any.then(|| total + current)
}

fn roman_to_text(mut value: i64) -> String {
    const TABLE: [(i64, &str); 9] = [
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut out = String::new();
    for (amount, text) in TABLE {
        while value >= amount {
            out.push_str(text);
            value -= amount;
        }
    }
    out
}

/// ローマ数字を読む。`civil` のように文字だけ揃った単語を弾くため、読んだ数を
/// 書き戻して元の綴りと一致する場合しか採用しない。一文字は `i` や `c` が
/// 英単語と衝突するため対象外にする。
fn roman_number(token: &str) -> Option<i64> {
    if token.chars().count() < 2 || token.chars().count() > 15 {
        return None;
    }
    let values = token
        .chars()
        .map(|value| match value {
            'i' => Some(1_i64),
            'v' => Some(5),
            'x' => Some(10),
            'l' => Some(50),
            'c' => Some(100),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    let mut total = 0_i64;
    for (index, current) in values.iter().enumerate() {
        if values[index + 1..].iter().any(|later| later > current) {
            total -= current;
        } else {
            total += current;
        }
    }
    (total > 0 && roman_to_text(total) == token).then_some(total)
}

fn parse_episode_number(token: &str) -> Option<i64> {
    token
        .parse::<i64>()
        .ok()
        .or_else(|| kanji_number(token))
        .filter(|value| *value >= 0)
}

/// 題名から比較用の共通幹と、読む順を表す数値を取り出す。
///
/// 入力は `normalize_search_text` を通してから扱う。その正規化は NFKC・小文字化・
/// カタカナ→ひらがな変換に加えて、**記号をすべて空白へ潰す**。`#12` はここへ届く
/// 時点で `12`、`【FANBOX】月下の約束（後編）` は `fanbox 月下の約束 後編` に
/// なっている。したがって記号を手掛かりにはできず、数字・漢数字・ローマ数字と
/// 語そのものだけで判定する。
///
/// 返す順序値は「話数 * 10 + 前中後編の位置」を基本とし、話数を持たない
/// プロローグ・最終話・エピローグ・番外編には前後へ十分離れた固定値を与える。
fn title_stem_and_order(title: &str) -> (String, Option<i64>) {
    static COUNTER_RE: OnceLock<Regex> = OnceLock::new();
    static DAI_RE: OnceLock<Regex> = OnceLock::new();
    static SONO_RE: OnceLock<Regex> = OnceLock::new();
    static MARKER_RE: OnceLock<Regex> = OnceLock::new();
    // 助数詞を伴う形。`第12話` `12話` `第三夜` `2部` のいずれも拾う。
    let counter_regex = COUNTER_RE.get_or_init(|| {
        Regex::new(
            r"(?:第\s*)?([0-9]+|[〇零一二三四五六七八九十百]+)\s*(?:話|章|回|編|夜|部|節|幕|巻)",
        )
        .expect("episode counter regex")
    });
    // 助数詞のない `第3` 形。
    let dai_regex = DAI_RE.get_or_init(|| {
        Regex::new(r"第\s*([0-9]+|[〇零一二三四五六七八九十百]+)").expect("episode dai regex")
    });
    // `その12` 形。正規化でカタカナはひらがなへ倒れている。
    let sono_regex = SONO_RE.get_or_init(|| {
        Regex::new(r"その\s*([0-9]+|[〇零一二三四五六七八九十百]+)").expect("episode sono regex")
    });
    // `final` は単語境界を要求する。`finalize` を完結編と誤らせない。
    let marker_regex = MARKER_RE.get_or_init(|| {
        Regex::new(
            r"前編|前篇|中編|中篇|後編|後篇|上巻|中巻|下巻|完結編|最終話|最終回|番外編|番外|外伝|ぷろろーぐ|えぴろーぐ|\bfinal\b",
        )
        .expect("episode marker regex")
    });

    let normalized = normalize_search_text(title);

    let mut number = None;
    for regex in [counter_regex, dai_regex, sono_regex] {
        if number.is_some() {
            break;
        }
        number = regex
            .captures(&normalized)
            .and_then(|capture| capture.get(1))
            .and_then(|value| parse_episode_number(value.as_str()));
    }

    let mut working = counter_regex.replace_all(&normalized, " ").into_owned();
    working = dai_regex.replace_all(&working, " ").into_owned();
    working = sono_regex.replace_all(&working, " ").into_owned();

    // 助数詞のない単独トークン。`航路 01` や `星の記憶 xii` を拾う。
    let mut bare_number_token = None;
    if number.is_none() {
        for token in working.split_whitespace() {
            let parsed = token
                .parse::<i64>()
                .ok()
                .filter(|value| (0..=MAX_BARE_EPISODE_NUMBER).contains(value))
                .or_else(|| kanji_number(token))
                .or_else(|| roman_number(token));
            if let Some(value) = parsed {
                number = Some(value);
                bare_number_token = Some(token.to_string());
                break;
            }
        }
    }

    let marker_offset =
        if working.contains("前編") || working.contains("前篇") || working.contains("上巻") {
            Some(1)
        } else if working.contains("中編") || working.contains("中篇") || working.contains("中巻")
        {
            Some(2)
        } else if working.contains("後編") || working.contains("後篇") || working.contains("下巻")
        {
            Some(3)
        } else {
            // 素の `上` `中` `下` は単独トークンのときだけ位置とみなす。
            // `月下の約束` の `下` を後編と読まないための条件である。
            working.split_whitespace().find_map(|token| match token {
                "上" => Some(1),
                "中" => Some(2),
                "下" => Some(3),
                _ => None,
            })
        };

    let special_order = if working.contains("ぷろろーぐ") {
        Some(ORDER_PROLOGUE)
    } else if working.contains("番外編") || working.contains("番外") || working.contains("外伝")
    {
        Some(ORDER_SIDE_STORY)
    } else if working.contains("えぴろーぐ") {
        Some(ORDER_EPILOGUE)
    } else if working.contains("完結編")
        || working.contains("最終話")
        || working.contains("最終回")
        || marker_regex
            .find(&working)
            .is_some_and(|found| found.as_str() == "final")
    {
        Some(ORDER_FINALE)
    } else {
        None
    };

    // 話数が読めていればそれを主軸にし、前中後編を枝番として足す。話数がない
    // ときだけ、プロローグ・最終話などの固定位置へ落とす。
    let order = match number {
        Some(value) => Some(value.saturating_mul(10) + marker_offset.unwrap_or(0)),
        None => special_order.or(marker_offset),
    };

    let without_markers = marker_regex.replace_all(&working, " ");
    let stem = without_markers
        .split_whitespace()
        .filter(|token| {
            !matches!(*token, "上" | "中" | "下") && bare_number_token.as_deref() != Some(*token)
        })
        .collect::<Vec<_>>()
        .join(" ");
    (stem, order)
}

/// 連載の初回は番号を振らないことが多い。番号付きの兄弟が同じ語幹で存在し、
/// 自身にだけ番号が無い作品は「第 0 話」として先頭へ置く。順序未定を表す
/// None は末尾へ流れるため、この補正が無いと初回が最後に並んでしまう。
/// 本文が二重に入る候補へ印を付け、既定の選択から外す。
///
/// 実ライブラリには 2 種類の重複がある。pixiv と FANBOX の両方へ投稿された
/// 同じ作品と、分冊をまとめ直した合本である。どちらもそのまま EPUB にすると
/// 同じ本文が 2 回入る。候補からは消さず、理由を見せたうえで利用者が明示的に
/// 選んだときだけ含める。どちらを採るかは利用者の判断に属するためである。
fn flag_duplicate_content_members(ranked: &mut [RankedSuggestionMember]) {
    const OMNIBUS_WORDS: [&str; 3] = ["まとめ", "合本", "総集編"];
    const POSITION_WORDS: [&str; 7] = ["前編", "中編", "後編", "完結編", "最終話", "上巻", "下巻"];

    // 取得元をまたぐ同一作品。題名と作者が一致し取得元だけが違うものを束ね、
    // 本文が長い方（取りこぼしの少ない方）を既定に残す。
    let mut by_title: HashMap<(String, String), Vec<usize>> = HashMap::new();
    for (index, member) in ranked.iter().enumerate() {
        let key = (
            normalize_search_text(&member.work.title),
            normalize_search_text(&member.work.author_name),
        );
        if key.0.is_empty() {
            continue;
        }
        by_title.entry(key).or_default().push(index);
    }
    let mut duplicates = Vec::new();
    for indexes in by_title.values() {
        if indexes.len() < 2 {
            continue;
        }
        let sources = indexes
            .iter()
            .map(|index| ranked[*index].work.source.clone())
            .collect::<HashSet<_>>();
        if sources.len() < 2 {
            continue;
        }
        // 基準作品は利用者自身が選んだもの。重複していても既定から外さず、
        // もう一方に印を付ける。選んだはずの作品が外れていると混乱する。
        let seeded = indexes
            .iter()
            .find(|index| ranked[**index].evidence.iter().any(|e| e.kind == "seed"));
        let keep = match seeded {
            Some(index) => *index,
            None => *indexes
                .iter()
                .max_by_key(|index| (ranked[**index].work.text_length, -ranked[**index].work.id))
                .expect("non-empty duplicate group"),
        };
        for index in indexes {
            if *index != keep {
                duplicates.push((*index, ranked[keep].work.source.clone()));
            }
        }
    }
    for (index, kept_source) in duplicates {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "duplicate_work".to_string(),
            label: format!("{kept_source} にも同じ題名の作品があります"),
            contribution: 0.0,
        });
    }

    // 合本。前編と中編のように位置を表す語を 2 つ以上含むか、まとめ・合本・
    // 総集編と名乗るもの。単に「④+エピローグ」のような 1 か所だけの表記は
    // 分冊そのものなので対象にしない。
    let mut omnibus = Vec::new();
    for (index, member) in ranked.iter().enumerate() {
        let title = normalize_search_text(&member.work.title);
        let positions = POSITION_WORDS
            .iter()
            .filter(|word| title.contains(**word))
            .count();
        let named = OMNIBUS_WORDS.iter().any(|word| title.contains(*word));
        if positions < 2 && !named {
            continue;
        }
        let siblings = ranked
            .iter()
            .filter(|other| {
                other.work.id != member.work.id
                    && stems_are_siblings(&other.title_stem, &member.title_stem)
            })
            .count();
        if siblings >= 2 {
            omnibus.push(index);
        }
    }
    // 抜粋・体験版は本文が途中までしかない。合本とは逆に「足りない」重複で、
    // 一冊にまとめると同じ話の断片が混ざる。
    const SAMPLE_WORDS: [&str; 4] = ["さんぷる", "体験版", "お試し", "抜粋"];
    let samples = ranked
        .iter()
        .enumerate()
        .filter(|(_, member)| {
            let title = normalize_search_text(&member.work.title);
            SAMPLE_WORDS.iter().any(|word| title.contains(*word))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in samples {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "partial_sample".to_string(),
            label: "本文の一部だけの版に見えます".to_string(),
            contribution: 0.0,
        });
    }

    for index in omnibus {
        ranked[index].default_selected = false;
        ranked[index].evidence.push(CollectionSuggestionEvidence {
            kind: "omnibus".to_string(),
            label: "分冊をまとめ直した版の可能性があります".to_string(),
            contribution: 0.0,
        });
    }
}

fn assign_unnumbered_opening_order(ranked: &mut [RankedSuggestionMember]) {
    let numbered_stems = ranked
        .iter()
        .filter(|value| value.episode_order.is_some() && !value.title_stem.is_empty())
        .map(|value| value.title_stem.clone())
        .collect::<Vec<_>>();
    let openings = ranked
        .iter()
        .enumerate()
        .filter(|(_, value)| value.episode_order.is_none() && !value.title_stem.is_empty())
        .filter(|(_, value)| {
            numbered_stems
                .iter()
                .any(|stem| stems_are_siblings(stem, &value.title_stem))
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    for index in openings {
        ranked[index].episode_order = Some(0);
    }
}

/// 同じ連載の一部と見なせる語幹かどうか。完全一致だけを見ると、副題や
/// 版を示す注記が付いた作品を取りこぼさない。
fn stems_are_siblings(left: &str, right: &str) -> bool {
    if left.is_empty() || right.is_empty() {
        return false;
    }
    left == right
        || left.contains(right)
        || right.contains(left)
        || normalized_levenshtein(left, right) >= 0.8
}

fn compare_ranked_suggestion_members(
    left: &RankedSuggestionMember,
    right: &RankedSuggestionMember,
) -> Ordering {
    // 読む順は明示的な手掛かりだけで決める。スコアは「順序の手掛かりが何も
    // 無い作品同士」の並びにしか使わない。単なる言及リンクで加点された作品が、
    // 題名に書かれた話数を追い越すのを防ぐためである。
    // link_depth は基準作品からのリンク到達段数、つまり「近さ」であって
    // 読む順ではない。話数や公式順より先に効かせると、単に基準作品から
    // 参照されているだけの作品が第1話を追い越す。同点処理にだけ使う。
    compare_optional_order(left.link_order, right.link_order)
        .then_with(|| compare_optional_order(left.series_order, right.series_order))
        .then_with(|| compare_optional_order(left.episode_order, right.episode_order))
        .then_with(|| compare_optional_order(left.link_depth, right.link_depth))
        .then_with(|| right.member_score.total_cmp(&left.member_score))
        .then_with(|| left.work.published_at.cmp(&right.work.published_at))
        .then_with(|| left.work.id.cmp(&right.work.id))
}

fn compare_optional_order<T: Ord>(left: Option<T>, right: Option<T>) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// 束の名前に使える長さ。棚のカードの行数とは関係が無い - 描くときに
/// 何行で畳むかは画面が決め、切れた先はホバーで読める。
const COLLECTION_NAME_MAX_CHARS: usize = collection_rules::NAME_STORAGE_MAX_CHARS;

/// 名前の案を、確からしい順に並べて返す。
///
/// 一つに決めて押し付けない。どの案も外れることがあり、外れたときに直せる
/// のは利用者だけだからである。先頭が既定の `proposed_name` になる。
///
/// 以前はここが `title_stem_and_order` の返り値をそのまま名前にしていた。
/// あれは**検索用の正規化キー**で、カタカナはひらがなへ倒れ記号も落ちている。
/// 「はいすぺいけめん女子の綾城さん」はそうやって生まれた。
fn collection_name_options(
    conn: &Connection,
    ranked: &[RankedSuggestionMember],
    seed_json: &str,
) -> Result<Vec<CollectionNameCandidate>, String> {
    fn push(options: &mut Vec<CollectionNameCandidate>, source: &str, label: &str, name: String) {
        let name = collection_rules::clamp_name(&name, COLLECTION_NAME_MAX_CHARS);
        // 1文字の名前は棚で見分けが付かない。案として立てない。
        if name.chars().count() < 2 || options.iter().any(|value| value.name == name) {
            return;
        }
        options.push(CollectionNameCandidate {
            source: source.to_string(),
            label: label.to_string(),
            name,
            ..Default::default()
        });
    }
    let mut options: Vec<CollectionNameCandidate> = Vec::new();

    // 案A 題名。いちばん多くのメンバーが共有する語幹を、いちばん短い題名の
    // 表記で出す。短いものを選ぶのは、長い方には枝葉が付いていることが多いため。
    let mut families: HashMap<String, Vec<&RankedSuggestionMember>> = HashMap::new();
    for member in ranked {
        let key = collection_rules::family_match_key(&member.work.title);
        if key.chars().count() >= 6 {
            families.entry(key).or_default().push(member);
        }
    }
    if let Some((_, group)) = families
        .iter()
        .max_by_key(|(key, group)| (group.len(), key.chars().count()))
    {
        if group.len() >= 2 {
            if let Some(shortest) = group
                .iter()
                .min_by_key(|member| member.work.title.chars().count())
            {
                push(
                    &mut options,
                    "title",
                    "題名の共通部分",
                    collection_rules::display_title_stem(&shortest.work.title),
                );
            }
        }
    }

    // 案B 公式シリーズ。ただし作者の管理用ラベルは名前にしない。
    // 「有償依頼」151作が1つの物語だったことは一度も無い。
    for member in ranked {
        if let Some((title, _)) = shared_series_with_seeds(conn, member.work.id, seed_json)?.first()
        {
            if !collection_rules::is_administrative_series_label(title) {
                push(
                    &mut options,
                    "series",
                    "公式シリーズ",
                    title.trim().to_string(),
                );
                break;
            }
        }
    }

    // 案C 共有タグ。原作・人物・題材の順に並ぶよう、珍しいタグを先に置く。
    let ids = ranked.iter().map(|value| value.work.id).collect::<Vec<_>>();
    let shared = shared_member_tags(conn, &ids, 3)?;
    if !shared.is_empty() {
        push(&mut options, "tags", "共有タグ", shared.join(" / "));
    }

    // 案D 作者。ここまで何も言えなかったときだけ。
    if let Some(first) = ranked.first() {
        let author = first.work.author_name.trim();
        if !author.is_empty() {
            push(
                &mut options,
                "author",
                "作者",
                format!("{author}のまとまり"),
            );
        }
    }
    if options.is_empty() {
        push(
            &mut options,
            "author",
            "既定",
            "読書コレクション".to_string(),
        );
    }
    Ok(options)
}

/// 候補と基準作品が、同じ公式シリーズで**隣り合っているか**。
///
/// 同じシリーズに載っていることと、続けて読むべきことは違う。151作の
/// 「有償依頼」に同居しているだけの2作は、隣り合ってはいない。返すのは
/// シリーズ名と話数の隔たりで、隔たりが小さいときだけ束の証拠として扱う。
fn shared_series_run_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Option<(String, i64)>, String> {
    conn.query_row(
        "SELECT candidate.title, MIN(ABS(candidate.content_order - seed.content_order))
         FROM download_series candidate
         JOIN download_series seed
           ON seed.series_source = candidate.series_source
          AND seed.series_key = candidate.series_key
         WHERE candidate.download_id = ?1
           AND seed.download_id IN (SELECT value FROM json_each(?2))
           AND seed.download_id != ?1
           AND candidate.content_order IS NOT NULL
           AND seed.content_order IS NOT NULL
         GROUP BY candidate.series_source, candidate.series_key
         ORDER BY 2 ASC
         LIMIT 1",
        params![candidate_id, seed_json],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
    )
    .optional()
    .map_err(|e| format!("Failed to query series adjacency: {e}"))
}

/// 候補と基準作品が共有している、束を言い表せるタグの数。
fn shared_tags_with_seeds(
    conn: &Connection,
    candidate_id: i64,
    seed_json: &str,
) -> Result<Vec<String>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT t.name
             FROM download_tags candidate
             JOIN download_tags seed ON seed.tag_id = candidate.tag_id
             JOIN tags t ON t.id = candidate.tag_id
             WHERE candidate.download_id = ?1
               AND seed.download_id IN (SELECT value FROM json_each(?2))
               AND seed.download_id != ?1
             ORDER BY t.name COLLATE NOCASE",
        )
        .map_err(|e| format!("Failed to prepare shared tag lookup: {e}"))?;
    let rows = stmt
        .query_map(params![candidate_id, seed_json], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|e| format!("Failed to query shared tags: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let name = row.map_err(|e| format!("Failed to read shared tags: {e}"))?;
        if collection_rules::is_informative_tag(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

/// 二つの投稿日が何日離れているか。読めない日付は「分からない」を返す。
fn published_days_apart(left: &str, right: &str) -> Option<i64> {
    let parse = |value: &str| {
        chrono::DateTime::parse_from_rfc3339(value)
            .ok()
            .map(|value| value.date_naive())
            .or_else(|| {
                value
                    .get(..10)
                    .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
            })
    };
    let left = parse(left)?;
    let right = parse(right)?;
    Some((left - right).num_days().abs())
}

#[cfg(test)]
#[test]
fn published_days_apart_ignores_invalid_utf8_boundaries() {
    assert_eq!(published_days_apart("ああああ", "2024-01-01"), None);
    assert_eq!(published_days_apart("2024-01-11", "2024-01-01"), Some(10));
}

/// なぜこれが束なのかを、一行で書く。
///
/// 「確度 74%」は根拠になっていないのに、数字であるという理由だけで信じて
/// しまう。何が根拠なのかを言葉で出せば、利用者はそれが当たっているかを
/// 自分で確かめられる。強い証拠から順に見て、最初に見つかったものを使う。
fn suggestion_evidence_summary(
    members: &[CollectionSuggestionMember],
    seed_ids: &HashSet<i64>,
) -> String {
    let others = members
        .iter()
        .filter(|member| member.download_id.is_none_or(|id| !seed_ids.contains(&id)))
        .collect::<Vec<_>>();
    let count = |kind: &str| {
        others
            .iter()
            .filter(|member| member.evidence.iter().any(|value| value.kind == kind))
            .count()
    };
    let total = members.len();
    let linked = count("content_link");
    if linked > 0 {
        return format!("本文のリンクで{}作がつながっています", linked + 1);
    }
    let ordered = members
        .iter()
        .filter(|member| collection_rules::has_ordinal_marker(&member.title))
        .count();
    if ordered * 2 > total && total > 1 {
        return format!("題名が連番になっている{total}作です");
    }
    if count("title_similarity") > 0 {
        return format!("題名の共通する{total}作です");
    }
    if let Some(label) = others
        .iter()
        .flat_map(|member| member.evidence.iter())
        .find(|value| value.kind == "official_series")
        .map(|value| value.label.clone())
    {
        return format!("{label}に並ぶ{total}作です");
    }
    if count("semantic_similarity") > 0 {
        return format!("本文の内容が近い{total}作です");
    }
    format!("同じ作者の{total}作です")
}

/// 束のメンバーが共有しているタグを、珍しい順に返す。
///
/// 半数以上が持っているものだけを共有とみなす。全員に揃えると、1作だけ
/// 抜けているタグが落ちて何も残らない束が出る。
fn shared_member_tags(conn: &Connection, ids: &[i64], limit: usize) -> Result<Vec<String>, String> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let encoded =
        serde_json::to_string(ids).map_err(|e| format!("Failed to encode tag member IDs: {e}"))?;
    let threshold = ids.len().div_ceil(2).max(2) as i64;
    let mut stmt = conn
        .prepare(
            "SELECT t.name, COUNT(*) AS shared,
                    (SELECT COUNT(*) FROM download_tags dt2 WHERE dt2.tag_id = t.id) AS total
             FROM download_tags dt
             JOIN tags t ON t.id = dt.tag_id
             WHERE dt.download_id IN (SELECT value FROM json_each(?1))
             GROUP BY t.id
             HAVING shared >= ?2
             ORDER BY shared DESC, total ASC, t.name COLLATE NOCASE ASC",
        )
        .map_err(|e| format!("Failed to prepare shared tags: {e}"))?;
    let rows = stmt
        .query_map(params![encoded, threshold], |row| row.get::<_, String>(0))
        .map_err(|e| format!("Failed to query shared tags: {e}"))?;
    let mut out = Vec::new();
    for row in rows {
        let name = row.map_err(|e| format!("Failed to read shared tags: {e}"))?;
        if collection_rules::is_informative_tag(&name) {
            out.push(name);
        }
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

fn canonical_work_key_pair(left: &WorkKey, right: &WorkKey) -> (WorkKey, WorkKey) {
    if (&left.source, &left.source_id) <= (&right.source, &right.source_id) {
        (left.clone(), right.clone())
    } else {
        (right.clone(), left.clone())
    }
}

#[derive(Debug, Clone)]
pub(crate) struct ExtractedWorkLink {
    pub(crate) to_source: String,
    pub(crate) to_source_id: String,
    pub(crate) relation_type: String,
    pub(crate) anchor_text: Option<String>,
    pub(crate) context_text: Option<String>,
    pub(crate) confidence: f64,
}

pub(crate) fn extract_work_link_evidence(
    text: &str,
    from_source: &str,
    from_source_id: &str,
) -> Vec<ExtractedWorkLink> {
    work_link_evidence::extract(text, from_source, from_source_id)
}

fn normalize_linked_work_url(raw: &str) -> Option<(String, String)> {
    if let Some(source_id) = raw
        .strip_prefix("pixiv://novels/")
        .or_else(|| raw.strip_prefix("PIXIV://NOVELS/"))
    {
        let source_id = source_id
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        return (!source_id.is_empty()).then(|| ("pixiv".to_string(), source_id));
    }
    let url = url::Url::parse(raw).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    let segments = url
        .path_segments()
        .map(|value| value.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if host == "pixiv.net" || host == "www.pixiv.net" {
        if url.path() == "/novel/show.php" {
            let id = url
                .query_pairs()
                .find(|(key, _)| key == "id")
                .map(|(_, value)| value.into_owned())?;
            return id
                .chars()
                .all(|value| value.is_ascii_digit())
                .then(|| ("pixiv".to_string(), id));
        }
        if segments.first().copied() == Some("novels") {
            let id = segments.get(1)?.to_string();
            return id
                .chars()
                .all(|value| value.is_ascii_digit())
                .then(|| ("pixiv".to_string(), id));
        }
    }
    if host == "fanbox.cc" || host.ends_with(".fanbox.cc") {
        let index = segments.iter().position(|segment| *segment == "posts")?;
        let id = segments.get(index + 1)?.to_string();
        return id
            .chars()
            .all(|value| value.is_ascii_digit())
            .then(|| ("fanbox".to_string(), id));
    }
    None
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

pub(crate) fn normalize_block_inputs(blocks: &[WorkBlockInput]) -> Vec<WorkBlockInput> {
    let mut normalized = Vec::new();
    for block in blocks {
        let block_type = match block.block_type.as_str() {
            "heading" | "image" | "separator" | "page_break" | "quote" | "link" => {
                block.block_type.clone()
            }
            _ => "paragraph".to_string(),
        };
        // 行頭の字下げは本文の一部。`trim` を掛けていたころは、全角空白で
        // 字下げした作品が保存のたびに左へ寄っていった。落とすのは中身が
        // 空白しかないブロックだけで、末尾の余りだけを削る。
        let text = block
            .text
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .map(|value| value.trim_end().to_string());
        if block_type != "image"
            && text.is_none()
            && block_type != "separator"
            && block_type != "page_break"
        {
            continue;
        }
        normalized.push(WorkBlockInput {
            block_type,
            text,
            asset_id: block.asset_id,
            attrs_json: block.attrs_json.clone(),
        });
    }
    if normalized.is_empty() {
        normalized.push(WorkBlockInput {
            block_type: "paragraph".to_string(),
            text: Some(String::new()),
            asset_id: None,
            attrs_json: None,
        });
    }
    normalized
}

fn plain_text_to_editor_blocks(text: &str) -> Vec<WorkBlock> {
    let chunks = text
        .split("\n\n")
        .map(str::trim)
        .filter(|chunk| !chunk.is_empty())
        .collect::<Vec<_>>();
    let chunks = if chunks.is_empty() {
        vec![text.trim()]
    } else {
        chunks
    };

    chunks
        .into_iter()
        .enumerate()
        .map(|(idx, text)| WorkBlock {
            id: 0,
            edit_revision_id: 0,
            order: idx as i64,
            block_type: if text.starts_with('#') {
                "heading".to_string()
            } else {
                "paragraph".to_string()
            },
            text: Some(text.trim_start_matches('#').trim().to_string()),
            asset_id: None,
            attrs_json: None,
        })
        .collect()
}

/// 取り込んだ本文を「行の並び」として読み直すための 1 行分。
enum SourceLine {
    /// 地の文の 1 行。空文字なら空行。
    Text(String),
    /// 見出し・画像・区切り・改ページ・リンクカード。それぞれ 1 行を占める。
    Token(WorkBlock),
    /// FANBOX が段落として書いた切れ目。行は占めないが、ここで段落は変わる。
    /// 落としていたころは、投稿者が分けて書いた段落が編集画面で 1 つの塊に
    /// 溶けていた。
    Boundary,
}

fn attrs_with(attrs: Option<String>, key: &str, value: serde_json::Value) -> Option<String> {
    let mut object = attrs
        .as_deref()
        .and_then(|attrs| serde_json::from_str::<serde_json::Value>(attrs).ok())
        .filter(serde_json::Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    object[key] = value;
    Some(object.to_string())
}

fn attrs_with_gap(attrs: Option<String>, gap: usize) -> Option<String> {
    attrs_with(
        attrs,
        BLOCK_GAP_KEY,
        serde_json::json!(gap.min(MAX_BLOCK_GAP)),
    )
}

/// 取り込んだ HTML の断片を行へ割る。
///
/// 末尾の要素は改行で閉じられていない「続き」なので、中身があるときだけ
/// 1 行として積む。`after_token` のときの先頭の空要素は、直前のトークンが
/// 載っていた行の終わりであって空行ではない。ここを空行として数えていたため、
/// 見出しのあとの本文が往復のたびに 1 行ずつ下がっていった。
fn push_fragment_lines(fragment: &str, after_token: bool, lines: &mut Vec<SourceLine>) {
    let text = html_fragment_to_text(&restore_inline_notation(fragment));
    if text.is_empty() {
        return;
    }
    let mut pieces: Vec<&str> = text.split('\n').collect();
    let trailing = pieces.pop().unwrap_or("");
    let mut complete = &pieces[..];
    if after_token && complete.first().is_some_and(|line| line.is_empty()) {
        complete = &complete[1..];
    }
    for line in complete {
        lines.push(SourceLine::Text((*line).to_string()));
    }
    if !trailing.is_empty() {
        lines.push(SourceLine::Text(trailing.to_string()));
    }
}

fn paragraph_block(lines: &[String], gap: usize, order: usize) -> WorkBlock {
    editor_block(
        order,
        "paragraph",
        // 行頭の字下げは本文の一部。ここで trim していたころは、全角空白で
        // 字下げした作品が編集を通すたびに左へ寄っていった。
        Some(lines.join("\n")),
        None,
        attrs_with_gap(None, gap),
    )
}

fn source_lines_to_blocks(lines: Vec<SourceLine>) -> Vec<WorkBlock> {
    let mut blocks: Vec<WorkBlock> = Vec::new();
    let mut paragraph: Vec<String> = Vec::new();
    let mut paragraph_gap = 0usize;
    let mut blank_run = 0usize;

    for line in lines {
        match line {
            SourceLine::Text(text) if text.trim().is_empty() => {
                if !paragraph.is_empty() {
                    let order = blocks.len();
                    blocks.push(paragraph_block(&paragraph, paragraph_gap, order));
                    paragraph.clear();
                    blank_run = 0;
                }
                blank_run += 1;
            }
            SourceLine::Text(text) => {
                if paragraph.is_empty() {
                    paragraph_gap = blank_run;
                    blank_run = 0;
                }
                paragraph.push(text);
            }
            SourceLine::Boundary => {
                if !paragraph.is_empty() {
                    let order = blocks.len();
                    blocks.push(paragraph_block(&paragraph, paragraph_gap, order));
                    paragraph.clear();
                }
            }
            SourceLine::Token(mut block) => {
                if !paragraph.is_empty() {
                    let order = blocks.len();
                    blocks.push(paragraph_block(&paragraph, paragraph_gap, order));
                    paragraph.clear();
                }
                block.order = blocks.len() as i64;
                block.attrs_json = attrs_with_gap(block.attrs_json, blank_run);
                blocks.push(block);
                blank_run = 0;
            }
        }
    }
    if !paragraph.is_empty() {
        let order = blocks.len();
        blocks.push(paragraph_block(&paragraph, paragraph_gap, order));
    }
    blocks
}

static EDITOR_TOKEN_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r#"(?is)(<!--\s*newpage\s*-->|<!--\s*content-block\s*-->|<h2\b[^>]*>.*?</h2>|<img\b[^>]*>|<hr\s*/?>|<a\b[^>]*class="[^"]*novel-link-card[^"]*"[^>]*>.*?</a>|<div\b[^>]*class="[^"]*missing-image-placeholder[^"]*"[^>]*>.*?</div>)"#,
    )
    .expect("valid editor HTML token regex")
});

static EDITOR_ATTR_RE: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"(?i)([a-z0-9_-]+)="([^"]*)""#).expect("valid editor attribute regex")
});

fn token_attribute(token: &str, name: &str) -> Option<String> {
    EDITOR_ATTR_RE
        .captures_iter(token)
        .find(|capture| {
            capture
                .get(1)
                .map(|value| value.as_str().eq_ignore_ascii_case(name))
                .unwrap_or(false)
        })
        .and_then(|capture| {
            capture
                .get(2)
                .map(|value| decode_editor_entities(value.as_str()))
        })
}

pub(crate) fn html_to_editor_blocks(html: &str, assets: &[AssetEntry]) -> Vec<WorkBlock> {
    if html.trim().is_empty() {
        return plain_text_to_editor_blocks("");
    }
    let mut lines: Vec<SourceLine> = Vec::new();
    let mut cursor = 0;
    let mut after_token = false;

    for matched in EDITOR_TOKEN_RE.find_iter(html) {
        push_fragment_lines(&html[cursor..matched.start()], after_token, &mut lines);
        let token = matched.as_str();
        let lower = token.to_ascii_lowercase();
        if lower.contains("content-block") {
            lines.push(SourceLine::Boundary);
            cursor = matched.end();
            // 区切りそのものは行を占めないが、直前の行はここで閉じている。
            after_token = true;
            continue;
        }
        let block = if lower.starts_with("<!--") {
            editor_block(0, "page_break", None, None, None)
        } else if lower.starts_with("<h2") {
            editor_block(
                0,
                "heading",
                // 見出しもルビを持てる。記法のまま編集画面へ渡す。
                Some(
                    html_fragment_to_text(&restore_inline_notation(token))
                        .trim()
                        .to_string(),
                ),
                None,
                None,
            )
        } else if lower.starts_with("<img") {
            let local_path = token_attribute(token, "data-local-path");
            let asset_id = local_path.as_deref().and_then(|path| {
                assets
                    .iter()
                    .find(|asset| asset.local_path == path || asset.local_path.ends_with(path))
                    .map(|asset| asset.id)
            });
            let alt = token_attribute(token, "alt");
            // 取り込んだ本文の alt は `image_<参照>` という生成物で、読み手に
            // 見せる説明ではない。**参照だけを控えて**、キャプション欄は空に
            // する。控えておかないと、編集した作品を EPUB にしたときに同じ絵を
            // 指し直せず、挿絵が丸ごと落ちる。
            let source_ref = alt
                .as_deref()
                .and_then(|value| value.strip_prefix("image_"))
                .map(str::to_string)
                .or_else(|| {
                    local_path.as_deref().and_then(|path| {
                        Path::new(path)
                            .file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_string)
                    })
                });
            let caption = alt.filter(|value| !value.starts_with("image_"));
            let attrs = source_ref
                .map(|value| attrs_with(None, BLOCK_SOURCE_REF_KEY, serde_json::json!(value)))
                .unwrap_or(None);
            editor_block(0, "image", caption, asset_id, attrs)
        } else if lower.starts_with("<hr") {
            editor_block(0, "separator", None, None, None)
        } else if lower.starts_with("<div") {
            // 取得できなかった挿絵の札。地の文として渡すと、札の文言
            // （「取得できなかった画像 …」）が本文の一部になり、何も触らずに
            // 反映しただけで本文へ書き込まれていた。絵の無い画像ブロックとして
            // 渡し、元の参照を控えておく ―― 書き戻すと元の記法に戻る。
            let attrs = token_attribute(token, "data-image-reference")
                .filter(|value| !value.trim().is_empty())
                .and_then(|value| attrs_with(None, BLOCK_SOURCE_REF_KEY, serde_json::json!(value)));
            let attrs = attrs_with(attrs, BLOCK_MISSING_KEY, serde_json::json!(true));
            editor_block(0, "image", None, None, attrs)
        } else {
            let href = token_attribute(token, "href").unwrap_or_default();
            let label = html_fragment_to_text(token)
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ");
            let attrs = serde_json::json!({ "label": label }).to_string();
            editor_block(0, "link", Some(href), None, Some(attrs))
        };
        lines.push(SourceLine::Token(block));
        cursor = matched.end();
        after_token = true;
    }
    push_fragment_lines(&html[cursor..], after_token, &mut lines);

    let blocks = source_lines_to_blocks(lines);
    if blocks.is_empty() {
        plain_text_to_editor_blocks("")
    } else {
        blocks
    }
}

fn editor_block(
    order: usize,
    block_type: &str,
    text: Option<String>,
    asset_id: Option<i64>,
    attrs_json: Option<String>,
) -> WorkBlock {
    WorkBlock {
        id: 0,
        edit_revision_id: 0,
        order: order as i64,
        block_type: block_type.to_string(),
        text,
        asset_id,
        attrs_json,
    }
}

/// 行の終わりを作るのは `<br />` **だけ**。
///
/// HTML では地の文に書かれた改行は畳める空白でしかなく、行を分けない。
/// 生成時の見た目のための改行まで行の区切りとして数えていたため、1 つの
/// 改行が 2 つに増え、行と空行の区別が最初の一往復で失われていた。
static HTML_LINE_BREAK: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?i)<br\s*/?>|</(?:p|div|h[1-6]|blockquote)>").expect("valid HTML break regex")
});

static HTML_ANY_TAG: std::sync::LazyLock<Regex> =
    std::sync::LazyLock::new(|| Regex::new(r"(?is)<[^>]+>").expect("valid HTML tag regex"));

/// 行の終わりの目印。地の文には現れない制御文字を借りて、畳める改行を
/// 落としきってから本物の改行へ戻す。
const LINE_BREAK_MARK: char = '\u{1}';

static RUBY_HTML: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r"(?is)<ruby>(.*?)<rt>(.*?)</rt></ruby>").expect("valid ruby regex")
});

static JUMP_LINK_HTML: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(
        r#"(?is)<a\b[^>]*\bclass="[^"]*jump-link[^"]*"[^>]*\bdata-page="(\d+)"[^>]*>.*?</a>"#,
    )
    .expect("valid jump link regex")
});

static EXTERNAL_ANCHOR_HTML: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
    Regex::new(r#"(?is)<a\b[^>]*\bhref="(https?://[^"]*)"[^>]*>(.*?)</a>"#)
        .expect("valid anchor regex")
});

/// 開かれた行内記法を、編集画面が扱う記法の形へ戻す。
///
/// タグを落とすだけだったころは、ルビが「漢字かんじ」という地の文に潰れ、
/// 外部リンクは宛先を、ページ内移動は飛び先を失った。**編集画面を一度開く
/// だけで**そうなるので、読み手には何が消えたのかも分からなかった。
fn restore_inline_notation(html: &str) -> String {
    let text = RUBY_HTML.replace_all(html, |captures: &regex::Captures| {
        format!(
            "[[rb:{}>{}]]",
            HTML_ANY_TAG.replace_all(&captures[1], ""),
            HTML_ANY_TAG.replace_all(&captures[2], "")
        )
    });
    let text = JUMP_LINK_HTML.replace_all(&text, "[jump:$1]");
    EXTERNAL_ANCHOR_HTML
        .replace_all(&text, |captures: &regex::Captures| {
            let label = HTML_ANY_TAG.replace_all(&captures[2], "");
            let label = label.trim();
            if label.is_empty() {
                captures[1].to_string()
            } else {
                format!("[[jumpuri:{}>{}]]", label, &captures[1])
            }
        })
        .to_string()
}

fn html_fragment_to_text(fragment: &str) -> String {
    let with_marks = HTML_LINE_BREAK.replace_all(fragment, LINE_BREAK_MARK.to_string().as_str());
    let without_tags = HTML_ANY_TAG.replace_all(&with_marks, "");
    let text: String = without_tags
        .chars()
        .filter(|ch| *ch != '\n' && *ch != '\r')
        .map(|ch| if ch == LINE_BREAK_MARK { '\n' } else { ch })
        .collect();
    // 空行の本数はそのまま残す。ここで詰めると、場面転換のための連続した
    // 空行が編集を通すたびに 1 行へ均されていく。
    decode_editor_entities(&text)
}

fn decode_editor_entities(value: &str) -> String {
    value
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

#[cfg(test)]
#[path = "title_order_tests.rs"]
mod title_order_tests;

#[cfg(test)]
#[path = "editor_roundtrip_tests.rs"]
mod editor_roundtrip_tests;

/// 絞り込みの選択肢を、問いに近い順に並べて `limit` 件に切る。
///
/// `normalized_query` は `normalize_search_text` を通したもの。一致・前方一致・部分一致のあとに、
/// 文字の重なりと編集距離で読みの揺れを拾い、使われている数を少し足す。
pub(crate) fn rank_facets(
    facets: Vec<FacetCount>,
    normalized_query: &str,
    limit: usize,
) -> Vec<FacetCount> {
    let query_grams = query_ngrams(normalized_query);
    let mut scored = facets
        .into_iter()
        .filter_map(|facet| {
            let normalized_name = normalize_search_text(&facet.name);
            let mut score = if normalized_name == normalized_query {
                1000.0
            } else if normalized_name.starts_with(normalized_query) {
                800.0
            } else if normalized_name.contains(normalized_query) {
                650.0
            } else {
                let name_grams = generate_ngrams_limited(&normalized_name, 512);
                let overlap = query_grams
                    .iter()
                    .filter(|gram| name_grams.contains(*gram))
                    .count();
                let ratio = if query_grams.is_empty() {
                    0.0
                } else {
                    overlap as f64 / query_grams.len() as f64
                };
                let fuzzy = normalized_levenshtein(&normalized_name, normalized_query);
                (ratio * 430.0).max(if fuzzy >= 0.72 { fuzzy * 360.0 } else { 0.0 })
            };

            if score <= 0.0 {
                return None;
            }
            score += (facet.count as f64 + 1.0).ln() * 12.0;
            Some((score, facet))
        })
        .collect::<Vec<(f64, FacetCount)>>();

    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| b.1.count.cmp(&a.1.count))
            .then_with(|| a.1.name.cmp(&b.1.name))
    });
    scored.truncate(limit);
    scored.into_iter().map(|(_, facet)| facet).collect()
}
