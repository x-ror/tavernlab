//! `tavernsim` — the batch simulator.
//!
//! Commands:
//!
//! ```text
//! tavernsim serve [port]              the local app: web UI + API
//! tavernsim bench [games] [threads]   throughput against a fixed mirror match
//! tavernsim matrix [games]            every class against every class
//! tavernsim demo [seed]               one game, turn by turn
//! tavernsim coverage                  how much of the card pool is implemented
//! tavernsim gauntlet [path]           how much of real deck lists resolves
//! tavernsim backlog <class>           exactly which Standard cards are missing
//! tavernsim art-urls [--heroes]       where to fetch the card art cache from
//! ```

use std::time::Instant;

use tavernlab_core::agent::{Scripted, Style};
use tavernlab_core::batch::{Contender, play_batch, play_batch_parallel, seeds};
use tavernlab_core::cards::{Class, Formats, PLAYABLE_CLASSES};
use tavernlab_core::deck::curve_deck;
use tavernlab_core::game::Agent;
use tavernlab_core::inline::Inline;
use tavernlab_core::state::{Game, Outcome, Side};
use tavernlab_json::Json;

mod serve;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("bench");
    let num = |i: usize, d: usize| args.get(i).and_then(|s| s.parse().ok()).unwrap_or(d);

    match cmd {
        "serve" => {
            let port = args
                .get(1)
                .and_then(|s| s.parse().ok())
                .or_else(|| {
                    std::env::var("TAVERNLAB_PORT")
                        .ok()
                        .and_then(|s| s.parse().ok())
                })
                .unwrap_or(serve::DEFAULT_PORT);
            // One thread is left to the OS and the HTTP handlers: a batch
            // that saturates every core makes the UI that started it stop
            // answering.
            let threads = default_threads().saturating_sub(1).max(1);
            let open = !args.iter().any(|a| a == "--no-open");
            if let Err(e) = serve::run(port, threads, open) {
                eprintln!("tavernsim serve: {e}");
                std::process::exit(1);
            }
        }
        "art-urls" => serve::art_urls(args.iter().any(|a| a == "--heroes")),
        "bench" => bench(num(1, 20_000), num(2, default_threads())),
        "matrix" => matrix(num(1, 200)),
        "demo" => demo(num(1, 1) as u64),
        "coverage" => coverage(),
        "implemented" => list_implemented(match args.get(1).map(String::as_str) {
            Some("wild") => Formats::WILD,
            _ => Formats::STANDARD,
        }),
        "gauntlet" => gauntlet(args.get(1).map(String::as_str)),
        "backlog" => backlog(args.get(1).map(String::as_str)),
        other => {
            eprintln!("unknown command {other:?}");
            eprintln!(
                "usage: tavernsim [serve|bench|matrix|demo|coverage|gauntlet|backlog|art-urls] [args]"
            );
            std::process::exit(2);
        }
    }
}

fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
}

fn bench(games: usize, threads: usize) {
    let deck = curve_deck(Class::Mage, Formats::STANDARD)
        .expect("Mage has enough implemented cards for a deck");
    let c = Contender {
        class: Class::Mage,
        cards: &deck,
        style: Style::Midrange,
    };
    let s = seeds(1, games);

    // A short warm-up so the first measurement is not paying for page faults
    // and branch predictors that have never seen this code.
    play_batch(c, c, &seeds(999, 200));

    // Both runs play the same games, so the two rates are directly comparable
    // and the speedup is a real ratio rather than an artefact of sample size.
    let t0 = Instant::now();
    let single = play_batch(c, c, &s);
    let serial_secs = t0.elapsed().as_secs_f64();
    let serial_rate = single.total() as f64 / serial_secs;
    println!(
        " 1 thread   {:>8} games  {:>7.3} s  {:>10.0} games/s",
        single.total(),
        serial_secs,
        serial_rate
    );

    let t0 = Instant::now();
    let par = play_batch_parallel(c, c, &s, threads);
    let par_secs = t0.elapsed().as_secs_f64();
    let par_rate = par.total() as f64 / par_secs;
    println!(
        "{threads:>2} threads   {:>8} games  {:>7.3} s  {:>10.0} games/s   ({:.1}× one thread)",
        par.total(),
        par_secs,
        par_rate,
        par_rate / serial_rate
    );

    assert_eq!(
        single, par,
        "threading changed the result; the run is not deterministic"
    );
    println!(
        "\nmirror win rate {:.3} for player 0 over {} games ({} draws)",
        par.rate(Side::Player0),
        par.total(),
        par.draws
    );
    println!("average game length {:.1} turns", par.avg_turns());
    println!("serial and parallel results identical: {}", single == par);
}

fn matrix(per_pair: usize) {
    let decks: Vec<(Class, Vec<_>)> = PLAYABLE_CLASSES
        .iter()
        .filter_map(|&c| curve_deck(c, Formats::STANDARD).map(|d| (c, d)))
        .collect();
    let skipped: Vec<Class> = PLAYABLE_CLASSES
        .iter()
        .copied()
        .filter(|c| curve_deck(*c, Formats::STANDARD).is_none())
        .collect();
    if !skipped.is_empty() {
        // Never silently drop a class from a matrix: a missing row reads as
        // "not measured", and it should be obvious which ones were.
        println!(
            "skipping {} class(es) with too few implemented cards: {}\n",
            skipped.len(),
            skipped
                .iter()
                .map(|c| format!("{c:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    let s = seeds(7, per_pair);
    let threads = default_threads();

    println!(
        "{} classes, {per_pair} games per ordered pair\n",
        decks.len()
    );
    print!("{:<14}", "");
    for (c, _) in &decks {
        print!("{:>6}", short(*c));
    }
    println!("{:>8}", "avg");

    let t0 = Instant::now();
    let mut total = 0u32;
    for (ca, da) in &decks {
        print!("{:<14}", format!("{ca:?}"));
        let mut sum = 0.0;
        let mut n = 0;
        for (cb, db) in &decks {
            let a = Contender {
                class: *ca,
                cards: da,
                style: Style::Midrange,
            };
            let b = Contender {
                class: *cb,
                cards: db,
                style: Style::Midrange,
            };
            let r = play_batch_parallel(a, b, &s, threads);
            total += r.total();
            let rate = r.rate(Side::Player0);
            print!("{:>6.2}", rate);
            if ca != cb {
                sum += rate;
                n += 1;
            }
        }
        println!("{:>8.3}", if n > 0 { sum / n as f64 } else { 0.0 });
    }
    let dt = t0.elapsed().as_secs_f64();
    println!(
        "\n{total} games in {dt:.2} s  ({:.0} games/s)",
        total as f64 / dt
    );
}

fn short(c: Class) -> &'static str {
    match c {
        Class::DeathKnight => "DK",
        Class::DemonHunter => "DH",
        Class::Druid => "DRU",
        Class::Hunter => "HUN",
        Class::Mage => "MAG",
        Class::Paladin => "PAL",
        Class::Priest => "PRI",
        Class::Rogue => "ROG",
        Class::Shaman => "SHA",
        Class::Warlock => "WRL",
        Class::Warrior => "WAR",
        _ => "?",
    }
}

fn demo(seed: u64) {
    let deck = curve_deck(Class::Mage, Formats::STANDARD)
        .expect("Mage has enough implemented cards for a deck");
    let mut g = Game::new((Class::Mage, &deck), (Class::Mage, &deck), seed).expect("valid classes");
    let mut a = Scripted::new(Style::Midrange);
    let mut b = Scripted::new(Style::Control);
    let mut agents: [&mut dyn Agent; 2] = [&mut a, &mut b];

    g.start(Side::Player0, &mut agents);
    println!("seed {seed}\n");
    let mut legal = Inline::new();
    while !g.is_over() && g.turn < 40 {
        g.turn += 1;
        g.begin_turn();
        if g.is_over() {
            break;
        }
        let side = g.current;
        println!(
            "turn {:>2}  P{}  {} mana   hero {}/{} vs {}/{}",
            g.turn,
            side.index(),
            g.me().mana,
            g.me().hero_hp,
            g.me().armor,
            g.them().hero_hp,
            g.them().armor
        );
        for _ in 0..64 {
            if g.is_over() {
                break;
            }
            g.legal_actions(&mut legal);
            let pick = agents[side.index()].choose(&g, legal.as_slice());
            if pick == tavernlab_core::game::Action::EndTurn {
                break;
            }
            println!("        {}", describe(&g, pick));
            if !g.apply(pick) {
                break;
            }
        }
        let board = |s: Side| {
            g.player(s)
                .board
                .iter()
                .map(|m| format!("{} {}/{}", m.card.name(), m.atk, m.health()))
                .collect::<Vec<_>>()
                .join(", ")
        };
        println!("        board P0: [{}]", board(Side::Player0));
        println!("        board P1: [{}]", board(Side::Player1));
        if g.is_over() {
            break;
        }
        g.end_turn();
        g.current = g.current.other();
    }
    match g.outcome {
        Some(Outcome::Win(s)) => println!("\nplayer {} wins on turn {}", s.index(), g.turn),
        Some(Outcome::Draw) => println!("\ndraw on turn {}", g.turn),
        None => println!("\nstopped after {} turns", g.turn),
    }
}

fn describe(g: &Game, a: tavernlab_core::game::Action) -> String {
    use tavernlab_core::game::Action::*;
    use tavernlab_core::state::Target;
    let name = |t: Target| match t {
        Target::Hero(s) => format!("hero P{}", s.index()),
        Target::Minion(s, i) => g
            .player(s)
            .board
            .get(i as usize)
            .map(|m| format!("{} (P{})", m.card.name(), s.index()))
            .unwrap_or_else(|| "?".into()),
    };
    match a {
        Play { hand, .. } => {
            let c = g
                .me()
                .hand
                .get(hand as usize)
                .map(|h| h.card.name())
                .unwrap_or("?");
            format!("play {c}")
        }
        Attack { from, target } => {
            let m = g
                .me()
                .board
                .get(from as usize)
                .map(|m| m.card.name())
                .unwrap_or("?");
            format!("attack {} -> {}", m, name(target))
        }
        HeroAttack { target } => format!("hero attack -> {}", name(target)),
        UseLocation { slot, target } => {
            let l = g
                .me()
                .board
                .get(slot as usize)
                .map(|m| m.card.name())
                .unwrap_or("?");
            format!(
                "use {l}{}",
                target
                    .map(|t| format!(" -> {}", name(t)))
                    .unwrap_or_default()
            )
        }
        Trade { hand } => {
            let c = g
                .me()
                .hand
                .get(hand as usize)
                .map(|h| h.card.name())
                .unwrap_or("?");
            format!("trade {c}")
        }
        Prepare { hand } => {
            let c = g
                .me()
                .hand
                .get(hand as usize)
                .map(|h| h.card.name())
                .unwrap_or("?");
            format!("prepare {c}")
        }
        HeroPower { target, second } => {
            format!(
                "hero power{}{}",
                if second { " (2nd)" } else { "" },
                target
                    .map(|t| format!(" -> {}", name(t)))
                    .unwrap_or_default()
            )
        }
        EndTurn => "end turn".into(),
    }
}

/// How much of the card pool the engine can actually play.
///
/// Printed rather than inferred from a passing test suite: the number that
/// matters for a simulation is what fraction of a real deck is understood, and
/// it should be visible without reading source.
fn coverage() {
    use tavernlab_core::cards::{Kind, all, is_approximate, is_implemented};
    use tavernlab_core::deck::{implemented_pool, pool};

    println!("{:<14}{:>7}{:>7}{:>7}   deck", "class", "pool", "impl", "%");
    for c in PLAYABLE_CLASSES {
        let p = pool(c, Formats::STANDARD).len();
        let i = implemented_pool(c, Formats::STANDARD).len();
        let buildable = if curve_deck(c, Formats::STANDARD).is_some() {
            "yes"
        } else {
            "no"
        };
        println!(
            "{:<14}{p:>7}{i:>7}{:>6.0}%   {buildable}",
            format!("{c:?}"),
            100.0 * i as f64 / p.max(1) as f64
        );
    }

    {
        use tavernlab_core::cards::APPROXIMATE;
        println!("
{} card(s) implemented only in part:", APPROXIMATE.len());
        for (name, note) in APPROXIMATE {
            println!("  {name} — {note}");
        }
    }

    for (label, fmt) in [("Standard", Formats::STANDARD), ("Wild", Formats::WILD)] {
        let deckable: Vec<_> = all()
            .filter(|c| {
                let d = c.def();
                d.collectible && d.deckable() && d.formats.has(fmt)
            })
            .collect();
        let imp = deckable.iter().filter(|c| is_implemented(**c)).count();
        let approx = deckable.iter().filter(|c| is_approximate(**c)).count();
        println!(
            "\n{label}: {imp} of {} deckable cards ({:.1}%){}",
            deckable.len(),
            100.0 * imp as f64 / deckable.len().max(1) as f64,
            if approx > 0 {
                format!(" — {approx} of them approximate")
            } else {
                String::new()
            }
        );
        for k in [Kind::Minion, Kind::Spell, Kind::Weapon, Kind::Location] {
            let of_kind: Vec<_> = deckable.iter().filter(|c| c.def().kind() == k).collect();
            let i = of_kind.iter().filter(|c| is_implemented(***c)).count();
            println!(
                "  {:<9}{i:>6} / {:<6}{:>5.0}%",
                format!("{k:?}"),
                of_kind.len(),
                100.0 * i as f64 / of_kind.len().max(1) as f64
            );
        }
    }
}

/// Every card the engine can play in `fmt`, one name per line.
///
/// Exists so coverage can be diffed against another implementation rather than
/// compared as two summary percentages that may not mean the same thing. The
/// format is a parameter because a Wild deck list checked against the Standard
/// answer reads every rotated-out vanilla body as unimplemented.
fn list_implemented(fmt: Formats) {
    use tavernlab_core::cards::{all, is_implemented};
    let mut names: Vec<&str> = all()
        .filter(|c| {
            let d = c.def();
            d.collectible && d.deckable() && d.formats.has(fmt) && is_implemented(*c)
        })
        .map(|c| c.name())
        .collect();
    names.sort_unstable();
    names.dedup();
    for n in names {
        println!("{n}");
    }
}

/// Standard-legal cards of `class` the engine does not understand yet, with
/// full text and cost -- the working list for a manual coverage batch.
///
/// `coverage` says how much is missing; this says exactly what. Sorted by
/// cost, since the cheap end of the curve tends to be the most mechanically
/// simple and is usually where a batch pass should start.
fn backlog(class_name: Option<&str>) {
    use tavernlab_core::cards::Class;
    use tavernlab_core::deck::{implemented_pool, pool};

    let Some(name) = class_name else {
        eprintln!("usage: tavernsim backlog <class>");
        eprintln!(
            "classes: neutral deathknight demonhunter druid hunter mage paladin priest rogue shaman warlock warrior"
        );
        std::process::exit(2);
    };
    let class = match name.to_ascii_lowercase().as_str() {
        "neutral" => Class::Neutral,
        "deathknight" => Class::DeathKnight,
        "demonhunter" => Class::DemonHunter,
        "druid" => Class::Druid,
        "hunter" => Class::Hunter,
        "mage" => Class::Mage,
        "paladin" => Class::Paladin,
        "priest" => Class::Priest,
        "rogue" => Class::Rogue,
        "shaman" => Class::Shaman,
        "warlock" => Class::Warlock,
        "warrior" => Class::Warrior,
        other => {
            eprintln!("unknown class {other:?}");
            std::process::exit(2);
        }
    };

    let all = pool(class, Formats::STANDARD);
    let done = implemented_pool(class, Formats::STANDARD);
    let mut missing: Vec<_> = all.into_iter().filter(|c| !done.contains(c)).collect();
    missing.sort_by_key(|c| c.def().cost);
    println!(
        "{} of {} {class:?} cards unimplemented:",
        missing.len(),
        missing.len() + done.len()
    );
    for c in missing {
        println!(
            "  [{}] cost={} kind={:?} :: {}",
            c.name(),
            c.def().cost,
            c.def().kind(),
            c.info().text.replace('\n', " ")
        );
    }
}

/// How much of a set of real deck lists the engine can actually field.
///
/// `coverage` measures the whole card pool; a percentage there can rise
/// without a single real deck getting any closer to playable, because the
/// pool is dominated by cards no meta deck runs. This instead resolves the
/// actual decklists in `path` (default: the repo's own `data/gauntlet_standard.json`)
/// against the implemented table, one slot at a time, and prints what is
/// still missing so the next card to port is obvious.
fn gauntlet(path: Option<&str>) {
    use tavernlab_core::cards::by_name;
    use tavernlab_core::deck::resolve_slots;

    let path = path.unwrap_or("../data/gauntlet_standard.json");
    let src = std::fs::read_to_string(path).unwrap_or_else(|e| {
        eprintln!("failed to read {path}: {e}");
        std::process::exit(1);
    });
    let doc = Json::parse(&src).unwrap_or_else(|e| {
        eprintln!("failed to parse {path}: {e}");
        std::process::exit(1);
    });
    let decks = doc.as_object().unwrap_or_else(|| {
        eprintln!("{path}: expected a top-level object of deck name -> deck");
        std::process::exit(1);
    });

    let describe = |name: &str| -> String {
        match by_name(name) {
            Some(c) => c.info().text.to_string(),
            None => "(not in the corpus under this name)".to_string(),
        }
    };

    let mut grand_ok = 0u32;
    let mut grand_total = 0u32;
    for (deck_name, deck) in decks {
        let cards = pairs(deck.get("cards"));
        let report = resolve_slots(&cards);
        grand_ok += report.ok;
        grand_total += report.total;
        println!("{:<28}{:>3}/{:<3}", deck_name, report.ok, report.total);
        for (name, count) in &report.missing {
            println!("    x{count}  {name} — {}", describe(name));
        }

        let sideboard = pairs(deck.get("sideboard"));
        if !sideboard.is_empty() {
            let sb = resolve_slots(&sideboard);
            println!("    sideboard: {}/{}", sb.ok, sb.total);
            for (name, count) in &sb.missing {
                println!("      x{count}  {name} — {}", describe(name));
            }
        }
    }
    println!(
        "\n{grand_ok}/{grand_total} slots resolve across {} decks",
        decks.len()
    );
}

/// The `[name, count]` pairs of a deck's `"cards"` or `"sideboard"` array.
fn pairs(v: Option<&Json>) -> Vec<(&str, u32)> {
    v.and_then(Json::as_array)
        .unwrap_or(&[])
        .iter()
        .filter_map(|entry| {
            let a = entry.as_array()?;
            let name = a.first()?.as_str()?;
            let count = a.get(1)?.as_i64()?;
            Some((name, count.max(0) as u32))
        })
        .collect()
}
