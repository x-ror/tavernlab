//! Reading a database file.
//!
//! The general case, unlike the writer: this has to cope with a file SQLite
//! wrote, which means interior pages, overflow chains, any page size, and
//! reserved space at the end of every page. What it does not do is indexes --
//! an index b-tree in the schema is skipped, because nothing here needs one
//! and half-reading it would be worse than not reading it.

use std::path::Path;

use crate::{Db, Error, Row, Table, Value, columns, decode_record, get_varint};

struct File {
    bytes: Vec<u8>,
    page_size: usize,
    /// Bytes at the end of every page that hold no b-tree content.
    reserved: usize,
}

impl File {
    fn page(&self, n: u32) -> Result<&[u8], Error> {
        if n == 0 {
            return Err(Error::Corrupt("page zero does not exist"));
        }
        let at = (n as usize - 1) * self.page_size;
        self.bytes
            .get(at..at + self.page_size)
            .ok_or(Error::Corrupt("page number past the end of the file"))
    }

    /// The b-tree content of a page: page one starts a hundred bytes in, and
    /// any page may end with reserved bytes.
    fn usable(&self, n: u32) -> Result<(&[u8], usize), Error> {
        let page = self.page(n)?;
        let end = self.page_size - self.reserved;
        let offset = if n == 1 { 100 } else { 0 };
        Ok((&page[..end], offset))
    }
}

fn be16(b: &[u8], at: usize) -> usize {
    ((b[at] as usize) << 8) | b[at + 1] as usize
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// Walk a table b-tree rooted at `root`, appending every row it holds.
///
/// Depth-first and left to right, which is rowid order -- the same order
/// `SELECT * FROM t` gives without an `ORDER BY`.
fn walk(file: &File, root: u32, out: &mut Vec<Row>, depth: usize) -> Result<(), Error> {
    // A b-tree deeper than this in a file of games is a loop, not a tree, and
    // following it would not terminate.
    if depth > 32 {
        return Err(Error::Corrupt("b-tree deeper than any real one"));
    }
    let (page, offset) = file.usable(root)?;
    let Some(&kind) = page.get(offset) else {
        return Err(Error::Corrupt("page header runs past the page"));
    };
    let header = match kind {
        13 => 8,
        5 => 12,
        // An index b-tree, or a page this crate has no reader for.
        2 | 10 => return Ok(()),
        _ => return Err(Error::Corrupt("not a b-tree page")),
    };
    if offset + header + 2 > page.len() {
        return Err(Error::Corrupt("page header runs past the page"));
    }
    let cells = be16(page, offset + 3);

    for i in 0..cells {
        let at = offset + header + i * 2;
        if at + 2 > page.len() {
            return Err(Error::Corrupt("cell pointer runs past the page"));
        }
        let cell = be16(page, at);
        if cell >= page.len() {
            return Err(Error::Corrupt("cell starts past the page"));
        }
        if kind == 5 {
            let child = be32(page, cell);
            walk(file, child, out, depth + 1)?;
        } else {
            let (payload_len, a) = get_varint(&page[cell..])?;
            let (rowid, b) = get_varint(&page[cell + a..])?;
            let start = cell + a + b;
            let payload = read_payload(file, page, start, payload_len as usize)?;
            out.push(Row {
                rowid,
                values: decode_record(&payload)?,
            });
        }
    }
    if kind == 5 {
        let right = be32(page, offset + 8);
        walk(file, right, out, depth + 1)?;
    }
    Ok(())
}

/// A cell's payload, following the overflow chain when it has one.
fn read_payload(file: &File, page: &[u8], start: usize, total: usize) -> Result<Vec<u8>, Error> {
    let usable = file.page_size - file.reserved;
    let max_local = usable - 35;
    let local = if total <= max_local {
        total
    } else {
        let min_local = ((usable - 12) * 32 / 255) - 23;
        let k = min_local + (total - min_local) % (usable - 4);
        if k <= max_local { k } else { min_local }
    };
    if start + local > page.len() {
        return Err(Error::Corrupt("payload runs past the page"));
    }
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&page[start..start + local]);
    if local == total {
        return Ok(out);
    }
    if start + local + 4 > page.len() {
        return Err(Error::Corrupt("overflow pointer runs past the page"));
    }
    let mut next = be32(page, start + local);
    let mut guard = 0;
    while next != 0 && out.len() < total {
        guard += 1;
        if guard > file.bytes.len() / file.page_size + 1 {
            return Err(Error::Corrupt("overflow chain loops"));
        }
        let p = file.page(next)?;
        let room = usable - 4;
        let take = room.min(total - out.len());
        out.extend_from_slice(&p[4..4 + take]);
        next = be32(p, 0);
    }
    if out.len() != total {
        return Err(Error::Corrupt("overflow chain ended early"));
    }
    Ok(out)
}

/// Put back what the storage format left out.
///
/// Two things a record does not carry, both of which SQLite reconstructs from
/// the schema when it reads:
///
///   * an `INTEGER PRIMARY KEY` column is the rowid, and is stored as NULL in
///     every record rather than written twice. A reader that skipped this
///     would hand back a table whose ids are all missing;
///   * a float with no fractional part is stored as an integer to save the
///     bytes, and the column's REAL affinity is what turns it back. Without
///     this, `0.0` and `2.0` come back as integers while `0.5` comes back as
///     a float, from the same column.
fn apply_affinity(sql: &str, rows: &mut [Row]) {
    let cols = columns(sql);
    if !cols.iter().any(|c| c.rowid_alias || c.real_affinity) {
        return;
    }
    for row in rows.iter_mut() {
        for (i, col) in cols.iter().enumerate() {
            let Some(v) = row.values.get_mut(i) else {
                break;
            };
            if col.rowid_alias && *v == Value::Null {
                *v = Value::Int(row.rowid);
            } else if col.real_affinity
                && let Value::Int(n) = *v
            {
                *v = Value::Real(n as f64);
            }
        }
    }
}

/// Read the database at `path`, or an empty one if the file is not there.
///
/// A missing file is not an error: the first game played is what creates the
/// history, and asking the caller to handle "no file yet" separately would put
/// the same `if` at every call site.
pub fn open(path: &Path) -> Result<Db, Error> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Db::new()),
        Err(e) => return Err(Error::Io(e)),
    };
    // A file SQLite has created but not yet written a page to is zero bytes,
    // and it means the same thing as no file.
    if bytes.is_empty() {
        return Ok(Db::new());
    }
    if bytes.len() < 100 || !bytes.starts_with(b"SQLite format 3\0") {
        return Err(Error::NotSqlite);
    }
    let page_size = match be16(&bytes, 16) {
        1 => 65536, // the format's own escape for a size that will not fit
        n if n >= 512 && n.is_power_of_two() => n,
        _ => return Err(Error::Unsupported("page size")),
    };
    if bytes.len() % page_size != 0 {
        return Err(Error::Corrupt("file is not a whole number of pages"));
    }
    if be32(&bytes, 56) != 1 {
        // UTF-16 databases exist and this reader would hand back mojibake.
        return Err(Error::Unsupported("text encoding is not UTF-8"));
    }
    let reserved = bytes[20] as usize;
    if reserved >= page_size {
        return Err(Error::Corrupt("reserved space fills the page"));
    }
    let file = File {
        bytes,
        page_size,
        reserved,
    };

    let mut schema = Vec::new();
    walk(&file, 1, &mut schema, 0)?;

    let mut db = Db::new();
    for row in schema {
        // (type, name, tbl_name, rootpage, sql)
        if row.get(0).as_str() != Some("table") {
            continue; // an index, a view, a trigger
        }
        let Some(name) = row.get(1).as_str() else {
            continue;
        };
        // SQLite's own bookkeeping tables are not the caller's data.
        if name.starts_with("sqlite_") {
            continue;
        }
        let Some(root) = row.get(3).as_i64() else {
            continue;
        };
        let sql = row.get(4).as_str().unwrap_or("").to_string();
        let mut rows = Vec::new();
        if root > 0 {
            walk(&file, root as u32, &mut rows, 0)?;
        }
        apply_affinity(&sql, &mut rows);
        db.tables.push(Table {
            name: name.to_string(),
            sql,
            rows,
        });
    }
    Ok(db)
}
