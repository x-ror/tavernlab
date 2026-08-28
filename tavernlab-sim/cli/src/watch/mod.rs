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

pub mod log;
pub mod tracker;

use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use tavernlab_core::agent::{Scripted, Style};
use tavernlab_core::cards::{CardId, Class, Keywords};
use tavernlab_core::game::{Action, Agent, hero_power_for};
use tavernlab_core::state::{Game, HandCard, Permanent};

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

/// The newest session directory that holds a log worth reading.
fn newest_logs(dir: &Path) -> Option<(PathBuf, Option<PathBuf>)> {
    let mut best: Option<(std::time::SystemTime, PathBuf, Option<PathBuf>)> = None;
    for entry in std::fs::read_dir(dir).ok()? {
        let Ok(entry) = entry else { continue };
        let power = entry.path().join("Power.log");
        let Ok(meta) = std::fs::metadata(&power) else {
            continue;
        };
        if meta.len() < MIN_POWER_BYTES {
            continue;
        }
        let Ok(when) = meta.modified() else { continue };
        let zone = entry.path().join("Zone.log");
        let zone = zone.exists().then_some(zone);
        if best.as_ref().is_none_or(|(b, _, _)| when >= *b) {
            best = Some((when, power, zone));
        }
    }
    best.map(|(_, p, z)| (p, z))
}

/// Read the new lines of both files into one tracker, in the order the client
/// wrote them.
///
/// Chronological, not file by file. Only Power.log carries `CREATE_GAME`, so
/// replaying one file and then the other puts every reset at the front and
/// then lays every zone move of every game in the session on top of the last
/// one — finished games' boards piling up on the current one, and their
/// heroes deciding the classes. That is what the first real log showed.
fn replay(tr: &mut Tracker, files: &[PathBuf], offsets: &mut Vec<u64>) -> std::io::Result<usize> {
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
    for (_, line) in &batch {
        if let Some(ev) = log::parse(line) {
            tr.feed(ev);
        }
    }
    Ok(lines)
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
        for b in tr.board[side].iter() {
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
    pub log_file: Option<PathBuf>,
    pub deck: String,
    /// Battletag, `Name#12345`. The log names both players and never says
    /// which is you.
    pub me: Option<String>,
    pub once: bool,
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
    let files: Vec<PathBuf> = if let Some(one) = args.log_file.clone() {
        vec![one]
    } else {
        let Some(dir) = args.logs_dir.clone().or_else(default_logs_dir) else {
            eprintln!(
                "не знаю, де логи гри. Вкажіть --logs <тека> або змінну HS_LOGS.\n\
                 Логування вмикається у log.config поруч із теками Logs."
            );
            return 2;
        };
        match newest_logs(&dir) {
            Some((power, Some(zone))) => vec![power, zone],
            Some((power, None)) => vec![power],
            None => {
                eprintln!(
                    "у {} немає жодного Power.log більшого за {MIN_POWER_BYTES} байт.\n\
                     Схоже, детальне логування вимкнене: увімкніть його в log.config \
                     і перезапустіть клієнт.",
                    dir.display()
                );
                return 2;
            }
        }
    };

    let mut tr = Tracker::new(args.me.clone());
    let mut offsets = Vec::new();
    if let Err(e) = replay(&mut tr, &files, &mut offsets) {
        eprintln!("не вдалося прочитати лог: {e}");
        return 2;
    }
    report(app, format, &tr, &args.deck);
    if args.once {
        return 0;
    }

    println!("\nстежу за логом. Ctrl-C щоб вийти.");
    let mut last = (tr.turn, tr.my_turn, tr.hand.len(), tr.board[1].len());
    loop {
        std::thread::sleep(std::time::Duration::from_millis(700));
        match replay(&mut tr, &files, &mut offsets) {
            Ok(0) => continue,
            Ok(_) => {}
            Err(e) => {
                eprintln!("лог зник: {e}");
                return 2;
            }
        }
        let now = (tr.turn, tr.my_turn, tr.hand.len(), tr.board[1].len());
        if now != last {
            last = now;
            report(app, format, &tr, &args.deck);
        }
    }
}
