//! What the rune data has to be true of, whoever regenerates the table.
//!
//! The costs come from Blizzard's card API through `data/runes.json` (see
//! `xtask/src/runes.rs`), and a regeneration that quietly loses them would
//! leave every card asking for no runes -- which compiles, passes every other
//! test, and is wrong. These are the properties that would notice.

use tavernlab_core::cards::{Class, DEFS, INFO, by_name};

#[test]
fn runes_are_a_death_knight_feature_and_nobody_elses() {
    // Stated by the wiki's Rune page: "Runes are a special feature found only
    // on death knight cards."
    let stray: Vec<&str> = DEFS
        .iter()
        .zip(INFO.iter())
        .filter(|(d, _)| d.runes.any() && d.class() != Class::DeathKnight)
        .map(|(_, i)| i.name)
        .collect();
    assert!(stray.is_empty(), "runes on non-Death-Knight cards: {stray:?}");
}

#[test]
fn no_card_asks_for_more_runes_than_a_deck_has() {
    // A deck has three slots across the three colours, so a card wanting four
    // could never be played -- and a packed field that overflowed would read
    // as one that could.
    let over: Vec<(&str, u8)> = DEFS
        .iter()
        .zip(INFO.iter())
        .filter(|(d, _)| d.runes.total() > 3)
        .map(|(d, i)| (i.name, d.runes.total()))
        .collect();
    assert!(over.is_empty(), "more than three runes: {over:?}");
}

#[test]
fn the_table_still_carries_rune_costs_at_all() {
    // The failure this whole file exists for: a regeneration without
    // `data/runes.json` produces a table where every card asks for none.
    let with = DEFS.iter().filter(|d| d.runes.any()).count();
    assert!(with > 100, "only {with} card(s) carry a rune cost");
}

#[test]
fn the_costs_are_the_ones_blizzard_serves() {
    // Spot checks against the API's own `runeCost`, one of each shape: a
    // single colour, and a card that wants all three.
    let runes = |name: &str| {
        let c = by_name(name).unwrap_or_else(|| panic!("{name} is in the corpus"));
        let r = c.def().runes;
        (r.blood(), r.frost(), r.unholy())
    };
    assert_eq!(runes("Hematurge"), (1, 0, 0));
    assert_eq!(runes("Climactic Necrotic Explosion"), (1, 1, 1));
    // And a card of another class asks for nothing, which is most of the table.
    assert_eq!(runes("Fireball"), (0, 0, 0));
}
