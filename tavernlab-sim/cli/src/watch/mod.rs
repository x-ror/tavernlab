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

pub mod advice;
pub mod log;
pub mod tracker;

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tavernlab_core::agent::Style;
use tavernlab_core::cards::{CardId, Class, Keywords};
use tavernlab_core::game::{Action, Agent, hero_power_for};
use tavernlab_core::planner::Planner;
use tavernlab_core::state::{Game, HandCard, Permanent};

use crate::history;
use crate::serve::state::App;
pub use advice::{Advice, Arg, Line, Locale, Section};
use tracker::Tracker;

/// Where the game keeps its logs, when it is being run in the usual place.
///
/// There is no standard location outside Windows, so a Wine or Proton
/// install has to say: `--logs`, or `HS_LOGS`.
pub fn default_logs_dir() -> Option<PathBuf> {
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
/// replaying one file and then the other would put every reset at the front
/// and then lay every zone move of every game in the session on top of the
/// last one.
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
/// What is left of your deck, as far as the deck code and the log together
/// can say.
///
/// Without it the rebuilt game has an empty deck, and every draw effect in
/// your hand reads as fatigue damage to your own hero, so the advice refuses
/// to play the card that draws.
///
/// The subtraction is approximate, and approximate in the safe direction. A
/// card played from hand has left the deck and a card in hand is not in it,
/// so both come off; a body summoned straight out of the deck by something
/// else does not, and is counted twice as present. The result is a deck of
/// roughly the right size holding roughly the right cards, which is all the
/// plan needs -- it is there to stop a draw being fatigue, not to predict the
/// top card. The search shuffles it before reading it anyway, exactly so that
/// it cannot plan around a card it has no business knowing.
fn remaining_deck(tr: &Tracker, deck: &str) -> Vec<CardId> {
    let Ok(resolved) = tavernlab_core::deckstring::resolve(deck) else {
        return Vec::new();
    };
    let mut left = resolved.ids;
    for seen in tr
        .hand
        .iter()
        .map(|b| b.card)
        .chain(tr.played[0].iter().copied())
    {
        if let Some(i) = left.iter().position(|c| *c == seen) {
            left.remove(i);
        }
    }
    left
}

fn plan(tr: &Tracker, deck: &str) -> Vec<Line> {
    let (Some(mine), Some(theirs)) = (tr.my_class(), tr.opponent_class()) else {
        return vec![Line::new("live.plan.no_classes")];
    };
    let (Ok(hp0), Ok(hp1)) = (hero_power_for(mine), hero_power_for(theirs)) else {
        return vec![Line::new("live.plan.no_hero_power")];
    };
    let mut g = match Game::new((mine, &[]), (theirs, &[]), 1) {
        Ok(g) => g,
        Err(e) => return vec![Line::new("live.plan.broken").with("why", e.to_string())],
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
        // Leaving it in would draw the plan over a board with a corpse on it.
        for b in tr.board[side].iter().filter(|b| b.stats().1 > 0) {
            let mut m = Permanent::summon(b.card);
            // Summoning sickness is kept for whatever landed this turn:
            // without it the plan swings with a minion that has only just
            // been put down, and sends a Rush minion face on the turn it
            // arrives. `Permanent::summon` sets the flag, so a body that has
            // been there since an earlier turn is the one that clears it.
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
    let deck_known = {
        let left = remaining_deck(tr, deck);
        let known = !left.is_empty();
        for card in left {
            g.players[0]
                .deck
                .push(tavernlab_core::state::DeckCard::new(card));
        }
        known
    };
    g.recompute_auras();

    // The search, not the greedy policy: live advice is one decision at a
    // time rather than a batch, so the cost that keeps the search out of the
    // engine is affordable here. The README compares the two.
    let mut agent = Planner::new(Style::Midrange, 4000, 4);
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
    while out
        .last()
        .is_some_and(|l| l.key == "live.plan.play" && arg_is(l, "card", "The Coin"))
    {
        out.pop();
    }
    if out.is_empty() {
        out.push(Line::new("live.plan.nothing"));
    }
    // Say it out loud when the mana was guessed rather than read. A plan
    // drawn at a made-up crystal count will happily spend more than you have,
    // and the reader has no way to tell that from a plan drawn at the real
    // number unless this line is here.
    if tr.crystals.is_none() {
        out.push(Line::new("live.plan.mana_guessed").with("mana", g.players[0].crystals as i64));
    }
    // An empty deck is fatigue, and fatigue makes the plan avoid every card
    // that draws. Silence about it would read as advice.
    if !deck_known {
        out.push(Line::new("live.plan.no_deck"));
    }
    out
}

/// Whether a line carries this exact literal, for the one place that has to
/// look at a line it has already built.
fn arg_is(line: &Line, name: &str, value: &str) -> bool {
    line.args
        .iter()
        .any(|(n, v)| *n == name && matches!(v, Arg::Text(t) if t == value))
}

fn describe(g: &Game, a: Action) -> Option<Line> {
    let me = g.current;
    Some(match a {
        Action::EndTurn => return None,
        Action::Play { hand, target, .. } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            let line = Line::new(match target {
                Some(_) => "live.plan.play_at",
                None => "live.plan.play",
            })
            .with("card", card.name());
            match target {
                Some(t) => line.with("target", target_name(g, t)),
                None => line,
            }
        }
        Action::Attack { from, target } => {
            let m = g.player(me).board.get(from as usize)?;
            Line::new("live.plan.attack")
                .with("card", m.card.name())
                .with("target", target_name(g, target))
        }
        Action::HeroAttack { target } => {
            Line::new("live.plan.hero_attack").with("target", target_name(g, target))
        }
        Action::HeroPower { target, .. } => match target {
            Some(t) => Line::new("live.plan.hero_power_at").with("target", target_name(g, t)),
            None => Line::new("live.plan.hero_power"),
        },
        Action::Trade { hand } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            Line::new("live.plan.trade").with("card", card.name())
        }
        Action::Prepare { hand } => {
            let card = g.player(me).hand.get(hand as usize)?.card;
            Line::new("live.plan.prepare").with("card", card.name())
        }
        Action::UseLocation { slot, .. } => {
            let m = g.player(me).board.get(slot as usize)?;
            Line::new("live.plan.location").with("card", m.card.name())
        }
    })
}

/// What a target is called, as one already-written phrase.
///
/// Composed here rather than left as parts because "your Chillwind Yeti"
/// inflects differently in the two languages, and a plan line that glues
/// "your" to a name is the shape that stops translating.
fn target_name(g: &Game, t: tavernlab_core::state::Target) -> Line {
    match t {
        tavernlab_core::state::Target::Hero(s) => Line::new(if s == g.current {
            "live.target.my_hero"
        } else {
            "live.target.their_hero"
        }),
        tavernlab_core::state::Target::Minion(s, i) => {
            let mine = s == g.current;
            match g.player(s).board.get(i as usize) {
                Some(m) => Line::new(if mine {
                    "live.target.my_minion"
                } else {
                    "live.target.their_minion"
                })
                .with("card", m.card.name()),
                None => Line::new(if mine {
                    "live.target.my_slot"
                } else {
                    "live.target.their_slot"
                })
                .with("slot", i as i64),
            }
        }
    }
}

/// Which gauntlet deck the opponent looks like, from what they have played.
fn opponent_read(app: &App, format: &str, tr: &Tracker) -> Vec<Line> {
    let Some(class) = tr.opponent_class() else {
        return vec![Line::new("live.opp.no_class")];
    };
    let who = Arg::Key(class_key(class));
    let seen: Vec<CardId> = tr.played[1].clone();
    let field = app.gauntlet(format);
    let reads = tavernlab_core::gauntlet::read_opponent(&field, class, &seen);
    if reads.is_empty() {
        return vec![Line::new("live.opp.no_decks").with("class", who)];
    }
    if seen.is_empty() {
        return vec![Line::new("live.opp.nothing_played").with("class", who)];
    }
    // `Read::frac` is 1.0 on no evidence by design, because the web UI wants
    // a neutral prior. Here that would name a deck before the opponent had
    // played a card, so a best match of nothing names nothing.
    if reads.iter().all(|r| r.hits == 0) {
        return vec![
            Line::new("live.opp.no_match")
                .with("class", who)
                .with("seen", seen.len() as i64),
        ];
    }
    let mut out = Vec::new();
    for r in reads.iter().take(3).filter(|r| r.hits > 0) {
        // The fraction and the count it came from: "43%" out of seven cards
        // is a read, out of two is a coincidence, and the line must not make
        // them look alike.
        let line = Line::new(if r.threats.is_empty() {
            "live.opp.match"
        } else {
            "live.opp.match_threats"
        })
        .with("deck", r.deck.clone())
        .with("pct", format!("{:.0}", r.frac * 100.0))
        .with("hits", r.hits as i64)
        .with("seen", r.seen as i64);
        let line = if r.threats.is_empty() {
            line
        } else {
            let names: Vec<String> = r
                .threats
                .iter()
                .take(4)
                .map(|c| format!("({}) {}", c.def().cost, c.name()))
                .collect();
            line.with("threats", names.join(", "))
        };
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
fn mulligan(app: &App, format: &str, tr: &Tracker, deck: &str) -> Vec<Line> {
    if tr.opening.is_empty() {
        return vec![Line::new("live.mull.not_dealt")];
    }
    if deck.is_empty() {
        let mut out = vec![Line::new("live.mull.no_deck")];
        out.extend(tr.opening.iter().map(|c| by_curve(*c)));
        return out;
    }
    let Some(class) = tr.opponent_class() else {
        let listed: Vec<String> = tr
            .opening
            .iter()
            .map(|c| format!("({}) {}", c.def().cost, c.name()))
            .collect();
        return vec![Line::new("live.mull.no_opp_class").with("hand", listed.join(", "))];
    };
    match app.mulligan_advice(format, deck, class, &tr.opening) {
        Ok(rows) => rows,
        Err(e) => vec![e],
    }
}

/// The only thing still true about a card when nothing has been measured:
/// what it costs. Shared with the measured path, which falls back to it.
pub fn by_curve(card: CardId) -> Line {
    let cost = card.def().cost;
    Line::new("live.mull.verdict")
        .with("verdict", Arg::Key(keep_or_toss(cost <= 3)))
        .with("cost", cost as i64)
        .with("card", card.name())
        .with("note", Arg::Key("live.mull.by_curve"))
}

pub fn keep_or_toss(keep: bool) -> &'static str {
    if keep {
        "live.mull.keep"
    } else {
        "live.mull.toss"
    }
}

/// A class as the key the app already has for it.
fn class_key(c: Class) -> &'static str {
    match class_name(c) {
        "DEATHKNIGHT" => "class.DEATHKNIGHT",
        "DEMONHUNTER" => "class.DEMONHUNTER",
        "DRUID" => "class.DRUID",
        "HUNTER" => "class.HUNTER",
        "MAGE" => "class.MAGE",
        "PALADIN" => "class.PALADIN",
        "PRIEST" => "class.PRIEST",
        "ROGUE" => "class.ROGUE",
        "SHAMAN" => "class.SHAMAN",
        "WARLOCK" => "class.WARLOCK",
        "WARRIOR" => "class.WARRIOR",
        "NEUTRAL" => "class.NEUTRAL",
        _ => "class.unknown",
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

/// Everything the tracker can currently say, built once so that the terminal
/// and the app's Live tab cannot drift apart.
pub fn build_advice(app: &App, format: &str, tr: &Tracker, deck: &str) -> Advice {
    // Straight after a `CREATE_GAME` there is a moment where nothing at all
    // has been read. A full block of empty boards and two untouched heroes
    // says nothing and reads like a position; one line is the honest size of
    // what is known.
    if tr.my_class().is_none() && tr.opponent_class().is_none() && tr.opening.is_empty() {
        return Advice {
            title: vec![Line::new("live.title.fresh")],
            sections: Vec::new(),
        };
    }
    let mut title = vec![
        Line::new(if tr.my_turn {
            "live.title.turn_mine"
        } else {
            "live.title.turn_theirs"
        })
        .with("turn", tr.turn as i64),
    ];
    title.push(match (tr.my_class(), tr.opponent_class()) {
        (Some(a), Some(b)) => Line::new("live.title.matchup")
            .with("mine", Arg::Key(class_key(a)))
            .with("theirs", Arg::Key(class_key(b))),
        (Some(a), None) => {
            Line::new("live.title.matchup_unknown").with("mine", Arg::Key(class_key(a)))
        }
        _ => Line::new("live.title.classes_unknown"),
    });
    if tr.over {
        title.push(Line::new("live.title.over"));
    }

    let mut sections: Vec<Section> = Vec::new();
    if !tr.started && !tr.opening.is_empty() {
        sections.push(section("live.head.mulligan", mulligan(app, format, tr, deck)));
        // The mulligan being on does not mean the turn is not: "started" is
        // `STEP=MAIN_READY` or a turn counter past one, and the first turn of
        // the player who goes first is turn one.
        //
        // The plan is added rather than the mulligan replaced, and `started`
        // is left alone: it also decides what counts as the opening hand, and
        // setting it from "someone is the current player" would empty every
        // recorded opening if that line arrived before the deal.
        if tr.my_turn && !tr.over {
            sections.push(section("live.head.turn", plan(tr, deck)));
        }
        return Advice { title, sections };
    }

    // The turn first: it is the thing being looked up mid-game, and a reader
    // glancing at a browser window beside the client should not have to
    // scroll past the board they can already see.
    if tr.my_turn && !tr.over {
        sections.push(section("live.head.turn", plan(tr, deck)));
    }
    sections.push(section("live.head.opponent", opponent_read(app, format, tr)));

    let mut position = Vec::new();
    match (tr.mana_left(), tr.crystals) {
        (Some(left), Some(total)) => position.push(
            Line::new("live.pos.mana")
                .with("left", left as i64)
                .with("total", total as i64),
        ),
        _ => {
            position.push(Line::new("live.pos.mana_unknown"));
            // The client does not always spell a battletag the way the
            // launcher shows it, so show what it wrote and let the user
            // pick rather than guessing.
            if !tr.names.is_empty() {
                position
                    .push(Line::new("live.pos.names").with("names", tr.names.join(", ")));
            }
        }
    }
    // A name the watcher worked out for itself is a claim, and a wrong one
    // would attribute the opponent's mana to you. Showing it is how a reader
    // can see it is right without having to know how it was found.
    if tr.me_learned && let Some(me) = tr.me_name.as_deref() {
        position.push(Line::new("live.pos.me").with("me", me));
    }
    position.push(
        Line::new("live.pos.heroes")
            .with("mine", hero_line(&tr.heroes[0]))
            .with("theirs", hero_line(&tr.heroes[1])),
    );
    position.push(Line::new("live.pos.my_board").with("board", side_line(&tr.board[0])));
    position.push(Line::new("live.pos.their_board").with("board", side_line(&tr.board[1])));
    let hand: Vec<&str> = tr.hand.iter().map(|b| b.card.name()).collect();
    position.push(match hand.is_empty() {
        true => Line::new("live.pos.hand").with("hand", Arg::Key("live.pos.empty")),
        false => Line::new("live.pos.hand").with("hand", hand.join(", ")),
    });
    sections.push(section("live.head.position", position));

    Advice { title, sections }
}

/// A heading and its lines, together.
fn section(key: &'static str, lines: Vec<Line>) -> Section {
    Section { key, lines }
}

/// A hero's health, and the armour on top of it when there is any.
fn hero_line(h: &tracker::Hero) -> Line {
    match h.armor {
        0 => Line::new("live.pos.hero").with("hp", h.health() as i64),
        n => Line::new("live.pos.hero_armour")
            .with("hp", h.health() as i64)
            .with("armour", n as i64),
    }
}

/// Print everything the tracker can currently say, in the app's language.
fn report(app: &App, format: &str, tr: &Tracker, deck: &str) {
    let advice = build_advice(app, format, tr, deck);
    let words = Locale::load(&app.root, &app.language());
    println!("\n─── {}", words.title(&advice));
    for s in &advice.sections {
        if s.lines.is_empty() {
            continue;
        }
        println!("\n  {}", words.get(s.key));
        for line in &s.lines {
            println!("    {}", words.line(line));
        }
    }
}

/// One board as a line, or the key for "empty".
///
/// Stats and keywords are the game's own notation rather than prose, so this
/// is one string in both languages -- an empty board is the only part with
/// a word in it.
fn side_line(board: &[tracker::Body]) -> Arg {
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
    if names.is_empty() {
        Arg::Key("live.pos.empty")
    } else {
        Arg::Text(names.join(", "))
    }
}

/// Fill in what was not typed.
///
/// The deck code is the one thing the watcher needs that the log does not
/// carry, and it is also the one thing the lab already knows: pasting a deck
/// into the web UI stores it in `settings.json`, which is where this reads it
/// from. A code given on the command line wins, and is written back -- so
/// passing `--deck` once is enough, and after that neither flag is typed
/// again.
fn settle_deck(app: &App, args: &mut Args) -> Option<String> {
    if !args.deck.is_empty() {
        let stored = app.settings().get("deckstring").cloned().unwrap_or_default();
        if stored != args.deck {
            // Remembered, not validated here: `--deck` is already checked
            // where it is used, and a code the watcher could not read should
            // not stop it watching.
            let _ = app.set_settings(&[("deckstring".to_string(), args.deck.clone())]);
        }
        return None;
    }
    let stored = app.settings().get("deckstring").cloned().unwrap_or_default();
    if stored.is_empty() {
        return None;
    }
    let name = app.settings().get("deck_name").cloned().unwrap_or_default();
    args.deck = stored;
    Some(if name.is_empty() {
        "колода з лабораторії".to_string()
    } else {
        format!("колода з лабораторії: {name}")
    })
}

pub fn run(app: &App, format: &str, mut args: Args) -> i32 {
    if let Some(line) = settle_deck(app, &mut args) {
        println!("{line}");
    }
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
    record(app, format, args, first.finished);
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
        record(app, format, args, batch.finished);
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

fn watching_line(file: Option<&Path>) -> String {
    match file {
        Some(p) => format!("\nстежу за логом ({}). Ctrl-C щоб вийти.", p.display()),
        None => "\nстежу за логом. Ctrl-C щоб вийти.".into(),
    }
}

/// The log-following loop, without a terminal attached.
///
/// The client starts a fresh session directory each launch and stops writing
/// to the old one, so a watcher is not tailing a file but tracking a folder.
/// Three things follow from that, and they are the whole of this type:
///
/// * it waits for the first session instead of giving up when the client is
///   not running yet;
/// * it ingests every session still on disk, so a restart does not drop the
///   games that finished in a folder the client has since rotated off;
/// * it reads the old files one last time before letting go of them, because
///   the client writes a session's final lines as it exits.
///
/// Both front ends drive it: the terminal command prints what a tick
/// produced, the web app publishes it. Neither owns the loop.
pub struct Runner {
    dir: PathBuf,
    files: Vec<PathBuf>,
    offsets: Vec<u64>,
    tr: Tracker,
    me: Option<String>,
    last: (u16, bool, usize, usize),
    /// Games that have ended since the caller last took them.
    finished: Vec<(i64, Tracker)>,
}

/// What one poll of the log directory produced.
pub enum Tick {
    /// The log has not grown.
    Quiet,
    /// No session worth reading is on disk. The client is not running, or is
    /// running with verbose logging off.
    Waiting,
    /// The client opened a new session directory, and this is now on it.
    Session(PathBuf),
    /// New lines were read. `changed` is whether the position moved, which is
    /// what decides whether advice is worth rebuilding.
    Read { changed: bool },
    /// The directory was readable and then was not. A key and its values,
    /// because this one reaches the page as well as the terminal.
    Lost(Line),
}

impl Runner {
    pub fn new(dir: PathBuf, me: Option<String>) -> Runner {
        Runner {
            dir,
            files: Vec::new(),
            offsets: Vec::new(),
            tr: Tracker::new(me.clone()),
            me,
            last: (0, false, 0, 0),
            finished: Vec::new(),
        }
    }

    /// The position as far as the log states it.
    pub fn tracker(&self) -> &Tracker {
        &self.tr
    }

    /// The Power.log currently being followed, if any.
    pub fn watching(&self) -> Option<&Path> {
        self.files.first().map(PathBuf::as_path)
    }

    /// The games that have ended since this was last called.
    pub fn take_finished(&mut self) -> Vec<(i64, Tracker)> {
        std::mem::take(&mut self.finished)
    }

    /// Read every session already on disk, oldest first.
    ///
    /// This is what makes pointing the watcher at a folder an import rather
    /// than a start: the games are already there, and replaying them is line
    /// parsing rather than simulation.
    pub fn catch_up(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        for (power, zone) in all_sessions(&self.dir) {
            let next = files_of(power, zone);
            self.tr = Tracker::new(self.me.clone());
            self.offsets.clear();
            match replay(&mut self.tr, &next, &mut self.offsets) {
                Ok(batch) => self.finished.extend(batch.finished),
                Err(e) => {
                    errors.push(format!("{}: {e}", next[0].display()));
                    continue;
                }
            }
            self.files = next;
            self.last = snapshot(&self.tr);
        }
        errors
    }

    /// One pass over the log directory.
    pub fn poll(&mut self) -> Tick {
        let Some((power, zone)) = newest_logs(&self.dir) else {
            if self.files.is_empty() {
                return Tick::Waiting;
            }
            self.files.clear();
            return Tick::Lost(
                Line::new("live.note.gone_dir").with("dir", self.dir.display().to_string()),
            );
        };
        let next = files_of(power, zone);
        if self.files.first() != next.first() {
            return self.rotate(next);
        }
        // Same Power.log; Zone.log may have appeared since.
        self.files = next;
        let batch = match replay(&mut self.tr, &self.files, &mut self.offsets) {
            Ok(b) if b.lines == 0 => return Tick::Quiet,
            Ok(b) => b,
            Err(e) => return Tick::Lost(Line::new("live.note.gone").with("why", e.to_string())),
        };
        self.finished.extend(batch.finished);
        let now = snapshot(&self.tr);
        let changed = now != self.last;
        self.last = now;
        Tick::Read { changed }
    }

    /// Move onto a session directory that was not the one being followed.
    fn rotate(&mut self, next: Vec<PathBuf>) -> Tick {
        if !self.files.is_empty()
            && let Ok(tail) = replay(&mut self.tr, &self.files.clone(), &mut self.offsets)
        {
            // The client writes a session's final lines as it exits. If the
            // next launch lands inside the same poll they are still unread,
            // which would drop the game that ended the old session.
            self.finished.extend(tail.finished);
        }
        self.tr = Tracker::new(self.me.clone());
        self.offsets.clear();
        self.files = next;
        match replay(&mut self.tr, &self.files, &mut self.offsets) {
            Ok(batch) => self.finished.extend(batch.finished),
            Err(e) => {
                let path = self.files[0].display().to_string();
                self.files.clear();
                return Tick::Lost(
                    Line::new("live.note.unreadable")
                        .with("path", path)
                        .with("why", e.to_string()),
                );
            }
        }
        self.last = snapshot(&self.tr);
        Tick::Session(self.files[0].clone())
    }
}

/// How often either front end asks the log whether it has grown.
pub const POLL: std::time::Duration = std::time::Duration::from_millis(700);

/// The terminal front end of [`Runner`].
fn live(app: &App, format: &str, args: &Args, dir: &Path) -> i32 {
    let words = Locale::load(&app.root, &app.language());
    let mut runner = Runner::new(dir.to_path_buf(), args.me.clone());
    for e in runner.catch_up() {
        eprintln!("{e}");
    }
    let mut watching = false;
    record(app, format, args, runner.take_finished());
    if runner.watching().is_none() {
        eprintln!(
            "чекаю на лог у {}. Увімкніть логування в log.config і запустіть клієнт.",
            dir.display()
        );
    } else {
        if !args.quiet {
            report(app, format, runner.tracker(), &args.deck);
        }
        println!("{}", watching_line(runner.watching()));
        watching = true;
    }

    loop {
        std::thread::sleep(POLL);
        let tick = runner.poll();
        record(app, format, args, runner.take_finished());
        match tick {
            Tick::Quiet => continue,
            Tick::Waiting => continue,
            Tick::Lost(why) => {
                if watching {
                    eprintln!("{}, чекаю знову.", words.line(&why));
                    watching = false;
                }
                continue;
            }
            Tick::Session(path) => {
                if watching {
                    println!("нова сесія: {}", path.display());
                }
                if !args.quiet {
                    report(app, format, runner.tracker(), &args.deck);
                }
                if !watching {
                    println!("{}", watching_line(runner.watching()));
                    watching = true;
                }
            }
            Tick::Read { changed } => {
                if changed && !args.quiet {
                    report(app, format, runner.tracker(), &args.deck);
                }
            }
        }
    }
}

/// Turn one finished game into the row the history keeps.
fn recorded(app: &App, format: &str, deck: &str, at: i64, tr: &Tracker) -> Option<history::Game> {
    let mine = tr.my_class().map(class_name).unwrap_or("");
    let theirs = tr.opponent_class().map(class_name).unwrap_or("");
    // A finished game with no classes at all is a log we could not attribute
    // — still a game, still recorded, rather than a silent hole in the file.
    if mine.is_empty() && theirs.is_empty() {
        eprintln!("бій завершився, але класи ще не видно — записую як є");
    }
    // The best read at the moment the game ended, and the count behind it.
    // Empty when nothing matched: a deck name with no evidence is exactly
    // what the report refuses to print, and the history keeps the same rule.
    let seen: Vec<CardId> = tr.played[1].clone();
    let best = tr.opponent_class().and_then(|class| {
        let field = app.gauntlet(format);
        tavernlab_core::gauntlet::read_opponent(&field, class, &seen)
            .into_iter()
            .find(|r| r.hits > 0)
    });
    Some(history::Game {
        played_at: at,
        my_class: mine.to_string(),
        opponent_class: theirs.to_string(),
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

/// Write whatever games have finished into the history file.
///
/// Failing to write is reported and not fatal. The watcher's job is the advice
/// on screen; a read-only data directory should cost you the record, not the
/// session.
fn record(app: &App, format: &str, args: &Args, finished: Vec<(i64, Tracker)>) {
    let path = match &args.history {
        // `--no-history` passes an empty path: play without keeping a record.
        Some(p) if p.as_os_str().is_empty() => return,
        Some(p) => Some(p.as_path()),
        None => None,
    };
    match record_games(app, format, &args.deck, path, finished) {
        Ok(0) => {}
        Ok(n) => println!(
            "\n  записано в історію: {n} {}",
            crate::games_word(n as i64)
        ),
        Err(e) => eprintln!("не вдалося записати історію: {e}"),
    }
}

/// Turn finished games into history rows and append them.
///
/// The count is what was actually new: `history::append` is idempotent, so a
/// session read twice adds nothing the second time.
pub fn record_games(
    app: &App,
    format: &str,
    deck: &str,
    path: Option<&Path>,
    finished: Vec<(i64, Tracker)>,
) -> Result<usize, String> {
    let games: Vec<history::Game> = finished
        .iter()
        .filter_map(|(at, tr)| recorded(app, format, deck, *at, tr))
        .collect();
    if games.is_empty() {
        return Ok(0);
    }
    let owned;
    let path = match path {
        Some(p) => p,
        None => {
            owned = history::default_path();
            &owned
        }
    };
    history::append(path, &games)
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
