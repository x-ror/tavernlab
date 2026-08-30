//! `tavernsim` — the batch simulator.
//!
//! Commands:
//!
//! ```text
//! tavernsim serve [port]              the local app: web UI + API
//! tavernsim watch [opts]              read the game log; --quiet records only
//! tavernsim history [file]            the games watch has recorded
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
use crate::serve::state::MULLIGAN_MIN_N;
use tavernlab_core::planner::Weights;
use tavernlab_core::batch::{Contender, play_batch, play_batch_parallel, seeds};
use tavernlab_core::cards::{Class, Formats, PLAYABLE_CLASSES};
use tavernlab_core::deck::curve_deck;
use tavernlab_core::game::Agent;
use tavernlab_core::inline::Inline;
use tavernlab_core::state::{Game, Outcome, Side};
use tavernlab_json::Json;

mod decks;
mod history;
mod serve;
#[path = "watch/mod.rs"]
mod watch_mod;

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
        "tiers" => tiers_by_policy(num(1, 100), args.get(2).map(String::as_str)),
        "policy" => policy(
            num(1, 200),
            num(2, 4000) as u32,
            num(3, 4) as u8,
            num(4, 1) as u8,
        ),
        "weights" => weights(num(1, 200), num(2, 4000) as u32, args.get(3).map(String::as_str)),
        "mulligan" => mulligan_bias(num(1, 300), args.get(2).map(String::as_str)),
        "demo" => demo(num(1, 1) as u64),
        "coverage" => coverage(),
        "implemented" => list_implemented(match args.get(1).map(String::as_str) {
            Some("wild") => Formats::WILD,
            _ => Formats::STANDARD,
        }),
        "gauntlet" => gauntlet(args.get(1).map(String::as_str)),
        "decks" => decks::run(args.get(1).map(String::as_str)),
        "watch" => watch(&args[1..]),
        "backlog" => backlog(args.get(1).map(String::as_str)),
        "history" => show_history(args.get(1).map(String::as_str)),
        other => {
            eprintln!("unknown command {other:?}");
            eprintln!(
                "usage: tavernsim [serve|watch|history|bench|matrix|policy|weights|mulligan|tiers|demo|coverage|gauntlet|decks|backlog|art-urls] [args]"
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

/// One weight in the evaluation, and the values to try for it.
///
/// A function rather than a field index because there is no way to name a
/// struct field at runtime, and three closures read better than the macro
/// that would avoid them.
type Sweep = (&'static str, fn(Weights, f32) -> Weights, &'static [f32]);

const SWEEPS: [Sweep; 3] = [
    (
        "own_health",
        |w, v| Weights { own_health: v, ..w },
        &[0.0, 0.15, 0.6, 1.0],
    ),
    ("card", |w, v| Weights { card: v, ..w }, &[0.5, 1.0, 1.5, 2.5, 3.0]),
    (
        "unspent",
        |w, v| Weights { unspent: v, ..w },
        &[0.0, 0.05, 0.4, 1.0],
    ),
];

/// Are the evaluation's numbers the right numbers?
///
/// Three of them were picked by hand when the search was written, to find
/// out whether searching helped at all. That it does (+37.5 points) says
/// nothing about whether 0.35, 0.5 and 0.15 are right, and those three
/// numbers now decide what the tool advises mid-game and what the Meta tab
/// prints. So each is played against the value it has, one weight at a time,
/// on identical decks and identical seeds -- the same head-to-head that gave
/// deepening its +10.2 and averaging its nothing.
///
/// One at a time on purpose. A joint sweep over three axes would need many
/// times the games to say anything, and the question here is not "what is
/// the optimum" but "is any of these three visibly wrong".
fn weights(per_deck: usize, budget: u32, gauntlet: Option<&str>) {
    use tavernlab_core::batch::{Policy, duel};

    // A weight found on one population and shipped to another is how an
    // overfit happens, so the same sweep can be pointed at the real gauntlet
    // -- which is the population the Meta tab actually ranks.
    let decks: Vec<(Class, Vec<_>)> = match gauntlet {
        Some(path) => serve::state::load_gauntlet(std::path::Path::new(path))
            .into_iter()
            .filter(|d| d.playable())
            .map(|d| (d.class, d.ids.clone()))
            .collect(),
        None => PLAYABLE_CLASSES
            .iter()
            .filter_map(|&c| curve_deck(c, Formats::STANDARD).map(|d| (c, d)))
            .collect(),
    };
    if decks.is_empty() {
        eprintln!("жодної колоди, з якою можна міряти");
        std::process::exit(1);
    }
    let s = seeds(23, per_deck);
    let threads = default_threads();
    let base = Weights::default();
    let plan = |w: Weights| Policy::Plan {
        budget,
        depth: 4,
        samples: 1,
        iterative: true,
        weights: w,
    };

    // Both sides search, so this is twice the work of a run against greedy.
    let run = |w: Weights| {
        let mut rec = tavernlab_core::batch::Record::default();
        for (class, cards) in &decks {
            let deck = Contender {
                class: *class,
                cards,
                style: Style::Midrange,
            };
            rec = rec.merge(duel(deck, [plan(w), plan(base)], &s, threads));
        }
        rec
    };
    let report = |label: String, rec: &tavernlab_core::batch::Record| {
        let p = rec.rate(Side::Player0);
        let se = (p * (1.0 - p) / rec.total() as f64).sqrt();
        // Two standard errors, so "beats the bar" means something a single
        // lucky run does not clear.
        let verdict = if (p - 0.5).abs() > 2.0 * se {
            if p > 0.5 { "краще" } else { "гірше" }
        } else {
            "—"
        };
        println!(
            "{label:<22}{:>9.1}%{:>8.1}{:>10}{:>9}",
            100.0 * p,
            100.0 * se,
            verdict,
            rec.total()
        );
    };

    println!(
        "{} колод, {per_deck} сідів кожна, обидва боки шукають (бюджет {budget}, глибина 4)\n\
         базові ваги: own_health {}, card {}, unspent {}\n",
        decks.len(),
        base.own_health,
        base.card,
        base.unspent
    );
    println!(
        "{:<22}{:>10}{:>8}{:>10}{:>9}",
        "вага = значення", "проти базових", "±1 s.e.", "вердикт", "ігор"
    );
    // The control first, and it must read 50.0%: the same weights on both
    // sides cannot differ, so anything else means the harness is measuring
    // itself rather than the weights.
    report("контроль (ті самі)".to_string(), &run(base));
    for (name, apply, values) in SWEEPS {
        for &v in values {
            report(format!("{name} = {v}"), &run(apply(base, v)));
        }
    }
}

/// Does the mulligan advice depend on how well the agent plays?
///
/// The README says the policy's bias largely cancels on this screen, because
/// a deck is measured against the same field either way. That argument is
/// about *decks*. The mulligan is a comparison between the **cards of one
/// deck**, and a card the greedy policy misplays looks bad against every
/// opponent -- nothing there cancels. So the claim is checked rather than
/// repeated: the same instrumented runs, on the same seeds, once with each
/// policy, and what is compared is the advice, not the win rate.
///
/// A card counts as flipped when the two policies disagree about keeping it.
/// That is the only difference a reader of the tab would ever see.
fn mulligan_bias(games: usize, gauntlet: Option<&str>) {
    use tavernlab_core::batch::Policy;
    use tavernlab_core::telemetry::instrumented_parallel_with;

    let path = gauntlet.unwrap_or("../data/gauntlet_standard.json");
    let field = serve::state::load_gauntlet(std::path::Path::new(path));
    let playable: Vec<_> = field.iter().filter(|d| d.playable()).collect();
    if playable.len() < 2 {
        eprintln!("замало колод у {path}");
        std::process::exit(1);
    }
    let threads = default_threads();
    let search = Policy::Plan {
        budget: 4000,
        depth: 4,
        samples: 1,
        iterative: true,
        weights: tavernlab_core::planner::Weights::default(),
    };
    // Two seed lists. The advice moves when the *sample* changes as well as
    // when the policy does, and at any affordable number of games a card's
    // opening record is a few dozen games wide -- so "greedy against itself
    // on different seeds" is the floor that any policy difference has to
    // clear. Without it this whole measurement would read noise as a finding.
    let s = seeds(31, games);
    let other = seeds(97, games);

    // Two rules over the same games, so the fix can be shown to work rather
    // than argued for. `binary` is what the tab used to say -- keep unless
    // the measured difference is below the line, whatever its error bar.
    // `guarded` is `opening_verdict`: a difference inside its own bar is no
    // difference, and falls back to the curve like a card with too few games.
    #[derive(PartialEq)]
    enum Say {
        Keep,
        Toss,
        NoData,
    }
    let binary =
        |st: &tavernlab_core::telemetry::CardStat, base: f64| match st
            .opening_delta(base, MULLIGAN_MIN_N)
        {
            Some(d) if d > -0.01 => Say::Keep,
            Some(_) => Say::Toss,
            None => Say::NoData,
        };
    let guarded =
        |st: &tavernlab_core::telemetry::CardStat, base: f64| match st
            .opening_verdict(base, MULLIGAN_MIN_N)
        {
            tavernlab_core::telemetry::Verdict::Keep => Say::Keep,
            tavernlab_core::telemetry::Verdict::Toss => Say::Toss,
            // Both fall back to the curve, which is the same answer under
            // either policy, so neither is a judgement to disagree about.
            _ => Say::NoData,
        };

    println!(
        "{path}: {} колод, {games} ігор на пару, по обидві політики\n",
        playable.len()
    );
    println!(
        "{:<22}{:>6}{:>13}{:>13}{:>13}{:>13}",
        "колода", "карт", "шум (було)", "шум (стало)", "політ.(було)", "політ.(стало)"
    );

    // Against the whole field, the way the tab builds a deck's telemetry,
    // summed rather than one opponent picked -- "which card to keep" is not a
    // question about one matchup.
    let against_field = |me: &tavernlab_core::gauntlet::MetaDeck,
                         policies: [Policy; 2],
                         seeds: &[u64]| {
        let mut all = tavernlab_core::telemetry::Matchup::default();
        for opp in &playable {
            if opp.name == me.name {
                continue;
            }
            all = all.merge(instrumented_parallel_with(
                me.contender(),
                opp.contender(),
                policies,
                seeds,
                threads,
            ));
        }
        all
    };

    // How many cards the two runs disagree about keeping, and how many
    // neither could judge.
    let compare = |a: &tavernlab_core::telemetry::Matchup,
                   b: &tavernlab_core::telemetry::Matchup,
                   rule: &dyn Fn(&tavernlab_core::telemetry::CardStat, f64) -> Say| {
        let (ab, bb) = (a.base(), b.base());
        let (mut same, mut diff, mut none) = (0usize, 0usize, 0usize);
        for (card, stat) in &a.cards {
            let (x, y) = match b.cards.iter().find(|(c, _)| c == card) {
                Some((_, st)) => (rule(stat, ab), rule(st, bb)),
                None => (Say::NoData, Say::NoData),
            };
            if x == Say::NoData || y == Say::NoData {
                none += 1;
            } else if x == y {
                same += 1;
            } else {
                diff += 1;
            }
        }
        (same, diff, none)
    };
    let share = |(same, diff, _): (usize, usize, usize)| {
        let judged = same + diff;
        if judged == 0 {
            return "—".to_string();
        }
        format!("{diff}/{judged} ({:.0}%)", 100.0 * diff as f64 / judged as f64)
    };

    let add =
        |a: (usize, usize, usize), b: (usize, usize, usize)| (a.0 + b.0, a.1 + b.1, a.2 + b.2);
    let zero = (0usize, 0usize, 0usize);
    let (mut cb, mut cg, mut pb, mut pg) = (zero, zero, zero, zero);
    for me in &playable {
        let greedy = against_field(me, [Policy::Greedy; 2], &s);
        let noise = against_field(me, [Policy::Greedy; 2], &other);
        let searched = against_field(me, [search; 2], &s);
        let row = (
            compare(&greedy, &noise, &binary),
            compare(&greedy, &noise, &guarded),
            compare(&greedy, &searched, &binary),
            compare(&greedy, &searched, &guarded),
        );
        println!(
            "{:<22}{:>6}{:>13}{:>13}{:>13}{:>13}",
            me.name,
            greedy.cards.len(),
            share(row.0),
            share(row.1),
            share(row.2),
            share(row.3)
        );
        cb = add(cb, row.0);
        cg = add(cg, row.1);
        pb = add(pb, row.2);
        pg = add(pg, row.3);
    }
    println!(
        "\n{:<22}{:>6}{:>13}{:>13}{:>13}{:>13}",
        "разом",
        cb.0 + cb.1 + cb.2,
        share(cb),
        share(cg),
        share(pb),
        share(pg)
    );
    let pct = |(same, diff, _): (usize, usize, usize)| {
        let judged = same + diff;
        if judged == 0 {
            0.0
        } else {
            100.0 * diff as f64 / judged as f64
        }
    };
    println!(
        "\nстаре правило: шум перевертає {:.0}% порад, зміна політики — {:.0}%",
        pct(cb),
        pct(pb)
    );
    println!(
        "нове правило:  шум перевертає {:.0}% порад, зміна політики — {:.0}%",
        pct(cg),
        pct(pg)
    );
}

/// Whether the tier list is a statement about decks or about the policy.
///
/// The same field, the same seeds, the same matchups -- built twice, once by
/// the greedy policy and once by the turn-planning search. A tier list that
/// says something about decks should not move much when the hands holding
/// them get better; one that reorders was never measuring the decks.
///
/// Printed as the two rankings side by side with each deck's change in
/// position, because a correlation on its own hides the case that matters:
/// one deck moving a long way while the rest sit still.
fn tiers_by_policy(per_pair: usize, path: Option<&str>) {
    use tavernlab_core::batch::Policy;
    use tavernlab_core::tiers;

    let path = path.unwrap_or("../data/gauntlet_standard.json");
    let field = serve::state::load_gauntlet(std::path::Path::new(path));
    if field.is_empty() {
        eprintln!("no decks read from {path}");
        std::process::exit(1);
    }
    let threads = default_threads();
    let plan = Policy::Plan {
        budget: 4000,
        depth: 4,
        samples: 1,
        iterative: true,
        weights: tavernlab_core::planner::Weights::default(),
    };

    println!("{path}: {per_pair} боїв на пару, {threads} потоків\n");
    let greedy = tiers::build_with(&field, [Policy::Greedy; 2], per_pair, threads, |_| {});
    println!("жадібний готовий, рахую пошуком (це у ~200 разів повільніше)…");
    let planned = tiers::build_with(&field, [plan; 2], per_pair, threads, |_| {});

    // Position in each ranking, by deck name. `Table::rows` comes back in
    // ranking order already.
    let rank = |t: &tiers::Table, name: &str| t.rows.iter().position(|r| r.name == name);

    println!(
        "\n{:<28}{:>10}{:>10}{:>9}{:>8}{:>8}",
        "колода", "жадібний", "пошук", "різниця", "місць", "тір"
    );
    let mut moved = 0usize;
    let mut retiered = 0usize;
    let mut worst = 0i32;
    for (i, row) in greedy.rows.iter().enumerate() {
        let Some(j) = rank(&planned, &row.name) else {
            continue;
        };
        let other = &planned.rows[j];
        let shift = i as i32 - j as i32;
        if shift != 0 {
            moved += 1;
        }
        worst = worst.max(shift.abs());
        // The tier is what a reader actually takes away, so a deck that
        // crosses a band is the finding -- a win rate that moved inside one
        // is not.
        if row.tier != other.tier {
            retiered += 1;
        }
        println!(
            "{:<28}{:>9.1}%{:>9.1}%{:>+8.1}{:>8}{:>8}",
            row.name,
            100.0 * row.winrate,
            100.0 * other.winrate,
            100.0 * (other.winrate - row.winrate),
            if shift == 0 {
                "—".to_string()
            } else {
                format!("{shift:+}")
            },
            if row.tier == other.tier {
                row.tier.to_string()
            } else {
                format!("{}→{}", row.tier, other.tier)
            }
        );
    }
    let n = greedy.rows.len();
    println!(
        "\n{moved} з {n} колод змінили місце, {retiered} змінили тір, \
         найбільший зсув {worst}; похибка на {per_pair} боях ±{:.1} в.п.",
        100.0 * tiers::margin(per_pair)
    );
}

/// How much the greedy policy leaves on the table.
///
/// Plays the turn-planning policy against the greedy one on identical decks
/// and identical seeds, one deck per class. The decks being the same on both
/// sides is the point: with the list fixed, a win rate away from 50% is play
/// and nothing else.
///
/// Every seed is played twice with the seats swapped (see `batch::duel`), so
/// the number is not a seat advantage wearing a policy's name. The greedy
/// policy against a second copy of itself is printed first, as the control:
/// it must read 50.0%, and anything the planner gains has to clear whatever
/// it does not.
///
/// `depth` is the knob that separates the two things a win could mean. At
/// depth 1 the planner does not search at all -- it applies one action and
/// scores the position -- so whatever it gains there is its *evaluation*
/// being better than greedy's action scorer, not lookahead. Everything from
/// depth 1 to depth 6 is the search.
fn policy(per_deck: usize, budget: u32, depth: u8, samples: u8) {
    use tavernlab_core::batch::{Policy, duel};

    let decks: Vec<(Class, Vec<_>)> = PLAYABLE_CLASSES
        .iter()
        .filter_map(|&c| curve_deck(c, Formats::STANDARD).map(|d| (c, d)))
        .collect();
    let s = seeds(11, per_deck);
    let threads = default_threads();
    let plan = Policy::Plan {
        budget,
        depth,
        samples,
        iterative: true,
        weights: tavernlab_core::planner::Weights::default(),
    };
    // The same search with the budget spent depth-first instead. Printed
    // beside the planner so the deepening is a measured claim rather than an
    // assertion in a comment.
    let dfs = Policy::Plan {
        budget,
        depth,
        samples,
        iterative: false,
        weights: tavernlab_core::planner::Weights::default(),
    };

    println!(
        "{} decks, {per_deck} seeds each, every seed played from both seats\n\
         planner budget {budget} positions per decision, depth {depth}\n",
        decks.len()
    );
    println!(
        "{:<14}{:>9}{:>14}{:>16}{:>9}",
        "", "A/A", "vs greedy", "vs depth-first", "games"
    );

    let mut ctl_total = tavernlab_core::batch::Record::default();
    let mut run_total = tavernlab_core::batch::Record::default();
    let mut head_total = tavernlab_core::batch::Record::default();
    for (class, cards) in &decks {
        let deck = Contender {
            class: *class,
            cards,
            style: Style::Midrange,
        };
        let control = duel(deck, [Policy::Greedy, Policy::Greedy], &s, threads);
        let against = duel(deck, [plan, Policy::Greedy], &s, threads);
        // Head to head at the same budget and the same depth, so the only
        // difference left is how the budget was spent.
        let head = duel(deck, [plan, dfs], &s, threads);
        println!(
            "{:<14}{:>8.1}%{:>13.1}%{:>15.1}%{:>9}",
            tavernlab_core::gauntlet::class_name(*class),
            100.0 * control.rate(Side::Player0),
            100.0 * against.rate(Side::Player0),
            100.0 * head.rate(Side::Player0),
            against.total()
        );
        ctl_total = ctl_total.merge(control);
        run_total = run_total.merge(against);
        head_total = head_total.merge(head);
    }
    let n = run_total.total() as f64;
    // Binomial standard error on the pooled rate, so the gap can be read
    // against something rather than eyeballed.
    let p = run_total.rate(Side::Player0);
    let se = (p * (1.0 - p) / n).sqrt();
    println!(
        "\n{:<14}{:>8.1}%{:>13.1}%{:>15.1}%{:>9}",
        "all",
        100.0 * ctl_total.rate(Side::Player0),
        100.0 * p,
        100.0 * head_total.rate(Side::Player0),
        run_total.total()
    );
    println!(
        "\nplanner {:+.1} points over greedy, +/- {:.1} (1 s.e.); control reads {:+.1}",
        100.0 * (p - 0.5),
        100.0 * se,
        100.0 * (ctl_total.rate(Side::Player0) - 0.5)
    );
    let h = head_total.rate(Side::Player0);
    let hse = (h * (1.0 - h) / head_total.total() as f64).sqrt();
    println!(
        "deepening {:+.1} points over spending the same budget depth-first, +/- {:.1}",
        100.0 * (h - 0.5),
        100.0 * hse
    );
    println!(
        "average game length: greedy mirror {:.1} turns, against the planner {:.1}",
        ctl_total.avg_turns(),
        run_total.avg_turns()
    );
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
/// `tavernsim watch` — read the game's own log and advise.
fn watch(args: &[String]) {
    let mut a = watch_mod::Args {
        logs_dir: None,
        log_file: None,
        history: None,
        deck: String::new(),
        me: None,
        once: false,
        quiet: false,
        serve: None,
    };
    let mut format = "standard".to_string();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--logs" => {
                a.logs_dir = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--log" => {
                a.log_file = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--deck" => {
                a.deck = args.get(i + 1).cloned().unwrap_or_default();
                i += 1;
            }
            "--format" => {
                format = args.get(i + 1).cloned().unwrap_or_else(|| "standard".into());
                i += 1;
            }
            "--me" => {
                a.me = args.get(i + 1).cloned();
                i += 1;
            }
            "--history" => {
                a.history = args.get(i + 1).map(std::path::PathBuf::from);
                i += 1;
            }
            "--serve" => {
                // The port is optional: `--serve` on its own takes the
                // default, so the common case is one word.
                a.serve = Some(match args.get(i + 1).and_then(|v| v.parse().ok()) {
                    Some(port) => {
                        i += 1;
                        port
                    }
                    None => 8766,
                });
            }
            "--no-history" => a.history = Some(std::path::PathBuf::new()),
            "--once" => a.once = true,
            "--quiet" => a.quiet = true,
            other => {
                eprintln!("unknown option {other:?}");
                eprintln!(
                    "usage: tavernsim watch [--logs DIR] [--log FILE] \
                     [--me BATTLETAG] [--deck CODE] [--format standard|wild] \
                     [--history FILE | --no-history] [--once] [--quiet]"
                );
                std::process::exit(2);
            }
        }
        i += 1;
    }
    // The deck code is personal, so it is taken from the environment as
    // readily as from the command line: a shell history is a worse place for
    // it than a shell profile.
    if a.deck.is_empty()
        && let Ok(code) = std::env::var("HS_DECK")
        && !code.is_empty()
    {
        a.deck = code;
    }
    if a.me.is_none()
        && let Ok(tag) = std::env::var("HS_ME")
    {
        a.me = Some(tag);
    }
    let Some(root) = serve::paths::repo_root() else {
        eprintln!("cannot find the data directory (expected data/gauntlet_standard.json)");
        std::process::exit(1);
    };
    let app = serve::state::App::new(root, serve::paths::data_home(), default_threads());
    // Which deck, and whether to remember it, is `watch::run`'s to settle --
    // it is the same question in both directions and belongs in one place.
    std::process::exit(watch_mod::run(&app, &format, a));
}

/// `tavernsim history` — the games `watch` has recorded.
///
/// The same file the web UI reads, printed. It is an ordinary SQLite database
/// and the path is shown, so anything this does not answer is one `sqlite3`
/// away.
fn show_history(path: Option<&str>) {
    let path = path
        .map(std::path::PathBuf::from)
        .unwrap_or_else(history::default_path);
    let games = match history::read(&path) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    println!("{}", path.display());
    if games.is_empty() {
        println!(
            "\nІсторія порожня. Її наповнює `tavernsim watch`: кожен \
             завершений бій лягає сюди."
        );
        return;
    }

    let s = history::summarise(&games);
    println!(
        "\n{} {}, з них завершених {}",
        s.games,
        games_word(s.games as i64),
        s.resolved
    );
    if s.resolved >= 5 {
        println!(
            "перемог {} — {:.0}%",
            s.wins,
            s.wins as f64 / s.resolved as f64 * 100.0
        );
    } else {
        // Under the floor there is a count and no rate, the same rule the
        // rest of this program prints under.
        println!("перемог {} (замало боїв для вінрейту)", s.wins);
    }

    let section = |title: &str, rows: &[history::Tally]| {
        if rows.is_empty() {
            return;
        }
        println!("\n{title}");
        for t in rows.iter().take(12) {
            match t.rate() {
                Some(r) => println!(
                    "  {:<28} {:>3} {:<5} {:>3.0}%",
                    t.key,
                    t.games,
                    games_word(t.games as i64),
                    r * 100.0
                ),
                None => println!(
                    "  {:<28} {:>3} {:<5}    —",
                    t.key,
                    t.games,
                    games_word(t.games as i64)
                ),
            }
        }
    };
    section("проти класу", &s.by_opponent);
    section("вашим класом", &s.by_my_class);
    section("проти колоди (за читанням гаунтлета)", &s.by_opponent_deck);

    println!("\nостанні бої");
    for g in games.iter().rev().take(20) {
        let outcome = match g.won {
            Some(true) => "перемога",
            Some(false) => "поразка",
            None => "не завершено",
        };
        let deck = if g.opponent_deck.is_empty() {
            String::new()
        } else {
            format!("  {} ({} з {})", g.opponent_deck, g.opponent_hits, g.opponent_seen)
        };
        println!(
            "  {}  {} проти {:<14} {:<13} {:>2} ходів{}",
            stamp(g.played_at),
            g.my_class,
            g.opponent_class,
            outcome,
            g.turns,
            deck
        );
    }
}

/// "бій", "бої", "боїв" — Ukrainian counts three ways, and `2 боїв` reads as
/// a bug in the program rather than in the grammar.
pub fn games_word(n: i64) -> &'static str {
    let (tens, ones) = (n.abs() % 100, n.abs() % 10);
    if (11..=14).contains(&tens) {
        return "боїв";
    }
    match ones {
        1 => "бій",
        2..=4 => "бої",
        _ => "боїв",
    }
}

/// Unix seconds as `YYYY-MM-DD HH:MM`, in UTC.
///
/// UTC because that is the only zone the standard library can name without a
/// timezone database, and a wrong local time would be worse than an honest
/// one. The civil-date arithmetic is Howard Hinnant's `civil_from_days`, which
/// is exact for every day this will ever be handed.
fn stamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02} {:02}:{:02}",
        secs / 3600,
        secs % 3600 / 60
    )
}

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

#[cfg(test)]
mod stamp_tests {
    use super::{games_word, stamp};

    #[test]
    fn ukrainian_counts_three_ways() {
        assert_eq!(games_word(1), "бій");
        assert_eq!(games_word(2), "бої");
        assert_eq!(games_word(5), "боїв");
        assert_eq!(games_word(11), "боїв", "eleven is not one");
        assert_eq!(games_word(12), "боїв");
        assert_eq!(games_word(21), "бій");
        assert_eq!(games_word(22), "бої");
        assert_eq!(games_word(111), "боїв");
        assert_eq!(games_word(0), "боїв");
    }

    #[test]
    fn the_civil_date_is_right_at_the_awkward_days() {
        // The epoch, a leap day, the century that is not a leap year, the one
        // that is, and a second before midnight.
        assert_eq!(stamp(0), "1970-01-01 00:00");
        assert_eq!(stamp(951_782_400), "2000-02-29 00:00");
        assert_eq!(stamp(4_107_542_400), "2100-03-01 00:00");
        assert_eq!(stamp(1_709_164_800), "2024-02-29 00:00");
        assert_eq!(stamp(1_756_425_599), "2025-08-28 23:59");
        // Before the epoch, which a clock skewed backwards can produce.
        assert_eq!(stamp(-1), "1969-12-31 23:59");
    }
}
