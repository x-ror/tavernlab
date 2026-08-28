//! The games you actually played, in a SQLite file you can copy.
//!
//! The one promise this makes is the file. It lives beside your settings, not
//! in a checkout, it is an ordinary database, and `sqlite3 history.sqlite
//! "select * from games"` is the supported way to get at anything this program
//! does not show you. Nothing here is a cache: if the file is deleted the
//! history is gone, which is why it is not written anywhere a reinstall
//! would tread.
//!
//! What goes in is only what the log stated outright, the same rule
//! `tavernsim watch` reads under. A column this could not read is NULL rather
//! than a guess, and the UI shows the gap.

use std::path::{Path, PathBuf};

use tavernlab_sqlite::{Value, column_names};

use crate::serve::paths;

/// The table, as `sqlite_schema` will hold it.
///
/// Columns are only ever added at the end. A row written by an older build is
/// short, and SQLite reads a short row's missing columns as NULL -- so an
/// added column costs nothing and a reordered one would silently relabel
/// every game already recorded.
pub const SCHEMA: &str = "CREATE TABLE games (\
id INTEGER PRIMARY KEY, \
played_at INTEGER NOT NULL, \
my_class TEXT NOT NULL, \
opponent_class TEXT NOT NULL, \
won INTEGER, \
turns INTEGER NOT NULL, \
coin INTEGER, \
deck_code TEXT, \
opponent_deck TEXT, \
opponent_hits INTEGER, \
opponent_seen INTEGER, \
opening TEXT, \
opponent_cards TEXT)";

/// One recorded game.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Game {
    /// Unix seconds. Derived from the log's own clock -- see
    /// `watch::replay` -- so re-reading the same log gives the same number,
    /// which is what makes an import idempotent.
    pub played_at: i64,
    pub my_class: String,
    pub opponent_class: String,
    /// `None` when the log ended without saying who won: a game abandoned,
    /// or a session that was still running when the file was read.
    pub won: Option<bool>,
    pub turns: i64,
    /// Whether I held the Coin, which is how the log says who was on the draw.
    pub coin: Option<bool>,
    pub deck_code: String,
    /// The gauntlet deck the opponent's cards looked most like, and the count
    /// that read was built on. Empty and zero when nothing matched -- a name
    /// with no evidence behind it is the thing this project does not print.
    pub opponent_deck: String,
    pub opponent_hits: i64,
    pub opponent_seen: i64,
    /// Card names, `;`-separated. Two lists rather than two tables: a game is
    /// read and shown whole, never joined against, and a join table would be
    /// a schema to migrate for no query anyone makes.
    pub opening: Vec<String>,
    pub opponent_cards: Vec<String>,
}

impl Game {
    fn to_values(&self) -> Vec<Value> {
        vec![
            Value::Null, // id: the rowid
            Value::Int(self.played_at),
            Value::Text(self.my_class.clone()),
            Value::Text(self.opponent_class.clone()),
            self.won.map_or(Value::Null, |w| Value::Int(w as i64)),
            Value::Int(self.turns),
            self.coin.map_or(Value::Null, |c| Value::Int(c as i64)),
            Value::Text(self.deck_code.clone()),
            Value::Text(self.opponent_deck.clone()),
            Value::Int(self.opponent_hits),
            Value::Int(self.opponent_seen),
            Value::Text(self.opening.join(";")),
            Value::Text(self.opponent_cards.join(";")),
        ]
    }

    fn from_row(row: &tavernlab_sqlite::Row) -> Game {
        let text = |i: usize| row.get(i).as_str().unwrap_or("").to_string();
        let list = |i: usize| {
            row.get(i)
                .as_str()
                .unwrap_or("")
                .split(';')
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect()
        };
        let flag = |i: usize| row.get(i).as_i64().map(|n| n != 0);
        Game {
            played_at: row.get(1).as_i64().unwrap_or(0),
            my_class: text(2),
            opponent_class: text(3),
            won: flag(4),
            turns: row.get(5).as_i64().unwrap_or(0),
            coin: flag(6),
            deck_code: text(7),
            opponent_deck: text(8),
            opponent_hits: row.get(9).as_i64().unwrap_or(0),
            opponent_seen: row.get(10).as_i64().unwrap_or(0),
            opening: list(11),
            opponent_cards: list(12),
        }
    }

    /// What makes two records the same game.
    ///
    /// The clock the log itself kept, plus who was playing. Reading the same
    /// session twice -- which happens every time the watcher starts against a
    /// log it has already seen -- must not double the history, and there is no
    /// game id in the log to key on.
    fn same_as(&self, other: &Game) -> bool {
        self.played_at == other.played_at
            && self.my_class == other.my_class
            && self.opponent_class == other.opponent_class
    }
}

/// Where the history lives: beside the settings, in the per-user data home.
pub fn default_path() -> PathBuf {
    paths::data_home().join("history.sqlite")
}

/// Read every recorded game, oldest first.
pub fn read(path: &Path) -> Result<Vec<Game>, String> {
    let db = tavernlab_sqlite::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let Some(table) = db.table("games") else {
        return Ok(Vec::new());
    };
    // A file written by a build with fewer columns is read against its own
    // schema, so a column that did not exist then reads as NULL rather than
    // shifting every later column along by one.
    let names = column_names(&table.sql);
    let mut out: Vec<Game> = table.rows.iter().map(Game::from_row).collect();
    if names.len() < column_names(SCHEMA).len() {
        // Nothing to repair -- `Row::get` already answers Null past the end --
        // but say so once rather than leave it looking like data loss.
        eprintln!(
            "{}: written by an older build ({} of {} columns); the rest read as empty",
            path.display(),
            names.len(),
            column_names(SCHEMA).len()
        );
    }
    out.sort_by_key(|g| g.played_at);
    Ok(out)
}

/// Add games that are not already recorded, and return how many were added.
///
/// Idempotent on purpose: the watcher replays whatever the log still holds
/// every time it starts, so "append" has to mean "make sure these are in
/// there" or a week of restarts would be a week of duplicates.
pub fn append(path: &Path, games: &[Game]) -> Result<usize, String> {
    if games.is_empty() {
        return Ok(0);
    }
    let mut db = tavernlab_sqlite::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let existing: Vec<Game> = db
        .table("games")
        .map(|t| t.rows.iter().map(Game::from_row).collect())
        .unwrap_or_default();

    let table = db.ensure("games", SCHEMA);
    let mut added = 0;
    let mut seen = existing;
    for game in games {
        if seen.iter().any(|g| g.same_as(game)) {
            continue;
        }
        table.push(game.to_values());
        seen.push(game.clone());
        added += 1;
    }
    if added == 0 {
        return Ok(0);
    }
    tavernlab_sqlite::save(&db, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(added)
}

// ------------------------------------------------------------------ summary

/// Games and wins for one grouping key.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    pub key: String,
    pub games: usize,
    pub wins: usize,
}

impl Tally {
    /// Win rate, or `None` below the point where one is worth printing.
    ///
    /// Five games is not a win rate, it is five games. The UI shows the count
    /// either way and the rate only once there is one.
    pub fn rate(&self) -> Option<f64> {
        (self.games >= 5).then(|| self.wins as f64 / self.games as f64)
    }
}

fn tally(games: &[Game], key: impl Fn(&Game) -> String) -> Vec<Tally> {
    let mut out: Vec<Tally> = Vec::new();
    for g in games {
        // A game the log never resolved is not a loss; it is not a game.
        let Some(won) = g.won else { continue };
        let k = key(g);
        if k.is_empty() {
            continue;
        }
        match out.iter_mut().find(|t| t.key == k) {
            Some(t) => {
                t.games += 1;
                t.wins += won as usize;
            }
            None => out.push(Tally {
                key: k,
                games: 1,
                wins: won as usize,
            }),
        }
    }
    out.sort_by(|a, b| b.games.cmp(&a.games).then(a.key.cmp(&b.key)));
    out
}

/// The whole record, and the two cuts of it worth having: who you played
/// against, and what you played.
pub struct Summary {
    pub games: usize,
    pub resolved: usize,
    pub wins: usize,
    pub by_opponent: Vec<Tally>,
    pub by_my_class: Vec<Tally>,
    pub by_opponent_deck: Vec<Tally>,
}

pub fn summarise(games: &[Game]) -> Summary {
    Summary {
        games: games.len(),
        resolved: games.iter().filter(|g| g.won.is_some()).count(),
        wins: games.iter().filter(|g| g.won == Some(true)).count(),
        by_opponent: tally(games, |g| g.opponent_class.clone()),
        by_my_class: tally(games, |g| g.my_class.clone()),
        by_opponent_deck: tally(games, |g| g.opponent_deck.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tavernlab-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("scratch");
        let p = dir.join(name);
        let _ = std::fs::remove_file(&p);
        p
    }

    fn game(at: i64, foe: &str, won: Option<bool>) -> Game {
        Game {
            played_at: at,
            my_class: "DEATHKNIGHT".into(),
            opponent_class: foe.into(),
            won,
            turns: 14,
            coin: Some(true),
            deck_code: "AAECAfHhBA".into(),
            opponent_deck: "Herald Warlock".into(),
            opponent_hits: 3,
            opponent_seen: 7,
            opening: vec!["Twilight Egg".into(), "The Coin".into()],
            opponent_cards: vec!["Rotheart Dryad".into()],
            ..Game::default()
        }
    }

    #[test]
    fn a_game_survives_the_round_trip_whole() {
        let path = scratch("round.sqlite");
        let g = game(1_700_000_000, "WARLOCK", Some(true));
        assert_eq!(append(&path, std::slice::from_ref(&g)).expect("append"), 1);
        assert_eq!(read(&path).expect("read"), vec![g]);
    }

    #[test]
    fn reading_the_same_session_twice_does_not_double_the_history() {
        // The watcher replays whatever the log still holds on every start.
        let path = scratch("dedupe.sqlite");
        let games = vec![
            game(1_700_000_000, "WARLOCK", Some(true)),
            game(1_700_000_900, "PALADIN", Some(false)),
        ];
        assert_eq!(append(&path, &games).expect("first"), 2);
        assert_eq!(append(&path, &games).expect("again"), 0, "nothing new");
        let mut more = games.clone();
        more.push(game(1_700_001_800, "MAGE", None));
        assert_eq!(append(&path, &more).expect("third"), 1, "only the new one");
        assert_eq!(read(&path).expect("read").len(), 3);
    }

    #[test]
    fn an_unresolved_game_is_kept_but_counts_towards_nothing() {
        let games = vec![
            game(1, "WARLOCK", Some(true)),
            game(2, "WARLOCK", Some(false)),
            game(3, "WARLOCK", None),
        ];
        let s = summarise(&games);
        assert_eq!((s.games, s.resolved, s.wins), (3, 2, 1));
        let foe = &s.by_opponent[0];
        assert_eq!((foe.games, foe.wins), (2, 1), "the unresolved game is not a loss");
    }

    #[test]
    fn a_win_rate_needs_more_than_a_handful_of_games() {
        let few = Tally { key: "WARLOCK".into(), games: 4, wins: 4 };
        assert_eq!(few.rate(), None, "four wins is four wins, not 100%");
        let enough = Tally { key: "WARLOCK".into(), games: 5, wins: 4 };
        assert_eq!(enough.rate(), Some(0.8));
    }

    #[test]
    fn the_file_is_an_ordinary_database_with_the_declared_columns() {
        let path = scratch("schema.sqlite");
        append(&path, &[game(1_700_000_000, "MAGE", Some(true))]).expect("append");
        let db = tavernlab_sqlite::open(&path).expect("open");
        let t = db.table("games").expect("games");
        assert_eq!(t.sql, SCHEMA);
        assert_eq!(
            column_names(&t.sql).first().map(String::as_str),
            Some("id"),
            "the rowid alias is the first column, as every reader expects"
        );
        assert_eq!(t.rows[0].get(0), &Value::Int(1), "and it reads back as the id");
    }
}
