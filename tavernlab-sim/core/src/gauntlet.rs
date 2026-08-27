//! The field a deck is measured against.
//!
//! A gauntlet is a fixed set of real deck lists. It is the source of the
//! percentage the app reports, and it is deliberately *not* a ranking: the
//! decks in it carry no win rate of their own here (that is [`tiers`], which
//! plays the same field against itself).
//!
//! Lists arrive as `(name, copies)` pairs and are resolved against the card
//! table one slot at a time. A deck holding a card the engine does not
//! implement is **not** fielded: it is kept, marked, and reported with the
//! card named. Dropping the card, or substituting another, would move the
//! measured win rate by an unknown amount and say nothing about it — the same
//! "implemented in full or excluded" rule the corpus follows.
//!
//! Parsing whatever file the lists came from belongs to the caller, the way
//! [`deck::resolve_slots`](crate::deck::resolve_slots) does it: `core` carries
//! no JSON dependency in production.

use crate::agent::Style;
use crate::batch::{Contender, Record, play_batch_parallel, seeds};
use crate::cards::{CardId, Class, by_name, is_implemented};
use crate::deck::DECK_SIZE;
use crate::state::Side;

/// One deck in the field.
#[derive(Clone, Debug)]
pub struct MetaDeck {
    pub name: String,
    pub class: Class,
    /// How the scripted agent plays it.
    pub style: Style,
    /// The list as written, `(name, copies)`.
    pub cards: Vec<(String, u32)>,
    /// The list as cards, one entry per copy. Empty when the deck cannot be
    /// fielded.
    pub ids: Vec<CardId>,
    /// Slots that did not resolve to an implemented card, with how many
    /// copies were asked for.
    pub missing: Vec<(String, u32)>,
    /// Copies in the list as the file wrote it, before a Beatrix sideboard is
    /// folded in. A deck list is thirty cards; anything else is an incomplete
    /// entry rather than a deck, and is not fielded.
    pub listed: u32,
}

/// Why a deck cannot be fielded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unfieldable {
    /// It holds cards the engine does not implement.
    Cards,
    /// The list is not thirty cards.
    Size,
}

impl MetaDeck {
    /// Build one deck from its written list.
    ///
    /// `sideboard` is folded in only for Commander Beatrix, who plays hers as
    /// ten copies of a single card — that is what the simulator has to field,
    /// and it is also why the exported deck code for such a deck is marked
    /// incomplete rather than handed over as if it were legal in game.
    pub fn new(
        name: impl Into<String>,
        class: Class,
        style: Style,
        cards: &[(String, u32)],
        sideboard: &[(String, u32)],
    ) -> MetaDeck {
        let mut list: Vec<(String, u32)> = cards.to_vec();
        if cards.iter().any(|(n, _)| n == "Commander Beatrix") {
            for (n, _) in sideboard {
                list.push((n.clone(), 10));
            }
        }
        let mut ids = Vec::new();
        let mut missing = Vec::new();
        for (n, count) in &list {
            match by_name(n) {
                Some(id) if is_implemented(id) => {
                    for _ in 0..*count {
                        ids.push(id);
                    }
                }
                _ => missing.push((n.clone(), *count)),
            }
        }
        if !missing.is_empty() {
            ids.clear();
        }
        MetaDeck {
            name: name.into(),
            class,
            style,
            cards: list,
            ids,
            missing,
            listed: cards.iter().map(|(_, n)| n).sum(),
        }
    }

    /// Whether the engine can field this deck as written.
    pub fn playable(&self) -> bool {
        self.problem().is_none()
    }

    /// What stops this deck from being fielded, if anything.
    ///
    /// Two different answers, and collapsing them would misname the problem:
    /// a list the engine cannot play is a coverage gap in the engine, while a
    /// list of twenty cards is an incomplete entry in the gauntlet file.
    ///
    /// Size is reported first when both are true, because it is the answer to
    /// "what would it take to field this": implementing every card in a
    /// twenty-card list still leaves twenty cards.
    pub fn problem(&self) -> Option<Unfieldable> {
        if self.listed != DECK_SIZE as u32 {
            return Some(Unfieldable::Size);
        }
        if !self.missing.is_empty() {
            return Some(Unfieldable::Cards);
        }
        // A thirty-card list that resolved to nothing has to be caught too:
        // the ids are what actually gets fielded.
        if self.ids.is_empty() {
            return Some(Unfieldable::Cards);
        }
        None
    }

    pub fn contender(&self) -> Contender<'_> {
        Contender {
            class: self.class,
            cards: &self.ids,
            style: self.style,
        }
    }

    /// Copies requested in total.
    pub fn total(&self) -> u32 {
        self.cards.iter().map(|(_, n)| n).sum()
    }
}

/// A deck's win rate against each playable deck in the field.
#[derive(Clone, Debug)]
pub struct Rates {
    /// `(opponent name, win rate)`, in field order.
    pub per_deck: Vec<(String, f64)>,
    /// Games played against each opponent.
    pub games_per_deck: usize,
    /// Decks in the field that could not be fielded, and are therefore not
    /// in `per_deck`. Reported rather than dropped: an average over seven
    /// decks that looks like an average over twelve is a lie by omission.
    pub skipped: Vec<String>,
}

impl Rates {
    /// The mean across the field, or `None` when nothing could be played.
    pub fn average(&self) -> Option<f64> {
        if self.per_deck.is_empty() {
            return None;
        }
        Some(self.per_deck.iter().map(|(_, r)| r).sum::<f64>() / self.per_deck.len() as f64)
    }
}

/// Play `deck` against every playable deck in `field`.
///
/// `per_opponent` games each, and every matchup uses the same seed list, so
/// two candidate decks compared through this function are compared on paired
/// samples rather than independent ones.
pub fn evaluate(
    deck: Contender,
    field: &[MetaDeck],
    per_opponent: usize,
    threads: usize,
    seed_base: u64,
) -> Rates {
    let s = seeds(seed_base, per_opponent);
    let mut per_deck = Vec::new();
    let mut skipped = Vec::new();
    for opp in field {
        if !opp.playable() {
            skipped.push(opp.name.clone());
            continue;
        }
        let r = play_batch_parallel(deck, opp.contender(), &s, threads);
        per_deck.push((opp.name.clone(), r.rate(Side::Player0)));
    }
    Rates {
        per_deck,
        games_per_deck: per_opponent,
        skipped,
    }
}

/// One matchup, for callers that want the record rather than the rate.
pub fn matchup(
    a: Contender,
    b: Contender,
    per_pair: usize,
    threads: usize,
    seed_base: u64,
) -> Record {
    play_batch_parallel(a, b, &seeds(seed_base, per_pair), threads)
}

/// The class a gauntlet file names, in Blizzard's spelling.
pub fn class_by_name(s: &str) -> Option<Class> {
    Some(match s.to_ascii_uppercase().as_str() {
        "DEATHKNIGHT" => Class::DeathKnight,
        "DEMONHUNTER" => Class::DemonHunter,
        "DRUID" => Class::Druid,
        "HUNTER" => Class::Hunter,
        "MAGE" => Class::Mage,
        "PALADIN" => Class::Paladin,
        "PRIEST" => Class::Priest,
        "ROGUE" => Class::Rogue,
        "SHAMAN" => Class::Shaman,
        "WARLOCK" => Class::Warlock,
        "WARRIOR" => Class::Warrior,
        "NEUTRAL" => Class::Neutral,
        _ => return None,
    })
}

/// Blizzard's spelling of a class, which is what the deck files, the API and
/// the front end all key on.
pub fn class_name(c: Class) -> &'static str {
    match c {
        Class::DeathKnight => "DEATHKNIGHT",
        Class::DemonHunter => "DEMONHUNTER",
        Class::Druid => "DRUID",
        Class::Hunter => "HUNTER",
        Class::Mage => "MAGE",
        Class::Paladin => "PALADIN",
        Class::Priest => "PRIEST",
        Class::Rogue => "ROGUE",
        Class::Shaman => "SHAMAN",
        Class::Warlock => "WARLOCK",
        Class::Warrior => "WARRIOR",
        Class::Dream => "DREAM",
        Class::Whizbang => "WHIZBANG",
        Class::Neutral => "NEUTRAL",
    }
}

/// The archetype a gauntlet file names, defaulting to midrange.
pub fn style_by_name(s: &str) -> Style {
    match s.to_ascii_lowercase().as_str() {
        "aggro" => Style::Aggro,
        "control" => Style::Control,
        _ => Style::Midrange,
    }
}

pub fn style_name(s: Style) -> &'static str {
    match s {
        Style::Aggro => "aggro",
        Style::Midrange => "midrange",
        Style::Control => "control",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cards::Formats;
    use crate::deck::curve_deck;

    fn pairs(v: &[(&str, u32)]) -> Vec<(String, u32)> {
        v.iter().map(|(n, c)| ((*n).to_string(), *c)).collect()
    }

    /// A class's curve deck as a written list: thirty cards, all implemented.
    fn thirty(class: Class) -> Vec<(String, u32)> {
        let deck = curve_deck(class, Formats::STANDARD).expect("a curve deck");
        let mut counts: Vec<(String, u32)> = Vec::new();
        for c in &deck {
            match counts.iter_mut().find(|(n, _)| n == c.name()) {
                Some((_, n)) => *n += 1,
                None => counts.push((c.name().to_string(), 1)),
            }
        }
        counts
    }

    #[test]
    fn a_deck_with_an_unimplemented_card_is_kept_but_not_fielded() {
        let unimpl = crate::cards::all()
            .find(|c| c.def().collectible && c.def().deckable() && !is_implemented(*c))
            .expect("the table has unimplemented cards");
        let mut list = thirty(Class::Mage);
        list[0] = (unimpl.name().to_string(), list[0].1);
        let d = MetaDeck::new("Test", Class::Mage, Style::Midrange, &list, &[]);
        assert_eq!(d.problem(), Some(Unfieldable::Cards));
        assert_eq!(d.missing, vec![(unimpl.name().to_string(), list[0].1)]);
        assert!(
            d.ids.is_empty(),
            "a deck with a missing card must not be half-fielded"
        );
        assert_eq!(d.total(), 30, "the list itself is still readable");
    }

    #[test]
    fn a_sideboard_is_folded_in_only_for_beatrix() {
        // The ten copies are what the engine must field; a deck without
        // Beatrix has a sideboard that belongs to something else entirely.
        // The count is kept so the list stays a legal thirty; what changes is
        // that one of its cards is now Beatrix.
        let mut list = thirty(Class::Mage);
        list[0] = ("Commander Beatrix".to_string(), list[0].1);
        let with = MetaDeck::new(
            "Beatrix",
            Class::Mage,
            Style::Midrange,
            &list,
            &pairs(&[("Wisp", 1)]),
        );
        assert_eq!(
            with.cards.last().cloned(),
            Some(("Wisp".to_string(), 10)),
            "the sideboard card joins as ten copies"
        );
        assert_eq!(with.total(), with.listed + 10);
        assert!(with.missing.is_empty(), "and it is a card the engine knows");

        let without = MetaDeck::new(
            "Plain",
            Class::Mage,
            Style::Midrange,
            &thirty(Class::Mage),
            &pairs(&[("Frostbolt", 1)]),
        );
        assert!(without.cards.iter().all(|(n, _)| n != "Frostbolt"));
    }

    #[test]
    fn an_unplayable_deck_is_skipped_and_named() {
        let good = MetaDeck::new(
            "Playable",
            Class::Mage,
            Style::Midrange,
            &thirty(Class::Mage),
            &[],
        );
        let mut broken = thirty(Class::Mage);
        broken[0] = ("Not A Real Card".to_string(), broken[0].1);
        let bad = MetaDeck::new("Broken", Class::Mage, Style::Midrange, &broken, &[]);
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let me = Contender {
            class: Class::Mage,
            cards: &deck,
            style: Style::Midrange,
        };
        let r = evaluate(me, &[good, bad], 12, 2, 5);
        assert_eq!(r.per_deck.len(), 1);
        assert_eq!(r.skipped, vec!["Broken".to_string()]);
        let avg = r.average().expect("one deck was playable");
        assert!((0.0..=1.0).contains(&avg));
    }

    #[test]
    fn evaluation_is_deterministic_and_symmetric_in_a_mirror() {
        let deck = curve_deck(Class::Mage, Formats::STANDARD).unwrap();
        let field = vec![MetaDeck::new(
            "Mirror",
            Class::Mage,
            Style::Midrange,
            &thirty(Class::Mage),
            &[],
        )];
        assert!(
            field[0].playable(),
            "a curve deck is implemented by construction"
        );
        let me = Contender {
            class: Class::Mage,
            cards: &deck,
            style: Style::Midrange,
        };
        let a = evaluate(me, &field, 200, 1, 3);
        let b = evaluate(me, &field, 200, 4, 3);
        assert_eq!(a.per_deck, b.per_deck, "thread count changed the answer");
        // A mirror with the same style and alternating starts is a coin
        // flip; anything far off it means the pairing is broken.
        let rate = a.average().unwrap();
        assert!((0.35..0.65).contains(&rate), "mirror rate {rate}");
    }

    #[test]
    fn a_list_that_is_not_thirty_cards_is_not_a_deck() {
        // Not a coverage gap: a twenty-card entry is an incomplete gauntlet
        // file, and it must not be fielded as if a short deck were a deck.
        let mut short = thirty(Class::Mage);
        short.truncate(5);
        let d = MetaDeck::new("Short", Class::Mage, Style::Midrange, &short, &[]);
        assert!(d.missing.is_empty(), "every card in it resolves");
        assert_eq!(d.problem(), Some(Unfieldable::Size));
        assert!(!d.playable());

        // Beatrix's ten sideboard copies push a legal thirty past it, and
        // that is the one list that may be longer.
        let mut list = thirty(Class::Mage);
        list[0] = ("Commander Beatrix".to_string(), list[0].1);
        let beatrix = MetaDeck::new(
            "Beatrix",
            Class::Mage,
            Style::Midrange,
            &list,
            &pairs(&[("Wisp", 1)]),
        );
        assert_eq!(beatrix.listed, 30);
        assert!(beatrix.total() > 30, "the sideboard is folded in");
        assert_eq!(beatrix.problem(), None);
    }

    #[test]
    fn class_names_round_trip() {
        for c in crate::cards::PLAYABLE_CLASSES {
            assert_eq!(class_by_name(class_name(c)), Some(c));
        }
        assert_eq!(class_by_name("not a class"), None);
    }
}
