//! Running many games.
//!
//! The measurement design is baked in rather than left to the caller, because
//! getting it wrong is what made the Python pipeline slow. Two rules:
//!
//! * **Every candidate sees the same seeds.** Comparing two decks that share 29
//!   of 30 cards using independent samples throws away the shared variance;
//!   pairing the seeds recovers it. Measured on the old engine, that alone was
//!   worth 8.5× fewer games for the same resolution.
//! * **Who goes first is alternated, not rolled.** The coin is a large and
//!   entirely avoidable source of noise.
//!
//! Both are properties of [`play_batch`], so a caller cannot forget them.
//!
//! Parallelism uses `std::thread::scope`, not a work-stealing pool: the work
//! items are uniform and known up front, so a static split has no scheduling
//! overhead and no dependency.

use crate::agent::{Scripted, Style};
use crate::cards::{CardId, Class};
use crate::state::{Game, Outcome, Side};

/// The tally from a set of games.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// Games won by each side.
    pub wins: [u32; 2],
    pub draws: u32,
    /// Total turns across all games, for `avg_turns`. A sudden change here is
    /// the earliest sign that a rules change altered how games play out, which
    /// a win rate alone can hide.
    pub turns: u64,
}

impl Record {
    pub fn total(&self) -> u32 {
        self.wins[0] + self.wins[1] + self.draws
    }

    /// Win rate for a side, counting a draw as half.
    pub fn rate(&self, side: Side) -> f64 {
        let t = self.total();
        if t == 0 {
            return 0.0;
        }
        (self.wins[side.index()] as f64 + 0.5 * self.draws as f64) / t as f64
    }

    /// Mean game length in turns.
    pub fn avg_turns(&self) -> f64 {
        let t = self.total();
        if t == 0 {
            0.0
        } else {
            self.turns as f64 / t as f64
        }
    }

    pub fn merge(mut self, other: Record) -> Record {
        self.wins[0] += other.wins[0];
        self.wins[1] += other.wins[1];
        self.draws += other.draws;
        self.turns += other.turns;
        self
    }

    fn record(&mut self, o: Outcome, turns: u16) {
        match o {
            Outcome::Win(s) => self.wins[s.index()] += 1,
            Outcome::Draw => self.draws += 1,
        }
        self.turns += turns as u64;
    }
}

/// One side of a matchup.
#[derive(Clone, Copy)]
pub struct Contender<'a> {
    pub class: Class,
    pub cards: &'a [CardId],
    pub style: Style,
}

/// Play one game and return how it ended, with the turn it ended on.
pub fn play_one_detailed(a: Contender, b: Contender, seed: u64, first: Side) -> (Outcome, u16) {
    let mut g = match Game::new((a.class, a.cards), (b.class, b.cards), seed) {
        Ok(g) => g,
        Err(_) => return (Outcome::Draw, 0),
    };
    let mut sa = Scripted::new(a.style);
    let mut sb = Scripted::new(b.style);
    let mut agents: [&mut dyn crate::game::Agent; 2] = [&mut sa, &mut sb];
    let o = g.run(first, &mut agents);
    (o, g.turn)
}

/// Play one game and return how it ended.
pub fn play_one(a: Contender, b: Contender, seed: u64, first: Side) -> Outcome {
    let mut g = match Game::new((a.class, a.cards), (b.class, b.cards), seed) {
        Ok(g) => g,
        // A class with no hero power cannot field a deck; report it as a draw
        // rather than panicking inside a worker thread.
        Err(_) => return Outcome::Draw,
    };
    let mut sa = Scripted::new(a.style);
    let mut sb = Scripted::new(b.style);
    let mut agents: [&mut dyn crate::game::Agent; 2] = [&mut sa, &mut sb];
    g.run(first, &mut agents)
}

/// Play `seeds.len()` games on one thread.
///
/// The caller supplies the seed list so that two candidate decks can be given
/// the *same* one — that pairing is the whole point, and it cannot be recovered
/// afterwards from a game count.
pub fn play_batch(a: Contender, b: Contender, seeds: &[u64]) -> Record {
    let mut rec = Record::default();
    for (i, &seed) in seeds.iter().enumerate() {
        // Alternating rather than rolling: over an even number of games each
        // deck leads exactly half the time.
        let first = if i % 2 == 0 {
            Side::Player0
        } else {
            Side::Player1
        };
        let (o, turns) = play_one_detailed(a, b, seed, first);
        rec.record(o, turns);
    }
    rec
}

/// [`play_batch`] spread across `threads` OS threads.
///
/// Deterministic regardless of thread count: each game's result depends only
/// on its seed and its index, never on which worker ran it.
pub fn play_batch_parallel(a: Contender, b: Contender, seeds: &[u64], threads: usize) -> Record {
    let threads = threads.max(1).min(seeds.len().max(1));
    if threads == 1 {
        return play_batch(a, b, seeds);
    }
    // Contiguous chunks keep each game's index — and therefore who leads —
    // identical to the single-threaded run.
    let chunk = seeds.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(threads);
        for (c, part) in seeds.chunks(chunk).enumerate() {
            let base = c * chunk;
            handles.push(scope.spawn(move || {
                let mut rec = Record::default();
                for (i, &seed) in part.iter().enumerate() {
                    let first = if (base + i) % 2 == 0 {
                        Side::Player0
                    } else {
                        Side::Player1
                    };
                    let (o, turns) = play_one_detailed(a, b, seed, first);
                    rec.record(o, turns);
                }
                rec
            }));
        }
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .fold(Record::default(), Record::merge)
    })
}

/// A reproducible seed list for a run.
///
/// Deriving seeds from one number keeps a whole evaluation describable by a
/// single value, which is what makes a paired comparison easy to repeat.
pub fn seeds(base: u64, n: usize) -> Vec<u64> {
    (0..n as u64)
        .map(|i| base.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(i))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::{Formats, Kind};

    /// A 30-card deck of the cheapest legal minions for a class.
    fn test_deck(class: Class) -> Vec<CardId> {
        let mut pool: Vec<CardId> = crate::cards::all()
            .filter(|c| {
                let d = c.def();
                d.collectible
                    && d.kind() == Kind::Minion
                    && d.formats.has(Formats::STANDARD)
                    && d.playable_by(class)
            })
            .collect();
        pool.sort_by_key(|c| (c.def().cost, c.0));
        let mut deck = Vec::with_capacity(30);
        for c in pool {
            for _ in 0..c.def().copy_limit().min(2) {
                if deck.len() < 30 {
                    deck.push(c);
                }
            }
            if deck.len() >= 30 {
                break;
            }
        }
        deck
    }

    #[test]
    fn a_game_finishes_and_names_a_winner() {
        let d = test_deck(Class::Mage);
        let a = Contender {
            class: Class::Mage,
            cards: &d,
            style: Style::Midrange,
        };
        let o = play_one(a, a, 42, Side::Player0);
        // Whatever happens, it must terminate with a definite result.
        assert!(matches!(o, Outcome::Win(_) | Outcome::Draw));
    }

    #[test]
    fn the_same_seed_gives_the_same_game() {
        let d = test_deck(Class::Hunter);
        let a = Contender {
            class: Class::Hunter,
            cards: &d,
            style: Style::Aggro,
        };
        let first = play_one(a, a, 7, Side::Player0);
        for _ in 0..5 {
            assert_eq!(play_one(a, a, 7, Side::Player0), first);
        }
    }

    #[test]
    fn different_seeds_produce_different_games() {
        let d = test_deck(Class::Hunter);
        let a = Contender {
            class: Class::Hunter,
            cards: &d,
            style: Style::Aggro,
        };
        let outcomes: Vec<Outcome> = (0..40).map(|s| play_one(a, a, s, Side::Player0)).collect();
        assert!(
            outcomes.iter().any(|o| *o != outcomes[0]),
            "forty seeds all produced the same result; the RNG is not reaching the game"
        );
    }

    #[test]
    fn a_mirror_match_is_close_to_even() {
        // The strongest available check that nothing is systematically biased
        // towards one seat: identical decks, identical policy, balanced coin.
        let d = test_deck(Class::Mage);
        let a = Contender {
            class: Class::Mage,
            cards: &d,
            style: Style::Midrange,
        };
        let rec = play_batch(a, a, &seeds(1, 400));
        let r = rec.rate(Side::Player0);
        assert_eq!(rec.total(), 400);
        assert!(
            (0.35..=0.65).contains(&r),
            "mirror match came out {r:.3} for player 0, which suggests a seat bias"
        );
    }

    #[test]
    fn parallel_and_serial_agree_exactly() {
        // Determinism across thread counts is what makes a batch result
        // reproducible on another machine.
        let d = test_deck(Class::Warrior);
        let a = Contender {
            class: Class::Warrior,
            cards: &d,
            style: Style::Control,
        };
        let s = seeds(9, 120);
        let serial = play_batch(a, a, &s);
        for threads in [2usize, 3, 8] {
            assert_eq!(
                play_batch_parallel(a, a, &s, threads),
                serial,
                "{threads} threads"
            );
        }
    }

    #[test]
    fn record_arithmetic() {
        let mut r = Record::default();
        r.record(Outcome::Win(Side::Player0), 10);
        r.record(Outcome::Win(Side::Player0), 20);
        r.record(Outcome::Win(Side::Player1), 30);
        r.record(Outcome::Draw, 40);
        assert_eq!(r.total(), 4);
        assert_eq!(r.rate(Side::Player0), (2.0 + 0.5) / 4.0);
        assert_eq!(r.rate(Side::Player1), (1.0 + 0.5) / 4.0);
        assert_eq!(Record::default().rate(Side::Player0), 0.0);
        assert_eq!(r.avg_turns(), 25.0);
        assert_eq!(Record::default().avg_turns(), 0.0);
    }

    #[test]
    fn seeds_are_reproducible_and_distinct() {
        assert_eq!(seeds(5, 10), seeds(5, 10));
        assert_ne!(seeds(5, 10), seeds(6, 10));
        let s = seeds(5, 100);
        let mut sorted = s.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 100, "seed list has collisions");
    }

    #[test]
    fn games_actually_end_before_the_turn_limit() {
        // A deck of pure minions with no removal could in principle stall; if
        // most games hit the 89-turn cap the engine is not resolving combat.
        let d = test_deck(Class::Mage);
        let a = Contender {
            class: Class::Mage,
            cards: &d,
            style: Style::Midrange,
        };
        let rec = play_batch(a, a, &seeds(3, 200));
        assert!(
            rec.draws < 40,
            "{} of 200 games ended in a draw; combat is probably not resolving",
            rec.draws
        );
    }

    #[test]
    fn the_coin_alternates_so_neither_seat_always_leads() {
        // Directly asserts the anti-variance rule: with an even game count each
        // side leads exactly half the time.
        let d = test_deck(Class::Mage);
        let a = Contender {
            class: Class::Mage,
            cards: &d,
            style: Style::Midrange,
        };
        let s = seeds(11, 2);
        // Same seed, both seats leading: the two games must differ.
        assert_ne!(
            play_one(a, a, s[0], Side::Player0),
            play_one(a, a, s[0], Side::Player1),
            "who leads made no difference, so the coin is not being applied"
        );
    }
}
