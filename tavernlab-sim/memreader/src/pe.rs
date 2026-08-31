//! Parsing a PE (`.dll`) file's export table well enough to find the RVA
//! of one named function — `mono_get_root_domain`. Reads only the bytes of
//! the file already on disk; nothing about this touches a running process.
//!
//! This is deliberately narrow: enough of the PE/COFF format to walk
//! `IMAGE_EXPORT_DIRECTORY`, not a general PE parser.
//!
//! One wrinkle a naive read gets wrong: RVA (offset from the image base
//! *once loaded*) is not the same number as the byte offset *in the file
//! on disk*, whenever a section's `FileAlignment` differs from its
//! `SectionAlignment` (routine, and true of this DLL — the first attempt
//! at this file assumed RVA==file-offset and choked on it). Every RVA
//! here is translated through the section table before it is used to
//! index into `file`.

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

struct Section {
    virtual_address: u32,
    virtual_size: u32,
    pointer_to_raw_data: u32,
}

fn rva_to_file_offset(sections: &[Section], rva: u32) -> Option<usize> {
    for s in sections {
        if rva >= s.virtual_address && rva < s.virtual_address + s.virtual_size {
            return Some((s.pointer_to_raw_data + (rva - s.virtual_address)) as usize);
        }
    }
    None
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
    if machine != 0x8664 {
        return Err(format!(
            "expected a 64-bit (AMD64) PE, machine field was 0x{machine:x}"
        ));
    }
    let number_of_sections = u16_at(file, coff + 2).ok_or("truncated COFF header")?;
    let size_of_optional_header = u16_at(file, coff + 16).ok_or("truncated COFF header")?;
    let optional_header = coff + 20;

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
    let export_rva = u32_at(file, data_directory).ok_or("truncated data directory")?;
    let export_dir_size = u32_at(file, data_directory + 4).ok_or("truncated data directory")?;
    if export_rva == 0 || export_dir_size == 0 {
        return Err("no export directory — DLL exports nothing".into());
    }

    // Section table follows the optional header.
    let section_table = optional_header + size_of_optional_header as usize;
    let mut sections = Vec::with_capacity(number_of_sections as usize);
    for i in 0..number_of_sections as usize {
        let base = section_table + i * 40;
        let virtual_size = u32_at(file, base + 8).ok_or("truncated section header")?;
        let virtual_address = u32_at(file, base + 12).ok_or("truncated section header")?;
        let pointer_to_raw_data = u32_at(file, base + 20).ok_or("truncated section header")?;
        sections.push(Section { virtual_address, virtual_size, pointer_to_raw_data });
    }

    let d = rva_to_file_offset(&sections, export_rva)
        .ok_or("export directory RVA falls outside every section")?;
    let number_of_names = u32_at(file, d + 0x18).ok_or("truncated export directory")?;
    let address_of_functions_rva =
        u32_at(file, d + 0x1c).ok_or("truncated export directory")?;
    let address_of_names_rva = u32_at(file, d + 0x20).ok_or("truncated export directory")?;
    let address_of_name_ordinals_rva =
        u32_at(file, d + 0x24).ok_or("truncated export directory")?;

    let address_of_functions = rva_to_file_offset(&sections, address_of_functions_rva)
        .ok_or("AddressOfFunctions RVA outside every section")?;
    let address_of_names = rva_to_file_offset(&sections, address_of_names_rva)
        .ok_or("AddressOfNames RVA outside every section")?;
    let address_of_name_ordinals = rva_to_file_offset(&sections, address_of_name_ordinals_rva)
        .ok_or("AddressOfNameOrdinals RVA outside every section")?;

    let mut out = HashMap::new();
    for i in 0..number_of_names {
        let name_rva = u32_at(file, address_of_names + 4 * i as usize)
            .ok_or("truncated names table")?;
        let name_off = rva_to_file_offset(&sections, name_rva)
            .ok_or_else(|| format!("export name #{i} RVA 0x{name_rva:x} outside every section"))?;
        let name = cstr_at(file, name_off).ok_or("truncated export name")?;
        let ordinal = u16_at(file, address_of_name_ordinals + 2 * i as usize)
            .ok_or("truncated ordinals table")?;
        // AddressOfFunctions is an array of RVAs, in file bytes (the table
        // itself was just translated to a file offset above; each 4-byte
        // entry in it is a plain RVA value, not itself needing translation
        // until the caller wants to dereference it).
        let func_rva = u32_at(file, address_of_functions + 4 * ordinal as usize)
            .ok_or("truncated functions table")?;
        out.insert(name, func_rva);
    }
    Ok(out)
}
