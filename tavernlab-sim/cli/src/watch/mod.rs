//! `tavernsim watch` — read the game's own log and say what to do.
//!
//! Three answers, and no more, because these are the three the engine can
//! actually stand behind:
//!
//!   * what to keep in the mulligan, from the same instrumented run the
//!     web UI's Mulligan tab uses;
//!   * which gauntlet deck the opponent looks like, from what they have
//!     played;
//!   * what to play this turn, by rebuilding the position and asking the
//!     engine's own agent.
//!
//! The reconstruction is printed beside the advice on purpose. A log gives
//! a partial view — the opponent's hand is face down, and a board this could
//! not read is a board this should not advise on — so showing the position
//! it built is what makes a wrong read visible instead of silent.
//!
//! `--quiet` skips the advice and only writes finished games to the history
//! file. That is the mode a long-running recorder wants: the mulligan
//! measurement is a batch of simulations, and a daemon that is only keeping
//! score should not run one on every turn.

pub mod log;
pub mod tracker;

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tavernlab_core::agent::{Scripted, Style};
use tavernlab_core::cards::{CardId, Class, Keywords};
use tavernlab_core::game::{Action, Agent, hero_power_for};
use tavernlab_core::state::{Game, HandCard, Permanent};

use crate::history;
use crate::serve::state::App;
use tracker::Tracker;

/// Where the game keeps its logs, when it is being run in the usual place.
///
/// There is no standard location outside Windows, so a Wine or Proton
/// install has to say: `--logs`, or `HS_LOGS`.
fn default_logs_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("HS_LOGS") {
        return Some(PathBuf::from(dir));
    }
    let local = std::env::var("LOCALAPPDATA").ok()?;
    Some(
        Path::new(&local)
            .join("Blizzard")
            .join("Hearthstone")
            .join("Logs"),
    )
}

/// Real verbose logging runs to hundreds of kilobytes per game. A tiny
/// Power.log is the error line the client writes even with logging off, and
/// picking it would leave the watcher silently waiting on a dead file.
const MIN_POWER_BYTES: u64 = 4096;

/// One session directory, if its Power.log is large enough to be real.
fn session_entry(path: &Path) -> Option<(std::time::SystemTime, PathBuf, Option<PathBuf>)> {
    let power = path.join("Power.log");
    let meta = std::fs::metadata(&power).ok()?;
    if meta.len() < MIN_POWER_BYTES {
        return None;
    }
    let when = meta.modified().ok()?;
    let zone = path.join("Zone.log");
    let zone = zone.exists().then_some(zone);
    Some((when, power, zone))
}

/// Every session the client has left on disk, oldest first.
///
/// A daemon that only tailed the newest folder would miss the previous
/// session's last games whenever the client rotated while it was stopped.
/// Replaying them all is cheap (it is line parsing, not a simulation) and
/// `history::append` is idempotent, so a session this has already ingested
/// is a no-op rather than a duplicate.
fn all_sessions(dir: &Path) -> Vec<(PathBuf, Option<PathBuf>)> {
    let mut v: Vec<(std::time::SystemTime, PathBuf, Option<PathBuf>)> = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        if let Some(s) = session_entry(&entry.path()) {
            v.push(s);
        }
    }
    v.sort_by(|a, b| a.0.cmp(&b.0));
    v.into_iter().map(|(_, p, z)| (p, z)).collect()
}

/// The newest session directory that holds a log worth reading.
fn newest_logs(dir: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    all_sessions(dir).pop()
}

fn files_of(power: PathBuf, zone: Option<PathBuf>) -> Vec<PathBuf> {
    match zone {
        Some(z) => vec![power, z],
        None => vec![power],
    }
}

/// Read the new lines of both files into one tracker, in the order the client
/// wrote them.
///
/// Chronological, not file by file. Only Power.log carries `CREATE_GAME`, so
/// replaying one file and then the other puts every reset at the front and
/// then lays every zone move of every game in the session on top of the last
/// one — finished games' boards piling up on the current one, and their
/// heroes deciding the classes. That is what the first real log showed.
/// What one pass over the new lines produced.
pub struct Batch {
    pub lines: usize,
    /// Games that ended inside this batch, each with the moment it ended.
    ///
    /// A whole session's worth on the first pass, because the first pass reads
    /// the file from the start -- which is how a log the watcher has never
    /// seen becomes history rather than being skipped.
    pub finished: Vec<(i64, Tracker)>,
}

/// When a line was written, in Unix seconds.
///
/// The log stamps its lines with a time of day and no date, and no timezone
/// this program can resolve without a database it does not carry. What it can
/// do is measure: the newest line in the batch was written when the file was
/// last written, and every earlier line is its own distance back from that.
/// Both times of day come from the log's own clock, so their difference is a
/// true elapsed duration whatever timezone wrote it.
///
/// Being derived from the log rather than from the clock is also what makes
/// re-reading a log idempotent -- the same line yields the same second, so
/// `history::append` recognises the game it already has.
fn stamp_to_unix(stamp: u64, newest: u64, mtime: i64) -> i64 {
    let back = newest.saturating_sub(stamp) / 1_000_000_000;
    mtime - back as i64
}

fn newest_mtime(files: &[PathBuf]) -> i64 {
    files
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok()?.modified().ok())
        .filter_map(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .max()
        .unwrap_or(0)
}

fn replay(tr: &mut Tracker, files: &[PathBuf], offsets: &mut Vec<u64>) -> std::io::Result<Batch> {
    offsets.resize(files.len(), 0);
    // `(stamp, file, seq)` keeps a stable order: lines that share a stamp, or
    // carry none, stay in the order their own file had them.
    let mut batch: Vec<((u64, usize, usize), String)> = Vec::new();
    for (i, path) in files.iter().enumerate() {
        let Ok(mut file) = std::fs::File::open(path) else {
            continue;
        };
        // A truncated file means the client started a new session; start over.
        let len = file.metadata()?.len();
        if len < offsets[i] {
            offsets[i] = 0;
        }
        file.seek(SeekFrom::Start(offsets[i]))?;
        let mut reader = BufReader::new(file);
        let mut buf = String::new();
        let mut last = 0u64;
        let mut seq = 0usize;
        loop {
            buf.clear();
            let n = reader.read_line(&mut buf)?;
            if n == 0 {
                break;
            }
            offsets[i] += n as u64;
            let line = buf.trim_end().to_string();
            // A line with no stamp of its own belongs to the one before it.
            last = log::stamp(&line).unwrap_or(last);
            batch.push(((last, i, seq), line));
            seq += 1;
        }
    }
    let lines = batch.len();
    batch.sort_by(|a, b| a.0.cmp(&b.0));
    let newest = batch.last().map(|((t, _, _), _)| *t).unwrap_or(0);
    let mtime = newest_mtime(files);

    let mut finished = Vec::new();
    for ((stamp, _, _), line) in &batch {
        let Some(ev) = log::parse(line) else { continue };
        let was_over = tr.over;
        tr.feed(ev);
        if tr.over && !was_over {
            finished.push((stamp_to_unix(*stamp, newest, mtime), tr.clone()));
        }
    }
    Ok(Batch { lines, finished })
}

// ------------------------------------------------------------------ advice

/// Keywords worth naming in the printed position, and how to spell them.
const KEYWORD_LABELS: &[(&Keywords, &str)] = &[
    (&Keywords::TAUNT, "Taunt"),
    (&Keywords::DIVINE_SHIELD, "Divine Shield"),
    (&Keywords::STEALTH, "Stealth"),
    (&Keywords::CHARGE, "Charge"),
    (&Keywords::RUSH, "Rush"),
    (&Keywords::WINDFURY, "Windfury"),
    (&Keywords::LIFESTEAL, "Lifesteal"),
    (&Keywords::POISONOUS, "Poisonous"),
    (&Keywords::REBORN, "Reborn"),
    (&Keywords::ELUSIVE, "Elusive"),
    (&Keywords::IMMUNE, "Immune"),
    (&Keywords::CANT_ATTACK, "Can't Attack"),
];

fn class_name(c: Class) -> &'static str {
    tavernlab_core::gauntlet::class_name(c)
}

/// Rebuild the position as far as the log states it, and ask the engine's own
/// agent what it would do.
///
/// What is missing is missing on purpose: the opponent's hand is face down,
/// so the rebuilt game has an empty one, and the advice is worth exactly what
/// a read of the board is worth. Health and armour are not tracked, so both
/// heroes start whole — which is why this prints the position it used.
fn plan(tr: &Tracker) -> Vec<String> {
    let (Some(mine), Some(theirs)) = (tr.my_class(), tr.opponent_class()) else {
        return vec!["не видно обох класів — ще нема з чого будувати позицію".into()];
    };
    let (Ok(hp0), Ok(hp1)) = (hero_power_for(mine), hero_power_for(theirs)) else {
        return vec!["клас без сили героя".into()];
    };
    let mut g = match Game::new((mine, &[]), (theirs, &[]), 1) {
        Ok(g) => g,
        Err(e) => return vec![format!("не вдалося зібрати позицію: {e}")],
    };
    g.players[0].hero_power = hp0;
    g.players[1].hero_power = hp1;
    for i in 0..2 {
        g.players[i].hero_hp = tr.heroes[i].health();
        g.players[i].armor = tr.heroes[i].armor;
    }
    // Without a battletag the log's mana lines cannot be attributed, so the
    // plan is drawn at the turn's worth of crystals rather than at a made-up
    // number: it will suggest more than you can pay for, and says so.
    g.players[0].crystals = tr.crystals.unwrap_or_else(|| (tr.turn as i16 / 2 + 1).min(10));
    g.players[0].mana = tr.mana_left().unwrap_or(g.players[0].crystals);
    g.turn = tr.turn;
    for side in 0..2 {
        // A body the log has taken to zero health is dead; the line that
        // moves it out of play arrives in a later batch, up to a poll behind.
        // Leaving it in means the plan is drawn over a board with a corpse on
        // it -- a real session showed `Accelerated Whelp 4/0` still standing.
        for b in tr.board[side].iter().filter(|b| b.stats().1 > 0) {
            let mut m = Permanent::summon(b.card);
            // Summoning sickness is kept for whatever landed this turn. It is
            // not a detail: without it the plan tells you to swing with the
            // minion you have only just put down, and a Rush minion is told
            // to go face on the turn it arrives, which is the one thing Rush
            // does not do. `Permanent::summon` sets the flag, so a body that
            // has been there since an earlier turn is the one that clears it.
            if b.turn < tr.turn {
                m.flags.remove(tavernlab_core::state::Flags::JUST_SUMMONED);
            }
            // What the log said about this body, over what the card prints:
            // a buffed minion, the damage already on it, the keywords it was
            // given, and how many swings it has already had. Silence means
            // the printed card, which is what `Body` falls back to.
            let d = b.card.def();
            m.atk = b.atk.unwrap_or(d.atk);
            m.max_hp = b.hp.unwrap_or(d.hp);
            m.damage = b.damage;
            m.keywords = b.keywords;
            m.attacks_done = b.attacks;
            if b.frozen {
                m.flags.insert(tavernlab_core::state::Flags::FROZEN);
            }
            g.players[side].board.push(m);
        }
    }
    for b in tr.hand.iter() {
        g.players[0].hand.push(HandCard::new(b.card));
    }
    g.recompute_auras();

    let mut agent = Scripted::new(Style::Midrange);
    let mut out = Vec::new();
    let mut legal: tavernlab_core::inline::Inline<Action, 512> =
        tavernlab_core::inline::Inline::new();
    // Walk the turn the way the engine would play it, stopping when the
    // agent decides it is done or nothing legal is left.
    for _ in 0..16 {
        g.legal_actions(&mut legal);
        if legal.is_empty() {
            break;
        }
        let action = agent.choose(&g, legal.as_slice());
        let Some(line) = describe(&g, action) else {
            break;
        };
        out.push(line);
        if !g.apply(action) {
            break;
        }
    }
    // A trailing Coin is a Coin spent on nothing. The engine's own agent
    // plays it whenever it is legal, which is right for a game it is playing
    // out and wrong as advice: what is left after the last line is a mana
    // crystal with nothing to buy. Only the last one goes -- a Coin that pays
    // for the play after it stays.
    while out.last().is_some_and(|l| l == "зіграти The Coin") {
        out.pop();
    }
    if out.is_empty() {
        out.push("нічого не робити цього ходу".into());
    }
    // Say it out loud when the mana was guessed rather than read. A plan
    // drawn at a made-up crystal count will happily spend more than you have,
    // and the reader has no way to tell that from a plan drawn at the real
    // number unless this line is here.
    if tr.crystals.is_none() {
        out.push(format!(
            "(мана невідома — рахував як {}; вкажіть --me <бойовий тег>, щоб було точно)",
            g.players[0].crystals
        ));
    }
    out
}

fn describe(g: &Game, a: Action) -> Option<String> {
    let me = g.current;
    Some(match a {
        Action::EndTurn => return None,
        Action::Play { hand, target, .. } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            match target {
                Some(t) => format!("зіграти {} → {}", card.name(), target_name(g, t)),
                None => format!("зіграти {}", card.name()),
            }
        }
        Action::Attack { from, target } => {
            let m = g.player(me).board.get(from as usize)?;
            format!("атакувати: {} → {}", m.card.name(), target_name(g, target))
        }
        Action::HeroAttack { target } => format!("бити героєм → {}", target_name(g, target)),
        Action::HeroPower { target, .. } => match target {
            Some(t) => format!("сила героя → {}", target_name(g, t)),
            None => "сила героя".into(),
        },
        Action::Trade { hand } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            format!("Trade {}", card.name())
        }
        Action::Prepare { hand } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            format!("Prepare {}", card.name())
        }
        Action::UseLocation { slot, .. } => {
            let m = g.player(me).board.get(slot as usize)?;
            format!("активувати {}", m.card.name())
        }
    })
}

fn target_name(g: &Game, t: tavernlab_core::state::Target) -> String {
    match t {
        tavernlab_core::state::Target::Hero(s) => {
            if s == g.current { "свій герой" } else { "ворожий герой" }.into()
        }
        tavernlab_core::state::Target::Minion(s, i) => {
            let who = if s == g.current { "свій" } else { "ворожий" };
            match g.player(s).board.get(i as usize) {
                Some(m) => format!("{who} {}", m.card.name()),
                None => format!("{who} мінйон {i}"),
            }
        }
    }
}

/// Which gauntlet deck the opponent looks like, from what they have played.
fn opponent_read(app: &App, format: &str, tr: &Tracker) -> Vec<String> {
    let Some(class) = tr.opponent_class() else {
        return vec!["клас суперника ще не видно".into()];
    };
    let seen: Vec<CardId> = tr.played[1].clone();
    let field = app.gauntlet(format);
    let reads = tavernlab_core::gauntlet::read_opponent(&field, class, &seen);
    if reads.is_empty() {
        return vec![format!(
            "{}: у гаунтлеті немає колод цього класу",
            class_name(class)
        )];
    }
    if seen.is_empty() {
        return vec![format!(
            "{}: ще нічого не зіграно, читати нема чого",
            class_name(class)
        )];
    }
    // `Read::frac` is 1.0 on no evidence by design -- the web UI wants a
    // neutral prior -- which printed here read as "Herald Warlock 100%"
    // before the opponent had played a card. The empty case is answered
    // above; a best match of nothing is answered here, because naming a deck
    // beside 0% is a claim with the confidence stripped off but the name
    // left standing.
    if reads.iter().all(|r| r.hits == 0) {
        return vec![format!(
            "{}: жодна колода гаунтлета не пояснює зіграного ({} карт)",
            class_name(class),
            seen.len()
        )];
    }
    let mut out = Vec::new();
    for r in reads.iter().take(3).filter(|r| r.hits > 0) {
        // The fraction and the count it came from: "43%" out of seven cards
        // is a read, out of two is a coincidence, and the line should not
        // make them look alike.
        let mut line = format!("{}  {:.0}% ({} з {})", r.deck, r.frac * 100.0, r.hits, r.seen);
        if !r.threats.is_empty() {
            let names: Vec<String> = r
                .threats
                .iter()
                .take(4)
                .map(|c| format!("({}) {}", c.def().cost, c.name()))
                .collect();
            line.push_str(&format!("  — чекай: {}", names.join(", ")));
        }
        out.push(line);
    }
    out
}

/// What to keep in the opening hand.
///
/// The same measurement the web UI's Mulligan tab prints: an instrumented run
/// of this deck against the opponent's gauntlet list, and per card the
/// difference between the win rate of the games that opened with it and the
/// win rate overall. Below the sample floor there is no number, and the
/// answer falls back to the only thing still true about the card -- its cost.
fn mulligan(app: &App, format: &str, tr: &Tracker, deck: &str) -> Vec<String> {
    if tr.opening.is_empty() {
        return vec!["ще не роздано".into()];
    }
    let listed: Vec<String> = tr
        .opening
        .iter()
        .map(|c| format!("({}) {}", c.def().cost, c.name()))
        .collect();
    if deck.is_empty() {
        let mut out = vec![
            "без --deck немає з чим міряти; лишається тільки крива:".to_string(),
        ];
        for c in &tr.opening {
            let keep = c.def().cost <= 3;
            out.push(format!(
                "{} ({}) {}",
                if keep { "ЛИШИТИ" } else { "СКИНУТИ" },
                c.def().cost,
                c.name()
            ));
        }
        return out;
    }
    let Some(class) = tr.opponent_class() else {
        return vec![format!(
            "клас суперника ще не видно; на руці: {}",
            listed.join(", ")
        )];
    };
    match app.mulligan_advice(format, deck, class, &tr.opening) {
        Ok(rows) => rows,
        Err(e) => vec![e],
    }
}

// ----------------------------------------------------------------- command

pub struct Args {
    pub logs_dir: Option<PathBuf>,
    /// Where to keep the history. Defaults to `history.sqlite` in the data
    /// home, beside the settings.
    pub history: Option<PathBuf>,
    pub log_file: Option<PathBuf>,
    pub deck: String,
    /// Battletag, `Name#12345`. The log names both players and never says
    /// which is you.
    pub me: Option<String>,
    pub once: bool,
    /// Record without printing advice. The mulligan measurement is a batch
    /// of simulations; a daemon that is only keeping history should not pay
    /// that on every turn.
    pub quiet: bool,
}

/// Print everything the tracker can currently say.
fn report(app: &App, format: &str, tr: &Tracker, deck: &str) {
    // Straight after a `CREATE_GAME` there is a moment where nothing at all
    // has been read. A full block of empty boards and two untouched heroes
    // says nothing and reads like a position; one line is the honest size of
    // what is known.
    if tr.my_class().is_none() && tr.opponent_class().is_none() && tr.opening.is_empty() {
        println!("\n─── нова гра — ще нічого не видно");
        return;
    }
    println!("\n─── хід {} {}", tr.turn, if tr.my_turn { "(ваш)" } else { "" });
    match (tr.my_class(), tr.opponent_class()) {
        (Some(a), Some(b)) => println!("  {} проти {}", class_name(a), class_name(b)),
        (Some(a), None) => println!("  {} проти ?", class_name(a)),
        _ => println!("  класи ще не видно"),
    }
    if tr.over {
        println!("  гру завершено");
    }

    if !tr.started && !tr.opening.is_empty() {
        println!("\n  МУЛІГАН");
        for line in mulligan(app, format, tr, deck) {
            println!("    {line}");
        }
        return;
    }

    println!("\n  СУПЕРНИК");
    for line in opponent_read(app, format, tr) {
        println!("    {line}");
    }

    println!("\n  ПОЗИЦІЯ (те, що вдалося прочитати з логу)");
    match (tr.mana_left(), tr.crystals) {
        (Some(left), Some(total)) => println!("    мана {left}/{total}"),
        _ => {
            println!(
                "    мана невідома — вкажіть --me <бойовий тег> або HS_ME, \
                 інакше рядки RESOURCES нема до кого віднести"
            );
            // The client does not always spell a battletag the way the
            // launcher shows it, and guessing is the one thing this must not
            // do -- so print what it actually wrote and let the user pick.
            if !tr.names.is_empty() {
                println!("      у лозі трапилися імена: {}", tr.names.join(", "));
            }
        }
    }
    println!(
        "    ваш герой {}{}, ворожий {}{}",
        tr.heroes[0].health(),
        armour(tr.heroes[0].armor),
        tr.heroes[1].health(),
        armour(tr.heroes[1].armor),
    );
    print_side("    ваша дошка", &tr.board[0]);
    print_side("    ворожа дошка", &tr.board[1]);
    let hand: Vec<&str> = tr.hand.iter().map(|b| b.card.name()).collect();
    println!(
        "    рука: {}",
        if hand.is_empty() {
            "порожня".to_string()
        } else {
            hand.join(", ")
        }
    );

    if tr.my_turn && !tr.over {
        println!("\n  ХІД");
        for line in plan(tr) {
            println!("    {line}");
        }
    }
}

fn armour(n: i16) -> String {
    if n > 0 { format!(" (+{n} броні)") } else { String::new() }
}

fn print_side(label: &str, board: &[tracker::Body]) {
    let names: Vec<String> = board
        .iter()
        // Same rule as the plan: a body at zero health is already dead, and
        // showing it would make the advice look like it ignored a minion.
        .filter(|b| b.stats().1 > 0)
        .map(|b| {
            let (atk, health) = b.stats();
            let mut s = format!("{} {atk}/{health}", b.card.name());
            // Only what the log granted on top of the card, so the line stays
            // readable: a printed Taunt is not news, one that was given is.
            let mut extra = b.keywords;
            extra.remove(b.card.def().keywords);
            for (k, label) in KEYWORD_LABELS {
                if extra.has(**k) {
                    s.push(' ');
                    s.push_str(label);
                }
            }
            if b.frozen {
                s.push_str(" Frozen");
            }
            s
        })
        .collect();
    println!(
        "{label}: {}",
        if names.is_empty() {
            "порожня".to_string()
        } else {
            names.join(", ")
        }
    );
}

pub fn run(app: &App, format: &str, args: Args) -> i32 {
    if let Some(one) = args.log_file.clone() {
        return follow(app, format, &args, vec![one], args.once);
    }
    let Some(dir) = args.logs_dir.clone().or_else(default_logs_dir) else {
        eprintln!(
            "не знаю, де логи гри. Вкажіть --logs <тека> або змінну HS_LOGS.\n\
             Логування вмикається у log.config поруч із теками Logs."
        );
        return 2;
    };
    if args.once {
        match newest_logs(&dir) {
            Some((power, zone)) => follow(app, format, &args, files_of(power, zone), true),
            None => {
                eprintln!(
                    "у {} немає жодного Power.log більшого за {MIN_POWER_BYTES} байт.\n\
                     Схоже, детальне логування вимкнене: увімкніть його в log.config \
                     і перезапустіть клієнт.",
                    dir.display()
                );
                2
            }
        }
    } else {
        live(app, format, &args, &dir)
    }
}

fn snapshot(tr: &Tracker) -> (u16, bool, usize, usize) {
    (tr.turn, tr.my_turn, tr.hand.len(), tr.board[1].len())
}

/// Tail a fixed set of files. `--once` is one pass; otherwise the same
/// files are polled until the process is stopped. A live directory of
/// sessions — the way the client actually writes — goes through `live`.
fn follow(app: &App, format: &str, args: &Args, files: Vec<PathBuf>, once: bool) -> i32 {
    let mut tr = Tracker::new(args.me.clone());
    let mut offsets = Vec::new();
    let first = match replay(&mut tr, &files, &mut offsets) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("не вдалося прочитати лог: {e}");
            return 2;
        }
    };
    // Whatever the log still holds becomes history on the first pass. It is
    // the same work the watcher would do a game at a time; doing it once at
    // the start is what makes pointing this at an old session an import.
    record(app, format, args, &first);
    if !args.quiet {
        report(app, format, &tr, &args.deck);
    }
    if once {
        return 0;
    }

    println!("\nстежу за логом. Ctrl-C щоб вийти.");
    let mut last = snapshot(&tr);
    loop {
        std::thread::sleep(std::time::Duration::from_millis(700));
        let batch = match replay(&mut tr, &files, &mut offsets) {
            Ok(b) if b.lines == 0 => continue,
            Ok(b) => b,
            Err(e) => {
                eprintln!("лог зник: {e}");
                return 2;
            }
        };
        record(app, format, args, &batch);
        if args.quiet {
            continue;
        }
        let now = snapshot(&tr);
        if now != last {
            last = now;
            report(app, format, &tr, &args.deck);
        }
    }
}

fn watching_line(files: &[PathBuf]) -> String {
    match files.first() {
        Some(p) => format!("\nстежу за логом ({}). Ctrl-C щоб вийти.", p.display()),
        None => "\nстежу за логом. Ctrl-C щоб вийти.".into(),
    }
}

/// Follow the client's log directory for as long as this process lives.
///
/// Three things a one-shot `follow` cannot do, and a recorder has to:
///
/// * wait for the first session instead of exiting when the client is
///   not running yet;
/// * ingest every session still on disk, so a restart does not drop the
///   games that finished in the folder the client has just rotated off;
/// * switch when a newer session appears, because the client starts a
///   fresh directory each launch and the old Power.log stops growing.
fn live(app: &App, format: &str, args: &Args, dir: &Path) -> i32 {
    let mut files: Vec<PathBuf> = Vec::new();
    let mut tr = Tracker::new(args.me.clone());
    let mut offsets: Vec<u64> = Vec::new();
    let mut last = (0u16, false, 0usize, 0usize);
    let mut watching = false;

    for (power, zone) in all_sessions(dir) {
        let next = files_of(power, zone);
        tr = Tracker::new(args.me.clone());
        offsets.clear();
        match replay(&mut tr, &next, &mut offsets) {
            Ok(batch) => record(app, format, args, &batch),
            Err(e) => {
                eprintln!("{}: {e}", next[0].display());
                continue;
            }
        }
        files = next;
        last = snapshot(&tr);
    }
    if files.is_empty() {
        eprintln!(
            "чекаю на лог у {}. Увімкніть логування в log.config і запустіть клієнт.",
            dir.display()
        );
    } else {
        if !args.quiet {
            report(app, format, &tr, &args.deck);
        }
        println!("{}", watching_line(&files));
        watching = true;
    }

    loop {
        std::thread::sleep(std::time::Duration::from_millis(700));
        let Some((power, zone)) = newest_logs(dir) else {
            if watching {
                eprintln!("лог зник, чекаю знову в {}.", dir.display());
                watching = false;
                files.clear();
            }
            continue;
        };
        let next = files_of(power, zone);
        if files.first() != next.first() {
            // A new session directory, or the first one after waiting.
            if !files.is_empty() {
                println!("нова сесія: {}", next[0].display());
            }
            tr = Tracker::new(args.me.clone());
            offsets.clear();
            files = next;
            match replay(&mut tr, &files, &mut offsets) {
                Ok(batch) => {
                    record(app, format, args, &batch);
                    if !args.quiet {
                        report(app, format, &tr, &args.deck);
                    }
                }
                Err(e) => {
                    eprintln!("не вдалося прочитати лог: {e}");
                    files.clear();
                    continue;
                }
            }
            last = snapshot(&tr);
            if !watching {
                println!("{}", watching_line(&files));
                watching = true;
            }
            continue;
        }
        // Same Power.log; Zone.log may have appeared since.
        files = next;
        let batch = match replay(&mut tr, &files, &mut offsets) {
            Ok(b) if b.lines == 0 => continue,
            Ok(b) => b,
            Err(e) => {
                eprintln!("лог зник: {e}");
                continue;
            }
        };
        record(app, format, args, &batch);
        if args.quiet {
            continue;
        }
        let now = snapshot(&tr);
        if now != last {
            last = now;
            report(app, format, &tr, &args.deck);
        }
    }
}

/// Turn one finished game into the row the history keeps.
fn recorded(app: &App, format: &str, deck: &str, at: i64, tr: &Tracker) -> Option<history::Game> {
    let (mine, theirs) = (tr.my_class()?, tr.opponent_class()?);
    // The best read at the moment the game ended, and the count behind it.
    // Empty when nothing matched: a deck name with no evidence is exactly
    // what the report refuses to print, and the history keeps the same rule.
    let seen: Vec<CardId> = tr.played[1].clone();
    let field = app.gauntlet(format);
    let best = tavernlab_core::gauntlet::read_opponent(&field, theirs, &seen)
        .into_iter()
        .find(|r| r.hits > 0);
    Some(history::Game {
        played_at: at,
        my_class: class_name(mine).to_string(),
        opponent_class: class_name(theirs).to_string(),
        won: tr.won,
        turns: tr.turn as i64,
        coin: tr.had_coin(),
        deck_code: deck.to_string(),
        opponent_deck: best.as_ref().map(|r| r.deck.clone()).unwrap_or_default(),
        opponent_hits: best.as_ref().map_or(0, |r| r.hits as i64),
        opponent_seen: seen.len() as i64,
        opening: tr.opening.iter().map(|c| c.name().to_string()).collect(),
        opponent_cards: seen.iter().map(|c| c.name().to_string()).collect(),
    })
}

/// Write whatever games this batch finished into the history file.
///
/// Failing to write is reported and not fatal. The watcher's job is the advice
/// on screen; a read-only data directory should cost you the record, not the
/// session.
fn record(app: &App, format: &str, args: &Args, batch: &Batch) {
    let games: Vec<history::Game> = batch
        .finished
        .iter()
        .filter_map(|(at, tr)| recorded(app, format, &args.deck, *at, tr))
        .collect();
    if games.is_empty() {
        return;
    }
    let path = match &args.history {
        // `--no-history` passes an empty path: play without keeping a record.
        Some(p) if p.as_os_str().is_empty() => return,
        Some(p) => p.clone(),
        None => history::default_path(),
    };
    match history::append(&path, &games) {
        Ok(0) => {}
        Ok(n) => println!(
            "\n  записано в історію: {n} {} ({})",
            crate::games_word(n as i64),
            path.display()
        ),
        Err(e) => eprintln!("не вдалося записати історію: {e}"),
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tavernlab-sessions-{}-{tag}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    fn write_power(dir: &Path, name: &str, body: &str, min_bytes: bool) {
        let session = dir.join(name);
        std::fs::create_dir_all(&session).expect("session");
        let mut padded = String::new();
        if min_bytes {
            while padded.len() < MIN_POWER_BYTES as usize {
                padded.push_str("D 00:00:00.0 [Power] padding that says nothing\n");
            }
        }
        padded.push_str(body);
        std::fs::write(session.join("Power.log"), padded).expect("Power.log");
    }

    #[test]
    fn a_tiny_power_log_is_not_a_session() {
        let dir = scratch("tiny");
        write_power(&dir, "small", "CREATE_GAME\n", false);
        assert!(newest_logs(&dir).is_none(), "below the verbose-logging floor");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_padded_power_log_is_a_session() {
        let dir = scratch("padded");
        write_power(&dir, "real", "CREATE_GAME\n", true);
        let (power, zone) = newest_logs(&dir).expect("one real session");
        assert!(power.ends_with("Power.log"), "{}", power.display());
        assert!(zone.is_none(), "no Zone.log was written");
        assert_eq!(all_sessions(&dir).len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn all_sessions_skip_the_tiny_one_and_keep_the_real() {
        let dir = scratch("mixed");
        write_power(&dir, "noise", "tiny\n", false);
        write_power(&dir, "real", "CREATE_GAME\n", true);
        let all = all_sessions(&dir);
        assert_eq!(all.len(), 1, "the stub is not a session");
        assert!(
            all[0].0.parent().unwrap().ends_with("real"),
            "{}",
            all[0].0.display()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
