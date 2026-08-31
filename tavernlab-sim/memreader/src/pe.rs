//! Parsing a PE (`.dll`) file's export table well enough to find the RVA
//! of one named function — `mono_get_root_domain`. Reads only the bytes of
//! the file already on disk; nothing about this touches a running process.
//!
//! This is deliberately narrow: enough of the PE/COFF format to walk
//! `IMAGE_EXPORT_DIRECTORY`, not a general PE parser.

use std::collections::HashMap;

fn u16_at(b: &[u8], off: usize) -> Option<u16> {
    b.get(off..off + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap()))
}

fn u32_at(b: &[u8], off: usize) -> Option<u32> {
    b.get(off..off + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
}

fn cstr_at(b: &[u8], off: usize) -> Option<String> {
    let start = b.get(off..)?;
    let end = start.iter().position(|&c| c == 0)?;
    String::from_utf8(start[..end].to_vec()).ok()
}

/// Every exported symbol name mapped to its RVA (address relative to the
/// image base, which is exactly what a normally loaded module's mapping
/// base in `/proc/PID/maps` corresponds to).
pub fn parse_exports(file: &[u8]) -> Result<HashMap<String, u32>, String> {
    if file.get(0..2) != Some(b"MZ") {
        return Err("no MZ signature — not a PE file".into());
    }
    let e_lfanew = u32_at(file, 0x3c).ok_or("truncated DOS header")? as usize;
    if file.get(e_lfanew..e_lfanew + 4) != Some(b"PE\0\0") {
        return Err("no PE signature at e_lfanew".into());
    }

    let coff = e_lfanew + 4;
    let machine = u16_at(file, coff).ok_or("truncated COFF header")?;
    // IMAGE_FILE_MACHINE_AMD64 = 0x8664. Every offset below assumes the
    // 64-bit optional header layout (PE32+); a 32-bit DLL would need
    // different optional-header offsets from here down.
    if machine != 0x8664 {
        return Err(format!(
            "expected a 64-bit (AMD64) PE, machine field was 0x{machine:x}"
        ));
    }
    let size_of_optional_header = u16_at(file, coff + 16).ok_or("truncated COFF header")?;
    let optional_header = coff + 20;

    // IMAGE_OPTIONAL_HEADER64.DataDirectory[IMAGE_DIRECTORY_ENTRY_EXPORT]
    // sits at a fixed offset from the start of the optional header for
    // PE32+; verified by checking the magic first.
    let magic = u16_at(file, optional_header).ok_or("truncated optional header")?;
    if magic != 0x20b {
        return Err(format!(
            "expected PE32+ optional header magic 0x20b, got 0x{magic:x}"
        ));
    }
    if (size_of_optional_header as usize) < 112 {
        return Err("optional header too small to hold a data directory".into());
    }
    let data_directory = optional_header + 112;
    let export_rva = u32_at(file, data_directory).ok_or("truncated data directory")? as usize;
    let export_size = u32_at(file, data_directory + 4).ok_or("truncated data directory")?;
    if export_rva == 0 || export_size == 0 {
        return Err("no export directory — DLL exports nothing".into());
    }

    // A normally loaded PE image is mapped with RVA == in-memory offset
    // from the module base, which usually also holds for a plain read of
    // the file for the header/rdata region layout of a typical DLL built
    // without unusual section alignment. If this parse comes back empty
    // for a real DLL, that assumption — not the export-walk logic below —
    // is the first thing to re-check.
    let d = export_rva;
    let number_of_names = u32_at(file, d + 0x18).ok_or("truncated export directory")?;
    let address_of_functions = u32_at(file, d + 0x1c).ok_or("truncated export directory")? as usize;
    let address_of_names = u32_at(file, d + 0x20).ok_or("truncated export directory")? as usize;
    let address_of_name_ordinals =
        u32_at(file, d + 0x24).ok_or("truncated export directory")? as usize;

    let mut out = HashMap::new();
    for i in 0..number_of_names {
        let name_rva = u32_at(file, address_of_names + 4 * i as usize)
            .ok_or("truncated names table")? as usize;
        let name = cstr_at(file, name_rva).ok_or("truncated export name")?;
        let ordinal = u16_at(file, address_of_name_ordinals + 2 * i as usize)
            .ok_or("truncated ordinals table")?;
        let func_rva = u32_at(file, address_of_functions + 4 * ordinal as usize)
            .ok_or("truncated functions table")?;
        out.insert(name, func_rva);
    }
    Ok(out)
}
