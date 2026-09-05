//! A SQLite database file, written and read without a dependency.
//!
//! The same bargain `tavernlab-json` makes. The workspace takes no third-party
//! crates -- the point of that rule is that the binary *cannot* wander off,
//! and `rusqlite` would bring a C library and a build script with it -- but
//! "your games live in a local SQLite file you can copy" is a promise about
//! the file, not about how it was produced. So the file format is implemented
//! here, and `sqlite3 tavernlab.sqlite "select * from games"` has to work on
//! it or this crate is wrong.
//!
//! **What is implemented.** Table b-trees with integer keys: `CREATE TABLE`,
//! rows, all five storage classes, and payloads of any size through overflow
//! chains. That is the whole of what a history file needs.
//!
//! **What is not.** Indexes, `WITHOUT ROWID` tables, views, triggers,
//! incremental update, the freelist, and every pragma. A save rewrites the
//! whole file from the rows it was handed -- history is thousands of rows, not
//! millions, and a bulk-loaded b-tree needs no page splitting, no rebalancing
//! and no free space to track. Reading is the general case and handles a file
//! this crate did not write, as long as its tables are ordinary rowid tables;
//! an index in the file is skipped rather than misread.
//!
//! A file written here is a legal database, not a special one: SQLite will
//! read it, add indexes to it, and vacuum it, and this crate will go on
//! reading what SQLite writes.

mod read;
mod write;

pub use read::open;
pub use write::save;

use std::fmt;

/// One SQLite value, in the five storage classes the format has.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            Value::Real(f) => Some(*f as i64),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
}

impl From<i64> for Value {
    fn from(n: i64) -> Value {
        Value::Int(n)
    }
}

impl From<&str> for Value {
    fn from(s: &str) -> Value {
        Value::Text(s.to_string())
    }
}

impl From<String> for Value {
    fn from(s: String) -> Value {
        Value::Text(s)
    }
}

/// One row: its rowid, and the values of its columns in declared order.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub rowid: i64,
    pub values: Vec<Value>,
}

impl Row {
    /// The value of column `i`, or `Value::Null` for a row that is short.
    ///
    /// A row written before a column was added really is short -- SQLite's own
    /// `ALTER TABLE ADD COLUMN` does not rewrite the rows that came before it,
    /// and reads them back as the default. `Null` is that default here.
    pub fn get(&self, i: usize) -> &Value {
        self.values.get(i).unwrap_or(&Value::Null)
    }
}

/// One table: the `CREATE TABLE` that declares it, and its rows.
#[derive(Clone, Debug)]
pub struct Table {
    pub name: String,
    /// The statement verbatim, as `sqlite_schema.sql` holds it. This crate
    /// never parses it beyond the column names -- it is what `sqlite3 .schema`
    /// prints, and what tells a later reader what the columns mean.
    pub sql: String,
    pub rows: Vec<Row>,
}

impl Table {
    pub fn new(name: &str, sql: &str) -> Table {
        Table {
            name: name.to_string(),
            sql: sql.to_string(),
            rows: Vec::new(),
        }
    }

    /// One past the largest rowid, which is what SQLite hands the next insert.
    pub fn next_rowid(&self) -> i64 {
        self.rows.iter().map(|r| r.rowid).max().unwrap_or(0) + 1
    }

    /// Append a row, giving it the next rowid.
    pub fn push(&mut self, values: Vec<Value>) -> i64 {
        let rowid = self.next_rowid();
        self.rows.push(Row { rowid, values });
        rowid
    }
}

/// A whole database, held in memory.
#[derive(Clone, Debug, Default)]
pub struct Db {
    pub tables: Vec<Table>,
}

impl Db {
    pub fn new() -> Db {
        Db::default()
    }

    pub fn table(&self, name: &str) -> Option<&Table> {
        self.tables.iter().find(|t| t.name == name)
    }

    pub fn table_mut(&mut self, name: &str) -> Option<&mut Table> {
        self.tables.iter_mut().find(|t| t.name == name)
    }

    /// The named table, created from `sql` if the file did not have it.
    pub fn ensure(&mut self, name: &str, sql: &str) -> &mut Table {
        if self.table(name).is_none() {
            self.tables.push(Table::new(name, sql));
        }
        self.table_mut(name).expect("just ensured")
    }
}

/// What went wrong reading or writing.
#[derive(Debug)]
pub enum Error {
    /// The file does not start with SQLite's magic string.
    NotSqlite,
    /// A header field this crate cannot honour: a page size that is not a
    /// power of two in range, a text encoding other than UTF-8, reserved
    /// space at the end of every page.
    Unsupported(&'static str),
    /// The file is a SQLite database but disagrees with itself.
    Corrupt(&'static str),
    /// More schema than fits on page one. Page one is where SQLite requires
    /// the schema to start, and this crate does not split it across a b-tree.
    /// It is thousands of tables away from a history file.
    SchemaTooBig,
    Io(std::io::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NotSqlite => write!(f, "not a SQLite database"),
            Error::Unsupported(what) => write!(f, "unsupported SQLite file: {what}"),
            Error::Corrupt(what) => write!(f, "corrupt SQLite database: {what}"),
            Error::SchemaTooBig => write!(f, "too many tables to fit the schema on page one"),
            Error::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Error {
        Error::Io(e)
    }
}

// ------------------------------------------------------------------ varints

/// SQLite's variable-length integer: big-endian, seven bits per byte, high
/// bit set on every byte but the last, and a ninth byte that contributes all
/// eight of its bits.
pub(crate) fn put_varint(out: &mut Vec<u8>, v: i64) {
    let v = v as u64;
    if v <= 0x7f {
        out.push(v as u8);
        return;
    }
    if v > 0x00ff_ffff_ffff_ffff {
        // Nine bytes: eight groups of seven, then a whole byte.
        let mut buf = [0u8; 9];
        buf[8] = v as u8;
        let mut rest = v >> 8;
        for i in (0..8).rev() {
            buf[i] = (rest as u8 & 0x7f) | 0x80;
            rest >>= 7;
        }
        out.extend_from_slice(&buf);
        return;
    }
    let mut buf = [0u8; 9];
    let mut n = 0;
    let mut rest = v;
    while rest > 0 {
        buf[n] = rest as u8 & 0x7f;
        rest >>= 7;
        n += 1;
    }
    for i in (0..n).rev() {
        let last = i == 0;
        out.push(buf[i] | if last { 0 } else { 0x80 });
    }
}

/// Read a varint, returning it and how many bytes it took.
pub(crate) fn get_varint(b: &[u8]) -> Result<(i64, usize), Error> {
    let mut v: u64 = 0;
    for i in 0..8 {
        let Some(&byte) = b.get(i) else {
            return Err(Error::Corrupt("varint runs past the end of the page"));
        };
        if byte < 0x80 {
            return Ok((((v << 7) | byte as u64) as i64, i + 1));
        }
        v = (v << 7) | (byte & 0x7f) as u64;
    }
    let Some(&byte) = b.get(8) else {
        return Err(Error::Corrupt("varint runs past the end of the page"));
    };
    Ok((((v << 8) | byte as u64) as i64, 9))
}

pub(crate) fn varint_len(v: i64) -> usize {
    let mut buf = Vec::with_capacity(9);
    put_varint(&mut buf, v);
    buf.len()
}

// ------------------------------------------------------------------ records

/// Encode one row's values in SQLite's record format: a header of serial
/// types, then the bodies in the same order.
pub(crate) fn encode_record(values: &[Value]) -> Vec<u8> {
    let mut types: Vec<i64> = Vec::with_capacity(values.len());
    let mut body: Vec<u8> = Vec::new();
    for v in values {
        match v {
            Value::Null => types.push(0),
            Value::Int(n) => {
                // The smallest of SQLite's six integer widths that holds it,
                // and the two constants that need no body at all.
                let n = *n;
                if n == 0 {
                    types.push(8);
                } else if n == 1 {
                    types.push(9);
                } else if (i8::MIN as i64..=i8::MAX as i64).contains(&n) {
                    types.push(1);
                    body.push(n as u8);
                } else if (i16::MIN as i64..=i16::MAX as i64).contains(&n) {
                    types.push(2);
                    body.extend_from_slice(&(n as i16).to_be_bytes());
                } else if (-(1 << 23)..(1 << 23)).contains(&n) {
                    types.push(3);
                    body.extend_from_slice(&n.to_be_bytes()[5..]);
                } else if (i32::MIN as i64..=i32::MAX as i64).contains(&n) {
                    types.push(4);
                    body.extend_from_slice(&(n as i32).to_be_bytes());
                } else if (-(1 << 47)..(1 << 47)).contains(&n) {
                    types.push(5);
                    body.extend_from_slice(&n.to_be_bytes()[2..]);
                } else {
                    types.push(6);
                    body.extend_from_slice(&n.to_be_bytes());
                }
            }
            Value::Real(f) => {
                types.push(7);
                body.extend_from_slice(&f.to_be_bytes());
            }
            Value::Text(s) => {
                types.push(13 + 2 * s.len() as i64);
                body.extend_from_slice(s.as_bytes());
            }
            Value::Blob(b) => {
                types.push(12 + 2 * b.len() as i64);
                body.extend_from_slice(b);
            }
        }
    }

    let mut serials: Vec<u8> = Vec::new();
    for t in &types {
        put_varint(&mut serials, *t);
    }
    // The header size counts itself, which makes it a fixed point: adding a
    // byte to the length can push the length itself over a varint boundary.
    let mut header_len = serials.len() + 1;
    while varint_len(header_len as i64) + serials.len() != header_len {
        header_len = varint_len(header_len as i64) + serials.len();
    }

    let mut out = Vec::with_capacity(header_len + body.len());
    put_varint(&mut out, header_len as i64);
    out.extend_from_slice(&serials);
    out.extend_from_slice(&body);
    out
}

/// Decode a record into its values.
pub(crate) fn decode_record(payload: &[u8]) -> Result<Vec<Value>, Error> {
    let (header_len, n) = get_varint(payload)?;
    let header_len = header_len as usize;
    if header_len > payload.len() || header_len < n {
        return Err(Error::Corrupt("record header longer than the record"));
    }
    let mut at = n;
    let mut types: Vec<i64> = Vec::new();
    while at < header_len {
        let (t, used) = get_varint(&payload[at..header_len])?;
        types.push(t);
        at += used;
    }

    let mut body = header_len;
    let mut out = Vec::with_capacity(types.len());
    let take = |body: &mut usize, n: usize| -> Result<&[u8], Error> {
        let end = body.checked_add(n).unwrap_or(usize::MAX);
        if end > payload.len() {
            return Err(Error::Corrupt("record body runs past the record"));
        }
        let s = &payload[*body..end];
        *body = end;
        Ok(s)
    };
    for t in types {
        let v = match t {
            0 => Value::Null,
            1 => Value::Int(take(&mut body, 1)?[0] as i8 as i64),
            2 => {
                let b = take(&mut body, 2)?;
                Value::Int(i16::from_be_bytes([b[0], b[1]]) as i64)
            }
            3 => {
                let b = take(&mut body, 3)?;
                // Sign-extend twenty-four bits by hand.
                let mut n = ((b[0] as i64) << 16) | ((b[1] as i64) << 8) | b[2] as i64;
                if n & 0x0080_0000 != 0 {
                    n -= 1 << 24;
                }
                Value::Int(n)
            }
            4 => {
                let b = take(&mut body, 4)?;
                Value::Int(i32::from_be_bytes([b[0], b[1], b[2], b[3]]) as i64)
            }
            5 => {
                let b = take(&mut body, 6)?;
                let mut n: i64 = 0;
                for &byte in b {
                    n = (n << 8) | byte as i64;
                }
                if n & 0x0000_8000_0000_0000 != 0 {
                    n -= 1 << 48;
                }
                Value::Int(n)
            }
            6 => {
                let b = take(&mut body, 8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                Value::Int(i64::from_be_bytes(a))
            }
            7 => {
                let b = take(&mut body, 8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                Value::Real(f64::from_be_bytes(a))
            }
            8 => Value::Int(0),
            9 => Value::Int(1),
            10 | 11 => return Err(Error::Corrupt("reserved serial type")),
            t if t % 2 == 0 => Value::Blob(take(&mut body, (t as usize - 12) / 2)?.to_vec()),
            t => {
                let raw = take(&mut body, (t as usize - 13) / 2)?;
                // A database written by something else may hold bytes that are
                // not UTF-8 even though the header says the encoding is. Losing
                // the row would be worse than losing the byte.
                Value::Text(String::from_utf8_lossy(raw).into_owned())
            }
        };
        out.push(v);
    }
    Ok(out)
}

/// One declared column of a `CREATE TABLE`.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    pub name: String,
    /// True for `INTEGER PRIMARY KEY`, which SQLite makes an alias for the
    /// rowid: such a column is stored as NULL in every record, and the value
    /// you get back is the rowid. A reader that did not know this would hand
    /// back a table of NULL ids.
    pub rowid_alias: bool,
    /// True when the declared type gives the column REAL affinity, which is
    /// how a value stored as an integer reads back as a float. SQLite stores
    /// a float with no fractional part as an integer to save the bytes, and
    /// the affinity is what undoes that on the way out.
    pub real_affinity: bool,
}

/// The declared columns of a `CREATE TABLE`, in order.
///
/// Enough of a parser for what a reader needs: the text between the outermost
/// parentheses, split on commas that are not inside parentheses, and the first
/// identifier of each part. A table-level `PRIMARY KEY (a, b)` or a
/// `CHECK (...)` is dropped rather than mistaken for a column.
pub fn columns(sql: &str) -> Vec<Column> {
    let Some(open) = sql.find('(') else {
        return Vec::new();
    };
    let Some(close) = sql.rfind(')') else {
        return Vec::new();
    };
    if close <= open {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut depth = 0usize;
    // Whichever quote is open, and what closes it. A comma inside one is part
    // of a default or a check, not the end of a column -- `DEFAULT 'a,b'` is
    // the case that found this.
    let mut quote: Option<char> = None;
    let mut part = String::new();
    for c in sql[open + 1..close].chars() {
        if let Some(closer) = quote {
            part.push(c);
            if c == closer {
                quote = None;
            }
            continue;
        }
        match c {
            '\'' | '"' | '`' => {
                quote = Some(c);
                part.push(c);
            }
            '[' => {
                quote = Some(']');
                part.push(c);
            }
            '(' => {
                depth += 1;
                part.push(c);
            }
            ')' => {
                depth = depth.saturating_sub(1);
                part.push(c);
            }
            ',' if depth == 0 => {
                push_column(&mut out, &part);
                part.clear();
            }
            _ => part.push(c),
        }
    }
    push_column(&mut out, &part);
    out
}

/// Just the names, for a caller that only wants to label what it read.
pub fn column_names(sql: &str) -> Vec<String> {
    columns(sql).into_iter().map(|c| c.name).collect()
}

fn push_column(out: &mut Vec<Column>, part: &str) {
    let Some(first) = part.split_whitespace().next() else {
        return;
    };
    let name = first.trim_matches(['"', '`', '[', ']'].as_slice());
    if name.is_empty() {
        return;
    }
    // Table-level constraints open a clause where a column name would be.
    const CONSTRAINTS: [&str; 6] = [
        "primary", "unique", "check", "foreign", "constraint", "key",
    ];
    let lower = part.to_ascii_lowercase();
    if CONSTRAINTS.contains(&name.to_ascii_lowercase().as_str()) {
        return;
    }
    let rest = lower[first.len()..].trim_start().to_string();
    // The rowid alias is the exact phrase, and only for a lone INTEGER: a
    // `BIGINT PRIMARY KEY` or a `PRIMARY KEY DESC` is an ordinary column with
    // an index over it.
    let rowid_alias = rest.starts_with("integer")
        && rest.contains("primary key")
        && !rest.contains("primary key desc");
    // SQLite's affinity rules, the two clauses that reach REAL.
    let real_affinity = !rowid_alias
        && ["real", "floa", "doub"].iter().any(|t| rest.contains(t))
        && !rest.contains("int")
        && !rest.contains("char")
        && !rest.contains("clob")
        && !rest.contains("text")
        && !rest.contains("blob");
    out.push(Column {
        name: name.to_string(),
        rowid_alias,
        real_affinity,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn varints_round_trip_at_every_width() {
        // The boundaries are where a varint grows a byte, plus the ninth-byte
        // case that carries eight bits instead of seven.
        for v in [
            0i64,
            1,
            0x7f,
            0x80,
            0x3fff,
            0x4000,
            1 << 20,
            1 << 27,
            1 << 34,
            1 << 41,
            1 << 48,
            1 << 55,
            i64::MAX,
            -1,
            i64::MIN,
        ] {
            let mut buf = Vec::new();
            put_varint(&mut buf, v);
            assert_eq!(buf.len(), varint_len(v));
            let (back, used) = get_varint(&buf).expect("reads back");
            assert_eq!((back, used), (v, buf.len()), "varint {v}");
        }
    }

    #[test]
    fn a_record_round_trips_every_storage_class() {
        let values = vec![
            Value::Null,
            Value::Int(0),
            Value::Int(1),
            Value::Int(-1),
            Value::Int(200),
            Value::Int(-40000),
            Value::Int(1 << 20),
            Value::Int(-(1 << 20)),
            Value::Int(1 << 40),
            Value::Int(i64::MIN),
            Value::Real(-0.5),
            Value::Text("Twilight Egg".into()),
            Value::Text(String::new()),
            Value::Blob(vec![0, 1, 2, 255]),
        ];
        let bytes = encode_record(&values);
        assert_eq!(decode_record(&bytes).expect("decodes"), values);
    }

    #[test]
    fn a_record_header_that_crosses_a_varint_boundary_is_still_right() {
        // A header of 127 bytes and one of 128 are a byte apart in how the
        // length is written, and the length counts itself -- so the naive
        // "serials plus one" is wrong for exactly one size.
        for n in 120..140 {
            let values: Vec<Value> = (0..n).map(|_| Value::Int(7)).collect();
            let bytes = encode_record(&values);
            assert_eq!(decode_record(&bytes).expect("decodes"), values, "{n} columns");
        }
    }

    #[test]
    fn column_names_reads_a_create_table() {
        let sql = "CREATE TABLE games (id INTEGER PRIMARY KEY, played_at INTEGER NOT NULL, \
                   note TEXT DEFAULT 'a,b', UNIQUE (id, played_at))";
        assert_eq!(column_names(sql), vec!["id", "played_at", "note"]);
    }

    #[test]
    fn the_rowid_alias_and_real_affinity_are_told_apart() {
        let cols = columns(
            "CREATE TABLE t (id INTEGER PRIMARY KEY, big BIGINT PRIMARY KEY, \
             ratio REAL, weight DOUBLE, n INTEGER, s TEXT, pointless FLOATING POINT)",
        );
        let by = |n: &str| cols.iter().find(|c| c.name == n).expect(n).clone();
        assert!(by("id").rowid_alias, "the one column SQLite aliases to the rowid");
        assert!(!by("big").rowid_alias, "BIGINT is not INTEGER for this rule");
        assert!(by("ratio").real_affinity);
        assert!(by("weight").real_affinity);
        assert!(!by("n").real_affinity);
        assert!(!by("s").real_affinity);
        // "FLOATING POINT" contains "INT", which SQLite's own rules test for
        // first -- so it has INTEGER affinity, however it reads.
        assert!(!by("pointless").real_affinity);
    }
}
