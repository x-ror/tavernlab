//! Comparing Arena draft picks by simulation.
//!
//! The question the engine can honestly answer about a pick
//! (docs/ARENA_RESEARCH.md §5.2): *if this card joins the cards already
//! taken, and the rest of the deck is filled with random legal cards of the
//! season pool, how does the whole thing fare against an Arena-shaped
//! field?* The number that comes out is a comparison between candidates,
//! never a season winrate — early in a draft the random tail is most of the
//! deck, and the caller is told how much of it was real picks.
//!
//! Fairness over realism: every candidate is completed with the *same* tail
//! seeds, so two decks differ by as little as the pick allows, and the same
//! game seeds, so neither candidate gets the luckier draws. What this does
//! not model is stated in [`fill`].

use crate::agent::Style;
use crate::batch::Contender;
use crate::cards::{CardId, Class, Formats};
use crate::deck::{DECK_SIZE, implemented_pool};
use crate::gauntlet::{MetaDeck, evaluate};
use crate::rng::Rand;

/// How one candidate scored.
#[derive(Clone, Debug)]
pub struct PickScore {
    pub card: CardId,
    /// Mean winrate against the field, across every tail. `None` when not
    /// one field deck could be played.
    pub winrate: Option<f64>,
    /// Games behind the number.
    pub games: u32,
    /// Support cards forced in with this candidate (a Legendary Group).
    pub extra: Vec<CardId>,
    /// `picked` + this candidate + extras that actually sat in the deck.
    pub real_cards: usize,
}

/// The knobs a comparison runs under.
#[derive(Clone, Copy, Debug)]
pub struct PickBudget {
    /// Random completions per candidate. Shared across candidates by seed.
    pub tails: usize,
    /// Games against each field deck, per tail.
    pub games_per_deck: usize,
    pub threads: usize,
    pub seed: u64,
}

impl Default for PickBudget {
    fn default() -> Self {
        // 3 tails x 25 games x an 11-deck field ~ 825 games per candidate:
        // the "live pick fits in seconds" budget the research asked for,
        // well under what one mulligan measurement already spends.
        PickBudget {
            tails: 3,
            games_per_deck: 25,
            threads: 1,
            seed: 47,
        }
    }
}

/// `picked` + `candidate` + `extra`, filled to [`DECK_SIZE`] with cards
/// sampled uniformly from the implemented season pool of `class`.
///
/// `extra` is a Legendary Group's support cards: the first pick is a
/// package, not a singleton (§5.2). Uniform is a simplification stated
/// openly: the client offers by quality bucket with weights nobody outside
/// Blizzard has (§2.2 of the research), so modelling them would be a guess
/// wearing a number. Two copies is the stack cap — an Arena deck may
/// legally hold more, but a random tail that piles four of one card is not
/// a deck anyone was offered; if the pool is too shallow to respect even
/// that, the cap yields rather than loops.
fn fill(
    picked: &[CardId],
    candidate: CardId,
    extra: &[CardId],
    pool: &[CardId],
    seed: u64,
) -> Vec<CardId> {
    let mut deck = Vec::with_capacity(DECK_SIZE);
    deck.extend_from_slice(picked);
    if deck.len() < DECK_SIZE {
        deck.push(candidate);
    }
    for &c in extra {
        if deck.len() >= DECK_SIZE {
            break;
        }
        deck.push(c);
    }
    let mut r = Rand::new(seed);
    let mut misses = 0;
    while deck.len() < DECK_SIZE {
        let c = pool[r.index(pool.len())];
        if misses > 1_000 || deck.iter().filter(|x| **x == c).count() < 2 {
            deck.push(c);
        } else {
            misses += 1;
        }
    }
    deck
}

/// Score each candidate: complete the draft around it and play the result
/// against the field.
///
/// The caller owns the honesty gates — every candidate and every picked
/// card implemented, or no simulation at all (§5.2: a card the engine does
/// not play is *not compared*, never silently dropped). What is enforced
/// here is what would otherwise return nonsense: an empty season pool is an
/// error, not an empty answer.
pub fn compare_picks(
    class: Class,
    picked: &[CardId],
    candidates: &[CardId],
    extras: &[Vec<CardId>],
    field: &[MetaDeck],
    budget: &PickBudget,
) -> Result<Vec<PickScore>, &'static str> {
    let pool = implemented_pool(class, Formats::ARENA);
    if pool.is_empty() {
        return Err("the corpus carries no Arena season pool");
    }
    if picked.len() >= DECK_SIZE {
        return Err("the draft already holds a full deck");
    }
    Ok(candidates
        .iter()
        .enumerate()
        .map(|(i, &card)| {
            let extra: &[CardId] = extras.get(i).map(Vec::as_slice).unwrap_or(&[]);
            let mut sum = 0.0;
            let mut tails = 0u32;
            let mut games = 0u32;
            for t in 0..budget.tails {
                let tail_seed = budget.seed ^ (t as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
                let deck = fill(picked, card, extra, &pool, tail_seed);
                let me = Contender {
                    class,
                    cards: &deck,
                    // The Arena default: a 5-loss format rewards the games
                    // that end before the late game (§6.4).
                    style: Style::Aggro,
                };
                let rates = evaluate(
                    me,
                    field,
                    budget.games_per_deck,
                    budget.threads,
                    budget.seed + t as u64,
                );
                if let Some(avg) = rates.average() {
                    sum += avg;
                    tails += 1;
                    games += (rates.per_deck.len() * budget.games_per_deck) as u32;
                }
            }
            let real = (picked.len() + 1 + extra.len()).min(DECK_SIZE);
            PickScore {
                card,
                winrate: (tails > 0).then(|| sum / tails as f64),
                games,
                extra: extra.to_vec(),
                real_cards: real,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::by_name;
    use crate::gauntlet::MetaDeck;

    fn field_of(class: Class) -> Vec<MetaDeck> {
        let deck = crate::deck::curve_deck(class, Formats::ARENA).expect("arena deck");
        let cards: Vec<(String, u32)> = {
            let mut counts: Vec<(String, u32)> = Vec::new();
            for id in &deck {
                match counts.iter_mut().find(|(n, _)| *n == id.name()) {
                    Some((_, c)) => *c += 1,
                    None => counts.push((id.name().to_string(), 1)),
                }
            }
            counts
        };
        vec![MetaDeck::new(
            "Test Arena Opponent",
            class,
            Style::Aggro,
            &cards,
            &[],
        )]
    }

    #[test]
    fn candidates_share_their_tails_and_seeds() {
        // The same candidate must score identically twice: everything that
        // varies is seeded. That is what makes a delta between two
        // candidates a statement about the cards.
        let picked: Vec<CardId> = ["Chillwind Yeti", "Fireball"]
            .iter()
            .map(|n| by_name(n).unwrap())
            .collect();
        let candidate = by_name("Water Elemental").unwrap();
        let field = field_of(Class::Mage);
        let budget = PickBudget {
            tails: 2,
            games_per_deck: 4,
            threads: 1,
            seed: 7,
        };
        let a = compare_picks(Class::Mage, &picked, &[candidate], &[], &field, &budget).unwrap();
        let b = compare_picks(Class::Mage, &picked, &[candidate], &[], &field, &budget).unwrap();
        assert_eq!(a[0].winrate, b[0].winrate);
        assert!(a[0].games > 0);
        assert!(a[0].winrate.is_some());
    }

    #[test]
    fn a_full_draft_refuses_another_pick() {
        let deck = crate::deck::curve_deck(Class::Mage, Formats::ARENA).expect("arena deck");
        let candidate = by_name("Fireball").unwrap();
        assert!(
            compare_picks(
                Class::Mage,
                &deck,
                &[candidate],
                &[],
                &field_of(Class::Mage),
                &PickBudget::default()
            )
            .is_err()
        );
    }

    #[test]
    fn a_legendary_group_sits_in_the_deck_before_the_tail() {
        let picked: Vec<CardId> = Vec::new();
        let candidate = by_name("Fireball").unwrap();
        let extra = vec![by_name("Chillwind Yeti").unwrap()];
        let pool = crate::deck::implemented_pool(Class::Mage, Formats::ARENA);
        let deck = fill(&picked, candidate, &extra, &pool, 1);
        assert_eq!(deck[0], candidate);
        assert_eq!(deck[1], extra[0]);
        assert_eq!(deck.len(), DECK_SIZE);
    }
}
