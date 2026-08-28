//! Writing a database file.
//!
//! Bulk load, not insert. Every save lays the whole file out from scratch:
//! rows are encoded, packed into leaf pages left to right, and interior pages
//! are built over them a level at a time until one page is left. That page is
//! the table's root.
//!
//! Doing it this way is what makes the writer small. An incremental insert has
//! to split a full page, rebalance its siblings, and track free space inside
//! every page it touches; a bulk load never revisits a page it has finished.
//! The cost is that saving is O(rows), which for a file of games is a
//! millisecond and for a million rows would be the wrong design.

use std::io::Write;
use std::path::Path;

use crate::{Db, Error, Value, encode_record, put_varint, varint_len};

/// The page size every file this crate writes uses.
///
/// SQLite's own default since 3.12, and big enough that a history row never
/// reaches an overflow page in practice -- though overflow is implemented, so
/// a long note does not become an error.
const PAGE: usize = 4096;

/// SQLite's own header leaves the first hundred bytes of page one to itself.
const FILE_HEADER: usize = 100;

/// The version number this writer stamps into the header.
///
/// It says which library last wrote the file, and nothing reads it for
/// meaning. 3.45.1 is the version whose format documentation this was built
/// against; a file it writes is format 4, which SQLite has read since 3.3.
const VERSION: u32 = 3_045_001;

struct Layout {
    /// Page images, indexed from zero for page one.
    pages: Vec<[u8; PAGE]>,
}

impl Layout {
    fn new() -> Layout {
        // Page one exists from the start: it is where the schema goes, and
        // SQLite requires the schema to be rooted there.
        Layout {
            pages: vec![[0u8; PAGE]],
        }
    }

    /// Take the next page number, one-based the way SQLite counts.
    fn alloc(&mut self) -> u32 {
        self.pages.push([0u8; PAGE]);
        self.pages.len() as u32
    }

    fn page_mut(&mut self, n: u32) -> &mut [u8; PAGE] {
        &mut self.pages[n as usize - 1]
    }
}

/// One cell waiting to be placed: its bytes, and the rowid that keys it.
struct Cell {
    rowid: i64,
    bytes: Vec<u8>,
}

/// How much of a payload lives on the leaf page itself.
///
/// Straight out of the format's own arithmetic. `max_local` is what fits
/// beside a cell header; past that the payload is cut so that the tail fills
/// overflow pages evenly rather than leaving a nearly empty last one.
fn local_len(payload: usize) -> usize {
    let u = PAGE;
    let max_local = u - 35;
    if payload <= max_local {
        return payload;
    }
    let min_local = ((u - 12) * 32 / 255) - 23;
    let k = min_local + (payload - min_local) % (u - 4);
    if k <= max_local { k } else { min_local }
}

/// Write a payload's tail into a chain of overflow pages and return the first.
fn spill(layout: &mut Layout, tail: &[u8]) -> u32 {
    let first = layout.alloc();
    let mut page = first;
    let mut at = 0;
    loop {
        let room = PAGE - 4;
        let take = room.min(tail.len() - at);
        let next = if at + take < tail.len() {
            Some(layout.alloc())
        } else {
            None
        };
        let buf = layout.page_mut(page);
        buf[0..4].copy_from_slice(&next.unwrap_or(0).to_be_bytes());
        buf[4..4 + take].copy_from_slice(&tail[at..at + take]);
        at += take;
        match next {
            Some(n) => page = n,
            None => break,
        }
    }
    first
}

/// A table-leaf cell: payload size, rowid, the local payload, and -- when the
/// payload did not fit -- the first page of its overflow chain.
fn leaf_cell(layout: &mut Layout, rowid: i64, payload: &[u8]) -> Cell {
    let local = local_len(payload.len());
    let mut bytes = Vec::with_capacity(local + 18);
    put_varint(&mut bytes, payload.len() as i64);
    put_varint(&mut bytes, rowid);
    bytes.extend_from_slice(&payload[..local]);
    if local < payload.len() {
        let first = spill(layout, &payload[local..]);
        bytes.extend_from_slice(&first.to_be_bytes());
    }
    Cell { rowid, bytes }
}

/// Fill a b-tree page with as many cells as fit, and return how many it took.
///
/// `offset` is where the page's own content starts: a hundred bytes in on page
/// one, where the file header sits, and zero everywhere else.
fn fill_page(buf: &mut [u8; PAGE], offset: usize, kind: u8, cells: &[Cell], right: u32) -> usize {
    let header = if kind == 5 { 12 } else { 8 };
    let mut content = PAGE;
    let mut placed = 0usize;
    for cell in cells {
        let need = cell.bytes.len() + 2;
        let used = offset + header + placed * 2;
        if content < need || content - need < used {
            break;
        }
        content -= cell.bytes.len();
        buf[content..content + cell.bytes.len()].copy_from_slice(&cell.bytes);
        let at = offset + header + placed * 2;
        buf[at..at + 2].copy_from_slice(&(content as u16).to_be_bytes());
        placed += 1;
    }

    buf[offset] = kind;
    buf[offset + 1..offset + 3].copy_from_slice(&0u16.to_be_bytes()); // no freeblocks
    buf[offset + 3..offset + 5].copy_from_slice(&(placed as u16).to_be_bytes());
    // A content area starting exactly at the end of a 65536-byte page is
    // written as zero; at 4096 it never is, but the rule is the format's.
    let start = if content == 65536 { 0 } else { content as u16 };
    buf[offset + 5..offset + 7].copy_from_slice(&start.to_be_bytes());
    buf[offset + 7] = 0; // no fragmented free bytes
    if kind == 5 {
        buf[offset + 8..offset + 12].copy_from_slice(&right.to_be_bytes());
    }
    placed
}

/// Build a whole table b-tree and return its root page.
///
/// Leaves first, then a level of interior pages over them, and so on until one
/// page is left. An interior cell holds the page number of a child and the
/// largest rowid in it; the last child of a page is its right-most pointer and
/// has no cell of its own.
fn build_tree(layout: &mut Layout, cells: Vec<Cell>) -> u32 {
    if cells.is_empty() {
        // An empty table is still a b-tree: one leaf page with no cells.
        let page = layout.alloc();
        fill_page(layout.page_mut(page), 0, 13, &[], 0);
        return page;
    }

    // --- leaves
    let mut level: Vec<(u32, i64)> = Vec::new(); // (page, largest rowid in it)
    let mut at = 0;
    while at < cells.len() {
        let page = layout.alloc();
        let took = fill_page(layout.page_mut(page), 0, 13, &cells[at..], 0);
        // A cell that cannot be placed on an empty page cannot be placed
        // anywhere, and `local_len` is what guarantees it fits.
        debug_assert!(took > 0, "a leaf cell did not fit an empty page");
        level.push((page, cells[at + took - 1].rowid));
        at += took;
    }

    // --- interior levels
    while level.len() > 1 {
        let mut up: Vec<(u32, i64)> = Vec::new();
        let mut at = 0;
        while at < level.len() {
            // Everything but the last child of this page gets a cell; the last
            // is the right-most pointer.
            let children: Vec<Cell> = level[at..]
                .iter()
                .map(|(page, rowid)| {
                    let mut bytes = Vec::with_capacity(4 + varint_len(*rowid));
                    bytes.extend_from_slice(&page.to_be_bytes());
                    put_varint(&mut bytes, *rowid);
                    Cell {
                        rowid: *rowid,
                        bytes,
                    }
                })
                .collect();
            let page = layout.alloc();
            // Place cells for all but one child, then point right at the next.
            let took = fill_page(layout.page_mut(page), 0, 5, &children[..children.len() - 1], 0)
                .min(children.len() - 1);
            let right = level[at + took].0;
            let last = level[at + took].1;
            layout.page_mut(page)[8..12].copy_from_slice(&right.to_be_bytes());
            up.push((page, last));
            at += took + 1;
        }
        level = up;
    }
    level[0].0
}

/// Write `db` to `path`, replacing whatever was there.
///
/// The write goes to a sibling file and is renamed over the target, so a save
/// interrupted half way leaves the old database intact rather than a truncated
/// one. That is the same guarantee SQLite's rollback journal gives, reached
/// the only way a whole-file writer can reach it.
pub fn save(db: &Db, path: &Path) -> Result<(), Error> {
    let mut layout = Layout::new();

    // Tables first: the schema records name their root pages, so the roots
    // have to exist before page one can be written.
    let mut schema_rows: Vec<(String, String, u32, String)> = Vec::new();
    for table in &db.tables {
        let cells: Vec<Cell> = table
            .rows
            .iter()
            .map(|row| {
                let payload = encode_record(&row.values);
                leaf_cell(&mut layout, row.rowid, &payload)
            })
            .collect();
        let root = build_tree(&mut layout, cells);
        schema_rows.push((table.name.clone(), table.name.clone(), root, table.sql.clone()));
    }

    // --- page one: the file header, then the schema table's own leaf.
    let schema_cells: Vec<Cell> = schema_rows
        .iter()
        .enumerate()
        .map(|(i, (name, tbl, root, sql))| {
            let values = vec![
                Value::Text("table".into()),
                Value::Text(name.clone()),
                Value::Text(tbl.clone()),
                Value::Int(*root as i64),
                Value::Text(sql.clone()),
            ];
            let payload = encode_record(&values);
            leaf_cell(&mut layout, i as i64 + 1, &payload)
        })
        .collect();
    let placed = fill_page(layout.page_mut(1), FILE_HEADER, 13, &schema_cells, 0);
    if placed < schema_cells.len() {
        return Err(Error::SchemaTooBig);
    }

    let total = layout.pages.len() as u32;
    write_file_header(&mut layout.pages[0], total, db.tables.len() as u32);

    let tmp = path.with_extension("sqlite.tmp");
    if let Some(dir) = tmp.parent()
        && !dir.as_os_str().is_empty()
    {
        std::fs::create_dir_all(dir)?;
    }
    {
        let mut f = std::fs::File::create(&tmp)?;
        for page in &layout.pages {
            f.write_all(page)?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn write_file_header(page: &mut [u8; PAGE], pages: u32, schema_cookie: u32) {
    page[0..16].copy_from_slice(b"SQLite format 3\0");
    page[16..18].copy_from_slice(&(PAGE as u16).to_be_bytes());
    page[18] = 1; // write version: legacy, no WAL
    page[19] = 1; // read version
    page[20] = 0; // no reserved space at the end of a page
    page[21] = 64; // maximum embedded payload fraction, fixed by the format
    page[22] = 32; // minimum
    page[23] = 32; // leaf payload fraction
    page[24..28].copy_from_slice(&1u32.to_be_bytes()); // file change counter
    page[28..32].copy_from_slice(&pages.to_be_bytes());
    page[32..36].copy_from_slice(&0u32.to_be_bytes()); // no freelist
    page[36..40].copy_from_slice(&0u32.to_be_bytes());
    // The schema cookie changes whenever the schema does, and a reader caches
    // against it. Rewriting the file with a different number of tables is a
    // schema change; using the count keeps it honest without keeping state.
    page[40..44].copy_from_slice(&schema_cookie.to_be_bytes());
    page[44..48].copy_from_slice(&4u32.to_be_bytes()); // schema format 4
    page[48..52].copy_from_slice(&0u32.to_be_bytes()); // suggested cache size
    page[52..56].copy_from_slice(&0u32.to_be_bytes()); // not auto-vacuum
    page[56..60].copy_from_slice(&1u32.to_be_bytes()); // text encoding: UTF-8
    page[60..64].copy_from_slice(&0u32.to_be_bytes()); // user version
    page[64..68].copy_from_slice(&0u32.to_be_bytes()); // no incremental vacuum
    page[68..72].copy_from_slice(&0u32.to_be_bytes()); // application id
    page[72..92].fill(0); // reserved for expansion
    page[92..96].copy_from_slice(&1u32.to_be_bytes()); // version-valid-for
    page[96..100].copy_from_slice(&VERSION.to_be_bytes());
}
