//! Building and validating deck lists.
//!
//! A deck is just a card list here; the 30-card limit and copy limits are
//! checked rather than assumed, because a deck arriving from a deck code or a
//! generator is untrusted input.

use crate::cards::{CardId, Class, Formats, Kind, all, by_name, is_implemented};

pub const DECK_SIZE: usize = 30;

/// What is wrong with a deck list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DeckError {
    WrongSize(usize),
    /// More copies than the card's rarity allows.
    TooManyCopies(CardId, u8),
    /// A card from another class.
    WrongClass(CardId),
    /// Not legal in the requested format.
    NotLegal(CardId),
    /// Not a card that can go in a deck at all.
    NotDeckable(CardId),
}

/// Check a deck list against the construction rules.
pub fn validate(cards: &[CardId], class: Class, format: Formats) -> Result<(), DeckError> {
    if cards.len() != DECK_SIZE {
        return Err(DeckError::WrongSize(cards.len()));
    }
    for &c in cards {
        let d = c.def();
        if !d.deckable() {
            return Err(DeckError::NotDeckable(c));
        }
        if !d.playable_by(class) {
            return Err(DeckError::WrongClass(c));
        }
        if !d.formats.has(format) {
            return Err(DeckError::NotLegal(c));
        }
        let n = cards.iter().filter(|x| **x == c).count() as u8;
        if n > d.copy_limit() {
            return Err(DeckError::TooManyCopies(c, n));
        }
    }
    Ok(())
}

/// Every collectible card a `class` may play in `format`.
pub fn pool(class: Class, format: Formats) -> Vec<CardId> {
    all()
        .filter(|c| {
            let d = c.def();
            d.collectible && d.deckable() && d.playable_by(class) && d.formats.has(format)
        })
        .collect()
}

/// Cards a `class` may play in `format` that the engine actually implements.
///
/// The distinction matters for a simulation: an unimplemented card in a deck
/// is a dead draw that quietly skews every result, so a generated deck must
/// never contain one.
pub fn implemented_pool(class: Class, format: Formats) -> Vec<CardId> {
    pool(class, format)
        .into_iter()
        .filter(|c| is_implemented(*c))
        .collect()
}

/// A legal 30-card deck drawn from the implemented pool, following a normal
/// mana curve rather than the cheapest thirty cards.
///
/// Still not a good deck. It is a *representative* one: enough spells and
/// removal that a benchmark measures the engine doing real work, and
/// reproducible so two runs are comparable.
///
/// Returns `None` when the implemented pool cannot fill thirty cards — which
/// is the honest answer for a class the engine barely covers yet, and better
/// than quietly handing back a short deck that fails validation later.
pub fn curve_deck(class: Class, format: Formats) -> Option<Vec<CardId>> {
    // Roughly a midrange curve, indexed by cost.
    const WANT: [usize; 9] = [0, 4, 6, 6, 5, 4, 3, 1, 1];
    let mut p = implemented_pool(class, format);
    p.sort_by_key(|c| (c.def().cost, c.0));

    let mut deck = Vec::with_capacity(DECK_SIZE);
    for (cost, want) in WANT.iter().enumerate() {
        let mut taken = 0;
        for c in p.iter().filter(|c| c.def().cost as usize == cost) {
            if taken >= *want || deck.len() >= DECK_SIZE {
                break;
            }
            for _ in 0..c.def().copy_limit() {
                if taken < *want && deck.len() < DECK_SIZE {
                    deck.push(*c);
                    taken += 1;
                }
            }
        }
    }
    // Top up with anything legal if the curve could not be filled.
    for c in &p {
        if deck.len() >= DECK_SIZE {
            break;
        }
        let held = deck.iter().filter(|x| *x == c).count() as u8;
        if held < c.def().copy_limit() {
            deck.push(*c);
        }
    }
    (deck.len() == DECK_SIZE).then_some(deck)
}

/// How a decklist's requested copies resolve against the implemented table.
pub struct SlotReport {
    /// Requested copies covered by an implemented card of that name.
    pub ok: u32,
    /// Copies requested in total.
    pub total: u32,
    /// Names that did not resolve to an implemented card, with how many
    /// copies were asked for.
    pub missing: Vec<(String, u32)>,
}

/// Resolve a decklist against the corpus: how many requested copies are
/// covered by an implemented card of that name.
///
/// Takes already-parsed `(name, count)` pairs rather than a file or a JSON
/// value, so this stays available to a `core` that carries no JSON
/// dependency in production — whatever reads the meta-deck file owns the
/// parsing and hands the pairs in.
pub fn resolve_slots(cards: &[(&str, u32)]) -> SlotReport {
    let mut ok = 0;
    let mut total = 0;
    let mut missing = Vec::new();
    for &(name, count) in cards {
        total += count;
        match by_name(name) {
            Some(id) if is_implemented(id) => ok += count,
            _ => missing.push((name.to_string(), count)),
        }
    }
    SlotReport { ok, total, missing }
}

/// A legal 30-card deck of the cheapest minions available.
///
/// Not a good deck — a fixed, reproducible one, for benchmarks and for tests
/// that need a deck without caring what is in it.
pub fn cheapest_minions(class: Class, format: Formats) -> Vec<CardId> {
    let mut p: Vec<CardId> = pool(class, format)
        .into_iter()
        .filter(|c| c.def().kind() == Kind::Minion && c.def().atk > 0)
        .collect();
    // Sorted by id as well as cost so the result never depends on table order.
    p.sort_by_key(|c| (c.def().cost, c.0));
    let mut deck = Vec::with_capacity(DECK_SIZE);
    for c in p {
        for _ in 0..c.def().copy_limit() {
            if deck.len() < DECK_SIZE {
                deck.push(c);
            }
        }
        if deck.len() >= DECK_SIZE {
            break;
        }
    }
    deck
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_decks_are_legal() {
        for class in crate::cards::PLAYABLE_CLASSES {
            let cheap = cheapest_minions(class, Formats::STANDARD);
            assert_eq!(
                validate(&cheap, class, Formats::STANDARD),
                Ok(()),
                "{class:?} cheap"
            );
            if let Some(d) = curve_deck(class, Formats::STANDARD) {
                assert_eq!(
                    validate(&d, class, Formats::STANDARD),
                    Ok(()),
                    "{class:?} curve"
                );
            }
        }
    }

    #[test]
    fn most_classes_can_field_a_curve_deck() {
        // Coverage is uneven and some classes cannot yet fill thirty cards.
        // This pins how many can, so the number cannot quietly fall.
        let n = crate::cards::PLAYABLE_CLASSES
            .iter()
            .filter(|c| curve_deck(**c, Formats::STANDARD).is_some())
            .count();
        assert!(n >= 8, "only {n} of 11 classes can field a curve deck");
    }

    #[test]
    fn a_curve_deck_contains_only_implemented_cards() {
        // An unimplemented card in a deck is a dead draw that skews every
        // result it appears in.
        for class in crate::cards::PLAYABLE_CLASSES {
            let Some(d) = curve_deck(class, Formats::STANDARD) else {
                continue;
            };
            for c in d {
                assert!(is_implemented(c), "{} is not implemented", c.name());
            }
        }
    }

    #[test]
    fn a_curve_deck_is_not_all_minions() {
        // The point of using the implemented pool is that spells get played.
        // If this ever fails, the pool has lost its spells and the benchmark
        // silently stopped measuring them.
        let total: usize = crate::cards::PLAYABLE_CLASSES
            .iter()
            .map(|c| {
                curve_deck(*c, Formats::STANDARD)
                    .unwrap_or_default()
                    .iter()
                    .filter(|x| x.def().kind() == Kind::Spell)
                    .count()
            })
            .sum();
        assert!(total > 0, "no class deck contains a single spell");
    }

    #[test]
    fn validation_catches_each_kind_of_mistake() {
        let class = Class::Mage;
        let good = cheapest_minions(class, Formats::STANDARD);

        let mut short = good.clone();
        short.pop();
        assert_eq!(
            validate(&short, class, Formats::STANDARD),
            Err(DeckError::WrongSize(29))
        );

        // Three copies of the first card.
        let mut dupes = good.clone();
        dupes[1] = good[0];
        dupes[2] = good[0];
        assert!(matches!(
            validate(&dupes, class, Formats::STANDARD),
            Err(DeckError::TooManyCopies(_, _))
        ));

        // A card from another class.
        let other = pool(Class::Hunter, Formats::STANDARD)
            .into_iter()
            .find(|c| c.def().class() == Class::Hunter)
            .unwrap();
        let mut wrong = good.clone();
        wrong[0] = other;
        assert_eq!(
            validate(&wrong, class, Formats::STANDARD),
            Err(DeckError::WrongClass(other))
        );
    }

    #[test]
    fn a_wild_only_card_is_rejected_from_standard() {
        let wild_only = all()
            .find(|c| {
                let d = c.def();
                d.collectible
                    && d.deckable()
                    && d.formats.has(Formats::WILD)
                    && !d.formats.has(Formats::STANDARD)
                    && d.playable_by(Class::Mage)
            })
            .expect("the corpus has Wild-only Mage cards");
        let mut deck = cheapest_minions(Class::Mage, Formats::STANDARD);
        deck[0] = wild_only;
        assert_eq!(
            validate(&deck, Class::Mage, Formats::STANDARD),
            Err(DeckError::NotLegal(wild_only))
        );
        // The same list is fine in Wild.
        assert_eq!(validate(&deck, Class::Mage, Formats::WILD), Ok(()));
    }

    #[test]
    fn the_pool_is_not_empty_for_any_class() {
        for class in crate::cards::PLAYABLE_CLASSES {
            assert!(pool(class, Formats::STANDARD).len() > 50, "{class:?}");
        }
    }

    #[test]
    fn resolve_slots_counts_implemented_copies_and_lists_the_rest() {
        let report = resolve_slots(&[
            ("Fireball", 2),
            ("Not A Real Card", 1),
            ("Deathwing, Worldbreaker", 1), // a real card, not yet implemented
        ]);
        assert_eq!(report.ok, 2);
        assert_eq!(report.total, 4);
        assert_eq!(
            report.missing,
            vec![
                ("Not A Real Card".to_string(), 1),
                ("Deathwing, Worldbreaker".to_string(), 1),
            ]
        );
    }
}
