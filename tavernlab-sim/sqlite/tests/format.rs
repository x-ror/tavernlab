//! The two directions that matter, and neither can be checked by this crate
//! alone agreeing with itself.
//!
//! * A file real SQLite wrote has to read here. `written_by_sqlite.sqlite` was
//!   produced by SQLite 3.45.1 with a 512-byte page size, an index, a row long
//!   enough to overflow, a `REAL` that is a whole number, and a row of nothing
//!   but NULLs. It is checked in so this holds without SQLite installed.
//! * A file this crate writes has to read *there*. That direction cannot be
//!   asserted from inside a test with no dependency to assert it with, so it
//!   was checked by hand against SQLite 3.45.1 -- `PRAGMA integrity_check`
//!   returns `ok` and the rows read back identical at 1, 2, 100, 5 000 and
//!   60 000 rows, the last of which is a b-tree three levels deep -- and what
//!   the tests here hold is the round trip through this crate's own reader,
//!   at sizes that force the same interior pages and overflow chains.

use std::path::{Path, PathBuf};

use tavernlab_sqlite::{Db, Value, open, save};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("tavernlab-sqlite-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir.join(name)
}

#[test]
fn a_database_written_by_real_sqlite_reads_here() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/written_by_sqlite.sqlite");
    let db = open(&path).expect("reads");

    let names: Vec<&str> = db.tables.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["games", "decks"], "the index is skipped, not read as a table");

    let games = db.table("games").expect("games");
    assert_eq!(games.rows.len(), 4);

    // `id INTEGER PRIMARY KEY` is the rowid: stored as NULL, read as the id.
    assert_eq!(games.rows[0].get(0), &Value::Int(1));
    assert_eq!(games.rows[3].get(0), &Value::Int(4));

    assert_eq!(games.rows[0].get(1), &Value::Int(1_700_000_000));
    assert_eq!(games.rows[0].get(2), &Value::Text("DEATHKNIGHT".into()));
    assert_eq!(games.rows[0].get(4), &Value::Real(0.5));
    assert_eq!(games.rows[0].get(5), &Value::Blob(vec![0, 1, 2]));

    // A negative integer, and a row long enough to spill onto overflow pages.
    assert_eq!(games.rows[1].get(1), &Value::Int(-1));
    assert_eq!(games.rows[1].get(3).as_str().map(str::chars).map(Iterator::count), Some(4000));
    // 2.0 has no fractional part, so SQLite stored it as an integer; the
    // column's REAL affinity is what has to turn it back.
    assert_eq!(games.rows[1].get(4), &Value::Real(2.0));

    assert_eq!(games.rows[2].get(1), &Value::Int(1 << 40));
    assert_eq!(games.rows[2].get(5).clone(), Value::Blob((0..=255u8).collect()));

    // A row of nothing but NULLs, except the id.
    for i in 1..6 {
        assert_eq!(games.rows[3].get(i), &Value::Null, "column {i}");
    }

    // A table with no rowid alias at all still reads.
    let decks = db.table("decks").expect("decks");
    assert_eq!(decks.rows[0].get(0), &Value::Text("AAECAfHhBA".into()));
    assert_eq!(decks.rows[0].get(1), &Value::Text("Egg Death Knight".into()));
}

#[test]
fn a_big_table_round_trips_through_interior_pages_and_overflow() {
    // Four thousand rows is far past one leaf page, so the tree grows an
    // interior level; every thirty-seventh row is longer than a page, so it
    // spills. Both are the cases a small table never reaches.
    const N: i64 = 4000;
    let mut db = Db::new();
    {
        let t = db.ensure(
            "games",
            "CREATE TABLE games (id INTEGER PRIMARY KEY, played_at INTEGER, \
             note TEXT, ratio REAL, raw BLOB)",
        );
        for i in 0..N {
            t.push(vec![
                Value::Null,
                Value::Int(1_700_000_000 + i),
                Value::Text(if i % 37 == 5 {
                    "довгий текст ".repeat(700)
                } else {
                    format!("гра {i}")
                }),
                Value::Real(i as f64 / 3.0),
                Value::Blob(vec![(i % 256) as u8; (i % 11) as usize]),
            ]);
        }
    }
    let path = scratch("big.sqlite");
    save(&db, &path).expect("saves");
    let back = open(&path).expect("reads");

    let before = db.table("games").expect("games");
    let after = back.table("games").expect("games");
    assert_eq!(after.rows.len(), N as usize);
    assert_eq!(after.sql, before.sql);
    for (i, (a, b)) in before.rows.iter().zip(&after.rows).enumerate() {
        assert_eq!(a.rowid, b.rowid, "row {i}");
        // The id column went in as NULL and comes back as the rowid, which is
        // what SQLite itself does; every other column is untouched.
        assert_eq!(b.get(0), &Value::Int(b.rowid), "row {i} id");
        assert_eq!(a.values[1..], b.values[1..], "row {i}");
    }
}

#[test]
fn a_missing_file_is_an_empty_database_and_a_save_creates_it() {
    let path = scratch("fresh.sqlite");
    let _ = std::fs::remove_file(&path);
    let db = open(&path).expect("a missing file is not an error");
    assert!(db.tables.is_empty());

    let mut db = db;
    db.ensure("games", "CREATE TABLE games (id INTEGER PRIMARY KEY, note TEXT)")
        .push(vec![Value::Null, Value::Text("перша гра".into())]);
    save(&db, &path).expect("saves");
    let back = open(&path).expect("reads");
    assert_eq!(back.table("games").expect("games").rows.len(), 1);
}

#[test]
fn an_empty_table_is_still_a_table() {
    let path = scratch("empty.sqlite");
    let mut db = Db::new();
    db.ensure("games", "CREATE TABLE games (id INTEGER PRIMARY KEY, note TEXT)");
    save(&db, &path).expect("saves");
    let back = open(&path).expect("reads");
    let t = back.table("games").expect("the table survives with no rows in it");
    assert!(t.rows.is_empty());
    assert_eq!(t.next_rowid(), 1);
}

#[test]
fn something_that_is_not_a_database_says_so() {
    let path = scratch("junk.sqlite");
    std::fs::write(&path, b"this is not a database, it is a note").expect("write");
    assert!(matches!(
        open(&path),
        Err(tavernlab_sqlite::Error::NotSqlite)
    ));
}

#[test]
fn a_save_that_replaces_a_file_leaves_the_old_one_until_the_new_one_is_whole() {
    // The write goes to a sibling and is renamed over the target. What this
    // can check is that the sibling does not survive the save -- a leftover
    // `.sqlite.tmp` beside the database would be the visible symptom of the
    // rename not happening.
    let path = scratch("replace.sqlite");
    let mut db = Db::new();
    db.ensure("games", "CREATE TABLE games (id INTEGER PRIMARY KEY, note TEXT)")
        .push(vec![Value::Null, Value::Text("one".into())]);
    save(&db, &path).expect("saves");
    db.table_mut("games")
        .expect("games")
        .push(vec![Value::Null, Value::Text("two".into())]);
    save(&db, &path).expect("saves again");

    assert!(!path.with_extension("sqlite.tmp").exists(), "no leftover temp file");
    assert_eq!(open(&path).expect("reads").table("games").expect("games").rows.len(), 2);
}
