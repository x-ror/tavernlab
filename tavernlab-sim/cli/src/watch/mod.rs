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
use tavernlab_core::game::{Action, hero_power_for};
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
    Some(resolve_logs_dir(
        Path::new(&local)
            .join("Blizzard")
            .join("Hearthstone")
            .join("Logs"),
    ))
}

/// The directory that actually holds `Hearthstone_YYYY_…` session folders.
///
/// The client writes those under `Logs/`. `log.config` sits beside that
/// folder, so the path people paste is often the install root. Follow
/// `Logs/` when it is there; leave an already-correct path alone.
pub fn resolve_logs_dir(dir: PathBuf) -> PathBuf {
    let nested = dir.join("Logs");
    if nested.is_dir() {
        nested
    } else {
        dir
    }
}

/// Where `log.config` sits for this logs directory: the install root, not
/// the session folder. `None` when this is just a path to a file, which is
/// how `--log` and the tests point at a fixture.
fn install_root(logs_dir: &Path) -> Option<&Path> {
    if logs_dir.join("log.config").is_file() {
        return Some(logs_dir);
    }
    let parent = logs_dir.parent()?;
    parent.join("log.config").is_file().then_some(parent)
}

/// Raise the client's per-file log cap, which otherwise silently kills
/// history after a handful of games.
///
/// `Power.log` writes `Truncating log, which has reached the size limit of
/// 10000KB` and then stops. Games after that line are not in any file this
/// can read. `FileSizeLimit.Int=-1` in `client.config` next to `log.config`
/// is the same switch Firestone writes; `-1` means no cap. The client reads
/// it at launch, so a session already running stays capped until restart.
///
/// `true` when the file was created or the key was changed. `false` when
/// the limit was already off, or this directory has no `log.config` to sit
/// beside. An I/O error is the only `Err`.
pub fn ensure_file_size_limit(logs_dir: &Path) -> Result<bool, String> {
    let Some(root) = install_root(logs_dir) else {
        return Ok(false);
    };
    let path = root.join("client.config");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if log_limit_is_unlimited(&existing) {
        return Ok(false);
    }
    let next = upsert_log_limit(&existing);
    std::fs::write(&path, next).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(true)
}

fn log_limit_is_unlimited(text: &str) -> bool {
    text.lines().any(|line| {
        let t = line.trim();
        t.starts_with("FileSizeLimit.Int")
            && t.rsplit('=')
                .next()
                .is_some_and(|v| v.trim() == "-1")
    })
}

fn upsert_log_limit(text: &str) -> String {
    if text.is_empty() {
        return "[Log]\nFileSizeLimit.Int=-1\n".into();
    }
    let mut out = String::new();
    let mut in_log = false;
    let mut replaced = false;
    let mut saw_log = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if in_log && !replaced {
                out.push_str("FileSizeLimit.Int=-1\n");
                replaced = true;
            }
            in_log = t.eq_ignore_ascii_case("[Log]");
            if in_log {
                saw_log = true;
            }
        } else if in_log && t.starts_with("FileSizeLimit.Int") {
            out.push_str("FileSizeLimit.Int=-1\n");
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        if in_log {
            out.push_str("FileSizeLimit.Int=-1\n");
        } else if !saw_log {
            if !out.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("\n[Log]\nFileSizeLimit.Int=-1\n");
        }
    }
    out
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
    /// The client wrote the 10 MB cap banner in this batch and stopped.
    pub capped: bool,
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
    let mut capped = false;
    for ((stamp, _, _), line) in &batch {
        let Some(ev) = log::parse(line) else { continue };
        if matches!(ev, log::Event::LogCapped) {
            capped = true;
            continue;
        }
        let was_over = tr.over;
        tr.feed(ev);
        if tr.over && !was_over {
            finished.push((stamp_to_unix(*stamp, newest, mtime), tr.clone()));
        }
    }
    Ok(Batch {
        lines,
        finished,
        capped,
    })
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

/// The position, rebuilt as far as the log states it.
///
/// Split out from the plan so that what was reconstructed can be asserted on
/// directly. The printed position beside the advice shows the tracker's own
/// reading; this is the game the search actually runs on, and the two are
/// not the same object.
///
/// `Err` is the line to show instead: the position could not be built at
/// all. The `bool` is whether the deck was restored -- see `remaining_deck`.
pub(crate) fn position(tr: &Tracker, deck: &str) -> Result<(Game, bool), Line> {
    let (Some(mine), Some(theirs)) = (tr.my_class(), tr.opponent_class()) else {
        return Err(Line::new("live.plan.no_classes"));
    };
    let (Ok(hp0), Ok(hp1)) = (hero_power_for(mine), hero_power_for(theirs)) else {
        return Err(Line::new("live.plan.no_hero_power"));
    };
    let mut g = match Game::new((mine, &[]), (theirs, &[]), 1) {
        Ok(g) => g,
        Err(e) => return Err(Line::new("live.plan.broken").with("why", e.to_string())),
    };
    g.players[0].hero_power = hp0;
    g.players[1].hero_power = hp1;
    for i in 0..2 {
        g.players[i].hero_hp = tr.heroes[i].health();
        g.players[i].armor = tr.heroes[i].armor;
        // A hero that has already swung cannot swing again, and a plan that
        // offers the attack twice is offering one that does not exist.
        g.players[i].hero_attacks_done = tr.heroes[i].attacks;
        // A Hero Power already pressed this turn is not a play the plan may
        // offer again. The log writes `EXHAUSTED` on the power's own entity;
        // without it every position looked freshly untouched.
        if tr.hero_powers[i].as_ref().is_some_and(|p| p.exhausted) {
            g.players[i].hero_power_uses = 1;
        }
        // The weapon as the log has it: the printed card where it said
        // nothing, what it said where it did. Without this the rebuilt hero
        // has bare hands, and the plan never suggests the swing -- which for
        // half the classes is most of what a turn is.
        // Yours by name; theirs never. A secret the log did not name is
        // not a secret this may pick: filling the opponent's zone from what
        // their deck usually plays would be a card the log never said, and
        // the plan would be drawn around a guess. The caveat below says so
        // instead.
        //
        // Yours go in because the position is meant to be the position, and
        // the engine's own rules then apply to it -- a secret already in the
        // zone cannot be set again. Whether that ever changes a plan is
        // another matter: see `live.plan.secret_in_hand`.
        if i == 0 {
            for card in tr.secrets[0].iter().filter_map(|s| s.card) {
                if g.players[0].secrets.len() < tavernlab_core::state::MAX_SECRETS
                    && !g.players[0].secrets.contains(&card)
                {
                    g.players[0].secrets.push(card);
                }
            }
        }
        g.players[i].weapon = tr.weapons[i].map(|w| {
            let (atk, durability) = w.stats();
            tavernlab_core::state::Weapon {
                card: w.card,
                atk,
                durability,
            }
        });
    }
    // Without a battletag the log's mana lines cannot be attributed, so the
    // plan is drawn at the turn's worth of crystals rather than at a made-up
    // number, and says that it guessed.
    //
    // Each player gains a crystal at the start of their own turn, and the
    // two alternate -- turns 1 and 2 are the first for their respective
    // players, 3 and 4 the second. So the count is the same for both sides
    // and is `ceil(turn / 2)`, which the old `turn / 2 + 1` overstated by one
    // on every even turn: it gave the player on the draw two crystals on
    // turn two, and a plan that spends what you do not have.
    // The Death Knight resource, when the log said it. Zero where it did
    // not, which is the same thing a fresh game has -- but a plan drawn at
    // zero for a deck built on Corpses spends none of what it has.
    g.players[0].corpses = tr.corpses.unwrap_or(0);
    g.players[0].crystals = tr
        .crystals
        .unwrap_or_else(|| ((tr.turn as i16 + 1) / 2).clamp(1, 10));
    g.players[0].mana = tr.mana_left().unwrap_or(g.players[0].crystals);
    g.turn = tr.turn;
    // What the log stated outright, kept so it can be put back after the
    // engine has had its say about auras. See below.
    let mut stated: Vec<(usize, usize, Option<i16>, Option<i16>)> = Vec::new();
    for side in 0..2 {
        // A body the log has taken to zero health is dead; the line that
        // moves it out of play arrives in a later batch, up to a poll behind.
        // Leaving it in would draw the plan over a board with a corpse on it.
        // Zero health is dead -- for a minion. A Location prints no Health
        // at all (its own number is durability, in another field), so the
        // same test read every Location as a corpse and left it off the
        // board entirely.
        for b in tr.board[side].iter().filter(|b| {
            b.card.def().kind() == tavernlab_core::cards::Kind::Location || b.stats().1 > 0
        }) {
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
            // A Location used this turn cannot be used again, and the log
            // says so with the same `EXHAUSTED` a spent Hero Power carries.
            if b.exhausted && b.card.def().kind() == tavernlab_core::cards::Kind::Location {
                m.flags.insert(tavernlab_core::state::Flags::USED);
            }
            stated.push((side, g.players[side].board.len(), b.atk, b.hp));
            g.players[side].board.push(m);
        }
    }
    for b in tr.hand.iter() {
        let mut hc = HandCard::new(b.card);
        // What the client says it costs now, over what the card prints:
        // discounts and taxes are already folded into the logged number, and
        // the engine carries the difference per copy.
        if let Some(cost) = b.cost {
            hc.cost_delta = cost - b.card.def().cost;
        }
        g.players[0].hand.push(hc);
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
    // The engine keeps a minion's aura share inside its total: `atk`
    // includes `aura_atk`, and `recompute_auras` subtracts the old share
    // before adding the new one. The log's `ATK` and `HEALTH` are the
    // client's totals and already include whatever auras were up, so a body
    // placed with the logged number and no share recorded gets its aura
    // counted twice -- once by the client, once here. A buffed minion came
    // out fatter in the plan than on screen.
    //
    // So the engine works out what it thinks the auras are, and then the
    // stated totals go back on top of that: `atk` is the client's number,
    // `aura_atk` is the engine's share of it, and the invariant holds again.
    //
    // Only where the log actually stated a number. A body it said nothing
    // about carries printed stats, which do *not* include auras -- there the
    // engine adding them is right, and putting the printed number back would
    // strip an aura the minion really has.
    g.recompute_auras();
    for (side, slot, atk, hp) in stated {
        let Some(m) = g.players[side].board.get_mut(slot) else {
            continue;
        };
        if let Some(a) = atk {
            m.atk = a;
        }
        if let Some(h) = hp {
            m.max_hp = h;
        }
    }
    Ok((g, deck_known))
}

fn plan(format: &str, tr: &Tracker, deck: &str) -> Vec<Line> {
    let mut caveats = Vec::new();
    if tr.whose_turn().is_none() {
        caveats.push(Line::new("live.plan.whose_turn_unknown"));
    }
    let (mut g, deck_known) = match position(tr, deck) {
        Ok(pair) => pair,
        Err(why) => return vec![why],
    };
    // A position rebuilt from the log has no full decks, so `Game::new`
    // could not infer a format and the Discover pools would span every
    // format at once. The session knows what it is playing; say so. An
    // Arena game gets the season pool when the corpus carries one, else
    // the whole corpus — too wide is honest, wrongly narrow is not.
    g.format = if tr.is_arena() {
        if tavernlab_core::cards::arena_pool_present() {
            tavernlab_core::cards::Formats::ARENA
        } else {
            tavernlab_core::cards::Formats::ANY
        }
    } else {
        match format {
            "wild" => tavernlab_core::cards::Formats::WILD,
            _ => tavernlab_core::cards::Formats::STANDARD,
        }
    };

    // The search, not the greedy policy: live advice is one decision at a
    // time rather than a batch, so the cost that keeps the search out of the
    // engine is affordable here. The README compares the two.
    //
    // depth=4, budget=4000 (the old numbers) meant an eight-action winning
    // line -- a wide board with a weapon and several attackers, exactly the
    // shape of a real turn -- could never reach `Game::is_over()` inside the
    // search: `search()` falls back to the non-terminal heuristic at
    // `depth == 0`, which has no way to know a line is lethal, so the search
    // preferred a locally-better trade over a real kill. Below is called up
    // to 16 times per poll (`for _ in 0..16` above); a scratch benchmark on a
    // synthetic wide board (`core/src/planner.rs`'s `scratch_timing` test,
    // since removed) measured budget=50_000/depth=10 at ~15ms per call, so
    // ~240ms worst case per poll -- still affordable against the 1s poll
    // interval `web/src/routes/Live.jsx` uses.
    let mut agent = Planner::new(Style::Midrange, 50_000, 10);
    let mut out = Vec::new();
    let mut legal: tavernlab_core::inline::Inline<Action, 512> =
        tavernlab_core::inline::Inline::new();
    // Walk the turn the way the engine would play it, stopping when the
    // agent decides it is done or nothing legal is left.
    //
    // A minion attacking past its own `max_attacks()` (1, or 2 with
    // Windfury) or the hero attacking twice is not a real Hearthstone turn,
    // so this can never suppress a legitimate line -- it only catches the
    // search re-offering an attack that should already be spent. That
    // contradiction has been observed in recorded advice with no
    // reproduction found in the engine itself; stopping the walk here is
    // safer than printing a plan the game could never actually play.
    let mut attacks_this_turn = [0u8; tavernlab_core::state::MAX_BOARD];
    let mut hero_attacked = false;
    for _ in 0..16 {
        g.legal_actions(&mut legal);
        if legal.is_empty() {
            break;
        }
        let (action, runner_up) = agent.choose_ranked(&g, legal.as_slice());
        match action {
            Action::Attack { from, .. }
                if attacks_this_turn[from as usize] >= g.players[0].board[from as usize].max_attacks() =>
            {
                break;
            }
            Action::HeroAttack { .. } if hero_attacked => break,
            _ => {}
        }
        let Some(line) = describe(&g, action) else {
            break;
        };
        out.push(line);
        // Named from the search's own numbers -- the runner-up action and
        // how far behind it scored -- so a reader can see *why* this beat
        // the alternative rather than take the plan on faith.
        if let Some((alt, margin)) = runner_up
            && let Some(alt_line) = describe(&g, alt)
        {
            out.push(
                Line::new("live.plan.alt")
                    .with("alt", alt_line)
                    .with("margin", format!("{margin:.1}")),
            );
        }
        match action {
            Action::Attack { from, .. } => attacks_this_turn[from as usize] += 1,
            Action::HeroAttack { .. } => hero_attacked = true,
            _ => {}
        }
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
    out.extend(caveats);
    // The opponent's secrets are known to exist and not known to be
    // anything. The plan is drawn without them, and the reader is the one
    // who can play around what the plan cannot see.
    if !tr.secrets[1].is_empty() {
        out.push(Line::new("live.plan.their_secrets").with("n", tr.secrets[1].len() as i64));
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
    // An Arena opponent drafted their deck; matching their cards against the
    // constructed gauntlet would print "Quest Mage 43%" about a deck that
    // does not exist. Until an Arena field is built, the honest read is none.
    if tr.is_arena() {
        return vec![Line::new("live.opp.arena")];
    }
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
            line.with("threats", crate::names::list(&names))
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
    // An Arena game is measured against the generated Arena field, never
    // the constructed gauntlet. With no deck list, or no field at all (a
    // corpus built without a season pool), the curve of the hand is what
    // remains.
    let format = if tr.is_arena() { "arena" } else { format };
    if tr.is_arena() && (deck.is_empty() || app.gauntlet("arena").is_empty()) {
        let mut out = vec![Line::new("live.mull.arena")];
        out.extend(tr.opening.iter().map(|c| by_curve(*c)));
        return out;
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
        return vec![Line::new("live.mull.no_opp_class").with("hand", crate::names::list(&listed))];
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
    let mine = tr.whose_turn();
    let mut title = vec![
        Line::new(match mine {
            Some(true) => "live.title.turn_mine",
            Some(false) => "live.title.turn_theirs",
            None => "live.title.turn_unknown",
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
        // A positive answer only. Before the game starts there is no turn to
        // plan, and "nothing to do this turn" during the mulligan is noise
        // rather than advice -- unlike mid-game, where not knowing whose
        // turn it is still leaves a plan worth showing.
        if mine == Some(true) && !tr.over {
            sections.push(section("live.head.turn", plan(format, tr, deck)));
        }
        return Advice { title, sections };
    }

    // The turn first: it is the thing being looked up mid-game, and a reader
    // glancing at a browser window beside the client should not have to
    // scroll past the board they can already see.
    // Silence is the one answer that helps nobody. When the log never said
    // whose turn it is and the opening did not settle it either, the plan is
    // still what you would do on your turn -- so it is shown, and says that
    // it could not tell.
    if mine != Some(false) && !tr.over {
        sections.push(section("live.head.turn", plan(format, tr, deck)));
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
    // Only when there is one. A line saying you have no weapon is true of
    // every turn of every game that is not about weapons.
    for (i, key) in [(0, "live.pos.my_weapon"), (1, "live.pos.their_weapon")] {
        if let Some(w) = tr.weapons[i] {
            let (atk, dur) = w.stats();
            position.push(
                Line::new(key)
                    .with("card", w.card.name())
                    .with("atk", atk as i64)
                    .with("durability", dur as i64),
            );
        }
    }
    // Named where the log named them, counted where it did not.
    if !tr.secrets[0].is_empty() {
        let named: Vec<&str> = tr.secrets[0].iter().filter_map(|s| s.card).map(|c| c.name()).collect();
        position.push(match named.len() == tr.secrets[0].len() {
            true => Line::new("live.pos.my_secrets").with("secrets", crate::names::list(&named)),
            false => Line::new("live.pos.my_secrets_count").with("n", tr.secrets[0].len() as i64),
        });
    }
    if !tr.secrets[1].is_empty() {
        position.push(Line::new("live.pos.their_secrets").with("n", tr.secrets[1].len() as i64));
    }
    if let Some(n) = tr.corpses {
        position.push(Line::new("live.pos.corpses").with("n", n as i64));
    }
    position.push(Line::new("live.pos.my_board").with("board", side_line(&tr.board[0])));
    position.push(Line::new("live.pos.their_board").with("board", side_line(&tr.board[1])));
    let hand: Vec<&str> = tr.hand.iter().map(|b| b.card.name()).collect();
    position.push(match hand.is_empty() {
        true => Line::new("live.pos.hand").with("hand", Arg::Key("live.pos.empty")),
        false => Line::new("live.pos.hand").with("hand", crate::names::list(&hand)),
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
        // showing it would make the advice look like it ignored a minion --
        // and, as there, a Location prints no Health and is not a corpse for
        // having none.
        .filter(|b| {
            b.card.def().kind() == tavernlab_core::cards::Kind::Location || b.stats().1 > 0
        })
        .map(|b| {
            // A Location has no Attack and no Health; what it carries is
            // durability, and `0/0` would be two numbers nobody wrote. The
            // log's own figure where it gave one, the printed one otherwise
            // -- the same rule everything else here follows. No word for
            // "location": the shape of the entry is the word, and prose in
            // this line would be prose the page cannot translate.
            if b.card.def().kind() == tavernlab_core::cards::Kind::Location {
                let left = b.durability.unwrap_or(b.card.def().dur);
                return format!("{} ({left})", b.card.name());
            }
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
        Arg::Text(crate::names::list(&names))
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
    let Some(dir) = args
        .logs_dir
        .clone()
        .or_else(default_logs_dir)
        .map(resolve_logs_dir)
    else {
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
    if first.capped {
        warn_capped(app);
    }
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
        if batch.capped {
            warn_capped(app);
        }
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
    /// The file currently being followed wrote the client's size-cap banner.
    capped: bool,
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
            dir: resolve_logs_dir(dir),
            files: Vec::new(),
            offsets: Vec::new(),
            tr: Tracker::new(me.clone()),
            me,
            last: (0, false, 0, 0),
            finished: Vec::new(),
            capped: false,
        }
    }

    /// Whether the log being followed has hit the client's size cap.
    pub fn log_capped(&self) -> bool {
        self.capped
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
                Ok(batch) => {
                    self.finished.extend(batch.finished);
                    // The newest session is last; its cap is the one that
                    // matters for the file still being followed.
                    self.capped = batch.capped;
                }
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
            self.capped = false;
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
        self.capped |= batch.capped;
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
            self.capped |= tail.capped;
        }
        self.tr = Tracker::new(self.me.clone());
        self.offsets.clear();
        self.files = next;
        self.capped = false;
        match replay(&mut self.tr, &self.files, &mut self.offsets) {
            Ok(batch) => {
                self.finished.extend(batch.finished);
                self.capped = batch.capped;
            }
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
    if let Err(e) = ensure_file_size_limit(dir) {
        eprintln!("{e}");
    }
    let mut runner = Runner::new(dir.to_path_buf(), args.me.clone());
    for e in runner.catch_up() {
        eprintln!("{e}");
    }
    let mut watching = false;
    let mut saw_cap = false;
    record(app, format, args, runner.take_finished());
    if runner.log_capped() {
        warn_capped(app);
        saw_cap = true;
    }
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
        if runner.log_capped() && !saw_cap {
            warn_capped(app);
            saw_cap = true;
        }
        if !runner.log_capped() {
            saw_cap = false;
        }
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
    // A drafted opponent matches no constructed gauntlet deck; a name with
    // no deck behind it must not end up in the history either.
    let best = if tr.is_arena() {
        None
    } else {
        tr.opponent_class().and_then(|class| {
            let field = app.gauntlet(format);
            tavernlab_core::gauntlet::read_opponent(&field, class, &seen)
                .into_iter()
                .find(|r| r.hits > 0)
        })
    };
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
        game_type: tr.game_type.clone().unwrap_or_default(),
        format_type: tr.format_type.clone().unwrap_or_default(),
    })
}

/// Write whatever games have finished into the history file.
///
/// Failing to write is reported and not fatal. The watcher's job is the advice
/// on screen; a read-only data directory should cost you the record, not the
/// session.
fn warn_capped(app: &App) {
    let words = Locale::load(&app.root, &app.language());
    eprintln!("{}", words.line(&Line::new("live.note.log_capped")));
}

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
mod position_tests {
    use super::*;
    use crate::watch_mod::log::parse;

    fn tracked(lines: &[&str]) -> Tracker {
        let mut t = Tracker::new(Some("Me#1".into()));
        for l in lines {
            if let Some(ev) = parse(l) {
                t.feed(ev);
            }
        }
        t
    }

    const HEROES: [&str; 3] = [
        "D 09:00:00.0 [Power] GameState.DebugPrintPower() - CREATE_GAME",
        "D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=1 local=False [entityName=Jaina Proudmoore id=64 zone=PLAY zonePos=0 cardId=HERO_08 player=1] zone from  -> FRIENDLY PLAY (Hero)",
        "D 09:00:00.1 [Zone] ZoneChangeList.ProcessChanges() - id=2 local=False [entityName=Garrosh Hellscream id=65 zone=PLAY zonePos=0 cardId=HERO_01 player=2] zone from  -> OPPOSING PLAY (Hero)",
    ];

    /// The log's ATK already carries the aura; the engine must not add it
    /// again.
    ///
    /// A Raid Leader gives friendly minions +1 Attack. The client writes the
    /// Raptor as 4, its printed 3 plus that 1. Placed with 4 and no aura
    /// share recorded, `recompute_auras` added the Raid Leader's bonus on top
    /// and the search planned around a 5/2 that is not on the board.
    #[test]
    fn an_aura_inside_the_logs_number_is_not_counted_twice() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Raid Leader id=20 zone=HAND zonePos=1 cardId=CS2_122 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY");
        lines.push("D 09:00:02.1 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Bloodfen Raptor id=21 zone=HAND zonePos=1 cardId=CS2_172 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY");
        lines.push("D 09:00:02.2 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Bloodfen Raptor id=21 zone=PLAY zonePos=1 cardId=CS2_172 player=1] tag=ATK value=4");
        let tr = tracked(&lines);
        let (g, _) = position(&tr, "").expect("a position");
        let raptor = g.players[0]
            .board
            .iter()
            .find(|m| m.card.name() == "Bloodfen Raptor")
            .expect("the Raptor is on the board");
        assert_eq!(raptor.atk, 4, "the client's total, not the total plus the aura again");
    }

    /// A body the log said nothing about is its printed self, and the aura
    /// does apply to it -- restoring a printed number would strip one the
    /// minion really has.
    #[test]
    fn an_aura_still_reaches_a_body_the_log_was_silent_about() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Raid Leader id=20 zone=HAND zonePos=1 cardId=CS2_122 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY");
        lines.push("D 09:00:02.1 [Zone] ZoneChangeList.ProcessChanges() - id=8 local=False [entityName=Bloodfen Raptor id=21 zone=HAND zonePos=1 cardId=CS2_172 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY");
        let tr = tracked(&lines);
        let (g, _) = position(&tr, "").expect("a position");
        let raptor = g.players[0]
            .board
            .iter()
            .find(|m| m.card.name() == "Bloodfen Raptor")
            .expect("the Raptor is on the board");
        assert_eq!(raptor.atk, 4, "printed 3 and the Raid Leader's +1");
    }

    /// A token sized in SETASIDE is that size once it lands, so one damage
    /// is not a corpse.
    ///
    /// Twilight Egg's Whelp is created at printed 2/1, then tagged 3/2
    /// before Zone.log moves it into PLAY. The plan used to drop those
    /// tags, read 2/1 minus one as zero health, and say there was nothing
    /// to do on a turn the Whelp could still attack.
    #[test]
    fn a_token_sized_before_it_lands_is_not_a_corpse_after_one_damage() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=4");
        lines.push("D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=100 tag=ATK value=3");
        lines.push("D 09:00:01.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=100 tag=HEALTH value=2");
        lines.push("D 09:00:01.2 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Accelerated Whelp id=100 zone=PLAY zonePos=0 cardId=CATA_210t player=1] zone from  -> FRIENDLY PLAY");
        lines.push("D 09:00:02.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:02.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Accelerated Whelp id=100 zone=PLAY zonePos=1 cardId=CATA_210t player=1] tag=DAMAGE value=1");
        let tr = tracked(&lines);
        assert_eq!(
            tr.board[0][0].stats(),
            (3, 1),
            "the tracker's own reading, before the plan"
        );
        let (g, _) = position(&tr, "").expect("a position");
        let whelp = g.players[0]
            .board
            .iter()
            .find(|m| m.card.name() == "Accelerated Whelp")
            .expect("the Whelp is on the rebuilt board, not filtered as a corpse");
        assert_eq!(whelp.atk, 3);
        assert_eq!(whelp.health(), 1);
        assert!(whelp.can_attack(), "it landed on an earlier turn");
    }

    /// A discounted card in hand is discounted in the plan.
    ///
    /// The log writes what a card costs now, taxes and discounts folded in.
    /// The plan used to read the printed cost, so it either refused a play
    /// the turn could afford or offered one it could not.
    #[test]
    fn the_cost_the_log_wrote_is_the_cost_the_plan_pays() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Fireball id=30 zone=DECK zonePos=0 cardId=CS2_029 player=1] zone from FRIENDLY DECK -> FRIENDLY HAND");
        lines.push("D 09:00:02.1 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Fireball id=30 zone=HAND zonePos=1 cardId=CS2_029 player=1] tag=COST value=1");
        let tr = tracked(&lines);
        let (g, _) = position(&tr, "").expect("a position");
        let hc = g.players[0].hand.first().expect("Fireball in hand");
        assert_eq!(hc.card.name(), "Fireball");
        assert_eq!(
            hc.card.def().cost + hc.cost_delta,
            1,
            "printed four, the log says one"
        );
    }

    /// A Hero Power already pressed is not a play the plan may offer again.
    #[test]
    fn a_spent_hero_power_is_spent_in_the_position() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:00.5 [Zone] ZoneChangeList.ProcessChanges() - id=5 local=False [entityName=Fireblast id=66 zone=PLAY zonePos=0 cardId=CS2_034 player=1] zone from  -> FRIENDLY PLAY (Hero Power)");
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        let fresh = tracked(&lines);
        let (g, _) = position(&fresh, "").expect("a position");
        assert_eq!(g.players[0].hero_power_uses, 0, "not pressed yet");

        lines.push("D 09:00:01.5 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Fireblast id=66 zone=PLAY zonePos=0 cardId=CS2_034 player=1] tag=EXHAUSTED value=1");
        let spent = tracked(&lines);
        let (g, _) = position(&spent, "").expect("a position");
        assert_eq!(g.players[0].hero_power_uses, 1, "the log said it was used");
    }

    /// A Location is a play, so it belongs on the board.
    ///
    /// The engine offers `UseLocation` for one in play, and the zone branch
    /// used to drop everything that was not a minion or a weapon -- so a
    /// whole play was missing from every turn that had one.
    #[test]
    fn a_location_is_on_the_board_and_can_be_used() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:02.0 [Zone] ZoneChangeList.ProcessChanges() - id=9 local=False [entityName=Ruby Sanctum id=40 zone=HAND zonePos=1 cardId=CATA_301 player=1] zone from FRIENDLY HAND -> FRIENDLY PLAY");
        let tr = tracked(&lines);
        let (g, _) = position(&tr, "").expect("a position");
        let m = g.players[0].board.first().expect("the Location is in play");
        assert_eq!(m.card.name(), "Ruby Sanctum");
        assert!(
            !m.flags.has(tavernlab_core::state::Flags::USED),
            "and it has not been used yet"
        );

        lines.push("D 09:00:02.5 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=[entityName=Ruby Sanctum id=40 zone=PLAY zonePos=1 cardId=CATA_301 player=1] tag=EXHAUSTED value=1");
        let spent = tracked(&lines);
        let (g, _) = position(&spent, "").expect("a position");
        assert!(
            g.players[0].board[0]
                .flags
                .has(tavernlab_core::state::Flags::USED),
            "used this turn, so not a play the plan may offer again"
        );
    }

    /// The Corpses the log banked reach the game the search runs on.
    #[test]
    fn the_corpses_reach_the_rebuilt_game() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        lines.push("D 09:00:01.5 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=Me#1 tag=CORPSES value=3");
        let tr = tracked(&lines);
        let (g, _) = position(&tr, "").expect("a position");
        assert_eq!(g.players[0].corpses, 3);
    }

    /// `plan`'s attack-once guard (added after recorded advice was seen
    /// recommending the same non-Windfury minion attacking twice in one
    /// turn) must not cap a real Windfury minion at a single swing.
    #[test]
    fn a_windfury_minion_is_still_planned_to_attack_twice() {
        let mut lines = HEROES.to_vec();
        // Landed before `TURN` advances to 7, so summoning sickness is gone.
        lines.push("D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Windfury Harpy id=100 zone=PLAY zonePos=0 cardId=EX1_033 player=1] zone from  -> FRIENDLY PLAY");
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        let tr = tracked(&lines);
        let out = plan("standard", &tr, "");
        let harpy_attacks = out
            .iter()
            .filter(|l| {
                l.key == "live.plan.attack"
                    && l.args.iter().any(|(n, v)| {
                        *n == "card" && matches!(v, Arg::Text(t) if t == "Windfury Harpy")
                    })
            })
            .count();
        assert_eq!(harpy_attacks, 2, "a Windfury minion may attack twice");
    }

    /// A plan step with a real alternative names it and how far behind it
    /// scored, from the search's own numbers -- so a reader can see why the
    /// engine picked one action over another instead of taking it on faith.
    #[test]
    fn a_plan_step_with_an_alternative_names_it_and_the_margin() {
        let mut lines = HEROES.to_vec();
        lines.push("D 09:00:00.2 [Zone] ZoneChangeList.ProcessChanges() - id=7 local=False [entityName=Windfury Harpy id=100 zone=PLAY zonePos=0 cardId=EX1_033 player=1] zone from  -> FRIENDLY PLAY");
        lines.push("D 09:00:01.0 [Power] GameState.DebugPrintPower() - TAG_CHANGE Entity=GameEntity tag=TURN value=7");
        let tr = tracked(&lines);
        let out = plan("standard", &tr, "");
        let alts: Vec<&Line> = out.iter().filter(|l| l.key == "live.plan.alt").collect();
        assert!(!alts.is_empty(), "a position with more than one legal action should name a runner-up");
        for l in alts {
            assert!(
                l.args.iter().any(|(n, _)| *n == "alt"),
                "the runner-up line must name the alternative action"
            );
            let margin: f32 = l
                .args
                .iter()
                .find_map(|(n, v)| (*n == "margin").then_some(v))
                .and_then(|v| match v {
                    Arg::Text(s) => s.parse().ok(),
                    _ => None,
                })
                .expect("the runner-up line must carry a parseable margin");
            assert!(margin >= 0.0, "the chosen action must score at least as well as the runner-up");
        }
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

    #[test]
    fn the_install_root_is_followed_into_logs() {
        let root = scratch("install");
        let logs = root.join("Logs");
        write_power(&logs, "real", "CREATE_GAME\n", true);
        let (power, _) =
            newest_logs(&resolve_logs_dir(root.clone())).expect("session under Logs");
        assert!(power.starts_with(&logs), "{}", power.display());
        let (again, _) =
            newest_logs(&resolve_logs_dir(logs.clone())).expect("already the Logs dir");
        assert_eq!(power, again);
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn an_empty_file_becomes_the_log_section() {
        assert_eq!(upsert_log_limit(""), "[Log]\nFileSizeLimit.Int=-1\n");
    }

    #[test]
    fn an_existing_section_keeps_its_other_keys() {
        let src = "[Aurora]\nClientCheck=false\n\n[Log]\nFilePrinting=true\n";
        let out = upsert_log_limit(src);
        assert!(out.contains("ClientCheck=false"), "{out}");
        assert!(out.contains("FilePrinting=true"), "{out}");
        assert!(out.contains("FileSizeLimit.Int=-1"), "{out}");
        assert!(log_limit_is_unlimited(&out));
    }

    #[test]
    fn a_default_cap_is_replaced() {
        let src = "[Log]\nFileSizeLimit.Int=10000\n";
        assert!(!log_limit_is_unlimited(src));
        assert_eq!(upsert_log_limit(src), "[Log]\nFileSizeLimit.Int=-1\n");
    }

    #[test]
    fn already_unlimited_is_left_alone() {
        assert!(log_limit_is_unlimited("[Log]\nFileSizeLimit.Int=-1\n"));
    }

    #[test]
    fn writing_sits_beside_log_config() {
        let dir = std::env::temp_dir().join(format!(
            "tavernlab-limit-{}",
            std::process::id()
        ));
        let logs = dir.join("Logs");
        std::fs::create_dir_all(&logs).expect("install");
        std::fs::write(dir.join("log.config"), "[Power]\nFilePrinting=true\n").expect("log.config");
        let _ = std::fs::remove_file(dir.join("client.config"));
        assert!(
            ensure_file_size_limit(&logs).expect("write"),
            "first call creates client.config"
        );
        let cfg = std::fs::read_to_string(dir.join("client.config")).expect("read");
        assert!(cfg.contains("FileSizeLimit.Int=-1"), "{cfg}");
        assert!(
            !ensure_file_size_limit(&logs).expect("again"),
            "second call is a no-op"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_folder_without_log_config_is_not_an_install() {
        let dir = std::env::temp_dir().join(format!(
            "tavernlab-nolimit-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch");
        let _ = std::fs::remove_file(dir.join("client.config"));
        assert!(!ensure_file_size_limit(&dir).expect("skip"));
        assert!(
            !dir.join("client.config").exists(),
            "must not invent a client.config away from log.config"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
