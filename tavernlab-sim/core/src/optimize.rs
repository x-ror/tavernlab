//! Looking for a better deck, one card at a time.
//!
//! The search itself is the easy half: swap one card, measure, keep it if it
//! helped. The hard half is not lying about the result, and there are two
//! ways to do that here.
//!
//! **Pairing.** Every candidate is measured on the same seed list as the
//! baseline. Two decks that share 29 of 30 cards differ by far less than the
//! noise of two independent samples, and pairing recovers that shared
//! variance instead of throwing it away.
//!
//! **The winner's curse.** Picking the best of twenty-four noisy measurements
//! systematically overstates it: the swap that came out on top is partly the
//! swap that got lucky. So the leader of each round is *re-measured* on a
//! fresh, larger seed set before it is accepted, and it is that second number
//! that gets reported. On the Python implementation this step did not exist,
//! and the README had to warn that the printed figure was inflated. Rather
//! than warn, this measures again — a re-measurement costs a fraction of a
//! second at this engine's throughput.
//!
//! A swap that survives both is still a swap against a scripted opponent on a
//! twelve-deck field, which is a claim about this simulator and not about
//! ranked play.

use crate::agent::Style;
use crate::batch::Contender;
use crate::cards::{CardId, Class, Formats};
use crate::deck::implemented_pool;
use crate::gauntlet::{MetaDeck, evaluate};
use crate::rng::Rand;

/// How hard to look.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Games per opponent when screening a proposal.
    pub screen_games: usize,
    /// Games per opponent when re-measuring a round's leader. Larger than
    /// `screen_games` on purpose — this is the number that gets published.
    pub confirm_games: usize,
    /// Proposals per round.
    pub proposals: usize,
    /// How many rounds of "swap the best card" to run.
    pub rounds: usize,
    /// A confirmed gain below this is not worth a card slot.
    pub keep_threshold: f64,
    pub threads: usize,
    /// Seed for the proposal RNG, so a run is reproducible.
    pub seed: u64,
}

impl Default for Budget {
    fn default() -> Self {
        Budget {
            screen_games: 400,
            confirm_games: 1600,
            proposals: 24,
            rounds: 3,
            keep_threshold: 0.015,
            threads: 4,
            seed: 11,
        }
    }
}

/// One proposed change.
#[derive(Clone, Debug, PartialEq)]
pub struct Swap {
    pub out: &'static str,
    pub inn: &'static str,
    /// Win rate over the screening run.
    pub screened: f64,
    /// Gain over the baseline, as screened.
    pub screen_delta: f64,
    /// Gain over the baseline re-measured on a fresh, larger sample —
    /// `None` for a proposal that never got that far.
    pub confirmed_delta: Option<f64>,
}

/// What a search found.
#[derive(Clone, Debug)]
pub struct Report {
    /// Win rate of the deck as submitted.
    pub base: f64,
    /// Win rate after the accepted swaps, measured at `confirm_games`.
    pub best: f64,
    /// Swaps that survived re-measurement, in the order they were applied.
    pub kept: Vec<Swap>,
    /// Proposals that screened positive but did not survive — the near
    /// misses, reported because "we tried and it did not hold up" is a
    /// different answer from "we did not try".
    pub near: Vec<Swap>,
    /// The improved deck.
    pub deck: Vec<CardId>,
    /// Games played in total, so the cost of an answer is visible.
    pub games: u64,
}

/// Hill-climb `deck` against `field`.
pub fn optimize(
    deck: &[CardId],
    class: Class,
    format: Formats,
    style: Style,
    field: &[MetaDeck],
    budget: Budget,
    mut log: impl FnMut(String),
) -> Report {
    let playable = field.iter().filter(|d| d.playable()).count();
    let mut games = 0u64;
    let mut rng = Rand::new(budget.seed);
    let mut current: Vec<CardId> = deck.to_vec();

    // Seed bases are fixed per role rather than drawn: screening always
    // plays the same games (that is the pairing), and confirmation always
    // plays a *different* fixed set (that is what makes it independent).
    const SCREEN_SEED: u64 = 0x5EED_5C4E;
    const CONFIRM_SEED: u64 = 0xC0FF_1234;

    let rate = |cards: &[CardId], n: usize, seed: u64| -> f64 {
        evaluate(
            Contender {
                class,
                cards,
                style,
            },
            field,
            n,
            budget.threads,
            seed,
        )
        .average()
        .unwrap_or(0.0)
    };

    let mut base_screen = rate(&current, budget.screen_games, SCREEN_SEED);
    let base_confirm = rate(&current, budget.confirm_games, CONFIRM_SEED);
    games += ((budget.screen_games + budget.confirm_games) * playable) as u64;
    log(format!(
        "baseline {:.1}% over {} field decks ({} games per opponent)",
        base_confirm * 100.0,
        playable,
        budget.confirm_games
    ));

    let pool = implemented_pool(class, format);
    let mut kept = Vec::new();
    let mut near = Vec::new();
    let mut best_confirm = base_confirm;

    for round in 0..budget.rounds {
        let mut tried: Vec<(&'static str, &'static str)> = Vec::new();
        let mut leader: Option<Swap> = None;

        for _ in 0..budget.proposals {
            let Some((out, inn)) = propose(&current, &pool, &mut rng, &tried) else {
                break;
            };
            tried.push((out.name(), inn.name()));
            let cand = swapped(&current, out, inn);
            let r = rate(&cand, budget.screen_games, SCREEN_SEED);
            games += (budget.screen_games * playable) as u64;
            let s = Swap {
                out: out.name(),
                inn: inn.name(),
                screened: r,
                screen_delta: r - base_screen,
                confirmed_delta: None,
            };
            if leader
                .as_ref()
                .is_none_or(|l| s.screen_delta > l.screen_delta)
            {
                if let Some(prev) = leader.replace(s)
                    && prev.screen_delta > 0.0
                {
                    near.push(prev);
                }
            } else if s.screen_delta > 0.0 {
                near.push(s);
            }
        }

        let Some(mut best) = leader else { break };
        if best.screen_delta <= 0.0 {
            log(format!("round {}: no swap improved the deck", round + 1));
            break;
        }

        // Re-measure the leader on games it has not seen. This is where most
        // apparent gains evaporate.
        let Some((out_id, in_id)) = pair(&pool, &current, best.out, best.inn) else {
            break;
        };
        let cand = swapped(&current, out_id, in_id);
        let confirmed = rate(&cand, budget.confirm_games, CONFIRM_SEED);
        games += (budget.confirm_games * playable) as u64;
        let delta = confirmed - best_confirm;
        best.confirmed_delta = Some(delta);
        log(format!(
            "round {}: −{} +{} — screened {:+.1} pts, re-measured {:+.1} pts",
            round + 1,
            best.out,
            best.inn,
            best.screen_delta * 100.0,
            delta * 100.0
        ));

        if delta <= budget.keep_threshold {
            near.push(best);
            break;
        }
        current = cand;
        best_confirm = confirmed;
        base_screen = rate(&current, budget.screen_games, SCREEN_SEED);
        games += (budget.screen_games * playable) as u64;
        kept.push(best);
    }

    near.sort_by(|a, b| b.screen_delta.total_cmp(&a.screen_delta));
    near.truncate(5);
    Report {
        base: base_confirm,
        best: best_confirm,
        kept,
        near,
        deck: current,
        games,
    }
}

/// Pick a card to cut and a card to add, avoiding pairs already tried.
fn propose(
    deck: &[CardId],
    pool: &[CardId],
    rng: &mut Rand,
    tried: &[(&'static str, &'static str)],
) -> Option<(CardId, CardId)> {
    let mut outs: Vec<CardId> = deck.to_vec();
    outs.sort_unstable_by_key(|c| c.0);
    outs.dedup();
    let ins: Vec<CardId> = pool
        .iter()
        .copied()
        .filter(|c| copies(deck, *c) < c.def().copy_limit() as usize)
        .collect();
    if outs.is_empty() || ins.is_empty() {
        return None;
    }
    for _ in 0..300 {
        let o = outs[rng.index(outs.len())];
        let i = ins[rng.index(ins.len())];
        if o != i && !tried.iter().any(|(a, b)| *a == o.name() && *b == i.name()) {
            return Some((o, i));
        }
    }
    None
}

/// Find the two cards a recorded swap names, so the leader can be re-applied
/// after the screening loop has moved on.
fn pair(pool: &[CardId], deck: &[CardId], out: &str, inn: &str) -> Option<(CardId, CardId)> {
    let o = deck.iter().copied().find(|c| c.name() == out)?;
    let i = pool.iter().copied().find(|c| c.name() == inn)?;
    Some((o, i))
}

fn copies(deck: &[CardId], card: CardId) -> usize {
    deck.iter().filter(|c| **c == card).count()
}

fn swapped(deck: &[CardId], out: CardId, inn: CardId) -> Vec<CardId> {
    let mut next = deck.to_vec();
    if let Some(pos) = next.iter().position(|c| *c == out) {
        next[pos] = inn;
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deck::{curve_deck, validate};
    use crate::gauntlet::MetaDeck;

    fn field() -> Vec<MetaDeck> {
        // Two real, fully implemented opponents built from the curve decks,
        // so the test needs no data file.
        [Class::Druid, Class::Hunter]
            .into_iter()
            .map(|c| {
                let deck = curve_deck(c, Formats::STANDARD).unwrap();
                let mut list: Vec<(String, u32)> = Vec::new();
                for id in &deck {
                    match list.iter_mut().find(|(n, _)| n == id.name()) {
                        Some((_, n)) => *n += 1,
                        None => list.push((id.name().to_string(), 1)),
                    }
                }
                MetaDeck::new(format!("{c:?}"), c, Style::Midrange, &list, &[])
            })
            .collect()
    }

    fn small_budget() -> Budget {
        Budget {
            screen_games: 40,
            confirm_games: 80,
            proposals: 4,
            rounds: 1,
            threads: 2,
            ..Budget::default()
        }
    }

    #[test]
    fn the_result_is_still_a_legal_deck() {
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let r = optimize(
            &deck,
            Class::Mage,
            Formats::STANDARD,
            Style::Midrange,
            &field(),
            small_budget(),
            |_| {},
        );
        assert_eq!(r.deck.len(), deck.len());
        assert_eq!(
            validate(&r.deck, Class::Mage, Formats::STANDARD),
            Ok(()),
            "the optimizer produced an illegal deck"
        );
    }

    #[test]
    fn a_run_is_reproducible() {
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let run = || {
            optimize(
                &deck,
                Class::Mage,
                Formats::STANDARD,
                Style::Midrange,
                &field(),
                small_budget(),
                |_| {},
            )
        };
        let a = run();
        let b = run();
        assert_eq!(a.base, b.base);
        assert_eq!(a.kept, b.kept);
        assert_eq!(a.deck, b.deck);
    }

    #[test]
    fn every_kept_swap_carries_a_confirmed_delta_above_the_threshold() {
        // The published number must be the re-measured one, never the
        // screening figure that selected it.
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let budget = Budget {
            proposals: 8,
            rounds: 2,
            ..small_budget()
        };
        let r = optimize(
            &deck,
            Class::Mage,
            Formats::STANDARD,
            Style::Midrange,
            &field(),
            budget,
            |_| {},
        );
        for s in &r.kept {
            let d = s.confirmed_delta.expect("a kept swap was never confirmed");
            assert!(
                d > budget.keep_threshold,
                "{} -> {} kept at {d}",
                s.out,
                s.inn
            );
        }
        for s in &r.near {
            assert!(
                s.screen_delta > 0.0,
                "a near miss that never screened positive"
            );
        }
        assert!(r.games > 0);
    }

    #[test]
    fn a_swap_never_breaks_the_copy_limit() {
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let pool = implemented_pool(Class::Mage, Formats::STANDARD);
        let mut rng = Rand::new(7);
        for _ in 0..200 {
            let Some((o, i)) = propose(&deck, &pool, &mut rng, &[]) else {
                break;
            };
            let next = swapped(&deck, o, i);
            assert!(copies(&next, i) <= i.def().copy_limit() as usize);
        }
    }
}
