//! Instrumented simulations: what each card in *your* deck is worth.
//!
//! For every game, this records which of the deck's cards were in the
//! post-mulligan opening hand, which of them left the deck during the game,
//! and who won. Aggregated per matchup that gives the two numbers a deck
//! tracker shows — "kept in opening hand" and "drawn during game" win rate —
//! against the matchup's own baseline, which is what makes them comparable
//! at all.
//!
//! # What "drawn" means here, exactly
//!
//! The cards that left the deck: drawn, tutored, milled or discarded from it.
//! It is measured as the difference between the shuffled deck at the end of
//! the mulligan and the deck when the game ended, rather than by counting
//! draw events, because that costs one comparison per game instead of a hook
//! on the hot path — and because a card that a tutor pulled straight into
//! play is exactly as "drawn" as one taken off the top for the purposes of
//! this question.
//!
//! Cards that never came from the deck — a Discover, a token, the Coin — are
//! not counted at all: the question is what the cards *you chose* are doing.
//!
//! # What these numbers are not
//!
//! A correlation, not a causal effect. "Kept in hand, won 8 points more
//! often" also fires for a card that is merely cheap enough to keep in the
//! hands that were going to win anyway. The API is expected to say so
//! wherever it prints one.

use crate::agent::Scripted;
use crate::batch::Contender;
use crate::cards::CardId;
use crate::game::Agent;
use crate::state::{Game, Outcome, Side};

/// One card's record in one matchup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CardStat {
    /// Games where the card was in the kept opening hand.
    pub open_n: u32,
    /// ...of which were won.
    pub open_w: u32,
    /// Games where the card left the deck at any point (the opening hand
    /// included).
    pub drawn_n: u32,
    /// ...of which were won.
    pub drawn_w: u32,
}

impl CardStat {
    fn merge(&mut self, o: &CardStat) {
        self.open_n += o.open_n;
        self.open_w += o.open_w;
        self.drawn_n += o.drawn_n;
        self.drawn_w += o.drawn_w;
    }

    /// Win rate when kept, against `base`, or `None` below `min_n` games —
    /// where the difference would be noise with a decimal point on it.
    pub fn opening_delta(&self, base: f64, min_n: u32) -> Option<f64> {
        (self.open_n >= min_n).then(|| self.open_w as f64 / self.open_n as f64 - base)
    }

    /// Win rate when it showed up at all, against `base`.
    pub fn drawn_delta(&self, base: f64, min_n: u32) -> Option<f64> {
        (self.drawn_n >= min_n).then(|| self.drawn_w as f64 / self.drawn_n as f64 - base)
    }
}

/// One deck's record against one opponent.
#[derive(Clone, Debug, Default)]
pub struct Matchup {
    pub games: u32,
    pub wins: u32,
    /// One entry per distinct card in the deck, in deck order.
    pub cards: Vec<(CardId, CardStat)>,
}

impl Matchup {
    /// Win rate over the games that finished.
    pub fn base(&self) -> f64 {
        if self.games == 0 {
            0.0
        } else {
            self.wins as f64 / self.games as f64
        }
    }

    pub fn stat(&self, card: CardId) -> Option<CardStat> {
        self.cards.iter().find(|(c, _)| *c == card).map(|(_, s)| *s)
    }

    fn merge(mut self, other: Matchup) -> Matchup {
        self.games += other.games;
        self.wins += other.wins;
        for (card, stat) in &other.cards {
            match self.cards.iter_mut().find(|(c, _)| c == card) {
                Some((_, s)) => s.merge(stat),
                None => self.cards.push((*card, *stat)),
            }
        }
        self
    }
}

/// Play `seeds.len()` instrumented games of `me` against `opp`.
///
/// The seed list and the alternating coin work exactly as they do in
/// [`batch::play_batch`](crate::batch::play_batch) — a telemetry run and a
/// win-rate run over the same seeds play the same games.
pub fn instrumented(me: Contender, opp: Contender, seeds: &[u64]) -> Matchup {
    run_range(me, opp, seeds, 0)
}

/// [`instrumented`] spread across `threads` OS threads. Deterministic
/// regardless of thread count.
pub fn instrumented_parallel(
    me: Contender,
    opp: Contender,
    seeds: &[u64],
    threads: usize,
) -> Matchup {
    let threads = threads.max(1).min(seeds.len().max(1));
    if threads == 1 {
        return instrumented(me, opp, seeds);
    }
    let chunk = seeds.len().div_ceil(threads);
    std::thread::scope(|scope| {
        let handles: Vec<_> = seeds
            .chunks(chunk)
            .enumerate()
            .map(|(c, part)| scope.spawn(move || run_range(me, opp, part, c * chunk)))
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_default())
            .fold(Matchup::default(), Matchup::merge)
    })
}

fn run_range(me: Contender, opp: Contender, seeds: &[u64], first_index: usize) -> Matchup {
    // One slot per distinct card in the deck. Thirty entries scanned
    // linearly beats a hash map at this size, and it keeps the result in
    // deck order rather than in whatever order a hasher produces.
    let mut cards: Vec<(CardId, CardStat)> = Vec::with_capacity(30);
    for c in me.cards {
        if !cards.iter().any(|(x, _)| x == c) {
            cards.push((*c, CardStat::default()));
        }
    }
    let mut out = Matchup {
        games: 0,
        wins: 0,
        cards,
    };
    let mut opening: Vec<CardId> = Vec::with_capacity(5);
    let mut before: Vec<CardId> = Vec::with_capacity(40);
    let mut left: Vec<CardId> = Vec::with_capacity(40);

    for (i, &seed) in seeds.iter().enumerate() {
        let first = if (first_index + i) % 2 == 0 {
            Side::Player0
        } else {
            Side::Player1
        };
        let Ok(mut g) = Game::new((me.class, me.cards), (opp.class, opp.cards), seed) else {
            continue;
        };
        let mut sa = Scripted::new(me.style);
        let mut sb = Scripted::new(opp.style);
        let mut agents: [&mut dyn Agent; 2] = [&mut sa, &mut sb];

        g.start(first, &mut agents);
        opening.clear();
        opening.extend(g.player(Side::Player0).starting_hand.iter().copied());
        before.clear();
        before.extend(g.player(Side::Player0).deck.iter().copied());

        let outcome = g.play_out(&mut agents);
        let won = matches!(outcome, Outcome::Win(Side::Player0));
        out.games += 1;
        out.wins += u32::from(won);

        left.clear();
        left.extend(g.player(Side::Player0).deck.iter().copied());

        for (card, stat) in out.cards.iter_mut() {
            let in_opening = opening.contains(card);
            // A card counts as drawn when fewer copies of it remain in the
            // deck than were shuffled into it, which is true of exactly the
            // copies that left.
            let gone = count(&before, *card) > count(&left, *card);
            if in_opening {
                stat.open_n += 1;
                stat.open_w += u32::from(won);
            }
            if in_opening || gone {
                stat.drawn_n += 1;
                stat.drawn_w += u32::from(won);
            }
        }
    }
    out
}

fn count(cards: &[CardId], card: CardId) -> usize {
    cards.iter().filter(|c| **c == card).count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::Style;
    use crate::batch::{play_batch, seeds};
    use crate::cards::{Class, Formats};
    use crate::deck::curve_deck;

    fn contender(class: Class, deck: &[CardId]) -> Contender<'_> {
        Contender {
            class,
            cards: deck,
            style: Style::Midrange,
        }
    }

    #[test]
    fn the_win_count_matches_a_plain_batch_over_the_same_seeds() {
        // The whole point of reusing the seed list and the coin rule: the
        // instrumented run must be the *same games*, or the per-card deltas
        // are measured against a baseline from a different sample.
        let mage = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let druid = curve_deck(Class::Druid, Formats::STANDARD).unwrap();
        let s = seeds(4, 200);
        let plain = play_batch(
            contender(Class::Mage, &mage),
            contender(Class::Druid, &druid),
            &s,
        );
        let t = instrumented(
            contender(Class::Mage, &mage),
            contender(Class::Druid, &druid),
            &s,
        );
        assert_eq!(t.games, plain.total());
        assert_eq!(t.wins, plain.wins[0]);
    }

    #[test]
    fn threading_does_not_change_the_numbers() {
        let mage = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let hunter = curve_deck(Class::Hunter, Formats::STANDARD).unwrap();
        let s = seeds(9, 300);
        let one = instrumented(
            contender(Class::Mage, &mage),
            contender(Class::Hunter, &hunter),
            &s,
        );
        let many = instrumented_parallel(
            contender(Class::Mage, &mage),
            contender(Class::Hunter, &hunter),
            &s,
            4,
        );
        assert_eq!(one.games, many.games);
        assert_eq!(one.wins, many.wins);
        let mut a = one.cards.clone();
        let mut b = many.cards.clone();
        a.sort_by_key(|(c, _)| c.0);
        b.sort_by_key(|(c, _)| c.0);
        assert_eq!(a, b);
    }

    #[test]
    fn every_counted_game_is_a_finished_game() {
        let mage = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let t = instrumented(
            contender(Class::Mage, &mage),
            contender(Class::Mage, &mage),
            &seeds(2, 120),
        );
        assert_eq!(t.games, 120);
        for (card, s) in &t.cards {
            assert!(
                s.open_n <= s.drawn_n,
                "{} kept more often than seen",
                card.name()
            );
            assert!(
                s.drawn_n <= t.games,
                "{} seen in more games than played",
                card.name()
            );
            assert!(s.open_w <= s.open_n && s.drawn_w <= s.drawn_n);
        }
    }

    #[test]
    fn a_card_in_the_deck_is_eventually_seen() {
        // If the deck diff were broken — comparing the wrong zone, say — every
        // card would read as never drawn and every delta would be `None`,
        // which looks like "not enough data" rather than like a bug.
        let mage = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let t = instrumented(
            contender(Class::Mage, &mage),
            contender(Class::Mage, &mage),
            &seeds(6, 150),
        );
        let seen = t.cards.iter().filter(|(_, s)| s.drawn_n > 0).count();
        assert_eq!(
            seen,
            t.cards.len(),
            "some deck cards were never seen in 150 games"
        );
        let base = t.base();
        assert!((0.0..=1.0).contains(&base));
        // With 150 games every card clears a 30-game floor for *some* stat.
        assert!(
            t.cards
                .iter()
                .any(|(_, s)| s.drawn_delta(base, 30).is_some())
        );
    }
}
