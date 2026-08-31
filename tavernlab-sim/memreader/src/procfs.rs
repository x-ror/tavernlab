//! Finding the process and its memory map through `/proc` — ordinary file
//! reads, nothing process-memory-specific about this file.

use std::collections::HashMap;
use std::fs;

/// Find a running process by (case-insensitive, substring) executable name,
/// the way `ps aux | grep -i` would.
pub fn find_pid_by_name(name_substr: &str) -> Option<u32> {
    let needle = name_substr.to_lowercase();
    let entries = fs::read_dir("/proc").ok()?;
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(pid_str) = file_name.to_str() else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else { continue };
        let cmdline = fs::read_to_string(format!("/proc/{pid}/cmdline")).unwrap_or_default();
        let cmdline = cmdline.replace('\0', " ");
        if cmdline.to_lowercase().contains(&needle) {
            return Some(pid);
        }
    }
    None
}

/// One line of `/proc/PID/maps`.
#[allow(dead_code)] // end/file_offset: kept for the next reader that needs section-level detail
pub struct MapEntry {
    pub start: u64,
    pub end: u64,
    /// Offset into the backing file this mapping starts at.
    pub file_offset: u64,
    /// `rwxp`/`rwxs` etc. as the kernel wrote it -- kept so a caller can
    /// tell a writable heap-like region from a read-only mapped file
    /// without re-parsing the line.
    pub perms: String,
    /// Empty for anonymous mappings (no backing file) -- exactly the
    /// regions a GC'd heap lives in, so *not* skipped here the way an
    /// earlier version of this reader did (it only ever needed named
    /// modules).
    pub pathname: String,
}

/// Every mapped region, in the order the kernel lists them (ascending by
/// address). A file backing several regions (one per PE section, typically)
/// appears as several entries with the same `pathname`; an anonymous
/// region (heap, stack, `mmap(MAP_ANONYMOUS)`) appears with an empty one.
pub fn read_maps(pid: u32) -> std::io::Result<Vec<MapEntry>> {
    let text = fs::read_to_string(format!("/proc/{pid}/maps"))?;
    let mut out = Vec::new();
    for line in text.lines() {
        // "55a1b2c00000-55a1b2c21000 r--p 00000000 08:01 123456  /path/to/file"
        let mut parts = line.splitn(6, char::is_whitespace).filter(|s| !s.is_empty());
        let Some(range) = parts.next() else { continue };
        let Some((start_s, end_s)) = range.split_once('-') else { continue };
        let (Ok(start), Ok(end)) = (
            u64::from_str_radix(start_s, 16),
            u64::from_str_radix(end_s, 16),
        ) else {
            continue;
        };
        let perms = parts.next().unwrap_or("").to_string();
        let Some(offset_s) = parts.next() else { continue };
        let Ok(file_offset) = u64::from_str_radix(offset_s, 16) else { continue };
        let _dev = parts.next();
        let _inode = parts.next();
        let pathname = parts.next().unwrap_or("").trim().to_string();
        out.push(MapEntry { start, end, file_offset, perms, pathname });
    }
    Ok(out)
}

/// Group mapped regions by path, keeping the lowest start address for each
/// — the header page, which is what a PE image's RVAs are relative to.
#[allow(dead_code)]
pub fn module_bases(entries: &[MapEntry]) -> HashMap<String, u64> {
    let mut out: HashMap<String, u64> = HashMap::new();
    for e in entries {
        out.entry(e.pathname.clone())
            .and_modify(|base| *base = (*base).min(e.start))
            .or_insert(e.start);
    }
    out
}
